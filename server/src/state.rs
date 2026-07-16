use crate::config::{ServerConfig, UserConfig};
use crate::oplog::Oplog;
use anyhow::{Context, Result, bail};
use notes_core::{Connection, Origin, acknowledge_server_op, apply, export_sync_snapshot};
use notes_sync::{Op, SequencedOp, SyncSnapshot};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot};

const REPLAY_BATCH_SIZE: usize = 1_000;

#[derive(Clone)]
pub struct UserRegistry {
    tokens: Arc<HashMap<String, Arc<UserState>>>,
}

pub struct UserState {
    pub id: String,
    pub notes: Connection,
    pub oplog: Oplog,
    pub snapshot_dir: PathBuf,
    command_tx: mpsc::Sender<UserCommand>,
    operations_tx: broadcast::Sender<SequencedOp>,
}

enum UserCommand {
    Ingest {
        operations: Vec<Op>,
        response: oneshot::Sender<Result<Vec<SequencedOp>>>,
    },
    Snapshot {
        response: oneshot::Sender<Result<SyncSnapshot>>,
    },
}

impl UserRegistry {
    pub async fn open(config: &ServerConfig) -> Result<Self> {
        tokio::fs::create_dir_all(&config.data_dir)
            .await
            .with_context(|| format!("creating data directory {}", config.data_dir.display()))?;
        let mut states = HashMap::new();
        let mut tokens = HashMap::new();
        for user in &config.users {
            let state = Arc::new(
                UserState::open(
                    user,
                    &config.data_dir,
                    &config.storage.embedding_provider_id,
                    config.storage.embedding_dimensions,
                    config.snapshot_every_ops,
                )
                .await?,
            );
            states.insert(user.id.clone(), state.clone());
            for token in &user.tokens {
                tokens.insert(token.clone(), state.clone());
            }
        }
        tracing::info!(users = states.len(), "opened server user replicas");
        Ok(Self {
            tokens: Arc::new(tokens),
        })
    }

    pub fn authenticate(&self, token: &str) -> Option<Arc<UserState>> {
        self.tokens.get(token).cloned()
    }
}

impl UserState {
    async fn open(
        user: &UserConfig,
        data_dir: &Path,
        embedding_provider_id: &str,
        embedding_dimensions: usize,
        snapshot_every_ops: u64,
    ) -> Result<Self> {
        let user_dir = data_dir.join("users").join(&user.id);
        let snapshot_dir = user_dir.join("snapshots");
        tokio::fs::create_dir_all(&snapshot_dir)
            .await
            .with_context(|| format!("creating user directory for {}", user.id))?;
        let notes = notes_core::db::open(
            user_dir.join("notes.db"),
            embedding_provider_id,
            embedding_dimensions,
        )
        .await
        .with_context(|| format!("opening notes replica for {}", user.id))?;
        let oplog = Oplog::open(&user_dir.join("oplog.db"))
            .await
            .with_context(|| format!("opening oplog for {}", user.id))?;
        oplog.assert_gapless().await?;
        replay_oplog(&notes, &oplog).await?;

        let (command_tx, command_rx) = mpsc::channel(64);
        let (operations_tx, _) = broadcast::channel(1_024);
        tokio::spawn(run_user_actor(
            notes.clone(),
            oplog.clone(),
            snapshot_dir.clone(),
            snapshot_every_ops,
            operations_tx.clone(),
            command_rx,
        ));
        Ok(Self {
            id: user.id.clone(),
            notes,
            oplog,
            snapshot_dir,
            command_tx,
            operations_tx,
        })
    }

    pub async fn ingest(&self, operations: Vec<Op>) -> Result<Vec<SequencedOp>> {
        let (response, receiver) = oneshot::channel();
        self.command_tx
            .send(UserCommand::Ingest {
                operations,
                response,
            })
            .await
            .context("user ingest actor stopped")?;
        receiver
            .await
            .context("user ingest actor dropped response")?
    }

    pub async fn snapshot(&self) -> Result<SyncSnapshot> {
        let (response, receiver) = oneshot::channel();
        self.command_tx
            .send(UserCommand::Snapshot { response })
            .await
            .context("user snapshot actor stopped")?;
        receiver
            .await
            .context("user snapshot actor dropped response")?
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SequencedOp> {
        self.operations_tx.subscribe()
    }
}

async fn replay_oplog(notes: &Connection, oplog: &Oplog) -> Result<()> {
    let mut cursor = 0;
    loop {
        let operations = oplog.ops_since(cursor, REPLAY_BATCH_SIZE).await?;
        if operations.is_empty() {
            return Ok(());
        }
        for operation in operations {
            apply(notes, &operation.envelope, Origin::Remote)
                .await
                .with_context(|| format!("replaying server op {}", operation.seq))?;
            acknowledge_server_op(notes, operation.envelope.op_id, operation.seq).await?;
            cursor = operation.seq;
        }
    }
}

async fn run_user_actor(
    notes: Connection,
    oplog: Oplog,
    snapshot_dir: PathBuf,
    snapshot_every_ops: u64,
    operations_tx: broadcast::Sender<SequencedOp>,
    mut commands: mpsc::Receiver<UserCommand>,
) {
    while let Some(command) = commands.recv().await {
        match command {
            UserCommand::Ingest {
                operations,
                response,
            } => {
                let result = ingest_operations(
                    &notes,
                    &oplog,
                    &snapshot_dir,
                    snapshot_every_ops,
                    &operations_tx,
                    operations,
                )
                .await;
                let _ = response.send(result);
            }
            UserCommand::Snapshot { response } => {
                let result = write_current_snapshot(&notes, &oplog, &snapshot_dir).await;
                let _ = response.send(result);
            }
        }
    }
}

async fn ingest_operations(
    notes: &Connection,
    oplog: &Oplog,
    snapshot_dir: &Path,
    snapshot_every_ops: u64,
    operations_tx: &broadcast::Sender<SequencedOp>,
    operations: Vec<Op>,
) -> Result<Vec<SequencedOp>> {
    if operations.len() > 256 {
        bail!("a sync batch cannot contain more than 256 operations");
    }
    let mut accepted = Vec::with_capacity(operations.len());
    for operation in operations {
        let append = oplog.append(operation).await?;
        if let Err(error) = apply(notes, &append.operation.envelope, Origin::Remote).await {
            if append.inserted {
                oplog.remove_tail(&append.operation).await?;
            }
            return Err(error.context("applying ingested operation"));
        }
        acknowledge_server_op(notes, append.operation.envelope.op_id, append.operation.seq).await?;
        if append.inserted {
            let _ = operations_tx.send(append.operation.clone());
            if snapshot_every_ops > 0 && append.operation.seq % snapshot_every_ops == 0 {
                write_snapshot_at(notes, snapshot_dir, append.operation.seq).await?;
            }
        }
        accepted.push(append.operation);
    }
    Ok(accepted)
}

async fn write_current_snapshot(
    notes: &Connection,
    oplog: &Oplog,
    snapshot_dir: &Path,
) -> Result<SyncSnapshot> {
    let seq = oplog.latest_seq().await?;
    write_snapshot_at(notes, snapshot_dir, seq).await
}

async fn write_snapshot_at(
    notes: &Connection,
    snapshot_dir: &Path,
    seq: u64,
) -> Result<SyncSnapshot> {
    let snapshot = export_sync_snapshot(notes, seq).await?;
    let contents = serde_json::to_vec(&snapshot)?;
    let temporary = snapshot_dir.join(format!(".{seq}-{}.tmp", uuid::Uuid::now_v7()));
    let target = snapshot_dir.join(format!("{seq}.json"));
    tokio::fs::write(&temporary, contents).await?;
    tokio::fs::rename(&temporary, &target).await?;
    Ok(snapshot)
}

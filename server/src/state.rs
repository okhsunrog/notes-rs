use crate::config::{ServerConfig, UserConfig};
use crate::oplog::Oplog;
use anyhow::{Context, Result, bail};
use notes_core::{
    Connection, apply_sequenced_batch, export_sync_snapshot, import_sync_snapshot, sync_cursor,
};
use notes_protocol::SequencedOp;
use notes_sync::{Op, SyncSnapshot};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tokio::sync::Notify;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

const REPLAY_BATCH_SIZE: usize = 1_000;

#[derive(Clone)]
pub struct UserRegistry {
    credentials: Arc<Vec<Credential>>,
    users: Arc<Vec<Arc<UserState>>>,
}

#[derive(Clone)]
struct Credential {
    digest: [u8; 32],
    user: Arc<UserState>,
}

pub struct UserState {
    pub id: String,
    pub admin: bool,
    pub notes: Connection,
    pub oplog: Oplog,
    pub snapshot_dir: PathBuf,
    command_tx: mpsc::Sender<UserCommand>,
    operations_tx: broadcast::Sender<SequencedOp>,
    outbox_wake: Arc<Notify>,
}

enum UserCommand {
    Ingest {
        operations: Vec<Op>,
        response: oneshot::Sender<Result<Vec<SequencedOp>>>,
    },
    Snapshot {
        response: oneshot::Sender<Result<SyncSnapshot>>,
    },
    Bootstrap {
        snapshot: SyncSnapshot,
        response: oneshot::Sender<Result<SyncSnapshot>>,
    },
}

impl UserRegistry {
    pub async fn open(config: &ServerConfig, shutdown: CancellationToken) -> Result<Self> {
        tokio::fs::create_dir_all(&config.data_dir)
            .await
            .with_context(|| format!("creating data directory {}", config.data_dir.display()))?;
        let mut states = HashMap::new();
        let mut credentials = Vec::new();
        for user in &config.users {
            let state = Arc::new(
                UserState::open(
                    user,
                    &config.data_dir,
                    config.snapshot_every_ops,
                    shutdown.child_token(),
                )
                .await?,
            );
            states.insert(user.id.clone(), state.clone());
            for digest in user.token_digests()? {
                credentials.push(Credential {
                    digest,
                    user: state.clone(),
                });
            }
            spawn_local_outbox_publisher(state.clone(), shutdown.child_token());
        }
        tracing::info!(users = states.len(), "opened server user replicas");
        Ok(Self {
            credentials: Arc::new(credentials),
            users: Arc::new(states.into_values().collect()),
        })
    }

    pub fn authenticate(&self, token: &str) -> Option<Arc<UserState>> {
        let candidate: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let mut matched = None;
        for credential in self.credentials.iter() {
            if bool::from(candidate.ct_eq(&credential.digest)) {
                matched = Some(credential.user.clone());
            }
        }
        matched
    }

    pub fn users(&self) -> impl Iterator<Item = &Arc<UserState>> {
        self.users.iter()
    }
}

fn spawn_local_outbox_publisher(user: Arc<UserState>, shutdown: CancellationToken) {
    tokio::spawn(async move {
        loop {
            loop {
                let operations = tokio::select! {
                    () = shutdown.cancelled() => return,
                    result = notes_core::pending_outbox(&user.notes, 256) => match result {
                        Ok(operations) => operations,
                        Err(error) => {
                            tracing::warn!(user = %user.id, ?error, "reading server outbox failed");
                            break;
                        }
                    },
                };
                if operations.is_empty() {
                    break;
                }
                let batch_is_full = operations.len() == 256;
                let result = tokio::select! {
                    () = shutdown.cancelled() => return,
                    result = user.ingest(operations) => result,
                };
                if let Err(error) = result {
                    tracing::warn!(user = %user.id, ?error, "publishing server-authored operations failed");
                    break;
                }
                if !batch_is_full {
                    break;
                }
            }
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = user.outbox_wake.notified() => {},
                () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {},
            }
        }
    });
}

impl UserState {
    async fn open(
        user: &UserConfig,
        data_dir: &Path,
        snapshot_every_ops: u64,
        shutdown: CancellationToken,
    ) -> Result<Self> {
        let user_dir = data_dir.join("users").join(&user.id);
        let snapshot_dir = user_dir.join("snapshots");
        tokio::fs::create_dir_all(&snapshot_dir)
            .await
            .with_context(|| format!("creating user directory for {}", user.id))?;
        let notes = notes_core::db::open(user_dir.join("notes.db"))
            .await
            .with_context(|| format!("opening notes replica for {}", user.id))?;
        let oplog = Oplog::open(&user_dir.join("oplog.db"))
            .await
            .with_context(|| format!("opening oplog for {}", user.id))?;
        oplog.assert_gapless().await?;
        replay_oplog(&notes, &oplog).await?;

        let (command_tx, command_rx) = mpsc::channel(64);
        let (operations_tx, _) = broadcast::channel(1_024);
        let outbox_wake = Arc::new(Notify::new());
        tokio::spawn(run_user_actor(
            notes.clone(),
            oplog.clone(),
            snapshot_dir.clone(),
            snapshot_every_ops,
            operations_tx.clone(),
            command_rx,
            shutdown,
        ));
        Ok(Self {
            id: user.id.clone(),
            admin: user.admin,
            notes,
            oplog,
            snapshot_dir,
            command_tx,
            operations_tx,
            outbox_wake,
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

    pub async fn bootstrap(&self, snapshot: SyncSnapshot) -> Result<SyncSnapshot> {
        let (response, receiver) = oneshot::channel();
        self.command_tx
            .send(UserCommand::Bootstrap { snapshot, response })
            .await
            .context("user bootstrap actor stopped")?;
        receiver
            .await
            .context("user bootstrap actor dropped response")?
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SequencedOp> {
        self.operations_tx.subscribe()
    }

    pub fn notify_outbox(&self) {
        self.outbox_wake.notify_one();
    }
}

async fn replay_oplog(notes: &Connection, oplog: &Oplog) -> Result<()> {
    let mut cursor = sync_cursor(notes).await?;
    loop {
        let operations = oplog.ops_since(cursor, REPLAY_BATCH_SIZE).await?;
        if operations.is_empty() {
            return Ok(());
        }
        cursor = operations.last().expect("non-empty oplog batch").seq;
        apply_sequenced_batch(
            notes,
            operations
                .into_iter()
                .map(|operation| (operation.seq, operation.envelope))
                .collect(),
        )
        .await
        .with_context(|| format!("replaying server operations through sequence {cursor}"))?;
    }
}

async fn run_user_actor(
    notes: Connection,
    oplog: Oplog,
    snapshot_dir: PathBuf,
    snapshot_every_ops: u64,
    operations_tx: broadcast::Sender<SequencedOp>,
    mut commands: mpsc::Receiver<UserCommand>,
    shutdown: CancellationToken,
) {
    loop {
        let command = tokio::select! {
            () = shutdown.cancelled() => return,
            command = commands.recv() => command,
        };
        let Some(command) = command else { return };
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
            UserCommand::Bootstrap { snapshot, response } => {
                let result = bootstrap_replica(&notes, &oplog, &snapshot_dir, snapshot).await;
                let _ = response.send(result);
            }
        }
    }
}

async fn bootstrap_replica(
    notes: &Connection,
    oplog: &Oplog,
    snapshot_dir: &Path,
    mut snapshot: SyncSnapshot,
) -> Result<SyncSnapshot> {
    if snapshot.seq != 0 {
        bail!("bootstrap snapshot sequence must be zero");
    }
    if snapshot.page_identities.is_empty()
        && snapshot.pages.is_empty()
        && snapshot.blocks.is_empty()
        && snapshot.tombstones.is_empty()
        && snapshot.attachments.is_empty()
    {
        return Err(notes_core::CoreError::conflict(
            "an empty client cannot replace the server workspace identity",
        )
        .into());
    }
    if oplog.latest_seq().await? != 0 {
        return Err(notes_core::CoreError::conflict(
            "server workspace has already been initialized",
        )
        .into());
    }
    let current = export_sync_snapshot(notes, 0).await?;
    if !current.page_identities.is_empty()
        || !current.pages.is_empty()
        || !current.blocks.is_empty()
        || !current.tombstones.is_empty()
        || !current.attachments.is_empty()
    {
        return Err(notes_core::CoreError::conflict(
            "server workspace has already been initialized",
        )
        .into());
    }
    snapshot.seq = 0;
    import_sync_snapshot(notes, snapshot).await?;
    write_snapshot_at(notes, snapshot_dir, 0).await
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
    let mut appends = Vec::with_capacity(operations.len());
    for operation in operations {
        match oplog.append(operation).await {
            Ok(append) => appends.push(append),
            Err(error) => {
                for append in appends.iter().rev().filter(|append| append.inserted) {
                    oplog.remove_tail(&append.operation).await?;
                }
                return Err(error.context("appending ingested operation"));
            }
        }
    }
    let sequenced = appends
        .iter()
        .map(|append| (append.operation.seq, append.operation.envelope.clone()))
        .collect();
    if let Err(error) = apply_sequenced_batch(notes, sequenced).await {
        for append in appends.iter().rev().filter(|append| append.inserted) {
            oplog.remove_tail(&append.operation).await?;
        }
        return Err(error.context("applying ingested operations"));
    }
    let mut accepted = Vec::with_capacity(appends.len());
    for append in appends {
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

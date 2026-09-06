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
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tokio::sync::Notify;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

const REPLAY_BATCH_SIZE: usize = 1_000;
const OUTBOX_BATCH_SIZE: u32 = 256;

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

/// The server's own graph writes. Every mutation goes through
/// [`UserState::write`], so the operations behind it are in the oplog by the
/// time the tool that asked for it sees a result.
#[async_trait::async_trait]
impl notes_ai::agent::GraphWriter for UserState {
    async fn create_page(&self, title: String, markdown: String) -> Result<notes_core::db::Page> {
        self.write(|conn| async move {
            let page = notes_core::db::create_page(&conn, title).await?;
            if !markdown.trim().is_empty() {
                notes_core::db::create_block(
                    &conn,
                    page.uuid,
                    None,
                    None,
                    notes_core::BlockStyle::Paragraph,
                    markdown,
                )
                .await?;
            }
            Ok(page)
        })
        .await
    }
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

/// Sweeps up operations that were authored but not published — a writer that
/// failed midway, or a process that died between applying and publishing. The
/// primary path is [`UserState::write`], which publishes before it returns;
/// this loop only guarantees that nothing stays stranded when that path breaks.
fn spawn_local_outbox_publisher(user: Arc<UserState>, shutdown: CancellationToken) {
    tokio::spawn(async move {
        loop {
            let published = tokio::select! {
                () = shutdown.cancelled() => return,
                result = user.publish_authored_ops() => result,
            };
            if let Err(error) = published {
                tracing::warn!(user = %user.id, ?error, "publishing server-authored operations failed");
            }
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = user.outbox_wake.notified() => {},
                () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {},
            }
        }
    });
}

/// Counts operations this replica applied but never queued for publication.
///
/// On the server that count must be zero. An authored operation enters the
/// outbox in the same transaction that applies it and leaves once the oplog has
/// given it a seq, so a row with neither is a write the materialized state
/// remembers and the log does not. It cannot be recovered as an operation
/// either: only the envelope carries the HLC, and it was never persisted.
async fn audit_unpublished_ops(notes: &Connection, user_id: &str) -> Result<()> {
    let stranded = notes
        .call(|database| {
            database.query_row(
                "SELECT COUNT(*) FROM applied_ops
                 WHERE seq IS NULL AND op_id NOT IN (SELECT op_id FROM sync_outbox)",
                [],
                |row| row.get::<_, i64>(0),
            )
        })
        .await
        .context("auditing unpublished operations")?;
    if stranded > 0 {
        tracing::error!(
            user = %user_id,
            stranded,
            "replica holds operations that never reached the oplog: they are invisible to \
             every device, absent from the log a snapshot claims to represent, and cannot be \
             reconstructed as operations"
        );
    }
    Ok(())
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
        notes_core::set_replica_role(&notes, notes_core::ReplicaRole::Server)
            .await
            .with_context(|| format!("marking the replica of {} as server-owned", user.id))?;
        replay_oplog(&notes, &oplog).await?;
        audit_unpublished_ops(&notes, &user.id).await?;

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

    /// Runs a write that authors operations on this replica, and publishes them
    /// before returning.
    ///
    /// Every server-side writer must come through here. On the server `notes`
    /// is a materialization of the oplog and nothing else, so a mutation that
    /// only touches it has changed what this process sees and nothing that any
    /// device, snapshot or restart will agree with. Publishing before returning
    /// also gives callers read-your-writes: a client polling `/v1/ops` the
    /// moment this resolves already sees the operations.
    pub async fn write<F, Fut, T>(&self, mutate: F) -> Result<T>
    where
        F: FnOnce(Connection) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        // A mutation that fails partway can still have committed earlier
        // operations, so publish regardless of the outcome and let the caller's
        // error win when both fail.
        let mutated = mutate(self.notes.clone()).await;
        let published = self.publish_authored_ops().await;
        match (mutated, published) {
            (Ok(value), Ok(())) => Ok(value),
            (Ok(_), Err(error)) => Err(error.context("publishing a server-authored write")),
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(publish_error)) => {
                tracing::warn!(
                    user = %self.id,
                    error = ?publish_error,
                    "publishing after a failed write also failed"
                );
                Err(error)
            }
        }
    }

    /// Moves every authored-but-unsequenced operation into the oplog.
    ///
    /// Idempotent and safe to run concurrently with itself: `ingest` skips
    /// operations already applied, stamps their seq, and clears them from the
    /// outbox.
    pub(crate) async fn publish_authored_ops(&self) -> Result<()> {
        loop {
            let operations = notes_core::pending_outbox(&self.notes, OUTBOX_BATCH_SIZE)
                .await
                .context("reading the server outbox")?;
            if operations.is_empty() {
                return Ok(());
            }
            let batch_is_full = operations.len() as u32 == OUTBOX_BATCH_SIZE;
            self.ingest(operations).await?;
            if !batch_is_full {
                return Ok(());
            }
        }
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

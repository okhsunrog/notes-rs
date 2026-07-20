use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use notes_blob::{BlobHash, BlobStore, BlobStoreError};
use notes_core::{Connection, acknowledge_server_ops, apply_sequenced_batch, export_sync_snapshot};
use notes_protocol::{ClientMessage, SequencedOp, ServerErrorCode, ServerMessage};
use notes_sync::{HttpTransport, SyncClient, SyncSnapshot, SyncTransport, TransportError};
use serde::Serialize;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tauri::AppHandle;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;

const SYNC_BATCH_SIZE: u32 = 256;
const MAX_ATTACHMENT_SIZE: u64 = 100 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
enum SyncSessionError {
    #[error(
        "local and server workspace contain different data; export one workspace and reset the other before enabling sync"
    )]
    WorkspaceConflict,
    #[error("sync server rejected the session: {0}")]
    ServerConflict(String),
}

fn is_permanent_failure(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<TransportError>()
        .is_some_and(TransportError::is_permanent)
        || error.downcast_ref::<SyncSessionError>().is_some()
        || matches!(
            error.downcast_ref::<notes_core::CoreError>(),
            Some(notes_core::CoreError::Conflict(_) | notes_core::CoreError::SyncConflict(_))
        )
}

struct DesktopTransport<'a> {
    http: &'a HttpTransport,
    blob_store: &'a BlobStore,
}

#[async_trait]
impl SyncTransport for DesktopTransport<'_> {
    async fn ops_since(&mut self, since: u64, limit: usize) -> Result<Vec<SequencedOp>> {
        self.http.ops_since(since, limit).await
    }

    async fn push(&mut self, operations: Vec<notes_core::Op>) -> Result<Vec<SequencedOp>> {
        self.http.push(operations).await
    }

    async fn prepare_push(&mut self, operations: &[notes_core::Op]) -> Result<()> {
        upload_operation_blobs(self.http, self.blob_store, operations).await
    }

    async fn prepare_pull(&mut self, operations: &[SequencedOp]) -> Result<()> {
        for operation in operations {
            download_operation_blob(self.http, self.blob_store, &operation.envelope).await?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SyncConnectionState {
    Disabled,
    Connecting,
    Syncing,
    Online,
    Offline,
    Conflict,
    Error,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub state: SyncConnectionState,
    pub server_url: Option<url::Url>,
    pub last_server_seq: u64,
    pub pending_operations: u32,
    pub message: Option<String>,
}

pub struct SyncRuntime {
    pub status: Arc<RwLock<SyncStatus>>,
    retry: watch::Sender<u64>,
}

impl SyncRuntime {
    pub fn disabled() -> Self {
        let (retry, _) = watch::channel(0);
        Self {
            status: Arc::new(RwLock::new(SyncStatus {
                state: SyncConnectionState::Disabled,
                server_url: None,
                last_server_seq: 0,
                pending_operations: 0,
                message: None,
            })),
            retry,
        }
    }

    pub fn request_retry(&self) {
        self.retry.send_modify(|generation| {
            *generation = generation.wrapping_add(1);
        });
    }

    pub fn sync_settings_changed(&self) {
        self.request_retry();
    }

    pub fn subscribe_retries(&self) -> watch::Receiver<u64> {
        self.retry.subscribe()
    }
}

pub fn spawn_worker(
    app: AppHandle,
    connection: Connection,
    server_url: url::Url,
    token: String,
    blob_store: BlobStore,
    status: Arc<RwLock<SyncStatus>>,
    mut retries: watch::Receiver<u64>,
) {
    tauri::async_runtime::spawn(async move {
        let mut credentials = Some((server_url, token));
        'configuration: loop {
            let Some((server_url, token)) = credentials.take() else {
                replace_status(
                    &app,
                    &status,
                    SyncStatus {
                        state: SyncConnectionState::Disabled,
                        server_url: None,
                        last_server_seq: notes_core::sync_cursor(&connection).await.unwrap_or(0),
                        pending_operations: 0,
                        message: None,
                    },
                );
                return;
            };
            let transport = match HttpTransport::new(server_url.clone(), token) {
                Ok(transport) => transport,
                Err(error) => {
                    replace_status(
                        &app,
                        &status,
                        SyncStatus {
                            state: SyncConnectionState::Error,
                            server_url: Some(server_url),
                            last_server_seq: 0,
                            pending_operations: 0,
                            message: Some(error.to_string()),
                        },
                    );
                    return;
                }
            };
            if let Err(error) =
                notes_core::configure_sync(&connection, transport.base_url().as_str()).await
            {
                replace_status(
                    &app,
                    &status,
                    SyncStatus {
                        state: SyncConnectionState::Error,
                        server_url: Some(server_url),
                        last_server_seq: 0,
                        pending_operations: 0,
                        message: Some(error.to_string()),
                    },
                );
                return;
            }

            let mut delay = Duration::from_secs(1);
            loop {
                let retry_generation = *retries.borrow_and_update();
                set_connection_state(
                    &app,
                    &status,
                    &connection,
                    &server_url,
                    SyncConnectionState::Connecting,
                    None,
                )
                .await;
                let result = synchronize_session(
                    &app,
                    &connection,
                    &transport,
                    &status,
                    &server_url,
                    &blob_store,
                )
                .await;
                let message = result
                    .as_ref()
                    .err()
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "sync connection closed".into());
                let failure_state = result
                    .as_ref()
                    .err()
                    .map_or(SyncConnectionState::Offline, failure_state);
                if failure_state == SyncConnectionState::Conflict {
                    set_connection_state(
                        &app,
                        &status,
                        &connection,
                        &server_url,
                        failure_state,
                        Some(format!(
                            "{message}; update sync settings or retry explicitly"
                        )),
                    )
                    .await;
                    if !wait_for_retry(&mut retries, retry_generation).await {
                        return;
                    }
                    credentials = match crate::settings::runtime(&app)
                        .and_then(|settings| settings.sync_credentials())
                    {
                        Ok(credentials) => credentials,
                        Err(error) => {
                            set_connection_state(
                                &app,
                                &status,
                                &connection,
                                &server_url,
                                SyncConnectionState::Error,
                                Some(error.to_string()),
                            )
                            .await;
                            return;
                        }
                    };
                    continue 'configuration;
                }
                set_connection_state(
                    &app,
                    &status,
                    &connection,
                    &server_url,
                    failure_state,
                    Some(message),
                )
                .await;
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(30));
            }
        }
    });
}

fn failure_state(error: &anyhow::Error) -> SyncConnectionState {
    if matches!(
        error.downcast_ref::<SyncSessionError>(),
        Some(SyncSessionError::WorkspaceConflict)
    ) {
        SyncConnectionState::Conflict
    } else if is_permanent_failure(error) {
        SyncConnectionState::Error
    } else {
        SyncConnectionState::Offline
    }
}

async fn wait_for_retry(retries: &mut watch::Receiver<u64>, generation: u64) -> bool {
    while *retries.borrow_and_update() == generation {
        if retries.changed().await.is_err() {
            return false;
        }
    }
    true
}

async fn synchronize_session(
    app: &AppHandle,
    connection: &Connection,
    transport: &HttpTransport,
    status: &Arc<RwLock<SyncStatus>>,
    server_url: &url::Url,
    blob_store: &BlobStore,
) -> Result<()> {
    transport.health().await?;
    initialize_replica(app, connection, transport, blob_store).await?;
    let server_info = transport.info().await?;
    if notes_core::db::workspace_uuid(connection).await? != server_info.workspace_uuid {
        return Err(SyncSessionError::WorkspaceConflict.into());
    }
    set_connection_state(
        app,
        status,
        connection,
        server_url,
        SyncConnectionState::Syncing,
        None,
    )
    .await;
    synchronize_http(app, connection, transport, blob_store).await?;
    let cursor = notes_core::sync_cursor(connection).await?;
    let mut socket = transport.connect(cursor).await?;
    set_connection_state(
        app,
        status,
        connection,
        server_url,
        SyncConnectionState::Online,
        None,
    )
    .await;
    let mut drain = tokio::time::interval(Duration::from_millis(400));
    drain.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            incoming = socket.next() => {
                let Some(incoming) = incoming else { bail!("sync websocket closed"); };
                match incoming? {
                    Message::Text(text) => {
                        let message: ServerMessage = serde_json::from_str(&text)
                            .context("decoding sync websocket message")?;
                        handle_server_message(app, connection, transport, blob_store, message).await?;
                        set_connection_state(
                            app,
                            status,
                            connection,
                            server_url,
                            SyncConnectionState::Online,
                            None,
                        ).await;
                    }
                    Message::Ping(payload) => socket.send(Message::Pong(payload)).await?,
                    Message::Close(_) => bail!("sync websocket closed"),
                    _ => {}
                }
            }
            _ = drain.tick() => {
                let pending = notes_core::pending_outbox(connection, SYNC_BATCH_SIZE).await?;
                if !pending.is_empty() {
                    upload_operation_blobs(transport, blob_store, &pending).await?;
                    let message = serde_json::to_string(&ClientMessage::Push { ops: pending })?;
                    socket.send(Message::Text(message.into())).await?;
                }
            }
        }
    }
}

async fn initialize_replica(
    app: &AppHandle,
    connection: &Connection,
    transport: &HttpTransport,
    blob_store: &BlobStore,
) -> Result<()> {
    let local_workspace_uuid = notes_core::db::workspace_uuid(connection).await?;
    if let Some(bound_workspace_uuid) = notes_core::sync_bound_workspace(connection).await? {
        let server_workspace_uuid = transport.info().await?.workspace_uuid;
        if bound_workspace_uuid != local_workspace_uuid
            || bound_workspace_uuid != server_workspace_uuid
        {
            return Err(SyncSessionError::WorkspaceConflict.into());
        }
        return Ok(());
    }

    let server = transport.snapshot().await?;
    let server_seq = server.seq;
    let (local, pending) = capture_local_sync_state(connection, server.seq).await?;
    let server_empty = snapshot_is_empty(&server);
    let local_empty = snapshot_is_empty(&local);
    match (server_empty, local_empty) {
        (true, false) => {
            upload_snapshot_blobs(transport, blob_store, &local).await?;
            transport.bootstrap(local.clone()).await?;
        }
        (false, true) => {
            download_snapshot_blobs(transport, blob_store, &server).await?;
            notes_core::import_sync_snapshot(connection, server.clone()).await?;
            emit_workspace_changed(app);
        }
        (false, false) => {
            if local.workspace_uuid != server.workspace_uuid
                || (local != server
                    && !outbox_explains_snapshot_difference(&server, &local, &pending).await?)
            {
                return Err(SyncSessionError::WorkspaceConflict.into());
            }
            download_snapshot_blobs(transport, blob_store, &server).await?;
        }
        (true, true) => {
            // The server's durable empty workspace is canonical. This prevents
            // successive fresh clients from replacing its Journal namespace.
            notes_core::import_sync_snapshot(connection, server).await?;
        }
    }

    let server_info = transport.info().await?;
    let local_workspace_uuid = notes_core::db::workspace_uuid(connection).await?;
    if local_workspace_uuid != server_info.workspace_uuid {
        return Err(SyncSessionError::WorkspaceConflict.into());
    }
    notes_core::bind_sync_workspace(
        connection,
        transport.base_url().as_str(),
        local_workspace_uuid,
        server_seq,
    )
    .await?;
    Ok(())
}

async fn capture_local_sync_state(
    connection: &Connection,
    seq: u64,
) -> Result<(SyncSnapshot, Vec<notes_core::Op>)> {
    for _ in 0..3 {
        let before = notes_core::pending_outbox(connection, u32::MAX).await?;
        let snapshot = export_sync_snapshot(connection, seq).await?;
        let after = notes_core::pending_outbox(connection, u32::MAX).await?;
        if before == after {
            return Ok((snapshot, after));
        }
    }
    bail!("local state kept changing while the sync baseline was inspected")
}

async fn outbox_explains_snapshot_difference(
    server: &SyncSnapshot,
    local: &SyncSnapshot,
    pending: &[notes_core::Op],
) -> Result<bool> {
    if pending.is_empty() {
        return Ok(false);
    }
    Ok(notes_core::project_sync_snapshot(server.clone(), pending).await? == *local)
}

fn snapshot_is_empty(snapshot: &SyncSnapshot) -> bool {
    snapshot.page_identities.is_empty()
        && snapshot.pages.is_empty()
        && snapshot.page_aliases.is_empty()
        && snapshot.blocks.is_empty()
        && snapshot.structures.is_empty()
        && snapshot.tombstones.is_empty()
        && snapshot.attachments.is_empty()
}

async fn synchronize_http(
    app: &AppHandle,
    connection: &Connection,
    transport: &HttpTransport,
    blob_store: &BlobStore,
) -> Result<()> {
    let mut transport = DesktopTransport {
        http: transport,
        blob_store,
    };
    let stats = SyncClient::new(connection.clone())
        .with_batch_size(SYNC_BATCH_SIZE)
        .sync_until_idle(&mut transport)
        .await?;
    let operations = stats
        .applied_operations
        .iter()
        .map(|applied| applied.operation.kind.clone())
        .collect::<Vec<_>>();
    let graph_changed = stats
        .applied_operations
        .iter()
        .filter(|applied| applied.graph_changed)
        .filter_map(|applied| match &applied.operation.kind {
            notes_core::OpKind::BlockSetMarkdown(payload) => Some(payload.uuid),
            _ => None,
        })
        .collect::<Vec<_>>();
    crate::commands::emit_events_for_ops(app, connection, &operations, &graph_changed).await;
    Ok(())
}

async fn handle_server_message(
    app: &AppHandle,
    connection: &Connection,
    transport: &HttpTransport,
    blob_store: &BlobStore,
    message: ServerMessage,
) -> Result<()> {
    match message {
        ServerMessage::Ops { ops } => {
            apply_server_operations(app, connection, transport, blob_store, ops).await
        }
        ServerMessage::Ack { ops } => {
            acknowledge_server_ops(
                connection,
                ops.into_iter()
                    .map(|operation| (operation.envelope.op_id, operation.seq))
                    .collect(),
            )
            .await?;
            Ok(())
        }
        ServerMessage::Pong => Ok(()),
        ServerMessage::Error {
            code: ServerErrorCode::Conflict,
            message,
        } => Err(SyncSessionError::ServerConflict(message).into()),
        ServerMessage::Error { code, message } => bail!("sync server error {code}: {message}"),
    }
}

async fn apply_server_operations(
    app: &AppHandle,
    connection: &Connection,
    transport: &HttpTransport,
    blob_store: &BlobStore,
    operations: Vec<SequencedOp>,
) -> Result<()> {
    for operation in &operations {
        download_operation_blob(transport, blob_store, &operation.envelope).await?;
    }
    let sequenced = operations
        .iter()
        .map(|operation| (operation.seq, operation.envelope.clone()))
        .collect();
    let outcomes = apply_sequenced_batch(connection, sequenced).await?;
    let mut applied = Vec::new();
    let mut graph_changed = Vec::new();
    for (operation, outcome) in operations.into_iter().zip(outcomes) {
        if !outcome.applied {
            continue;
        }
        if outcome.graph_changed
            && let notes_core::OpKind::BlockSetMarkdown(payload) = &operation.envelope.kind
        {
            graph_changed.push(payload.uuid);
        }
        applied.push(operation.envelope.kind);
    }
    crate::commands::emit_events_for_ops(app, connection, &applied, &graph_changed).await;
    Ok(())
}

async fn upload_operation_blobs(
    transport: &HttpTransport,
    blob_store: &BlobStore,
    operations: &[notes_core::Op],
) -> Result<()> {
    for operation in operations {
        if let notes_core::OpKind::AttachmentAdd(attachment) = &operation.kind {
            upload_blob(transport, blob_store, attachment.blob_hash, attachment.size).await?;
        }
    }
    Ok(())
}

async fn download_operation_blob(
    transport: &HttpTransport,
    blob_store: &BlobStore,
    operation: &notes_core::Op,
) -> Result<()> {
    if let notes_core::OpKind::AttachmentAdd(attachment) = &operation.kind {
        download_blob(transport, blob_store, attachment.blob_hash, attachment.size).await?;
    }
    Ok(())
}

async fn upload_snapshot_blobs(
    transport: &HttpTransport,
    blob_store: &BlobStore,
    snapshot: &SyncSnapshot,
) -> Result<()> {
    for attachment in snapshot
        .attachments
        .iter()
        .filter(|attachment| attachment.present)
    {
        let size = attachment
            .size
            .context("snapshot attachment is missing its size")?;
        upload_blob(transport, blob_store, attachment.blob_hash, size).await?;
    }
    Ok(())
}

async fn download_snapshot_blobs(
    transport: &HttpTransport,
    blob_store: &BlobStore,
    snapshot: &SyncSnapshot,
) -> Result<()> {
    for attachment in snapshot
        .attachments
        .iter()
        .filter(|attachment| attachment.present)
    {
        let size = attachment
            .size
            .context("snapshot attachment is missing its size")?;
        download_blob(transport, blob_store, attachment.blob_hash, size).await?;
    }
    Ok(())
}

async fn upload_blob(
    transport: &HttpTransport,
    blob_store: &BlobStore,
    hash: BlobHash,
    expected_size: u64,
) -> Result<()> {
    anyhow::ensure!(
        expected_size <= MAX_ATTACHMENT_SIZE,
        "attachment blob {hash} exceeds the {MAX_ATTACHMENT_SIZE}-byte limit"
    );
    let stored = blob_store.open_verified(hash, MAX_ATTACHMENT_SIZE)?;
    anyhow::ensure!(
        stored.blob.size == expected_size,
        "attachment blob {hash} has size {}, expected {expected_size}",
        stored.blob.size
    );
    transport.upload_blob_file(hash, stored.into_file()).await
}

async fn download_blob(
    transport: &HttpTransport,
    blob_store: &BlobStore,
    hash: BlobHash,
    expected_size: u64,
) -> Result<()> {
    anyhow::ensure!(
        expected_size <= MAX_ATTACHMENT_SIZE,
        "attachment blob {hash} exceeds the {MAX_ATTACHMENT_SIZE}-byte limit"
    );
    match blob_store.open_verified(hash, MAX_ATTACHMENT_SIZE) {
        Ok(stored) => {
            anyhow::ensure!(
                stored.blob.size == expected_size,
                "attachment blob {hash} has size {}, expected {expected_size}",
                stored.blob.size
            );
            return Ok(());
        }
        Err(BlobStoreError::NotFound { .. }) => {}
        Err(error) => return Err(error.into()),
    }

    let staging_directory = blob_store.root().join(".blob-downloads");
    tokio::fs::create_dir_all(&staging_directory).await?;
    let staging = staging_directory.join(format!("{}.download", uuid::Uuid::now_v7()));
    let result = async {
        transport
            .download_blob(hash, &staging, MAX_ATTACHMENT_SIZE)
            .await?;
        let installed = blob_store.install_file(&staging, hash, MAX_ATTACHMENT_SIZE)?;
        anyhow::ensure!(
            installed.blob.size == expected_size,
            "downloaded attachment blob {hash} has size {}, expected {expected_size}",
            installed.blob.size
        );
        Ok::<(), anyhow::Error>(())
    }
    .await;
    match tokio::fs::remove_file(&staging).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) if result.is_err() => {
            tracing::warn!(path = %staging.display(), %error, "failed to clean staged blob after download failure");
        }
        Err(error) => return Err(error.into()),
    }
    result
}

async fn set_connection_state(
    app: &AppHandle,
    status: &Arc<RwLock<SyncStatus>>,
    connection: &Connection,
    server_url: &url::Url,
    state: SyncConnectionState,
    message: Option<String>,
) {
    let cursor = notes_core::sync_cursor(connection)
        .await
        .unwrap_or_default();
    let pending = notes_core::pending_outbox(connection, u32::MAX)
        .await
        .map(|operations| operations.len().try_into().unwrap_or(u32::MAX))
        .unwrap_or_default();
    replace_status(
        app,
        status,
        SyncStatus {
            state,
            server_url: Some(server_url.clone()),
            last_server_seq: cursor,
            pending_operations: pending,
            message,
        },
    );
}

fn replace_status(app: &AppHandle, status: &Arc<RwLock<SyncStatus>>, next: SyncStatus) {
    *status.write().unwrap_or_else(|error| error.into_inner()) = next;
    crate::commands::emit_domain(app, crate::commands::DomainEvent::SyncStatusChanged);
}

fn emit_workspace_changed(app: &AppHandle) {
    crate::commands::emit_domain(app, crate::commands::DomainEvent::WorkspaceChanged);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn workspace_conflict_parks_until_settings_or_explicit_retry_changes_generation() {
        let conflict = anyhow::Error::from(SyncSessionError::WorkspaceConflict);
        assert_eq!(failure_state(&conflict), SyncConnectionState::Conflict);

        let runtime = SyncRuntime::disabled();
        let mut retries = runtime.subscribe_retries();
        let generation = *retries.borrow_and_update();
        let mut parked =
            tokio::spawn(async move { wait_for_retry(&mut retries, generation).await });
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut parked)
                .await
                .is_err(),
            "workspace conflict must park the loop",
        );

        runtime.sync_settings_changed();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), parked)
                .await
                .expect("retry generation resumes the parked loop")
                .expect("parked task completes")
        );
    }

    #[test]
    fn sequence_gaps_are_retriable_but_other_sync_conflicts_are_permanent() {
        let gap = anyhow::Error::from(notes_core::CoreError::sync_sequence_gap(1_001, 1_501));
        assert!(!is_permanent_failure(&gap));

        let conflict = anyhow::Error::from(notes_core::CoreError::sync_conflict(
            "operation id was reused",
        ));
        assert!(is_permanent_failure(&conflict));
    }

    #[test]
    fn empty_snapshot_has_no_source_records() {
        assert!(snapshot_is_empty(&SyncSnapshot {
            format_version: notes_sync::FORMAT_VERSION,
            workspace_uuid: uuid::Uuid::from_u128(1),
            seq: 0,
            page_identities: Vec::new(),
            pages: Vec::new(),
            page_aliases: Vec::new(),
            blocks: Vec::new(),
            structures: Vec::new(),
            tombstones: Vec::new(),
            attachments: Vec::new(),
        }));
    }

    #[tokio::test]
    async fn seq_zero_snapshot_difference_is_accepted_only_when_the_outbox_explains_it() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let connection = notes_core::db::open(directory.path().join("notes.db"))
            .await
            .expect("notes database");
        let page = notes_core::db::create_page(&connection, "Layout toggle".into())
            .await
            .expect("create page");
        let server = notes_core::export_sync_snapshot(&connection, 0)
            .await
            .expect("server baseline");
        notes_core::configure_sync(&connection, "https://notes.example.test/")
            .await
            .expect("configure sync");

        notes_core::db::set_page_layout(&connection, page.uuid, notes_core::PageLayout::Document)
            .await
            .expect("switch to document");
        notes_core::db::set_page_layout(&connection, page.uuid, notes_core::PageLayout::Outline)
            .await
            .expect("switch back to outline");
        let local = notes_core::export_sync_snapshot(&connection, 0)
            .await
            .expect("local snapshot");

        assert_ne!(
            local, server,
            "the newer layout HLC must change the snapshot"
        );
        assert_eq!(
            notes_core::pending_outbox(&connection, u32::MAX)
                .await
                .expect("pending outbox")
                .len(),
            2
        );
        let pending = notes_core::pending_outbox(&connection, u32::MAX)
            .await
            .expect("pending outbox");
        assert!(
            outbox_explains_snapshot_difference(&server, &local, &pending)
                .await
                .expect("project outbox")
        );
        assert!(
            !outbox_explains_snapshot_difference(&server, &local, &pending[..1])
                .await
                .expect("reject incomplete projection")
        );

        notes_core::bind_sync_workspace(
            &connection,
            "https://notes.example.test/",
            server.workspace_uuid,
            server.seq,
        )
        .await
        .expect("bind verified baseline");
        let mut transport = notes_sync::LoopbackServer::new();
        let stats = notes_sync::SyncClient::new(connection.clone())
            .sync_until_idle(&mut transport)
            .await
            .expect("push recovered outbox");
        assert_eq!(stats.pushed, 2);
        assert_eq!(stats.cursor, 2);
        assert_eq!(transport.log().len(), 2);
        assert!(
            notes_core::pending_outbox(&connection, u32::MAX)
                .await
                .expect("drained outbox")
                .is_empty()
        );
    }
}

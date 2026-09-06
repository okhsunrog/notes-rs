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
use tauri::Manager;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;

const SYNC_BATCH_SIZE: u32 = 256;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(45);
const HEARTBEAT_SEND_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_ATTACHMENT_SIZE: u64 = 100 * 1024 * 1024;

#[derive(Default)]
struct Heartbeat {
    sent_at: Option<tokio::time::Instant>,
}

impl Heartbeat {
    fn should_send(&self, now: tokio::time::Instant) -> Result<bool> {
        if let Some(sent_at) = self.sent_at {
            if now.duration_since(sent_at) >= HEARTBEAT_TIMEOUT {
                bail!("sync heartbeat timed out: server did not respond to ping");
            }
            Ok(false)
        } else {
            Ok(true)
        }
    }

    fn sent(&mut self, now: tokio::time::Instant) {
        self.sent_at = Some(now);
    }

    fn pong(&mut self) {
        self.sent_at = None;
    }
}

#[derive(Debug, thiserror::Error)]
enum SyncSessionError {
    #[error(
        "local and server workspace contain different data; export one workspace and reset the other before enabling sync"
    )]
    WorkspaceConflict,
    #[error("sync server rejected the session: {0}")]
    ServerConflict(String),
    #[error(
        "the sync server writes format {server_version}; this app reads {}. Update the app to keep syncing",
        notes_sync::FORMAT_VERSION
    )]
    UpdateRequired { server_version: u32 },
}

/// The server refused the exchange because this build is too old. Retrying the
/// same binary can only fail again, so the worker parks instead of backing off.
///
/// Either the announced version was ahead of ours, or an operation endpoint
/// answered with the dedicated refusal code — the case an already-shipped old
/// client hits, where nothing but the code identifies the reason.
fn update_required(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<SyncSessionError>(),
        Some(SyncSessionError::UpdateRequired { .. })
    ) || matches!(
        notes_sync::transport_error(error),
        Some(TransportError::Http {
            code: Some(notes_protocol::ApiErrorCode::FormatUnsupported),
            ..
        })
    )
}

fn announced_server_version(error: &anyhow::Error) -> Option<u32> {
    match error.downcast_ref::<SyncSessionError>() {
        Some(SyncSessionError::UpdateRequired { server_version }) => Some(*server_version),
        _ => None,
    }
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
    connection: &'a Connection,
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
        upload_operation_blobs(self.connection, self.http, self.blob_store, operations).await
    }

    async fn prepare_pull(&mut self, operations: &[SequencedOp]) -> Result<()> {
        for operation in operations {
            download_operation_blob(
                self.connection,
                self.http,
                self.blob_store,
                &operation.envelope,
            )
            .await?;
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
    /// Terminal: the server writes a sync format this build cannot read.
    UpdateRequired,
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
    /// Sync format the server writes, known only once it has refused this
    /// build. `None` at every other time, including a plain offline server.
    pub server_format_version: Option<u32>,
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
                server_format_version: None,
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
                        server_format_version: None,
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
                            server_format_version: None,
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
                        server_format_version: None,
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
                let result = tokio::select! {
                    result = synchronize_session(
                        &app,
                        &connection,
                        &transport,
                        &status,
                        &server_url,
                        &blob_store,
                    ) => result,
                    changed = retries.changed() => {
                        if changed.is_err() {
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
                };
                let message = result
                    .as_ref()
                    .err()
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "sync connection closed".into());
                let failure_state = result
                    .as_ref()
                    .err()
                    .map_or(SyncConnectionState::Offline, failure_state);
                // Both parked states stop the backoff loop: neither an
                // identity conflict nor an out-of-date build resolves itself by
                // reconnecting. They wait for changed settings or an explicit
                // retry instead.
                if matches!(
                    failure_state,
                    SyncConnectionState::Conflict | SyncConnectionState::UpdateRequired
                ) {
                    let server_format_version =
                        result.as_ref().err().and_then(announced_server_version);
                    let parked_message = if failure_state == SyncConnectionState::UpdateRequired {
                        message
                    } else {
                        format!("{message}; update sync settings or retry explicitly")
                    };
                    set_connection_detail(
                        &app,
                        &status,
                        &connection,
                        &server_url,
                        failure_state,
                        Some(parked_message),
                        server_format_version,
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
                if wait_for_retry_during_backoff(&mut retries, delay).await {
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
                delay = (delay * 2).min(Duration::from_secs(30));
            }
        }
    });
}

fn failure_state(error: &anyhow::Error) -> SyncConnectionState {
    if update_required(error) {
        SyncConnectionState::UpdateRequired
    } else if matches!(
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

async fn wait_for_retry_during_backoff(
    retries: &mut watch::Receiver<u64>,
    delay: Duration,
) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(delay) => false,
        changed = retries.changed() => changed.is_ok(),
    }
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
    // Checked before any exchange: a server writing a newer format would hand
    // this build operations it cannot decode, and the replica initialization
    // below would import them.
    let announced = transport.info().await?;
    if announced.format_version > notes_sync::FORMAT_VERSION {
        return Err(SyncSessionError::UpdateRequired {
            server_version: announced.format_version,
        }
        .into());
    }
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
    let mut heartbeat_tick = tokio::time::interval_at(
        tokio::time::Instant::now() + HEARTBEAT_INTERVAL,
        HEARTBEAT_INTERVAL,
    );
    heartbeat_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut heartbeat = Heartbeat::default();

    loop {
        tokio::select! {
            incoming = socket.next() => {
                let Some(incoming) = incoming else { bail!("sync websocket closed"); };
                match incoming? {
                    Message::Text(text) => {
                        let message: ServerMessage = serde_json::from_str(&text)
                            .context("decoding sync websocket message")?;
                        if matches!(message, ServerMessage::Pong) {
                            heartbeat.pong();
                            // Heartbeats must not acknowledge outbox items or change sync state.
                            continue;
                        }
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
            _ = heartbeat_tick.tick() => {
                if heartbeat.should_send(tokio::time::Instant::now())? {
                    let message = serde_json::to_string(&ClientMessage::Ping)?;
                    tokio::time::timeout(
                        HEARTBEAT_SEND_TIMEOUT,
                        socket.send(Message::Text(message.into())),
                    ).await.context("sending sync heartbeat timed out")??;
                    heartbeat.sent(tokio::time::Instant::now());
                }
            }
            _ = drain.tick() => {
                let pending = notes_core::pending_outbox(connection, SYNC_BATCH_SIZE).await?;
                if !pending.is_empty() {
                    if status
                        .read()
                        .unwrap_or_else(|error| error.into_inner())
                        .state
                        != SyncConnectionState::Syncing
                    {
                        set_connection_state(
                            app,
                            status,
                            connection,
                            server_url,
                            SyncConnectionState::Syncing,
                            None,
                        )
                        .await;
                    }
                    upload_operation_blobs(connection,transport, blob_store, &pending).await?;
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
            upload_snapshot_blobs(connection, transport, blob_store, &local).await?;
            transport.bootstrap(local.clone()).await?;
        }
        (false, true) => {
            download_snapshot_blobs(connection, transport, blob_store, &server).await?;
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
            download_snapshot_blobs(connection, transport, blob_store, &server).await?;
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
        connection,
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
        download_operation_blob(connection, transport, blob_store, &operation.envelope).await?;
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
    connection: &Connection,
    transport: &HttpTransport,
    blob_store: &BlobStore,
    operations: &[notes_core::Op],
) -> Result<()> {
    for operation in operations {
        if let notes_core::OpKind::InkPublish(p) = &operation.kind {
            transport.upload_ink_graph(connection, p.root_hash).await?;
        }
        if let notes_core::OpKind::AttachmentAdd(attachment) = &operation.kind {
            upload_blob(transport, blob_store, attachment.blob_hash, attachment.size).await?;
        }
    }
    Ok(())
}

async fn download_operation_blob(
    connection: &Connection,
    transport: &HttpTransport,
    blob_store: &BlobStore,
    operation: &notes_core::Op,
) -> Result<()> {
    if let notes_core::OpKind::InkPublish(p) = &operation.kind {
        transport
            .download_ink_graph(connection, p.root_hash)
            .await?;
    }
    if let notes_core::OpKind::AttachmentAdd(attachment) = &operation.kind {
        download_blob(transport, blob_store, attachment.blob_hash, attachment.size).await?;
    }
    Ok(())
}

async fn upload_snapshot_blobs(
    connection: &Connection,
    transport: &HttpTransport,
    blob_store: &BlobStore,
    snapshot: &SyncSnapshot,
) -> Result<()> {
    for version in &snapshot.ink_versions {
        transport
            .upload_ink_graph(connection, version.publication.root_hash)
            .await?;
    }
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
    connection: &Connection,
    transport: &HttpTransport,
    blob_store: &BlobStore,
    snapshot: &SyncSnapshot,
) -> Result<()> {
    for version in &snapshot.ink_versions {
        transport
            .download_ink_graph(connection, version.publication.root_hash)
            .await?;
    }
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
    set_connection_detail(app, status, connection, server_url, state, message, None).await;
}

async fn set_connection_detail(
    app: &AppHandle,
    status: &Arc<RwLock<SyncStatus>>,
    connection: &Connection,
    server_url: &url::Url,
    state: SyncConnectionState,
    message: Option<String>,
    server_format_version: Option<u32>,
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
            server_format_version,
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

/// Best-effort upload on orderly exit. The outbox remains authoritative on
/// timeout/offline failure; shutting down never waits indefinitely for a server.
pub(crate) async fn finish_before_exit(app: &AppHandle) {
    let Some(state) = app.try_state::<crate::commands::AppState>() else {
        return;
    };
    let attempt = async {
        let Some((url, token)) = crate::settings::runtime(app)?.sync_credentials()? else {
            return Ok::<(), anyhow::Error>(());
        };
        let transport = HttpTransport::new(url, token)?;
        loop {
            let pending = notes_core::pending_outbox(&state.conn, SYNC_BATCH_SIZE).await?;
            if pending.is_empty() {
                return Ok(());
            }
            upload_operation_blobs(&state.conn, &transport, &state.blob_store, &pending).await?;
            let accepted = transport.push(pending).await?;
            notes_core::acknowledge_server_ops(
                &state.conn,
                accepted.iter().map(|o| (o.envelope.op_id, o.seq)).collect(),
            )
            .await?;
        }
    };
    match tokio::time::timeout(Duration::from_secs(3), attempt).await {
        Ok(Ok(())) => {}
        failure => tracing::warn!(?failure, "Exit synchronization deferred; outbox retained"),
    }
}

#[cfg(test)]
mod tests {
    /// Both routes into the terminal state: the version the server announces,
    /// and the refusal code an already-shipped old client would receive.
    #[test]
    fn a_newer_server_format_parks_sync_instead_of_retrying() {
        let announced =
            anyhow::Error::new(super::SyncSessionError::UpdateRequired { server_version: 9 });
        assert_eq!(
            super::failure_state(&announced),
            super::SyncConnectionState::UpdateRequired
        );
        assert_eq!(super::announced_server_version(&announced), Some(9));
        assert!(super::is_permanent_failure(&announced));
        assert!(announced.to_string().contains("Update the app"));

        let refused = anyhow::Error::new(notes_sync::TransportError::Http {
            status: 426,
            code: Some(notes_protocol::ApiErrorCode::FormatUnsupported),
            message: "update the app to keep syncing".into(),
        });
        assert_eq!(
            super::failure_state(&refused),
            super::SyncConnectionState::UpdateRequired
        );
        assert!(super::is_permanent_failure(&refused));

        // An ordinary server error stays an ordinary, retried failure.
        let unavailable = anyhow::Error::new(notes_sync::TransportError::Http {
            status: 503,
            code: None,
            message: "restarting".into(),
        });
        assert_eq!(
            super::failure_state(&unavailable),
            super::SyncConnectionState::Offline
        );
    }

    #[test]
    fn heartbeat_waits_for_pong_and_detects_dead_peer() {
        let start = tokio::time::Instant::now();
        let mut heartbeat = super::Heartbeat::default();
        assert!(heartbeat.should_send(start).unwrap());
        heartbeat.sent(start);
        assert!(
            !heartbeat
                .should_send(start + super::HEARTBEAT_INTERVAL)
                .unwrap()
        );
        assert!(
            heartbeat
                .should_send(start + super::HEARTBEAT_TIMEOUT)
                .is_err()
        );
        heartbeat.pong();
        assert!(
            heartbeat
                .should_send(start + super::HEARTBEAT_TIMEOUT)
                .unwrap()
        );
    }

    #[test]
    fn heartbeat_uses_existing_protocol_without_changing_operations() {
        assert_eq!(
            serde_json::to_string(&notes_protocol::ClientMessage::Ping).unwrap(),
            r#"{"type":"ping"}"#
        );
        assert!(matches!(
            serde_json::from_str::<notes_protocol::ServerMessage>(r#"{"type":"pong"}"#).unwrap(),
            notes_protocol::ServerMessage::Pong
        ));
    }
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

    #[tokio::test]
    async fn explicit_retry_interrupts_reconnect_backoff() {
        let runtime = SyncRuntime::disabled();
        let mut retries = runtime.subscribe_retries();
        let mut backoff = tokio::spawn(async move {
            wait_for_retry_during_backoff(&mut retries, Duration::from_secs(30)).await
        });

        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut backoff)
                .await
                .is_err(),
            "backoff should wait without a retry request",
        );
        runtime.request_retry();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), backoff)
                .await
                .expect("retry interrupts backoff")
                .expect("backoff task completes")
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
            ink_versions: Vec::new(),
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

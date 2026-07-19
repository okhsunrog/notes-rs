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
}

impl SyncRuntime {
    pub fn disabled() -> Self {
        Self {
            status: Arc::new(RwLock::new(SyncStatus {
                state: SyncConnectionState::Disabled,
                server_url: None,
                last_server_seq: 0,
                pending_operations: 0,
                message: None,
            })),
        }
    }
}

pub fn spawn_worker(
    app: AppHandle,
    connection: Connection,
    server_url: url::Url,
    token: String,
    blob_store: BlobStore,
    status: Arc<RwLock<SyncStatus>>,
) {
    tauri::async_runtime::spawn(async move {
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
            let permanent = result.as_ref().is_err_and(is_permanent_failure);
            set_connection_state(
                &app,
                &status,
                &connection,
                &server_url,
                if permanent {
                    SyncConnectionState::Error
                } else {
                    SyncConnectionState::Offline
                },
                Some(message),
            )
            .await;
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_secs(30));
        }
    });
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
    if notes_core::sync_cursor(connection).await? != 0 {
        return Ok(());
    }
    let server = transport.snapshot().await?;
    let mut local = export_sync_snapshot(connection, 0).await?;
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
            local.seq = server.seq;
            if local != server {
                return Err(SyncSessionError::WorkspaceConflict.into());
            }
            download_snapshot_blobs(transport, blob_store, &server).await?;
            notes_core::import_sync_snapshot(connection, server.clone()).await?;
        }
        (true, true) => {
            // The server's durable empty workspace is canonical. This prevents
            // successive fresh clients from replacing its Journal namespace.
            notes_core::import_sync_snapshot(connection, server).await?;
        }
    }
    Ok(())
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
    crate::commands::emit_events_for_ops(app, connection, &operations).await;
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
    let applied = operations
        .into_iter()
        .zip(outcomes)
        .filter(|(_, outcome)| outcome.applied)
        .map(|(operation, _)| operation.envelope.kind)
        .collect::<Vec<_>>();
    crate::commands::emit_events_for_ops(app, connection, &applied).await;
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
}

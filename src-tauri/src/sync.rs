use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use notes_core::{
    Connection, OpKind, acknowledge_server_ops, apply_sequenced_batch, export_sync_snapshot,
};
use notes_protocol::{ClientMessage, SequencedOp, ServerMessage};
use notes_sync::{
    AppliedRemoteOperation, HttpTransport, SyncClient, SyncSnapshot, SyncTransport, TransportError,
};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tauri::AppHandle;
use tokio_tungstenite::tungstenite::Message;

const SYNC_BATCH_SIZE: u32 = 256;

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
    data_dir: &'a Path,
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
        upload_operation_blobs(self.http, self.data_dir, operations).await
    }

    async fn prepare_pull(&mut self, operations: &[SequencedOp]) -> Result<()> {
        for operation in operations {
            download_operation_blob(self.http, self.data_dir, &operation.envelope).await?;
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
    data_dir: PathBuf,
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
                &data_dir,
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
    data_dir: &Path,
) -> Result<()> {
    transport.health().await?;
    initialize_replica(app, connection, transport, data_dir).await?;
    set_connection_state(
        app,
        status,
        connection,
        server_url,
        SyncConnectionState::Syncing,
        None,
    )
    .await;
    synchronize_http(app, connection, transport, data_dir).await?;
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
                        handle_server_message(app, connection, transport, data_dir, message).await?;
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
                    upload_operation_blobs(transport, data_dir, &pending).await?;
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
    data_dir: &Path,
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
            transport.bootstrap(local.clone()).await?;
            upload_snapshot_blobs(transport, data_dir, &local).await?;
        }
        (false, true) => {
            notes_core::import_sync_snapshot(connection, server.clone()).await?;
            download_snapshot_blobs(transport, data_dir, &server).await?;
            emit_workspace_changed(app);
        }
        (false, false) => {
            local.seq = server.seq;
            if local != server {
                return Err(SyncSessionError::WorkspaceConflict.into());
            }
            notes_core::import_sync_snapshot(connection, server.clone()).await?;
            download_snapshot_blobs(transport, data_dir, &server).await?;
        }
        (true, true) => {}
    }
    Ok(())
}

fn snapshot_is_empty(snapshot: &SyncSnapshot) -> bool {
    snapshot.nodes.is_empty()
        && snapshot.tombstones.is_empty()
        && snapshot.edges.is_empty()
        && snapshot.attachments.is_empty()
}

async fn synchronize_http(
    app: &AppHandle,
    connection: &Connection,
    transport: &HttpTransport,
    data_dir: &Path,
) -> Result<()> {
    let mut transport = DesktopTransport {
        http: transport,
        data_dir,
    };
    let stats = SyncClient::new(connection.clone())
        .with_batch_size(SYNC_BATCH_SIZE)
        .sync_until_idle(&mut transport)
        .await?;
    emit_operation_changes(app, connection, &stats.applied_operations).await;
    Ok(())
}

async fn handle_server_message(
    app: &AppHandle,
    connection: &Connection,
    transport: &HttpTransport,
    data_dir: &Path,
    message: ServerMessage,
) -> Result<()> {
    match message {
        ServerMessage::Ops { ops } => {
            apply_server_operations(app, connection, transport, data_dir, ops).await
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
        ServerMessage::Error { code, message } if code == "conflict" => {
            Err(SyncSessionError::ServerConflict(message).into())
        }
        ServerMessage::Error { code, message } => bail!("sync server error {code}: {message}"),
    }
}

async fn apply_server_operations(
    app: &AppHandle,
    connection: &Connection,
    transport: &HttpTransport,
    data_dir: &Path,
    operations: Vec<SequencedOp>,
) -> Result<()> {
    for operation in &operations {
        download_operation_blob(transport, data_dir, &operation.envelope).await?;
    }
    let previous_contents = previous_operation_contents(connection, &operations).await?;
    let sequenced = operations
        .iter()
        .map(|operation| (operation.seq, operation.envelope.clone()))
        .collect();
    let outcomes = apply_sequenced_batch(connection, sequenced).await?;
    let applied = operations
        .into_iter()
        .zip(outcomes)
        .filter(|(_, outcome)| outcome.applied)
        .map(|(operation, _)| AppliedRemoteOperation {
            previous_content: previous_contents
                .get(&operation.envelope.op_id)
                .cloned()
                .flatten(),
            operation: operation.envelope,
        })
        .collect::<Vec<_>>();
    emit_operation_changes(app, connection, &applied).await;
    Ok(())
}

async fn previous_operation_contents(
    connection: &Connection,
    operations: &[SequencedOp],
) -> Result<std::collections::HashMap<uuid::Uuid, Option<String>>> {
    let content_ops = operations
        .iter()
        .filter_map(|operation| match &operation.envelope.kind {
            OpKind::NodeSetContent(payload) => Some((operation.envelope.op_id, payload.uuid)),
            _ => None,
        })
        .collect::<Vec<_>>();
    if content_ops.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let nodes = notes_core::db::get_nodes_by_uuids(
        connection,
        content_ops.iter().map(|(_, uuid)| *uuid).collect(),
    )
    .await?;
    let contents = nodes
        .into_iter()
        .map(|node| (node.uuid, node.content))
        .collect::<std::collections::HashMap<_, _>>();
    Ok(content_ops
        .into_iter()
        .map(|(op_id, uuid)| (op_id, contents.get(&uuid).cloned()))
        .collect())
}

async fn emit_operation_changes(
    app: &AppHandle,
    connection: &Connection,
    operations: &[AppliedRemoteOperation],
) {
    use std::collections::BTreeSet;

    if operations.is_empty() {
        return;
    }
    let mut changed = BTreeSet::new();
    let mut deleted = BTreeSet::new();
    let mut structure = BTreeSet::new();
    let mut graph = BTreeSet::new();
    let mut attachment_parents = BTreeSet::new();

    for applied in operations {
        match &applied.operation.kind {
            OpKind::NodeCreate(payload) => {
                changed.insert(payload.uuid);
                if payload.node_kind == notes_core::NodeKind::Block
                    && notes_core::content_references_changed("", &payload.content)
                {
                    graph.insert(payload.uuid);
                }
                if payload.parent_uuid.is_some() {
                    structure.insert(payload.uuid);
                }
            }
            OpKind::NodeSetContent(payload) => {
                changed.insert(payload.uuid);
                if applied.previous_content.as_deref().is_none_or(|previous| {
                    notes_core::content_references_changed(previous, &payload.content)
                }) {
                    graph.insert(payload.uuid);
                }
            }
            OpKind::NodeSetTitle(payload) => {
                changed.insert(payload.uuid);
            }
            OpKind::NodeMove(payload) => {
                changed.insert(payload.uuid);
                structure.insert(payload.uuid);
            }
            OpKind::NodeDelete(payload) => {
                deleted.insert(payload.uuid);
                structure.insert(payload.uuid);
                graph.insert(payload.uuid);
            }
            OpKind::EdgeAdd(payload) => {
                graph.extend([payload.src_uuid, payload.dst_uuid]);
            }
            OpKind::EdgeRemove(payload) => {
                graph.extend([payload.src_uuid, payload.dst_uuid]);
            }
            OpKind::AttachmentAdd(payload) => {
                attachment_parents.insert(payload.node_uuid);
                graph.insert(payload.node_uuid);
            }
            OpKind::AttachmentRemove(payload) => {
                attachment_parents.insert(payload.node_uuid);
                graph.insert(payload.node_uuid);
            }
        }
    }

    if !changed.is_empty() {
        match notes_core::db::get_nodes_by_uuids(connection, changed.into_iter().collect()).await {
            Ok(nodes) => crate::commands::emit_nodes_changed(app, connection, &nodes, []).await,
            Err(error) => tracing::warn!(%error, "resolving remotely changed nodes failed"),
        }
    }
    if !deleted.is_empty() {
        crate::commands::emit_domain(
            app,
            crate::commands::DomainEvent::NodeDeleted {
                node_uuids: deleted.into_iter().collect(),
                parent_uuids: Vec::new(),
            },
        );
    }
    if !structure.is_empty() {
        crate::commands::emit_domain(
            app,
            crate::commands::DomainEvent::StructureChanged {
                node_uuids: structure.into_iter().collect(),
            },
        );
    }
    if !graph.is_empty() {
        crate::commands::emit_domain(
            app,
            crate::commands::DomainEvent::GraphChanged {
                node_uuids: graph.into_iter().collect(),
            },
        );
    }
    if !attachment_parents.is_empty() {
        crate::commands::emit_domain(
            app,
            crate::commands::DomainEvent::AttachmentsChanged {
                parent_uuids: attachment_parents.into_iter().collect(),
            },
        );
    }
}

async fn upload_operation_blobs(
    transport: &HttpTransport,
    data_dir: &Path,
    operations: &[notes_core::Op],
) -> Result<()> {
    for operation in operations {
        if let notes_core::OpKind::AttachmentAdd(attachment) = &operation.kind {
            let path = attachment_path(data_dir, &attachment.blob_hash, &attachment.filename)?;
            transport.upload_blob(&attachment.blob_hash, &path).await?;
        }
    }
    Ok(())
}

async fn download_operation_blob(
    transport: &HttpTransport,
    data_dir: &Path,
    operation: &notes_core::Op,
) -> Result<()> {
    if let notes_core::OpKind::AttachmentAdd(attachment) = &operation.kind {
        let path = attachment_path(data_dir, &attachment.blob_hash, &attachment.filename)?;
        if !tokio::fs::try_exists(&path).await? {
            transport
                .download_blob(&attachment.blob_hash, &path, 100 * 1024 * 1024)
                .await?;
        }
    }
    Ok(())
}

async fn upload_snapshot_blobs(
    transport: &HttpTransport,
    data_dir: &Path,
    snapshot: &SyncSnapshot,
) -> Result<()> {
    for attachment in snapshot
        .attachments
        .iter()
        .filter(|attachment| attachment.present)
    {
        let filename = attachment
            .filename
            .as_deref()
            .context("snapshot attachment is missing its filename")?;
        let path = attachment_path(data_dir, &attachment.blob_hash, filename)?;
        transport.upload_blob(&attachment.blob_hash, &path).await?;
    }
    Ok(())
}

async fn download_snapshot_blobs(
    transport: &HttpTransport,
    data_dir: &Path,
    snapshot: &SyncSnapshot,
) -> Result<()> {
    for attachment in snapshot
        .attachments
        .iter()
        .filter(|attachment| attachment.present)
    {
        let filename = attachment
            .filename
            .as_deref()
            .context("snapshot attachment is missing its filename")?;
        let path = attachment_path(data_dir, &attachment.blob_hash, filename)?;
        if !tokio::fs::try_exists(&path).await? {
            transport
                .download_blob(&attachment.blob_hash, &path, 100 * 1024 * 1024)
                .await?;
        }
    }
    Ok(())
}

fn attachment_path(data_dir: &Path, hash: &str, filename: &str) -> Result<PathBuf> {
    let mut components = Path::new(filename).components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        bail!("attachment filename is not a safe path component");
    }
    Ok(data_dir.join("attachments").join(hash).join(filename))
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
    fn empty_snapshot_has_no_source_records() {
        assert!(snapshot_is_empty(&SyncSnapshot {
            format_version: notes_sync::FORMAT_VERSION,
            seq: 0,
            nodes: Vec::new(),
            tombstones: Vec::new(),
            edges: Vec::new(),
            attachments: Vec::new(),
        }));
    }
}

use anyhow::Context;
use base64::Engine;
use notes_core::Connection;
use notes_core::db::{self, Node, SearchHit};
use notes_core::{NodeKind, ReorderDirection};
use notes_protocol::{AiIndexStatus, AiRuntimeSettings, ChatEvent, ChatTurn};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
#[cfg(not(target_os = "android"))]
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tauri_specta::Event;
use tokio_util::sync::CancellationToken;

#[cfg(target_os = "android")]
use tauri_plugin_mobile_system::MobileSystemExt;

pub type CommandResult<T> = Result<T, CommandError>;

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MobileSystemInfo {
    pub device_name: String,
    pub safe_area: SafeAreaInsets,
}

#[derive(Debug, Clone, Copy, Default, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SafeAreaInsets {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

#[tauri::command]
#[specta::specta]
pub fn mobile_system_info(app: AppHandle) -> CommandResult<Option<MobileSystemInfo>> {
    #[cfg(target_os = "android")]
    {
        let integration = app.mobile_system();
        let native = integration.safe_area_insets().map_err(err)?;
        Ok(Some(MobileSystemInfo {
            device_name: integration.device_name().map_err(err)?,
            safe_area: SafeAreaInsets {
                top: native.top,
                right: native.right,
                bottom: native.bottom,
                left: native.left,
            },
        }))
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(None)
    }
}

#[tauri::command]
#[specta::specta]
pub fn set_system_bars_style(app: AppHandle, dark_background: bool) -> CommandResult<()> {
    #[cfg(target_os = "android")]
    {
        app.mobile_system()
            .set_system_bars_style(dark_background)
            .map_err(err)
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, dark_background);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CommandErrorCode {
    InvalidInput,
    NotFound,
    Conflict,
    Unavailable,
    Internal,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: CommandErrorCode,
    pub message: String,
}

impl CommandError {
    fn new(code: CommandErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::new(CommandErrorCode::InvalidInput, message)
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self::new(CommandErrorCode::Conflict, message)
    }
}

impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self::invalid(message)
    }
}

impl From<&str> for CommandError {
    fn from(message: &str) -> Self {
        Self::invalid(message)
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CommandError {}

pub struct AppState {
    pub conn: Connection,
    pub remote_ai: Option<notes_sync::HttpTransport>,
    pub chat_cancellations: Arc<std::sync::Mutex<HashMap<uuid::Uuid, CancellationToken>>>,
}

/// The single frontend invalidation stream for persisted Rust state.
/// Payloads carry affected IDs when a command can identify them; whole-workspace
/// replacements (import/sync) deliberately request a full cache refresh.
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DomainEvent {
    NodeChanged {
        node_uuids: Vec<uuid::Uuid>,
        parent_uuids: Vec<uuid::Uuid>,
        node_kinds: Vec<NodeKind>,
    },
    NodeDeleted {
        node_uuids: Vec<uuid::Uuid>,
        parent_uuids: Vec<uuid::Uuid>,
    },
    GraphChanged {
        node_uuids: Vec<uuid::Uuid>,
    },
    HistoryChanged,
    SettingsChanged,
    SyncStatusChanged,
    ServerAiChanged,
    WorkspaceChanged,
}

#[derive(Debug, Clone, Serialize, specta::Type, tauri_specta::Event)]
#[tauri_specta(event_name = "domain:event")]
pub struct DomainEventMessage(pub DomainEvent);

pub(crate) fn emit_domain(app: &AppHandle, event: DomainEvent) {
    if let Err(error) = DomainEventMessage(event).emit(app) {
        tracing::warn!(%error, "failed to emit domain event");
    }
}

async fn node_uuids_for_ids(
    connection: &Connection,
    ids: impl IntoIterator<Item = i64>,
) -> Vec<uuid::Uuid> {
    match db::node_uuids_for_ids(connection, ids).await {
        Ok(uuids) => uuids,
        Err(error) => {
            tracing::warn!(%error, "failed to resolve node UUIDs for domain event");
            Vec::new()
        }
    }
}

async fn emit_nodes_changed(
    app: &AppHandle,
    connection: &Connection,
    nodes: &[Node],
    additional_parent_ids: impl IntoIterator<Item = i64>,
) {
    let parent_ids = nodes
        .iter()
        .filter_map(|node| node.parent_id)
        .chain(additional_parent_ids);
    emit_domain(
        app,
        DomainEvent::NodeChanged {
            node_uuids: nodes.iter().map(|node| node.uuid).collect(),
            parent_uuids: node_uuids_for_ids(connection, parent_ids).await,
            node_kinds: nodes
                .iter()
                .map(|node| node.kind)
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
        },
    );
}

/// Registered immediately in setup so the frontend can ask whether the heavy
/// initialization (embedder download, db open) has finished. Without this,
/// commands invoked during the ~minutes-long first-run model fetch fail with
/// an opaque "state not managed" error.
pub struct Startup {
    pub status: Arc<RwLock<StartupStatus>>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum StartupStatus {
    Starting { message: String },
    Ready,
    Error { message: String },
}

fn err<E: std::fmt::Display + 'static>(error: E) -> CommandError {
    let any = &error as &dyn std::any::Any;
    if let Some(error) = any.downcast_ref::<anyhow::Error>() {
        let message = format!("{error:#}");
        if let Some(error) = error.downcast_ref::<notes_core::CoreError>() {
            let code = match error {
                notes_core::CoreError::InvalidInput(_) => CommandErrorCode::InvalidInput,
                notes_core::CoreError::NotFound(_) => CommandErrorCode::NotFound,
                notes_core::CoreError::Conflict(_) | notes_core::CoreError::SyncConflict(_) => {
                    CommandErrorCode::Conflict
                }
                notes_core::CoreError::Database(_) => CommandErrorCode::Internal,
            };
            return CommandError::new(code, message);
        }
        if notes_sync::is_transport_failure(error) {
            return CommandError::new(CommandErrorCode::Unavailable, message);
        }
        return CommandError::new(CommandErrorCode::Internal, message);
    }
    if let Some(error) = any.downcast_ref::<notes_core::CoreError>() {
        let code = match error {
            notes_core::CoreError::InvalidInput(_) => CommandErrorCode::InvalidInput,
            notes_core::CoreError::NotFound(_) => CommandErrorCode::NotFound,
            notes_core::CoreError::Conflict(_) | notes_core::CoreError::SyncConflict(_) => {
                CommandErrorCode::Conflict
            }
            notes_core::CoreError::Database(_) => CommandErrorCode::Internal,
        };
        return CommandError::new(code, error.to_string());
    }
    CommandError::new(CommandErrorCode::Internal, error.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn is_ready(state: State<'_, Startup>) -> bool {
    matches!(
        *state.status.read().unwrap_or_else(|e| e.into_inner()),
        StartupStatus::Ready
    )
}

#[tauri::command]
#[specta::specta]
pub fn startup_status(state: State<'_, Startup>) -> StartupStatus {
    state
        .status
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

#[tauri::command]
#[specta::specta]
pub fn sync_status(state: State<'_, crate::sync::SyncRuntime>) -> crate::sync::SyncStatus {
    state
        .status
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

fn remote_ai(state: &AppState) -> CommandResult<&notes_sync::HttpTransport> {
    state.remote_ai.as_ref().ok_or_else(|| CommandError {
        code: CommandErrorCode::Unavailable,
        message: "server AI requires a configured notes-rs server".into(),
    })
}

#[tauri::command]
#[specta::specta]
pub async fn server_ai_status(state: State<'_, AppState>) -> CommandResult<AiIndexStatus> {
    remote_ai(&state)?.ai_status().await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn save_server_ai_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: AiRuntimeSettings,
) -> CommandResult<AiIndexStatus> {
    let status = remote_ai(&state)?
        .update_ai_settings(settings)
        .await
        .map_err(err)?;
    emit_domain(&app, DomainEvent::ServerAiChanged);
    Ok(status)
}

#[tauri::command]
#[specta::specta]
pub async fn reindex_server_ai(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<AiIndexStatus> {
    let status = remote_ai(&state)?.reindex_ai().await.map_err(err)?;
    emit_domain(&app, DomainEvent::ServerAiChanged);
    Ok(status)
}

#[tauri::command]
#[specta::specta]
pub fn load_settings(app: AppHandle) -> CommandResult<crate::settings::SettingsSnapshot> {
    crate::settings::load(&app).map_err(err)
}

#[tauri::command]
#[specta::specta]
pub fn save_settings(
    app: AppHandle,
    update: crate::settings::SettingsUpdate,
) -> CommandResult<crate::settings::SettingsSnapshot> {
    let settings = crate::settings::save(&app, update).map_err(err)?;
    emit_domain(&app, DomainEvent::SettingsChanged);
    Ok(settings)
}

#[tauri::command]
#[specta::specta]
pub fn restart_app(app: AppHandle) {
    app.restart()
}

#[tauri::command]
#[specta::specta]
pub async fn create_node(
    app: AppHandle,
    state: State<'_, AppState>,
    kind: NodeKind,
    title: Option<String>,
    content: String,
    content_json: Option<String>,
) -> CommandResult<Node> {
    let node = db::create_node(&state.conn, kind, title, content, content_json)
        .await
        .map_err(err)?;
    emit_nodes_changed(&app, &state.conn, std::slice::from_ref(&node), []).await;
    Ok(node)
}

#[tauri::command]
#[specta::specta]
pub async fn update_node(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    title: Option<String>,
    content: String,
    content_json: Option<String>,
) -> CommandResult<()> {
    db::update_node(&state.conn, id, title, content, content_json)
        .await
        .map_err(err)?;
    let node = db::get_node(&state.conn, id)
        .await
        .map_err(err)?
        .ok_or_else(|| CommandError::new(CommandErrorCode::Internal, "updated node disappeared"))?;
    emit_nodes_changed(&app, &state.conn, std::slice::from_ref(&node), []).await;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn rename_page(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
    title: Option<String>,
) -> CommandResult<Node> {
    let node = db::rename_page(&state.conn, uuid, title)
        .await
        .map_err(err)?;
    emit_nodes_changed(&app, &state.conn, std::slice::from_ref(&node), []).await;
    Ok(node)
}

#[tauri::command]
#[specta::specta]
pub async fn create_note(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<db::CreatedNote> {
    let note = db::create_note(&state.conn).await.map_err(err)?;
    emit_nodes_changed(
        &app,
        &state.conn,
        &[note.page.clone(), note.initial_block.clone()],
        [],
    )
    .await;
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(note)
}

#[tauri::command]
#[specta::specta]
pub async fn set_block_content(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
    block: db::BlockContent,
) -> CommandResult<(Node, u32)> {
    let (node, broken_refs, graph_changed) = db::set_block_content(&state.conn, uuid, block)
        .await
        .map_err(err)?;
    emit_nodes_changed(&app, &state.conn, std::slice::from_ref(&node), []).await;
    if graph_changed {
        emit_domain(
            &app,
            DomainEvent::GraphChanged {
                node_uuids: vec![node.uuid],
            },
        );
    }
    Ok((node, broken_refs))
}

#[tauri::command]
#[specta::specta]
pub async fn split_block(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    parts: Vec<db::BlockContent>,
) -> CommandResult<Vec<Node>> {
    let nodes = db::split_block(&state.conn, id, parts).await.map_err(err)?;
    emit_nodes_changed(&app, &state.conn, &nodes, []).await;
    emit_domain(
        &app,
        DomainEvent::GraphChanged {
            node_uuids: nodes.iter().map(|node| node.uuid).collect(),
        },
    );
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(nodes)
}

#[tauri::command]
#[specta::specta]
pub async fn link_nodes(
    app: AppHandle,
    state: State<'_, AppState>,
    src: i64,
    dst: i64,
    kind: String,
    weight: Option<f64>,
) -> CommandResult<()> {
    db::link_nodes(&state.conn, src, dst, kind, weight.unwrap_or(1.0))
        .await
        .map_err(err)?;
    emit_domain(
        &app,
        DomainEvent::GraphChanged {
            node_uuids: node_uuids_for_ids(&state.conn, [src, dst]).await,
        },
    );
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn get_node(state: State<'_, AppState>, id: i64) -> CommandResult<Option<Node>> {
    db::get_node(&state.conn, id).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn neighbors(
    state: State<'_, AppState>,
    id: i64,
    depth: u32,
) -> CommandResult<Vec<Node>> {
    db::neighbors(&state.conn, id, depth).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn list_entities(
    state: State<'_, AppState>,
    limit: Option<u32>,
) -> CommandResult<Vec<Node>> {
    db::list_entities(&state.conn, limit.unwrap_or(50))
        .await
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn list_pages(
    state: State<'_, AppState>,
    limit: Option<u32>,
) -> CommandResult<Vec<Node>> {
    db::list_pages(&state.conn, limit.unwrap_or(200))
        .await
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn delete_page(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> CommandResult<bool> {
    write_backup(&app, &state.conn, "before-delete")
        .await
        .map_err(err)?;
    let deleted = db::delete_page(&state.conn, id).await.map_err(err)?;
    if let Some(deleted) = &deleted {
        // Retain attachment payloads so structural Undo can restore their
        // database nodes. Explicit attachment deletion removes the file.
        emit_domain(
            &app,
            DomainEvent::NodeDeleted {
                node_uuids: deleted.node_uuids.clone(),
                parent_uuids: Vec::new(),
            },
        );
        emit_domain(&app, DomainEvent::HistoryChanged);
    }
    Ok(deleted.is_some())
}

#[tauri::command]
#[specta::specta]
pub async fn find_backlinks(
    state: State<'_, AppState>,
    id: i64,
    kind: Option<String>,
) -> CommandResult<Vec<Node>> {
    db::find_backlinks(&state.conn, id, kind).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn graph_snapshot(
    state: State<'_, AppState>,
    focus_id: Option<i64>,
) -> CommandResult<db::GraphSnapshot> {
    db::graph_snapshot(&state.conn, focus_id).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn export_data(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<Option<String>> {
    let filename = format!(
        "notes-rs-{}.json",
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    );
    let mut archive = db::export_archive(&state.conn).await.map_err(err)?;
    add_archive_files(&app, &mut archive).map_err(err)?;
    let json = serde_json::to_string_pretty(&archive).map_err(err)?;

    #[cfg(not(target_os = "android"))]
    {
        let Some(path) = app
            .dialog()
            .file()
            .add_filter("notes-rs archive", &["json"])
            .set_file_name(filename)
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = path.into_path().map_err(err)?;
        std::fs::write(&path, json).map_err(err)?;
        Ok(Some(path.to_string_lossy().into_owned()))
    }

    #[cfg(target_os = "android")]
    {
        use tauri_plugin_android_fs::AndroidFsExt;

        let api = app.android_fs_async();
        let Some(uri) = api
            .picker()
            .save_file(None, filename, Some("application/json"), false)
            .await
            .map_err(err)?
        else {
            return Ok(None);
        };
        api.write(&uri, json.as_bytes()).await.map_err(err)?;
        Ok(Some(uri.uri))
    }
}

#[tauri::command]
#[specta::specta]
pub async fn import_data(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<Option<String>> {
    #[cfg(not(target_os = "android"))]
    let (source, json) = {
        let Some(path) = app
            .dialog()
            .file()
            .add_filter("notes-rs archive", &["json"])
            .blocking_pick_file()
        else {
            return Ok(None);
        };
        let path = path.into_path().map_err(err)?;
        let json = std::fs::read_to_string(&path).map_err(err)?;
        (path.to_string_lossy().into_owned(), json)
    };

    #[cfg(target_os = "android")]
    let (source, json) = {
        use tauri_plugin_android_fs::AndroidFsExt;

        let api = app.android_fs_async();
        let Some(uri) = api
            .picker()
            .pick_file(None, &["application/json"], false)
            .await
            .map_err(err)?
        else {
            return Ok(None);
        };
        let json = api.read_to_string(&uri).await.map_err(err)?;
        (uri.uri, json)
    };

    let archive: db::DataArchive = serde_json::from_str(&json).map_err(err)?;
    write_backup(&app, &state.conn, "before-import")
        .await
        .map_err(err)?;
    restore_archive(&app, &state.conn, archive)
        .await
        .map_err(err)?;
    emit_domain(&app, DomainEvent::WorkspaceChanged);
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(Some(source))
}

#[tauri::command]
#[specta::specta]
pub async fn create_backup(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<std::path::PathBuf> {
    write_backup(&app, &state.conn, "manual").await.map_err(err)
}

async fn write_backup(
    app: &AppHandle,
    connection: &Connection,
    reason: &str,
) -> anyhow::Result<std::path::PathBuf> {
    let directory = app.path().app_data_dir()?.join("backups");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join(format!(
        "notes-rs-{reason}-{}.json",
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    ));
    let mut archive = db::export_archive(connection).await?;
    add_archive_files(app, &mut archive)?;
    std::fs::write(&path, serde_json::to_string_pretty(&archive)?)?;
    Ok(path)
}

#[tauri::command]
#[specta::specta]
pub fn choose_sync_directory(app: AppHandle) -> CommandResult<Option<std::path::PathBuf>> {
    #[cfg(not(mobile))]
    {
        app.dialog()
            .file()
            .blocking_pick_folder()
            .map(|path| path.into_path())
            .transpose()
            .map_err(err)
    }

    #[cfg(mobile)]
    {
        let _ = app;
        Err(
            "directory-based sync is unavailable on mobile; configure the sync server instead"
                .into(),
        )
    }
}

#[tauri::command]
#[specta::specta]
pub async fn sync_push(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<std::path::PathBuf> {
    let settings = crate::settings::load(&app).map_err(err)?;
    let directory = sync_directory(settings.sync_directory.as_deref()).map_err(err)?;
    std::fs::create_dir_all(&directory).map_err(err)?;
    let mut archive = db::export_archive(&state.conn).await.map_err(err)?;
    add_archive_files(&app, &mut archive).map_err(err)?;
    let path = directory.join("notes-rs-sync.json");
    let temporary = directory.join("notes-rs-sync.json.tmp");
    std::fs::write(
        &temporary,
        serde_json::to_string_pretty(&archive).map_err(err)?,
    )
    .map_err(err)?;
    std::fs::rename(&temporary, &path).map_err(err)?;
    Ok(path)
}

#[tauri::command]
#[specta::specta]
pub async fn sync_pull(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<std::path::PathBuf> {
    let settings = crate::settings::load(&app).map_err(err)?;
    let path = sync_directory(settings.sync_directory.as_deref())
        .map_err(err)?
        .join("notes-rs-sync.json");
    let archive =
        serde_json::from_str(&std::fs::read_to_string(&path).map_err(err)?).map_err(err)?;
    write_backup(&app, &state.conn, "before-sync-pull")
        .await
        .map_err(err)?;
    restore_archive(&app, &state.conn, archive)
        .await
        .map_err(err)?;
    emit_domain(&app, DomainEvent::WorkspaceChanged);
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(path)
}

fn sync_directory(value: Option<&std::path::Path>) -> anyhow::Result<std::path::PathBuf> {
    value
        .map(std::path::Path::to_path_buf)
        .context("choose a sync directory in Settings first")
}

#[tauri::command]
#[specta::specta]
pub async fn attach_file(
    app: AppHandle,
    state: State<'_, AppState>,
    parent_id: i64,
) -> CommandResult<Option<Node>> {
    #[cfg(not(target_os = "android"))]
    let (title, mime_type, bytes) = {
        let Some(source) = app.dialog().file().blocking_pick_file() else {
            return Ok(None);
        };
        let source = source.into_path().map_err(err)?;
        let metadata = source.metadata().map_err(err)?;
        if !metadata.is_file() {
            return Err(CommandError::invalid("attachments must be regular files"));
        }
        if metadata.len() > 100 * 1024 * 1024 {
            return Err(CommandError::invalid("attachments are limited to 100 MiB"));
        }
        let title = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| CommandError::invalid("attachment filename is not valid UTF-8"))?
            .to_string();
        let bytes = std::fs::read(&source).map_err(err)?;
        (title, "application/octet-stream".to_string(), bytes)
    };

    #[cfg(target_os = "android")]
    let (title, mime_type, bytes) = {
        use tauri_plugin_android_fs::AndroidFsExt;

        let api = app.android_fs_async();
        let Some(uri) = api
            .picker()
            .pick_file(None, &[], false)
            .await
            .map_err(err)?
        else {
            return Ok(None);
        };
        let size = api.get_len(&uri).await.map_err(err)?;
        if size > 100 * 1024 * 1024 {
            return Err(CommandError::invalid("attachments are limited to 100 MiB"));
        }
        let title = api.get_name(&uri).await.map_err(err)?;
        let mime_type = api
            .get_mime_type(&uri)
            .await
            .unwrap_or_else(|_| "application/octet-stream".into());
        let bytes = api.read(&uri).await.map_err(err)?;
        (title, mime_type, bytes)
    };

    let size = bytes.len() as u64;
    let blob_hash = format!("{:x}", Sha256::digest(&bytes));
    let relative = std::path::PathBuf::from("attachments")
        .join(&blob_hash)
        .join(&title);
    let destination = app.path().app_data_dir().map_err(err)?.join(&relative);
    std::fs::create_dir_all(destination.parent().expect("attachment has parent")).map_err(err)?;
    std::fs::write(&destination, bytes).map_err(err)?;
    match db::create_attachment(&state.conn, parent_id, blob_hash, title, mime_type, size).await {
        Ok(node) => {
            emit_nodes_changed(&app, &state.conn, std::slice::from_ref(&node), [parent_id]).await;
            emit_domain(
                &app,
                DomainEvent::GraphChanged {
                    node_uuids: node_uuids_for_ids(&state.conn, [parent_id]).await,
                },
            );
            Ok(Some(node))
        }
        Err(error) => {
            let _ = std::fs::remove_file(destination);
            Err(err(error))
        }
    }
}

#[tauri::command]
#[specta::specta]
pub async fn list_attachments(
    state: State<'_, AppState>,
    parent_id: i64,
) -> CommandResult<Vec<Node>> {
    db::list_attachments(&state.conn, parent_id)
        .await
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn open_attachment(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> CommandResult<()> {
    let node = db::get_node(&state.conn, id)
        .await
        .map_err(err)?
        .filter(|node| node.kind == NodeKind::Attachment)
        .ok_or_else(|| CommandError::new(CommandErrorCode::NotFound, "attachment not found"))?;
    let path = safe_app_data_path(&app, &node.content).map_err(err)?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn delete_attachment(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> CommandResult<bool> {
    write_backup(&app, &state.conn, "before-attachment-delete")
        .await
        .map_err(err)?;
    let Some(node) = db::delete_attachment(&state.conn, id).await.map_err(err)? else {
        return Ok(false);
    };
    let path = safe_app_data_path(&app, &node.content).map_err(err)?;
    let still_referenced = db::attachment_path_ref_count(&state.conn, node.content)
        .await
        .map_err(err)?
        > 0;
    if !still_referenced && path.exists() {
        std::fs::remove_file(&path).map_err(err)?;
    }
    emit_domain(
        &app,
        DomainEvent::NodeDeleted {
            node_uuids: vec![node.uuid],
            parent_uuids: Vec::new(),
        },
    );
    emit_domain(
        &app,
        DomainEvent::GraphChanged {
            node_uuids: vec![node.uuid],
        },
    );
    Ok(true)
}

fn add_archive_files(app: &AppHandle, archive: &mut db::DataArchive) -> anyhow::Result<()> {
    for node in archive
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::Attachment)
    {
        let path = safe_app_data_path(app, &node.content)?;
        let bytes = std::fs::read(&path)
            .with_context(|| format!("reading attachment {}", path.display()))?;
        archive.files.insert(
            node.content.clone(),
            base64::engine::general_purpose::STANDARD.encode(bytes),
        );
    }
    Ok(())
}

async fn restore_archive(
    app: &AppHandle,
    connection: &Connection,
    archive: db::DataArchive,
) -> anyhow::Result<()> {
    let data_dir = app.path().app_data_dir()?;
    let staging = data_dir.join(format!("attachments-import-{}", uuid::Uuid::new_v4()));
    for (relative, encoded) in &archive.files {
        let relative = safe_relative_path(relative)?;
        let destination = staging.join(relative.strip_prefix("attachments").unwrap_or(&relative));
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            destination,
            base64::engine::general_purpose::STANDARD.decode(encoded)?,
        )?;
    }
    if let Err(error) = db::import_archive(connection, archive).await {
        let _ = std::fs::remove_dir_all(staging);
        return Err(error);
    }
    let attachments = data_dir.join("attachments");
    if attachments.exists() {
        std::fs::remove_dir_all(&attachments)?;
    }
    if staging.exists() {
        std::fs::rename(staging, attachments)?;
    }
    Ok(())
}

fn safe_app_data_path(app: &AppHandle, relative: &str) -> anyhow::Result<std::path::PathBuf> {
    Ok(app
        .path()
        .app_data_dir()?
        .join(safe_relative_path(relative)?))
}

fn safe_relative_path(value: &str) -> anyhow::Result<std::path::PathBuf> {
    let path = std::path::Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        anyhow::bail!("archive contains an unsafe attachment path");
    }
    if !path.starts_with("attachments") {
        anyhow::bail!("attachment paths must be below the attachments directory");
    }
    Ok(path.to_path_buf())
}

#[tauri::command]
#[specta::specta]
pub async fn list_block_children(
    state: State<'_, AppState>,
    parent_id: i64,
) -> CommandResult<Vec<Node>> {
    db::list_block_children(&state.conn, parent_id)
        .await
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn create_block(
    app: AppHandle,
    state: State<'_, AppState>,
    parent_id: Option<i64>,
    position: Option<f64>,
    content: String,
    content_json: Option<String>,
) -> CommandResult<Node> {
    let node = db::create_block(&state.conn, parent_id, position, content, content_json)
        .await
        .map_err(err)?;
    emit_nodes_changed(&app, &state.conn, std::slice::from_ref(&node), []).await;
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(node)
}

#[tauri::command]
#[specta::specta]
pub async fn indent_block(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<Node> {
    let old_parent_id = db::get_node_by_uuid(&state.conn, uuid)
        .await
        .map_err(err)?
        .and_then(|node| node.parent_id);
    let node = db::indent_block(&state.conn, uuid).await.map_err(err)?;
    emit_nodes_changed(
        &app,
        &state.conn,
        std::slice::from_ref(&node),
        old_parent_id,
    )
    .await;
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(node)
}

#[tauri::command]
#[specta::specta]
pub async fn outdent_block(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<Node> {
    let old_parent_id = db::get_node_by_uuid(&state.conn, uuid)
        .await
        .map_err(err)?
        .and_then(|node| node.parent_id);
    let node = db::outdent_block(&state.conn, uuid).await.map_err(err)?;
    emit_nodes_changed(
        &app,
        &state.conn,
        std::slice::from_ref(&node),
        old_parent_id,
    )
    .await;
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(node)
}

async fn move_block_in_direction(
    app: &AppHandle,
    state: &AppState,
    uuid: uuid::Uuid,
    direction: ReorderDirection,
) -> CommandResult<Node> {
    let node = db::move_block_in_direction(&state.conn, uuid, direction)
        .await
        .map_err(err)?;
    emit_nodes_changed(app, &state.conn, std::slice::from_ref(&node), []).await;
    emit_domain(app, DomainEvent::HistoryChanged);
    Ok(node)
}

#[tauri::command]
#[specta::specta]
pub async fn move_block_up(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<Node> {
    move_block_in_direction(&app, &state, uuid, ReorderDirection::Up).await
}

#[tauri::command]
#[specta::specta]
pub async fn move_block_down(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<Node> {
    move_block_in_direction(&app, &state, uuid, ReorderDirection::Down).await
}

#[tauri::command]
#[specta::specta]
pub async fn delete_block(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> CommandResult<bool> {
    let deleted_node = db::get_node(&state.conn, id)
        .await
        .map_err(err)?
        .filter(|node| node.kind == NodeKind::Block);
    let parent_uuids = node_uuids_for_ids(
        &state.conn,
        deleted_node.iter().filter_map(|node| node.parent_id),
    )
    .await;
    let deleted = db::delete_block(&state.conn, id).await.map_err(err)?;
    if deleted {
        emit_domain(
            &app,
            DomainEvent::NodeDeleted {
                node_uuids: deleted_node.into_iter().map(|node| node.uuid).collect(),
                parent_uuids,
            },
        );
        emit_domain(&app, DomainEvent::HistoryChanged);
    }
    Ok(deleted)
}

#[tauri::command]
#[specta::specta]
pub async fn get_page_by_title(
    state: State<'_, AppState>,
    title: String,
) -> CommandResult<Option<Node>> {
    db::get_page_by_title(&state.conn, title).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn get_node_by_uuid(
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<Option<Node>> {
    db::get_node_by_uuid(&state.conn, uuid).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn get_or_create_page_by_title(
    app: AppHandle,
    state: State<'_, AppState>,
    title: String,
) -> CommandResult<Node> {
    let page = db::get_or_create_page_by_title(&state.conn, title)
        .await
        .map_err(err)?;
    emit_nodes_changed(&app, &state.conn, std::slice::from_ref(&page), []).await;
    Ok(page)
}

#[tauri::command]
#[specta::specta]
pub async fn create_page(
    app: AppHandle,
    state: State<'_, AppState>,
    title: String,
) -> CommandResult<Node> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err(CommandError::invalid("title is required"));
    }
    let page = db::create_page(&state.conn, title).await.map_err(err)?;
    emit_nodes_changed(&app, &state.conn, std::slice::from_ref(&page), []).await;
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(page)
}

#[tauri::command]
#[specta::specta]
pub async fn history_status(state: State<'_, AppState>) -> CommandResult<(i64, i64)> {
    db::history_status(&state.conn).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn undo(app: AppHandle, state: State<'_, AppState>) -> CommandResult<bool> {
    let changed = db::undo_history(&state.conn).await.map_err(err)?;
    if changed {
        emit_domain(&app, DomainEvent::WorkspaceChanged);
        emit_domain(&app, DomainEvent::HistoryChanged);
    }
    Ok(changed)
}

#[tauri::command]
#[specta::specta]
pub async fn redo(app: AppHandle, state: State<'_, AppState>) -> CommandResult<bool> {
    let changed = db::redo_history(&state.conn).await.map_err(err)?;
    if changed {
        emit_domain(&app, DomainEvent::WorkspaceChanged);
        emit_domain(&app, DomainEvent::HistoryChanged);
    }
    Ok(changed)
}

#[tauri::command]
#[specta::specta]
pub async fn get_containing_page(
    state: State<'_, AppState>,
    id: i64,
) -> CommandResult<Option<Node>> {
    db::get_containing_page(&state.conn, id).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn search_pages_by_title(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> CommandResult<Vec<Node>> {
    db::search_pages_by_title(&state.conn, query, limit)
        .await
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn search_blocks_fts(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> CommandResult<Vec<Node>> {
    db::search_blocks_fts(&state.conn, query, limit)
        .await
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn search_fts(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> CommandResult<Vec<SearchHit>> {
    let limit = validate_search_request(&query, limit)?;
    db::search_fts(&state.conn, query, limit).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn search_vec(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> CommandResult<Vec<SearchHit>> {
    remote_search(&state, query, limit).await
}

#[tauri::command]
#[specta::specta]
pub async fn search_hybrid(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> CommandResult<Vec<SearchHit>> {
    remote_search(&state, query, limit).await
}

/// Retrieve via hybrid RRF, then rerank with the configured provider.
#[tauri::command]
#[specta::specta]
pub async fn search_agentic(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> CommandResult<Vec<SearchHit>> {
    remote_search(&state, query, limit).await
}

async fn remote_search(
    state: &AppState,
    query: String,
    limit: u32,
) -> CommandResult<Vec<SearchHit>> {
    let limit = validate_search_request(&query, limit)?;
    let remote = state.remote_ai.as_ref().ok_or_else(|| CommandError {
        code: CommandErrorCode::Unavailable,
        message: "semantic search requires a configured notes-rs server; local FTS remains available offline".into(),
    })?;
    remote.search(query, limit).await.map_err(err)
}

fn validate_search_request(query: &str, limit: u32) -> CommandResult<u32> {
    if query.trim().is_empty() || query.chars().count() > 4_096 {
        return Err(CommandError::invalid(
            "query must contain 1 to 4096 characters",
        ));
    }
    if !(1..=100).contains(&limit) {
        return Err(CommandError::invalid("limit must be between 1 and 100"));
    }
    Ok(limit)
}

#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub async fn chat_stream(
    state: State<'_, AppState>,
    history: Vec<ChatTurn>,
    message: String,
    allow_writes: bool,
    active_node_uuid: Option<uuid::Uuid>,
    request_id: uuid::Uuid,
    on_event: Channel<ChatEvent>,
) -> CommandResult<String> {
    let cancellation = CancellationToken::new();
    {
        let mut active = state
            .chat_cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if active.insert(request_id, cancellation.clone()).is_some() {
            return Err(CommandError::conflict(
                "a chat request with this ID is already active",
            ));
        }
    }
    let remote = state.remote_ai.as_ref().ok_or_else(|| CommandError {
        code: CommandErrorCode::Unavailable,
        message: "AI chat requires a configured notes-rs server".into(),
    })?;
    let result = remote
        .chat_stream(
            history,
            message,
            allow_writes,
            active_node_uuid,
            cancellation,
            move |event| {
                let _ = on_event.send(event);
            },
        )
        .await;
    state
        .chat_cancellations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&request_id);
    result.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub fn cancel_chat(state: State<'_, AppState>, request_id: uuid::Uuid) -> bool {
    let active = state
        .chat_cancellations
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(cancellation) = active.get(&request_id) {
        cancellation.cancel();
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_errors_expose_stable_categories() {
        assert!(matches!(
            err(anyhow::Error::from(notes_core::CoreError::invalid(
                "title is required"
            )))
            .code,
            CommandErrorCode::InvalidInput
        ));
        assert!(matches!(
            err(anyhow::Error::from(notes_core::CoreError::not_found(
                "block was not found"
            )))
            .code,
            CommandErrorCode::NotFound
        ));
        assert!(matches!(
            err(anyhow::Error::from(notes_core::CoreError::conflict(
                "request is already active"
            )))
            .code,
            CommandErrorCode::Conflict
        ));
        assert!(matches!(
            err(anyhow::Error::from(
                notes_sync::TransportError::Unauthorized
            ))
            .code,
            CommandErrorCode::Unavailable
        ));
    }
}

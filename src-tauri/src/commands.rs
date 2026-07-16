use crate::agent::{ChatEvent, ChatTurn};
use crate::db::{self, Node, SearchHit};
use crate::embed::{EmbedderBackend, RerankBackend};
use crate::sqlite::Connection;
use anyhow::Context;
use base64::Engine;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

pub struct AppState {
    pub conn: Connection,
    pub embedder: Arc<dyn EmbedderBackend>,
    pub reranker: Arc<dyn RerankBackend>,
    pub background_paused: Arc<AtomicBool>,
    pub chat_cancellations: Arc<std::sync::Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundStatus {
    pub paused: bool,
    pub embeddings_pending: i64,
    pub embeddings_failed: i64,
    pub extractions_pending: i64,
    pub extractions_failed: i64,
}

/// Registered immediately in setup so the frontend can ask whether the heavy
/// initialization (embedder download, db open) has finished. Without this,
/// commands invoked during the ~minutes-long first-run model fetch fail with
/// an opaque "state not managed" error.
pub struct Startup {
    pub status: Arc<RwLock<StartupStatus>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum StartupStatus {
    Starting { message: String },
    Ready,
    Error { message: String },
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[tauri::command]
pub fn is_ready(state: State<'_, Startup>) -> bool {
    matches!(
        *state.status.read().unwrap_or_else(|e| e.into_inner()),
        StartupStatus::Ready
    )
}

#[tauri::command]
pub fn startup_status(state: State<'_, Startup>) -> StartupStatus {
    state
        .status
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

#[tauri::command]
pub fn load_settings(app: AppHandle) -> Result<crate::settings::SettingsSnapshot, String> {
    crate::settings::load(&app).map_err(err)
}

#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    update: crate::settings::SettingsUpdate,
) -> Result<crate::settings::SettingsSnapshot, String> {
    crate::settings::save(&app, update).map_err(err)
}

#[tauri::command]
pub fn restart_app(app: AppHandle) {
    app.restart()
}

#[tauri::command]
pub async fn background_status(state: State<'_, AppState>) -> Result<BackgroundStatus, String> {
    let queues = db::queue_status(&state.conn).await.map_err(err)?;
    Ok(BackgroundStatus {
        paused: state.background_paused.load(Ordering::Acquire),
        embeddings_pending: queues.0,
        embeddings_failed: queues.1,
        extractions_pending: queues.2,
        extractions_failed: queues.3,
    })
}

#[tauri::command]
pub fn set_background_paused(state: State<'_, AppState>, paused: bool) {
    state.background_paused.store(paused, Ordering::Release);
}

#[tauri::command]
pub async fn retry_background_jobs(state: State<'_, AppState>) -> Result<(), String> {
    db::retry_background_jobs(&state.conn).await.map_err(err)
}

#[tauri::command]
pub async fn clear_background_jobs(state: State<'_, AppState>) -> Result<(), String> {
    db::clear_background_jobs(&state.conn).await.map_err(err)
}

#[tauri::command]
pub async fn create_node(
    state: State<'_, AppState>,
    kind: String,
    title: Option<String>,
    content: String,
    content_json: Option<String>,
) -> Result<Node, String> {
    db::create_node(&state.conn, kind, title, content, content_json)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn update_node(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    title: Option<String>,
    content: String,
    content_json: Option<String>,
) -> Result<(), String> {
    db::update_node(&state.conn, id, title, content, content_json)
        .await
        .map_err(err)?;
    let _ = app.emit("pages:changed", ());
    Ok(())
}

#[tauri::command]
pub async fn update_block_with_refs(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    block: db::BlockContent,
) -> Result<(Node, u32), String> {
    let result = db::update_block_with_refs(&state.conn, id, block)
        .await
        .map_err(err)?;
    let _ = app.emit("pages:changed", ());
    Ok(result)
}

#[tauri::command]
pub async fn split_block(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    parts: Vec<db::BlockContent>,
) -> Result<Vec<Node>, String> {
    db::checkpoint_history(&state.conn, "split block")
        .await
        .map_err(err)?;
    let nodes = db::split_block(&state.conn, id, parts).await.map_err(err)?;
    let _ = app.emit("pages:changed", ());
    Ok(nodes)
}

#[tauri::command]
pub async fn link_nodes(
    state: State<'_, AppState>,
    src: i64,
    dst: i64,
    kind: String,
    weight: Option<f64>,
) -> Result<(), String> {
    db::link_nodes(&state.conn, src, dst, kind, weight.unwrap_or(1.0))
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn get_node(state: State<'_, AppState>, id: i64) -> Result<Option<Node>, String> {
    db::get_node(&state.conn, id).await.map_err(err)
}

#[tauri::command]
pub async fn neighbors(
    state: State<'_, AppState>,
    id: i64,
    depth: u32,
) -> Result<Vec<Node>, String> {
    db::neighbors(&state.conn, id, depth).await.map_err(err)
}

#[tauri::command]
pub async fn list_entities(
    state: State<'_, AppState>,
    limit: Option<u32>,
) -> Result<Vec<Node>, String> {
    db::list_entities(&state.conn, limit.unwrap_or(50))
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn list_pages(
    state: State<'_, AppState>,
    limit: Option<u32>,
) -> Result<Vec<Node>, String> {
    db::list_pages(&state.conn, limit.unwrap_or(200))
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn delete_page(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<bool, String> {
    db::checkpoint_history(&state.conn, "delete page")
        .await
        .map_err(err)?;
    write_backup(&app, &state.conn, "before-delete")
        .await
        .map_err(err)?;
    let attachments = db::delete_page(&state.conn, id).await.map_err(err)?;
    if attachments.is_some() {
        // Retain attachment payloads so structural Undo can restore their
        // database nodes. Explicit attachment deletion removes the file.
        let _ = app.emit("pages:changed", ());
        let _ = app.emit("entities:changed", ());
    }
    Ok(attachments.is_some())
}

#[tauri::command]
pub async fn find_backlinks(
    state: State<'_, AppState>,
    id: i64,
    kind: Option<String>,
) -> Result<Vec<Node>, String> {
    db::find_backlinks(&state.conn, id, kind).await.map_err(err)
}

#[tauri::command]
pub async fn graph_snapshot(
    state: State<'_, AppState>,
    focus_id: Option<i64>,
) -> Result<db::GraphSnapshot, String> {
    db::graph_snapshot(&state.conn, focus_id).await.map_err(err)
}

#[tauri::command]
pub async fn export_data(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("notes-rs archive", &["json"])
        .set_file_name(format!(
            "notes-rs-{}.json",
            chrono::Utc::now().format("%Y%m%d-%H%M%S")
        ))
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = path.into_path().map_err(err)?;
    let mut archive = db::export_archive(&state.conn).await.map_err(err)?;
    add_archive_files(&app, &mut archive).map_err(err)?;
    let json = serde_json::to_string_pretty(&archive).map_err(err)?;
    std::fs::write(&path, json).map_err(err)?;
    Ok(Some(path.display().to_string()))
}

#[tauri::command]
pub async fn import_data(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
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
    let archive: db::DataArchive = serde_json::from_str(&json).map_err(err)?;
    write_backup(&app, &state.conn, "before-import")
        .await
        .map_err(err)?;
    restore_archive(&app, &state.conn, archive)
        .await
        .map_err(err)?;
    let _ = app.emit("pages:changed", ());
    let _ = app.emit("entities:changed", ());
    Ok(Some(path.display().to_string()))
}

#[tauri::command]
pub async fn create_backup(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    write_backup(&app, &state.conn, "manual")
        .await
        .map(|path| path.display().to_string())
        .map_err(err)
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
pub fn choose_sync_directory(app: AppHandle) -> Result<Option<String>, String> {
    app.dialog()
        .file()
        .blocking_pick_folder()
        .map(|path| path.into_path().map(|path| path.display().to_string()))
        .transpose()
        .map_err(err)
}

#[tauri::command]
pub async fn sync_push(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let settings = crate::settings::load(&app).map_err(err)?;
    let directory = sync_directory(&settings.sync_directory).map_err(err)?;
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
    Ok(path.display().to_string())
}

#[tauri::command]
pub async fn sync_pull(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let settings = crate::settings::load(&app).map_err(err)?;
    let path = sync_directory(&settings.sync_directory)
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
    let _ = app.emit("pages:changed", ());
    let _ = app.emit("entities:changed", ());
    Ok(path.display().to_string())
}

fn sync_directory(value: &str) -> anyhow::Result<std::path::PathBuf> {
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("choose a sync directory in Settings first");
    }
    Ok(std::path::PathBuf::from(value))
}

#[tauri::command]
pub async fn attach_file(
    app: AppHandle,
    state: State<'_, AppState>,
    parent_id: i64,
) -> Result<Option<Node>, String> {
    let Some(source) = app.dialog().file().blocking_pick_file() else {
        return Ok(None);
    };
    let source = source.into_path().map_err(err)?;
    let metadata = source.metadata().map_err(err)?;
    if !metadata.is_file() {
        return Err("attachments must be regular files".into());
    }
    if metadata.len() > 100 * 1024 * 1024 {
        return Err("attachments are limited to 100 MiB".into());
    }
    let title = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "attachment filename is not valid UTF-8".to_string())?
        .to_string();
    let uuid = uuid::Uuid::new_v4().to_string();
    let relative = std::path::PathBuf::from("attachments")
        .join(&uuid)
        .join(&title);
    let destination = app.path().app_data_dir().map_err(err)?.join(&relative);
    std::fs::create_dir_all(destination.parent().expect("attachment has parent")).map_err(err)?;
    std::fs::copy(&source, &destination).map_err(err)?;
    let metadata_json = serde_json::json!({ "size": metadata.len() }).to_string();
    match db::create_attachment(
        &state.conn,
        parent_id,
        uuid,
        title,
        relative.to_string_lossy().into_owned(),
        metadata_json,
    )
    .await
    {
        Ok(node) => Ok(Some(node)),
        Err(error) => {
            let _ = std::fs::remove_file(destination);
            Err(err(error))
        }
    }
}

#[tauri::command]
pub async fn list_attachments(
    state: State<'_, AppState>,
    parent_id: i64,
) -> Result<Vec<Node>, String> {
    db::list_attachments(&state.conn, parent_id)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn open_attachment(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<(), String> {
    let node = db::get_node(&state.conn, id)
        .await
        .map_err(err)?
        .filter(|node| node.kind == "attachment")
        .ok_or_else(|| "attachment not found".to_string())?;
    let path = safe_app_data_path(&app, &node.content).map_err(err)?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(err)
}

#[tauri::command]
pub async fn delete_attachment(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<bool, String> {
    write_backup(&app, &state.conn, "before-attachment-delete")
        .await
        .map_err(err)?;
    let Some(node) = db::delete_attachment(&state.conn, id).await.map_err(err)? else {
        return Ok(false);
    };
    let path = safe_app_data_path(&app, &node.content).map_err(err)?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(err)?;
    }
    Ok(true)
}

fn add_archive_files(app: &AppHandle, archive: &mut db::DataArchive) -> anyhow::Result<()> {
    for node in archive
        .nodes
        .iter()
        .filter(|node| node.kind == "attachment")
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
pub async fn list_block_children(
    state: State<'_, AppState>,
    parent_id: i64,
) -> Result<Vec<Node>, String> {
    db::list_block_children(&state.conn, parent_id)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn create_block(
    state: State<'_, AppState>,
    parent_id: Option<i64>,
    position: Option<f64>,
    content: String,
    content_json: Option<String>,
) -> Result<Node, String> {
    db::checkpoint_history(&state.conn, "create block")
        .await
        .map_err(err)?;
    db::create_block(&state.conn, parent_id, position, content, content_json)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn move_block(
    state: State<'_, AppState>,
    id: i64,
    new_parent_id: Option<i64>,
    new_position: Option<f64>,
) -> Result<Node, String> {
    db::checkpoint_history(&state.conn, "move block")
        .await
        .map_err(err)?;
    db::move_block(&state.conn, id, new_parent_id, new_position)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn reorder_block(
    state: State<'_, AppState>,
    id: i64,
    direction: String,
) -> Result<Node, String> {
    db::checkpoint_history(&state.conn, "reorder block")
        .await
        .map_err(err)?;
    db::reorder_block(&state.conn, id, direction)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn delete_block(state: State<'_, AppState>, id: i64) -> Result<bool, String> {
    db::checkpoint_history(&state.conn, "delete block")
        .await
        .map_err(err)?;
    db::delete_block(&state.conn, id).await.map_err(err)
}

#[tauri::command]
pub async fn replace_block_refs(
    app: AppHandle,
    state: State<'_, AppState>,
    block_id: i64,
    wikilink_titles: Vec<String>,
    block_uuids: Vec<String>,
) -> Result<u32, String> {
    let broken = db::replace_block_refs(&state.conn, block_id, wikilink_titles, block_uuids)
        .await
        .map_err(err)?;
    let _ = app.emit("pages:changed", ());
    Ok(broken)
}

#[tauri::command]
pub async fn get_page_by_title(
    state: State<'_, AppState>,
    title: String,
) -> Result<Option<Node>, String> {
    db::get_page_by_title(&state.conn, title).await.map_err(err)
}

#[tauri::command]
pub async fn get_node_by_uuid(
    state: State<'_, AppState>,
    uuid: String,
) -> Result<Option<Node>, String> {
    db::get_node_by_uuid(&state.conn, uuid).await.map_err(err)
}

#[tauri::command]
pub async fn get_or_create_page_by_title(
    app: AppHandle,
    state: State<'_, AppState>,
    title: String,
) -> Result<Node, String> {
    let page = db::get_or_create_page_by_title(&state.conn, title)
        .await
        .map_err(err)?;
    let _ = app.emit("pages:changed", ());
    Ok(page)
}

#[tauri::command]
pub async fn create_page(
    app: AppHandle,
    state: State<'_, AppState>,
    title: String,
) -> Result<Node, String> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("title is required".into());
    }
    db::checkpoint_history(&state.conn, "create page")
        .await
        .map_err(err)?;
    let page = db::create_node(&state.conn, "page".into(), Some(title), String::new(), None)
        .await
        .map_err(err)?;
    let _ = app.emit("pages:changed", ());
    Ok(page)
}

#[tauri::command]
pub async fn history_status(state: State<'_, AppState>) -> Result<(i64, i64), String> {
    db::history_status(&state.conn).await.map_err(err)
}

#[tauri::command]
pub async fn undo(app: AppHandle, state: State<'_, AppState>) -> Result<bool, String> {
    let changed = db::undo_history(&state.conn).await.map_err(err)?;
    if changed {
        let _ = app.emit("pages:changed", ());
        let _ = app.emit("entities:changed", ());
        let _ = app.emit("history:changed", ());
    }
    Ok(changed)
}

#[tauri::command]
pub async fn redo(app: AppHandle, state: State<'_, AppState>) -> Result<bool, String> {
    let changed = db::redo_history(&state.conn).await.map_err(err)?;
    if changed {
        let _ = app.emit("pages:changed", ());
        let _ = app.emit("entities:changed", ());
        let _ = app.emit("history:changed", ());
    }
    Ok(changed)
}

#[tauri::command]
pub async fn get_containing_page(
    state: State<'_, AppState>,
    id: i64,
) -> Result<Option<Node>, String> {
    db::get_containing_page(&state.conn, id).await.map_err(err)
}

#[tauri::command]
pub async fn search_pages_by_title(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> Result<Vec<Node>, String> {
    db::search_pages_by_title(&state.conn, query, limit)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn search_blocks_fts(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> Result<Vec<Node>, String> {
    db::search_blocks_fts(&state.conn, query, limit)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn search_fts(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> Result<Vec<SearchHit>, String> {
    let limit = validate_search_request(&query, limit)?;
    db::search_fts(&state.conn, query, limit).await.map_err(err)
}

#[tauri::command]
pub async fn search_vec(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> Result<Vec<SearchHit>, String> {
    let limit = validate_search_request(&query, limit)?;
    let emb = state.embedder.embed_query(query).await.map_err(err)?;
    db::search_vec(&state.conn, emb, limit).await.map_err(err)
}

#[tauri::command]
pub async fn search_hybrid(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> Result<Vec<SearchHit>, String> {
    let limit = validate_search_request(&query, limit)?;
    let emb = state
        .embedder
        .embed_query(query.clone())
        .await
        .map_err(err)?;
    db::search_hybrid(&state.conn, query, emb, limit)
        .await
        .map_err(err)
}

/// Retrieve via hybrid RRF, then rerank with the configured provider.
#[tauri::command]
pub async fn search_agentic(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> Result<Vec<SearchHit>, String> {
    let limit = validate_search_request(&query, limit)?;
    let emb = state
        .embedder
        .embed_query(query.clone())
        .await
        .map_err(err)?;
    // See SearchAgentic for rationale; widening the rerank pool matters even
    // more here because the UI can ask for `limit = 3` and starve the
    // reranker otherwise.
    let pool = limit.saturating_mul(4).max(32);
    let candidates = db::search_hybrid(&state.conn, query.clone(), emb, pool)
        .await
        .map_err(err)?;
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let docs: Vec<String> = candidates
        .iter()
        .map(|h| {
            let title = h.node.title.as_deref().unwrap_or("");
            format!("{title}\n{}", h.node.content)
        })
        .collect();
    let scored = state.reranker.rerank(query, docs).await.map_err(err)?;
    let mut out: Vec<SearchHit> = scored
        .into_iter()
        .take(limit as usize)
        .filter_map(|(idx, score)| {
            candidates.get(idx).map(|candidate| SearchHit {
                node: candidate.node.clone(),
                score: score as f64,
            })
        })
        .collect();
    out.truncate(limit as usize);
    Ok(out)
}

#[tauri::command]
pub async fn rerank(
    state: State<'_, AppState>,
    query: String,
    documents: Vec<String>,
) -> Result<Vec<(usize, f32)>, String> {
    if query.trim().is_empty() || query.chars().count() > 4_096 {
        return Err("query must contain 1 to 4096 characters".into());
    }
    if documents.len() > 128 || documents.iter().any(|document| document.len() > 100_000) {
        return Err("reranking accepts at most 128 documents of at most 100000 bytes each".into());
    }
    state.reranker.rerank(query, documents).await.map_err(err)
}

fn validate_search_request(query: &str, limit: u32) -> Result<u32, String> {
    if query.trim().is_empty() || query.chars().count() > 4_096 {
        return Err("query must contain 1 to 4096 characters".into());
    }
    if !(1..=100).contains(&limit) {
        return Err("limit must be between 1 and 100".into());
    }
    Ok(limit)
}

#[tauri::command]
pub async fn chat(state: State<'_, AppState>, message: String) -> Result<String, String> {
    crate::agent::run_chat(
        state.conn.clone(),
        state.embedder.clone(),
        state.reranker.clone(),
        message,
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn chat_stream(
    state: State<'_, AppState>,
    history: Vec<ChatTurn>,
    message: String,
    allow_writes: bool,
    active_node_id: Option<i64>,
    request_id: String,
    on_event: Channel<ChatEvent>,
) -> Result<String, String> {
    if request_id.len() > 128 || request_id.trim().is_empty() {
        return Err("invalid chat request ID".into());
    }
    let cancellation = Arc::new(AtomicBool::new(false));
    {
        let mut active = state
            .chat_cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if active
            .insert(request_id.clone(), cancellation.clone())
            .is_some()
        {
            return Err("a chat request with this ID is already active".into());
        }
    }
    let on_event = Arc::new(on_event);
    let emit = move |ev: ChatEvent| {
        let _ = on_event.send(ev);
    };
    let result = crate::agent::run_chat_stream(
        state.conn.clone(),
        state.embedder.clone(),
        state.reranker.clone(),
        history,
        message,
        allow_writes,
        active_node_id,
        cancellation,
        emit,
    )
    .await;
    state
        .chat_cancellations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&request_id);
    result.map_err(|error| error.to_string())
}

#[tauri::command]
pub fn cancel_chat(state: State<'_, AppState>, request_id: String) -> bool {
    let active = state
        .chat_cancellations
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(cancellation) = active.get(&request_id) {
        cancellation.store(true, Ordering::Release);
        true
    } else {
        false
    }
}

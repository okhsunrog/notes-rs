use crate::agent::{ChatEvent, ChatTurn};
use crate::db::{self, Node, SearchHit};
use crate::embed::{EmbedderBackend, RerankBackend};
use crate::sqlite::Connection;
use serde::Serialize;
use std::sync::{Arc, RwLock};
use tauri::{AppHandle, Emitter, State};
use tauri::ipc::Channel;

pub struct AppState {
    pub conn: Connection,
    pub embedder: Arc<dyn EmbedderBackend>,
    pub reranker: Arc<dyn RerankBackend>,
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
    let nodes = db::split_block(&state.conn, id, parts)
        .await
        .map_err(err)?;
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
    db::reorder_block(&state.conn, id, direction)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn delete_block(state: State<'_, AppState>, id: i64) -> Result<bool, String> {
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
    let page = db::create_node(&state.conn, "page".into(), Some(title), String::new(), None)
        .await
        .map_err(err)?;
    let _ = app.emit("pages:changed", ());
    Ok(page)
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
    db::search_fts(&state.conn, query, limit).await.map_err(err)
}

#[tauri::command]
pub async fn search_vec(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> Result<Vec<SearchHit>, String> {
    let emb = state.embedder.embed_query(query).await.map_err(err)?;
    db::search_vec(&state.conn, emb, limit).await.map_err(err)
}

#[tauri::command]
pub async fn search_hybrid(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> Result<Vec<SearchHit>, String> {
    let emb = state
        .embedder
        .embed_query(query.clone())
        .await
        .map_err(err)?;
    db::search_hybrid(&state.conn, query, emb, limit)
        .await
        .map_err(err)
}

/// Retrieve via hybrid RRF, then rerank with BGE-Reranker-v2-M3.
#[tauri::command]
pub async fn search_agentic(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> Result<Vec<SearchHit>, String> {
    let emb = state
        .embedder
        .embed_query(query.clone())
        .await
        .map_err(err)?;
    // See SearchAgentic for rationale; widening the rerank pool matters even
    // more here because the UI can ask for `limit = 3` and starve the
    // reranker otherwise.
    let pool = (limit * 4).max(32);
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
        .map(|(idx, score)| SearchHit {
            node: candidates[idx].node.clone(),
            score: score as f64,
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
    state.reranker.rerank(query, documents).await.map_err(err)
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
    on_event: Channel<ChatEvent>,
) -> Result<String, String> {
    let on_event = Arc::new(on_event);
    let emit = move |ev: ChatEvent| {
        let _ = on_event.send(ev);
    };
    crate::agent::run_chat_stream(
        state.conn.clone(),
        state.embedder.clone(),
        state.reranker.clone(),
        history,
        message,
        emit,
    )
    .await
    .map_err(|e| e.to_string())
}

use crate::agent::{ChatEvent, ChatTurn};
use crate::db::{self, Node, SearchHit};
use crate::embed::{EmbedderBackend, Reranker};
use std::sync::Arc;
use tauri::State;
use tauri::ipc::Channel;
use tokio_rusqlite::Connection;

pub struct AppState {
    pub conn: Connection,
    pub embedder: Arc<dyn EmbedderBackend>,
    pub reranker: Arc<Reranker>,
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
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
    state: State<'_, AppState>,
    id: i64,
    title: Option<String>,
    content: String,
    content_json: Option<String>,
) -> Result<(), String> {
    db::update_node(&state.conn, id, title, content, content_json)
        .await
        .map_err(err)
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
pub async fn create_page(state: State<'_, AppState>, title: String) -> Result<Node, String> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("title is required".into());
    }
    db::create_node(&state.conn, "page".into(), Some(title), String::new(), None)
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
    let candidates = db::search_hybrid(&state.conn, query.clone(), emb, limit * 4)
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

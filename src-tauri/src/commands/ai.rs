use super::*;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    Fts,
    Semantic,
}

#[tauri::command]
#[specta::specta]
pub async fn search_notes(
    state: State<'_, AppState>,
    mode: SearchMode,
    query: String,
    limit: u32,
) -> CommandResult<Vec<SearchHit>> {
    let limit = validate_search_request(&query, limit)?;
    match mode {
        SearchMode::Fts => db::search_fts(&state.conn, query, limit).await.map_err(err),
        SearchMode::Semantic => remote_search(&state, query, limit).await,
    }
}

async fn remote_search(
    state: &AppState,
    query: String,
    limit: u32,
) -> CommandResult<Vec<SearchHit>> {
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

use super::*;

#[tauri::command]
#[specta::specta]
pub async fn list_block_children(
    state: State<'_, AppState>,
    page_uuid: uuid::Uuid,
    parent_uuid: Option<uuid::Uuid>,
) -> CommandResult<Vec<db::Block>> {
    db::list_block_children(&state.conn, page_uuid, parent_uuid)
        .await
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn get_block(
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<Option<db::Block>> {
    db::get_block(&state.conn, uuid).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn create_block(
    app: AppHandle,
    state: State<'_, AppState>,
    page_uuid: uuid::Uuid,
    parent_uuid: Option<uuid::Uuid>,
    after_uuid: Option<uuid::Uuid>,
    style: BlockStyle,
    markdown: String,
) -> CommandResult<db::Block> {
    let applied = db::create_block_with_ops(
        &state.conn,
        page_uuid,
        parent_uuid,
        after_uuid,
        style,
        markdown,
    )
    .await
    .map_err(err)?;
    emit_events_for_ops(&app, &state.conn, &applied.operations, &[]).await;
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(applied.value)
}

#[tauri::command]
#[specta::specta]
pub async fn set_block_content(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
    content: db::BlockContent,
    expected_revision: ContentRevision,
) -> CommandResult<db::Block> {
    let applied =
        db::set_block_content_if_revision_with_ops(&state.conn, uuid, content, expected_revision)
            .await
            .map_err(err)?;
    let graph_changed = applied
        .value
        .1
        .then_some(uuid)
        .into_iter()
        .collect::<Vec<_>>();
    emit_events_for_ops(&app, &state.conn, &applied.operations, &graph_changed).await;
    Ok(applied.value.0)
}

#[tauri::command]
#[specta::specta]
pub async fn set_block_style(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
    style: BlockStyle,
) -> CommandResult<db::Block> {
    let applied = db::set_block_style_with_ops(&state.conn, uuid, style)
        .await
        .map_err(err)?;
    emit_events_for_ops(&app, &state.conn, &applied.operations, &[]).await;
    if !applied.operations.is_empty() {
        emit_domain(&app, DomainEvent::HistoryChanged);
    }
    Ok(applied.value)
}

#[tauri::command]
#[specta::specta]
pub async fn set_task_state(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
    task_state: TaskState,
) -> CommandResult<db::Block> {
    let applied = db::set_task_state_with_ops(&state.conn, uuid, task_state)
        .await
        .map_err(err)?;
    emit_events_for_ops(&app, &state.conn, &applied.operations, &[]).await;
    if !applied.operations.is_empty() {
        emit_domain(&app, DomainEvent::HistoryChanged);
    }
    Ok(applied.value)
}

#[tauri::command]
#[specta::specta]
pub async fn split_block(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
    parts: Vec<db::BlockContent>,
    expected_revision: ContentRevision,
) -> CommandResult<Vec<db::Block>> {
    let applied = db::split_block_with_ops(&state.conn, uuid, parts, expected_revision)
        .await
        .map_err(err)?;
    let graph_changed = applied
        .value
        .1
        .then_some(uuid)
        .into_iter()
        .collect::<Vec<_>>();
    emit_events_for_ops(&app, &state.conn, &applied.operations, &graph_changed).await;
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(applied.value.0)
}

async fn change_indent(
    app: &AppHandle,
    state: &AppState,
    uuid: uuid::Uuid,
    indent: bool,
) -> CommandResult<db::Block> {
    let applied = if indent {
        db::indent_block_with_ops(&state.conn, uuid).await
    } else {
        db::outdent_block_with_ops(&state.conn, uuid).await
    }
    .map_err(err)?;
    emit_events_for_ops(app, &state.conn, &applied.operations, &[]).await;
    if !applied.operations.is_empty() {
        emit_domain(app, DomainEvent::HistoryChanged);
    }
    Ok(applied.value)
}

#[tauri::command]
#[specta::specta]
pub async fn indent_block(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<db::Block> {
    change_indent(&app, &state, uuid, true).await
}

#[tauri::command]
#[specta::specta]
pub async fn outdent_block(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<db::Block> {
    change_indent(&app, &state, uuid, false).await
}

async fn move_direction(
    app: &AppHandle,
    state: &AppState,
    uuid: uuid::Uuid,
    direction: ReorderDirection,
) -> CommandResult<db::Block> {
    let applied = db::move_block_in_direction_with_ops(&state.conn, uuid, direction)
        .await
        .map_err(err)?;
    emit_events_for_ops(app, &state.conn, &applied.operations, &[]).await;
    if !applied.operations.is_empty() {
        emit_domain(app, DomainEvent::HistoryChanged);
    }
    Ok(applied.value)
}

#[tauri::command]
#[specta::specta]
pub async fn move_block_up(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<db::Block> {
    move_direction(&app, &state, uuid, ReorderDirection::Up).await
}

#[tauri::command]
#[specta::specta]
pub async fn move_block_down(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<db::Block> {
    move_direction(&app, &state, uuid, ReorderDirection::Down).await
}

#[tauri::command]
#[specta::specta]
pub async fn delete_block(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<bool> {
    let applied = db::delete_block_with_ops(&state.conn, uuid)
        .await
        .map_err(err)?;
    emit_events_for_ops(&app, &state.conn, &applied.operations, &[]).await;
    if applied.value {
        emit_domain(&app, DomainEvent::HistoryChanged);
    }
    Ok(applied.value)
}

#[tauri::command]
#[specta::specta]
pub async fn history_status(state: State<'_, AppState>) -> CommandResult<db::HistoryStatus> {
    db::history_status(&state.conn).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn undo(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<db::HistoryMoveResult> {
    let result = db::undo_history(&state.conn).await.map_err(err)?;
    match result {
        db::HistoryMoveResult::Applied => {
            emit_domain(&app, DomainEvent::WorkspaceChanged);
            emit_domain(&app, DomainEvent::HistoryChanged);
        }
        db::HistoryMoveResult::Skipped => emit_domain(&app, DomainEvent::HistoryChanged),
        db::HistoryMoveResult::Empty => {}
    }
    Ok(result)
}

#[tauri::command]
#[specta::specta]
pub async fn redo(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<db::HistoryMoveResult> {
    let result = db::redo_history(&state.conn).await.map_err(err)?;
    match result {
        db::HistoryMoveResult::Applied => {
            emit_domain(&app, DomainEvent::WorkspaceChanged);
            emit_domain(&app, DomainEvent::HistoryChanged);
        }
        db::HistoryMoveResult::Skipped => emit_domain(&app, DomainEvent::HistoryChanged),
        db::HistoryMoveResult::Empty => {}
    }
    Ok(result)
}

#[tauri::command]
#[specta::specta]
pub async fn search_pages_by_title(
    state: State<'_, AppState>,
    query: String,
    limit: u32,
) -> CommandResult<Vec<db::Page>> {
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
    token_mode: notes_core::SearchTokenMode,
) -> CommandResult<Vec<db::Block>> {
    db::search_blocks_fts(&state.conn, query, limit, token_mode)
        .await
        .map_err(err)
}

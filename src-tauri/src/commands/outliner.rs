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
    let block = db::create_block(
        &state.conn,
        page_uuid,
        parent_uuid,
        after_uuid,
        style,
        markdown,
    )
    .await
    .map_err(err)?;
    emit_blocks_changed(&app, std::slice::from_ref(&block), []);
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(block)
}

#[tauri::command]
#[specta::specta]
pub async fn set_block_content(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
    content: db::BlockContent,
) -> CommandResult<db::Block> {
    let (block, graph_changed) = db::set_block_content(&state.conn, uuid, content)
        .await
        .map_err(err)?;
    emit_blocks_changed(&app, std::slice::from_ref(&block), []);
    if graph_changed {
        emit_domain(
            &app,
            DomainEvent::GraphChanged {
                content_uuids: vec![block.uuid],
            },
        );
    }
    Ok(block)
}

#[tauri::command]
#[specta::specta]
pub async fn set_block_style(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
    style: BlockStyle,
) -> CommandResult<db::Block> {
    let block = db::set_block_style(&state.conn, uuid, style)
        .await
        .map_err(err)?;
    emit_blocks_changed(&app, std::slice::from_ref(&block), []);
    Ok(block)
}

#[tauri::command]
#[specta::specta]
pub async fn split_block(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
    parts: Vec<db::BlockContent>,
) -> CommandResult<Vec<db::Block>> {
    let blocks = db::split_block(&state.conn, uuid, parts)
        .await
        .map_err(err)?;
    emit_blocks_changed(&app, &blocks, []);
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(blocks)
}

async fn change_indent(
    app: &AppHandle,
    state: &AppState,
    uuid: uuid::Uuid,
    indent: bool,
) -> CommandResult<db::Block> {
    let old_container = db::get_block(&state.conn, uuid)
        .await
        .map_err(err)?
        .map(|block| block.parent_uuid.unwrap_or(block.page_uuid));
    let block = if indent {
        db::indent_block(&state.conn, uuid).await
    } else {
        db::outdent_block(&state.conn, uuid).await
    }
    .map_err(err)?;
    emit_blocks_changed(app, std::slice::from_ref(&block), old_container);
    emit_domain(app, DomainEvent::HistoryChanged);
    Ok(block)
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
    let block = db::move_block_in_direction(&state.conn, uuid, direction)
        .await
        .map_err(err)?;
    emit_blocks_changed(app, std::slice::from_ref(&block), []);
    emit_domain(app, DomainEvent::HistoryChanged);
    Ok(block)
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
    let previous = db::get_block(&state.conn, uuid).await.map_err(err)?;
    let deleted = db::delete_block(&state.conn, uuid).await.map_err(err)?;
    if deleted {
        emit_domain(
            &app,
            DomainEvent::BlocksDeleted {
                block_uuids: vec![uuid],
                container_uuids: previous
                    .map(|block| block.parent_uuid.unwrap_or(block.page_uuid))
                    .into_iter()
                    .collect(),
            },
        );
        emit_domain(&app, DomainEvent::HistoryChanged);
    }
    Ok(deleted)
}

#[tauri::command]
#[specta::specta]
pub async fn history_status(state: State<'_, AppState>) -> CommandResult<db::HistoryStatus> {
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
) -> CommandResult<Vec<db::Block>> {
    db::search_blocks_fts(&state.conn, query, limit)
        .await
        .map_err(err)
}

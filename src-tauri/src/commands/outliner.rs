use super::*;

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

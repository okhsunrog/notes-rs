use super::*;

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
    super::data::write_backup(&app, &state.conn, "before-delete")
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

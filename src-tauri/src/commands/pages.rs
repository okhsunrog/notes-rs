use super::*;

#[tauri::command]
#[specta::specta]
pub async fn rename_page(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
    title: Option<String>,
) -> CommandResult<db::Page> {
    let page = db::rename_page(&state.conn, uuid, title)
        .await
        .map_err(err)?;
    emit_pages_changed(&app, std::slice::from_ref(&page));
    Ok(page)
}

#[tauri::command]
#[specta::specta]
pub async fn set_page_layout(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
    layout: PageLayout,
) -> CommandResult<db::Page> {
    let page = db::set_page_layout(&state.conn, uuid, layout)
        .await
        .map_err(err)?;
    emit_pages_changed(&app, std::slice::from_ref(&page));
    Ok(page)
}

#[tauri::command]
#[specta::specta]
pub async fn create_note(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<db::CreatedNote> {
    let note = db::create_note(&state.conn).await.map_err(err)?;
    emit_pages_changed(&app, std::slice::from_ref(&note.page));
    emit_blocks_changed(&app, std::slice::from_ref(&note.initial_block), []);
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(note)
}

#[tauri::command]
#[specta::specta]
pub async fn create_page(
    app: AppHandle,
    state: State<'_, AppState>,
    title: String,
) -> CommandResult<db::Page> {
    let page = db::create_page(&state.conn, title).await.map_err(err)?;
    emit_pages_changed(&app, std::slice::from_ref(&page));
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(page)
}

#[tauri::command]
#[specta::specta]
pub async fn list_pages(
    state: State<'_, AppState>,
    filter: Option<notes_core::PageListFilter>,
    limit: Option<u32>,
) -> CommandResult<Vec<db::Page>> {
    db::list_pages_filtered(
        &state.conn,
        filter.unwrap_or_default(),
        limit.unwrap_or(200),
    )
    .await
    .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn get_page(
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<Option<db::Page>> {
    db::get_page(&state.conn, uuid).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn get_containing_page(
    state: State<'_, AppState>,
    block_uuid: uuid::Uuid,
) -> CommandResult<Option<db::Page>> {
    db::get_containing_page(&state.conn, block_uuid)
        .await
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn get_page_by_title(
    state: State<'_, AppState>,
    title: String,
) -> CommandResult<Option<db::Page>> {
    db::get_page_by_title(&state.conn, title).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn get_or_create_page_by_title(
    app: AppHandle,
    state: State<'_, AppState>,
    title: String,
) -> CommandResult<db::Page> {
    let page = db::get_or_create_page_by_title(&state.conn, title)
        .await
        .map_err(err)?;
    emit_pages_changed(&app, std::slice::from_ref(&page));
    Ok(page)
}

#[tauri::command]
#[specta::specta]
pub async fn delete_page(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<bool> {
    super::data::write_backup(&app, &state.conn, &state.blob_store, "before-delete")
        .await
        .map_err(err)?;
    let deleted = db::delete_page(&state.conn, uuid).await.map_err(err)?;
    if let Some(deleted) = &deleted {
        emit_domain(
            &app,
            DomainEvent::PagesDeleted {
                page_uuids: vec![deleted.page_uuid],
            },
        );
        emit_domain(
            &app,
            DomainEvent::BlocksDeleted {
                block_uuids: deleted.block_uuids.clone(),
                container_uuids: vec![deleted.page_uuid],
            },
        );
        emit_domain(&app, DomainEvent::HistoryChanged);
    }
    Ok(deleted.is_some())
}

#[tauri::command]
#[specta::specta]
pub async fn neighbors(
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
    depth: u32,
) -> CommandResult<Vec<db::Content>> {
    db::neighbors(&state.conn, uuid, depth).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn find_backlinks(
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<Vec<db::Content>> {
    db::find_backlinks(&state.conn, uuid).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn graph_snapshot(
    state: State<'_, AppState>,
    focus_uuid: Option<uuid::Uuid>,
) -> CommandResult<db::GraphSnapshot> {
    db::graph_snapshot(&state.conn, focus_uuid)
        .await
        .map_err(err)
}

use super::*;

#[tauri::command]
#[specta::specta]
pub async fn rename_page(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
    title: Option<String>,
    expected_revision: ContentRevision,
) -> CommandResult<db::Page> {
    let applied = db::rename_page_if_revision_with_ops(&state.conn, uuid, title, expected_revision)
        .await
        .map_err(err)?;
    emit_events_for_ops(&app, &state.conn, &applied.operations, &[]).await;
    Ok(applied.value)
}

#[tauri::command]
#[specta::specta]
pub async fn set_page_layout(
    app: AppHandle,
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
    layout: PageLayout,
) -> CommandResult<db::Page> {
    let applied = db::set_page_layout_with_ops(&state.conn, uuid, layout)
        .await
        .map_err(err)?;
    emit_events_for_ops(&app, &state.conn, &applied.operations, &[]).await;
    Ok(applied.value)
}

#[tauri::command]
#[specta::specta]
pub async fn create_note(
    app: AppHandle,
    state: State<'_, AppState>,
    title: Option<String>,
) -> CommandResult<db::CreateNoteResult> {
    let applied = db::create_note_with_ops(&state.conn, title)
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
pub async fn create_page(
    app: AppHandle,
    state: State<'_, AppState>,
    title: String,
) -> CommandResult<db::Page> {
    let applied = db::create_page_with_ops(&state.conn, title)
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
pub async fn get_page_document(
    state: State<'_, AppState>,
    uuid: uuid::Uuid,
) -> CommandResult<Option<db::PageDocumentSnapshot>> {
    db::get_page_document(&state.conn, uuid).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn replace_page_document(
    app: AppHandle,
    state: State<'_, AppState>,
    page_uuid: uuid::Uuid,
    expected_revision: DocumentRevision,
    units: Vec<db::DocumentUnitDraft>,
) -> CommandResult<db::PageDocumentSnapshot> {
    let outcome =
        db::replace_page_document_with_outcome(&state.conn, page_uuid, expected_revision, units)
            .await
            .map_err(err)?;
    let graph_changed = if outcome.graph_changed {
        outcome.block_uuids.as_slice()
    } else {
        &[]
    };
    emit_events_for_ops(&app, &state.conn, &outcome.operations, graph_changed).await;
    if !outcome.operations.is_empty() {
        emit_domain(&app, DomainEvent::HistoryChanged);
    }
    Ok(outcome.snapshot)
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
    let applied = db::get_or_create_page_by_title_with_ops(&state.conn, title)
        .await
        .map_err(err)?;
    emit_events_for_ops(&app, &state.conn, &applied.operations, &[]).await;
    Ok(applied.value)
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
        emit_events_for_ops(&app, &state.conn, &deleted.operations, &[]).await;
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

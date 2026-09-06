//! Note-addressed handwriting commands. All authoritative data lives in notes.db.
use super::*;
use notes_core::ink::{
    self, InkDraft, InkDraftPatch, InkHistorySnapshot, InkHistoryUpdate, InkNoteStatus,
};
mod maintenance;
pub use maintenance::HandwritingStore;

#[tauri::command]
#[specta::specta]
pub async fn create_handwritten_note(
    app: AppHandle,
    state: State<'_, AppState>,
    title: Option<String>,
) -> CommandResult<db::Page> {
    let result = db::create_handwritten_note_with_ops(&state.conn, title)
        .await
        .map_err(err)?;
    emit_events_for_ops(&app, &state.conn, &result.operations, &[]).await;
    Ok(result.value)
}
#[tauri::command]
#[specta::specta]
pub async fn load_handwriting_note(
    app: AppHandle,
    _state: State<'_, AppState>,
    store: State<'_, HandwritingStore>,
    page_uuid: uuid::Uuid,
    editing: bool,
) -> CommandResult<InkHistorySnapshot> {
    store
        .session(&app, page_uuid)?
        .open(editing)
        .await
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn save_handwriting_patch(
    app: AppHandle,
    _state: State<'_, AppState>,
    store: State<'_, HandwritingStore>,
    page_uuid: uuid::Uuid,
    patch: InkDraftPatch,
    expected_revision: Option<String>,
) -> CommandResult<String> {
    let session = store.session(&app, page_uuid)?;
    let revision = session.patch(patch, expected_revision).await.map_err(err)?;
    if store.is_background() {
        store.finish_in_background(session);
    }
    Ok(revision)
}
#[tauri::command]
#[specta::specta]
pub async fn handwriting_history(
    app: AppHandle,
    _state: State<'_, AppState>,
    store: State<'_, HandwritingStore>,
    page_uuid: uuid::Uuid,
    redo: bool,
    expected_revision: Option<String>,
) -> CommandResult<InkHistoryUpdate> {
    let session = store.session(&app, page_uuid)?;
    let result = session
        .history(redo, expected_revision)
        .await
        .map_err(err)?;
    if store.is_background() {
        store.finish_in_background(session);
    }
    Ok(result)
}
#[tauri::command]
#[specta::specta]
pub async fn complete_handwriting_note(
    app: AppHandle,
    _state: State<'_, AppState>,
    store: State<'_, HandwritingStore>,
    page_uuid: uuid::Uuid,
) -> CommandResult<InkNoteStatus> {
    let session = store.session(&app, page_uuid)?;
    session.complete().await.map_err(err)?;
    session.access(ink::Store::status).await.map_err(err)
}
#[tauri::command]
#[specta::specta]
pub async fn complete_all_handwriting(store: State<'_, HandwritingStore>) -> CommandResult<()> {
    store.complete_all().await
}
#[tauri::command]
#[specta::specta]
pub async fn handwriting_note_status(
    app: AppHandle,
    _state: State<'_, AppState>,
    store: State<'_, HandwritingStore>,
    page_uuid: uuid::Uuid,
) -> CommandResult<InkNoteStatus> {
    store
        .session(&app, page_uuid)?
        .access(ink::Store::status)
        .await
        .map_err(err)
}
#[tauri::command]
#[specta::specta]
pub async fn preview_handwriting_version(
    app: AppHandle,
    _state: State<'_, AppState>,
    store: State<'_, HandwritingStore>,
    page_uuid: uuid::Uuid,
    version_uuid: uuid::Uuid,
) -> CommandResult<InkDraft> {
    store
        .session(&app, page_uuid)?
        .access(move |s| s.preview(version_uuid))
        .await
        .map_err(err)
}
#[tauri::command]
#[specta::specta]
pub async fn resolve_handwriting_conflict(
    app: AppHandle,
    _state: State<'_, AppState>,
    store: State<'_, HandwritingStore>,
    page_uuid: uuid::Uuid,
    expected_heads: Vec<uuid::Uuid>,
    keep: Vec<uuid::Uuid>,
) -> CommandResult<Vec<uuid::Uuid>> {
    let name = maintenance::device_name(&app);
    let pages = store
        .session(&app, page_uuid)?
        .access(move |s| s.resolve(expected_heads, keep, name))
        .await
        .map_err(err)?;
    emit_domain(
        &app,
        DomainEvent::PagesChanged {
            page_uuids: pages.clone(),
        },
    );
    Ok(pages)
}

#[tauri::command]
#[specta::specta]
pub fn set_handwriting_background(store: State<'_, HandwritingStore>, background: bool) {
    store.set_background(background);
}

use super::*;

#[tauri::command]
#[specta::specta]
pub async fn ensure_journal(
    app: AppHandle,
    state: State<'_, AppState>,
    date: notes_core::JournalDate,
) -> CommandResult<db::Page> {
    let applied = db::ensure_journal_with_ops(&state.conn, date)
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
pub async fn get_journal(
    state: State<'_, AppState>,
    date: notes_core::JournalDate,
) -> CommandResult<Option<db::Page>> {
    db::get_journal(&state.conn, date).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn list_journals(
    state: State<'_, AppState>,
    before_date: Option<notes_core::JournalDate>,
    limit: Option<db::JournalListLimit>,
) -> CommandResult<Vec<db::Page>> {
    db::list_journals(&state.conn, before_date, limit.unwrap_or_default())
        .await
        .map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn append_to_journal(
    app: AppHandle,
    state: State<'_, AppState>,
    date: notes_core::JournalDate,
    content: db::BlockContent,
    style: BlockStyle,
) -> CommandResult<db::Block> {
    let applied = db::append_to_journal_with_ops(&state.conn, date, content, style)
        .await
        .map_err(err)?;
    emit_events_for_ops(&app, &state.conn, &applied.operations, &[]).await;
    // The journals query contains only page/date metadata, not block previews. Existing
    // journals therefore need only the BlocksChanged invalidation emitted above.
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(applied.value)
}

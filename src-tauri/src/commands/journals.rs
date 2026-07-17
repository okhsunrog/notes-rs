use super::*;

#[tauri::command]
#[specta::specta]
pub async fn ensure_journal(
    app: AppHandle,
    state: State<'_, AppState>,
    date: notes_core::JournalDate,
) -> CommandResult<db::Page> {
    let journal = db::ensure_journal(&state.conn, date).await.map_err(err)?;
    emit_domain(
        &app,
        DomainEvent::PagesChanged {
            page_uuids: vec![journal.uuid],
        },
    );
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(journal)
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
    let block = db::append_to_journal(&state.conn, date, content, style)
        .await
        .map_err(err)?;
    emit_domain(
        &app,
        DomainEvent::PagesChanged {
            page_uuids: vec![block.page_uuid],
        },
    );
    emit_blocks_changed(&app, std::slice::from_ref(&block), []);
    if notes_core::content_references_changed("", &block.markdown) {
        emit_domain(
            &app,
            DomainEvent::GraphChanged {
                content_uuids: vec![block.uuid],
            },
        );
    }
    emit_domain(&app, DomainEvent::HistoryChanged);
    Ok(block)
}

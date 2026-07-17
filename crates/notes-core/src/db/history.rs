use super::*;
use crate::operation::{
    AttachmentAdd, AttachmentRemove, BlockCreate, BlockDelete, BlockMove, BlockSetMarkdown,
    BlockSetStyle, PageCreate, PageDelete, PageSetLayout, PageSetTitle,
};
use rusqlite::OptionalExtension;

type HistoryRow = (i64, uuid::Uuid, String, String, String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryStatus {
    pub undo_count: i64,
    pub redo_count: i64,
}

pub(crate) fn capture_inverse_kinds(
    database: &rusqlite::Connection,
    forward: &[OpKind],
) -> rusqlite::Result<Vec<OpKind>> {
    let mut inverse = Vec::new();
    for kind in forward {
        capture_inverse(database, kind, &mut inverse)?;
    }
    inverse.reverse();
    Ok(inverse)
}

fn capture_inverse(
    database: &rusqlite::Connection,
    kind: &OpKind,
    inverse: &mut Vec<OpKind>,
) -> rusqlite::Result<()> {
    match kind {
        OpKind::PageCreate(payload) => {
            inverse.push(OpKind::PageDelete(PageDelete { uuid: payload.uuid }))
        }
        OpKind::PageSetTitle(payload) => {
            if let Some(title) = database
                .query_row(
                    "SELECT title FROM pages WHERE uuid = ?1",
                    [payload.uuid],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
            {
                inverse.push(OpKind::PageSetTitle(PageSetTitle {
                    uuid: payload.uuid,
                    title,
                }));
            }
        }
        OpKind::PageSetLayout(payload) => {
            if let Some(layout) = database
                .query_row(
                    "SELECT layout FROM pages WHERE uuid = ?1",
                    [payload.uuid],
                    |row| row.get::<_, PageLayout>(0),
                )
                .optional()?
            {
                inverse.push(OpKind::PageSetLayout(PageSetLayout {
                    uuid: payload.uuid,
                    layout,
                }));
            }
        }
        OpKind::PageDelete(payload) => {
            if let Some(page) = capture_page(database, payload.uuid)? {
                inverse.push(OpKind::PageCreate(page));
            }
        }
        OpKind::BlockCreate(payload) => inverse.push(OpKind::BlockDelete(BlockDelete {
            uuid: payload.uuid,
            page_uuid: payload.page_uuid,
        })),
        OpKind::BlockSetMarkdown(payload) => {
            if let Some(markdown) = database
                .query_row(
                    "SELECT markdown FROM blocks WHERE uuid = ?1",
                    [payload.uuid],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
            {
                inverse.push(OpKind::BlockSetMarkdown(BlockSetMarkdown {
                    uuid: payload.uuid,
                    markdown,
                }));
            }
        }
        OpKind::BlockSetStyle(payload) => {
            if let Some(style) = database
                .query_row(
                    "SELECT style FROM blocks WHERE uuid = ?1",
                    [payload.uuid],
                    |row| row.get::<_, BlockStyle>(0),
                )
                .optional()?
            {
                inverse.push(OpKind::BlockSetStyle(BlockSetStyle {
                    uuid: payload.uuid,
                    style,
                }));
            }
        }
        OpKind::BlockMove(payload) => {
            if let Some((page_uuid, parent_uuid, order_key)) = database
                .query_row(
                    "SELECT page_uuid, parent_uuid, order_key FROM blocks WHERE uuid = ?1",
                    [payload.uuid],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?
            {
                inverse.push(OpKind::BlockMove(BlockMove {
                    uuid: payload.uuid,
                    page_uuid,
                    parent_uuid,
                    order_key,
                }));
            }
        }
        OpKind::BlockDelete(payload) => {
            if let Some(block) = capture_block(database, payload.uuid)? {
                inverse.push(OpKind::BlockCreate(block));
            }
        }
        OpKind::AttachmentAdd(payload) => {
            if let Some(previous) = attachment_intent(database, payload.owner, &payload.blob_hash)?
            {
                inverse.push(OpKind::AttachmentAdd(previous));
            } else {
                inverse.push(OpKind::AttachmentRemove(AttachmentRemove {
                    owner: payload.owner,
                    blob_hash: payload.blob_hash.clone(),
                }));
            }
        }
        OpKind::AttachmentRemove(payload) => {
            if let Some(previous) = attachment_intent(database, payload.owner, &payload.blob_hash)?
            {
                inverse.push(OpKind::AttachmentAdd(previous));
            }
        }
    }
    Ok(())
}

fn capture_page(
    database: &rusqlite::Connection,
    uuid: uuid::Uuid,
) -> rusqlite::Result<Option<PageCreate>> {
    database
        .query_row(
            "SELECT title, layout, created_at,
                    (SELECT page_kind FROM page_identities WHERE page_uuid = pages.uuid),
                    (SELECT journal_date FROM page_identities WHERE page_uuid = pages.uuid)
               FROM pages WHERE uuid = ?1",
            [uuid],
            |row| {
                let page_kind = row.get::<_, String>(3)?;
                let journal_date = row.get::<_, Option<crate::model::JournalDate>>(4)?;
                let kind = match (page_kind.as_str(), journal_date) {
                    ("note", None) => PageKind::Note,
                    ("journal", Some(date)) => PageKind::Journal { date },
                    _ => return Err(rusqlite::Error::InvalidQuery),
                };
                Ok(PageCreate {
                    uuid,
                    kind,
                    title: row.get(0)?,
                    layout: row.get(1)?,
                    created_at: row.get(2)?,
                })
            },
        )
        .optional()
}

fn capture_block(
    database: &rusqlite::Connection,
    uuid: uuid::Uuid,
) -> rusqlite::Result<Option<BlockCreate>> {
    database
        .query_row(
            "SELECT page_uuid, parent_uuid, order_key, style, markdown, created_at
               FROM blocks WHERE uuid = ?1",
            [uuid],
            |row| {
                Ok(BlockCreate {
                    uuid,
                    page_uuid: row.get(0)?,
                    parent_uuid: row.get(1)?,
                    order_key: row.get(2)?,
                    style: row.get(3)?,
                    markdown: row.get(4)?,
                    created_at: row.get(5)?,
                })
            },
        )
        .optional()
}

fn attachment_intent(
    database: &rusqlite::Connection,
    owner: AttachmentOwner,
    blob_hash: &str,
) -> rusqlite::Result<Option<AttachmentAdd>> {
    database
        .query_row(
            "SELECT filename, mime, size FROM attachment_lww
              WHERE owner_kind = ?1 AND owner_uuid = ?2 AND blob_hash = ?3 AND present = 1",
            rusqlite::params![owner.kind(), owner.uuid(), blob_hash],
            |row| {
                let size = u64::try_from(row.get::<_, i64>(2)?).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Integer,
                        Box::new(error),
                    )
                })?;
                Ok(AttachmentAdd {
                    owner,
                    blob_hash: blob_hash.to_owned(),
                    filename: row.get(0)?,
                    mime: row.get(1)?,
                    size,
                })
            },
        )
        .optional()
}

pub(crate) fn record_action(
    transaction: &rusqlite::Transaction<'_>,
    action: &str,
    forward: Vec<OpKind>,
    inverse: Vec<OpKind>,
) -> rusqlite::Result<()> {
    if inverse.is_empty() {
        return Ok(());
    }
    let action_uuid = uuid::Uuid::now_v7();
    let action = action.to_owned();
    let forward_json = serde_json::to_string(&forward)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let inverse_json = serde_json::to_string(&inverse)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    transaction.execute(
        "INSERT INTO history_undo(action_uuid, action, forward_json, inverse_json, created_at)
         VALUES (?1, ?2, ?3, ?4, unixepoch())",
        rusqlite::params![action_uuid, action, forward_json, inverse_json],
    )?;
    transaction.execute("DELETE FROM history_redo", [])?;
    transaction.execute(
        "DELETE FROM history_undo WHERE id NOT IN (
           SELECT id FROM history_undo ORDER BY id DESC LIMIT 50
         )",
        [],
    )?;
    Ok(())
}

pub async fn history_status(conn: &Connection) -> Result<HistoryStatus> {
    conn.call(|database| {
        database.query_row(
            "SELECT
               (SELECT COUNT(*) FROM history_undo),
               (SELECT COUNT(*) FROM history_redo)",
            [],
            |row| {
                Ok(HistoryStatus {
                    undo_count: row.get(0)?,
                    redo_count: row.get(1)?,
                })
            },
        )
    })
    .await
}

pub async fn undo_history(conn: &Connection) -> Result<bool> {
    move_history(conn, true).await
}

pub async fn redo_history(conn: &Connection) -> Result<bool> {
    move_history(conn, false).await
}

async fn move_history(conn: &Connection, undo: bool) -> Result<bool> {
    conn.call_domain(move |database| -> anyhow::Result<bool> {
        let source = if undo { "history_undo" } else { "history_redo" };
        let target = if undo { "history_redo" } else { "history_undo" };
        let transaction = database.transaction()?;
        let entry: Option<HistoryRow> = transaction
            .query_row(
                &format!(
                    "SELECT id, action_uuid, action, forward_json, inverse_json
                       FROM {source} ORDER BY id DESC LIMIT 1"
                ),
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((entry_id, action_uuid, action, forward_json, inverse_json)) = entry else {
            return Ok(false);
        };
        let kinds: Vec<OpKind> =
            serde_json::from_str(if undo { &inverse_json } else { &forward_json })?;
        operation::apply_local_kinds_in_transaction(&transaction, kinds)?;
        transaction.execute(&format!("DELETE FROM {source} WHERE id = ?1"), [entry_id])?;
        transaction.execute(
            &format!(
                "INSERT INTO {target}(action_uuid, action, forward_json, inverse_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, unixepoch())"
            ),
            rusqlite::params![action_uuid, action, forward_json, inverse_json],
        )?;
        transaction.commit()?;
        Ok(true)
    })
    .await
}

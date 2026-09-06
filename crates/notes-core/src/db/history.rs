use super::*;
use crate::operation::{
    AttachmentAdd, AttachmentRemove, BlockCreate, BlockDelete, BlockMove, BlockSetMarkdown,
    BlockSetStyle, PageAliasSet, PageCreate, PageDelete, PageSetLayout, PageSetTitle,
};
use crate::{Hlc, PageAlias};
use rusqlite::OptionalExtension;
use std::collections::{HashMap, HashSet};

type HistoryRow = (i64, uuid::Uuid, String, String, String);
const HISTORY_PAYLOAD_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryStatus {
    pub undo_count: i64,
    pub redo_count: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum HistoryMoveResult {
    Applied,
    Empty,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "field", rename_all = "snake_case")]
enum HistoryField {
    PageExistence {
        uuid: uuid::Uuid,
    },
    PageTitle {
        uuid: uuid::Uuid,
    },
    PageLayout {
        uuid: uuid::Uuid,
    },
    PageAlias {
        uuid: uuid::Uuid,
        alias: PageAlias,
    },
    BlockExistence {
        uuid: uuid::Uuid,
    },
    BlockMarkdown {
        uuid: uuid::Uuid,
    },
    BlockStyle {
        uuid: uuid::Uuid,
    },
    BlockStructure {
        uuid: uuid::Uuid,
    },
    Attachment {
        owner: AttachmentOwner,
        blob_hash: notes_blob::BlobHash,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistoryGuard {
    field: HistoryField,
    expected_hlc: Option<Hlc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
enum HistoryScopeGuard {
    PageBlocks {
        page_uuid: uuid::Uuid,
        expected_block_uuids: Vec<uuid::Uuid>,
    },
    BlockDescendants {
        block_uuid: uuid::Uuid,
        expected_block_uuids: Vec<uuid::Uuid>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GuardedHistoryPayload {
    history_format_version: u32,
    operations: Vec<OpKind>,
    guards: Vec<HistoryGuard>,
    #[serde(default)]
    scope_guards: Vec<HistoryScopeGuard>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredHistoryPayload {
    Guarded(GuardedHistoryPayload),
    Legacy(Vec<OpKind>),
}

fn encode_history_payload(payload: &GuardedHistoryPayload) -> serde_json::Result<String> {
    operation::encode_persisted_envelope(payload)
}

fn decode_history_payload(value: &str) -> serde_json::Result<GuardedHistoryPayload> {
    match operation::decode_persisted_envelope::<StoredHistoryPayload>(value)? {
        StoredHistoryPayload::Guarded(payload)
            if payload.history_format_version == HISTORY_PAYLOAD_VERSION =>
        {
            Ok(payload)
        }
        StoredHistoryPayload::Guarded(payload) => {
            Err(<serde_json::Error as serde::de::Error>::custom(format!(
                "unsupported history payload format version {}",
                payload.history_format_version
            )))
        }
        StoredHistoryPayload::Legacy(operations) => Ok(GuardedHistoryPayload {
            history_format_version: HISTORY_PAYLOAD_VERSION,
            operations,
            // Pre-guard history cannot be checked retroactively. Preserve its existing
            // behavior once; the entry becomes guarded when moved to the opposite stack.
            guards: Vec::new(),
            scope_guards: Vec::new(),
        }),
    }
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
        OpKind::InkPublish(_) => return Err(rusqlite::Error::InvalidQuery),
        OpKind::PageCreate(payload) => {
            inverse.push(OpKind::PageDelete(PageDelete { uuid: payload.uuid }))
        }
        OpKind::PageAliasSet(payload) => {
            let previous = database
                .query_row(
                    "SELECT present FROM page_alias_lww
                      WHERE page_uuid = ?1 AND alias = ?2",
                    rusqlite::params![payload.uuid, payload.alias],
                    |row| row.get::<_, bool>(0),
                )
                .optional()?
                .unwrap_or(false);
            inverse.push(OpKind::PageAliasSet(PageAliasSet {
                uuid: payload.uuid,
                alias: payload.alias.clone(),
                present: previous,
            }));
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
                    blob_hash: payload.blob_hash,
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
                    (SELECT CASE WHEN content_type = 'ink' THEN 'handwriting' ELSE page_kind END FROM page_identities WHERE page_uuid = pages.uuid),
                    (SELECT journal_date FROM page_identities WHERE page_uuid = pages.uuid)
               FROM pages WHERE uuid = ?1",
            [uuid],
            |row| {
                let page_kind = row.get::<_, String>(3)?;
                let journal_date = row.get::<_, Option<crate::model::JournalDate>>(4)?;
                let kind = match (page_kind.as_str(), journal_date) {
                    ("note", None) => PageKind::Note,
        ("handwriting", None) => PageKind::Handwriting,
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
    blob_hash: &notes_blob::BlobHash,
) -> rusqlite::Result<Option<AttachmentAdd>> {
    database
        .query_row(
            "SELECT filename, mime, size FROM attachment_lww
              WHERE owner_kind = ?1 AND owner_uuid = ?2 AND blob_hash = ?3 AND present = 1",
            rusqlite::params![owner.kind(), owner.uuid(), blob_hash_bytes(blob_hash)],
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
                    blob_hash: *blob_hash,
                    filename: row.get(0)?,
                    mime: row.get(1)?,
                    size,
                })
            },
        )
        .optional()
}

fn capture_history_guards(
    database: &rusqlite::Connection,
    operations: &[OpKind],
) -> rusqlite::Result<Vec<HistoryGuard>> {
    let mut fields = Vec::new();
    let mut seen = HashSet::new();
    for operation in operations {
        for field in history_guard_fields(operation) {
            if seen.insert(field.clone()) {
                fields.push(field);
            }
        }
    }
    fields
        .into_iter()
        .map(|field| {
            Ok(HistoryGuard {
                expected_hlc: current_field_hlc(database, &field)?,
                field,
            })
        })
        .collect()
}

fn capture_history_scope_guards(
    database: &rusqlite::Connection,
    operations: &[OpKind],
) -> rusqlite::Result<Vec<HistoryScopeGuard>> {
    let mut scopes = Vec::new();
    let mut seen_pages = HashSet::new();
    let mut seen_blocks = HashSet::new();
    for operation in operations {
        match operation {
            OpKind::PageDelete(payload) if seen_pages.insert(payload.uuid) => {
                scopes.push(HistoryScopeGuard::PageBlocks {
                    page_uuid: payload.uuid,
                    expected_block_uuids: page_block_uuids(database, payload.uuid)?,
                });
            }
            OpKind::BlockDelete(payload) if seen_blocks.insert(payload.uuid) => {
                scopes.push(HistoryScopeGuard::BlockDescendants {
                    block_uuid: payload.uuid,
                    expected_block_uuids: block_descendant_uuids(database, payload.uuid)?,
                });
            }
            _ => {}
        }
    }
    Ok(scopes)
}

fn page_block_uuids(
    database: &rusqlite::Connection,
    page_uuid: uuid::Uuid,
) -> rusqlite::Result<Vec<uuid::Uuid>> {
    let mut statement = database.prepare("SELECT uuid FROM blocks WHERE page_uuid = ?1")?;
    let mut uuids = statement
        .query_map([page_uuid], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    uuids.sort_unstable();
    Ok(uuids)
}

fn block_descendant_uuids(
    database: &rusqlite::Connection,
    block_uuid: uuid::Uuid,
) -> rusqlite::Result<Vec<uuid::Uuid>> {
    let mut statement = database.prepare(
        "WITH RECURSIVE descendants(uuid) AS (
           SELECT uuid FROM blocks WHERE parent_uuid = ?1
           UNION
           SELECT block.uuid FROM blocks block
             JOIN descendants parent ON block.parent_uuid = parent.uuid
         )
         SELECT uuid FROM descendants",
    )?;
    let mut uuids = statement
        .query_map([block_uuid], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    uuids.sort_unstable();
    Ok(uuids)
}

fn history_guard_fields(operation: &OpKind) -> Vec<HistoryField> {
    match operation {
        OpKind::InkPublish(_) => vec![],
        OpKind::PageCreate(payload) => vec![
            HistoryField::PageExistence { uuid: payload.uuid },
            HistoryField::PageTitle { uuid: payload.uuid },
            HistoryField::PageLayout { uuid: payload.uuid },
        ],
        OpKind::PageAliasSet(payload) => vec![
            HistoryField::PageAlias {
                uuid: payload.uuid,
                alias: payload.alias.clone(),
            },
            HistoryField::PageExistence { uuid: payload.uuid },
        ],
        OpKind::PageSetTitle(payload) => vec![
            HistoryField::PageTitle { uuid: payload.uuid },
            HistoryField::PageExistence { uuid: payload.uuid },
        ],
        OpKind::PageSetLayout(payload) => vec![
            HistoryField::PageLayout { uuid: payload.uuid },
            HistoryField::PageExistence { uuid: payload.uuid },
        ],
        OpKind::PageDelete(payload) => vec![
            HistoryField::PageExistence { uuid: payload.uuid },
            HistoryField::PageTitle { uuid: payload.uuid },
            HistoryField::PageLayout { uuid: payload.uuid },
        ],
        OpKind::BlockCreate(payload) => {
            let mut fields = vec![
                HistoryField::BlockExistence { uuid: payload.uuid },
                HistoryField::BlockMarkdown { uuid: payload.uuid },
                HistoryField::BlockStyle { uuid: payload.uuid },
                HistoryField::BlockStructure { uuid: payload.uuid },
                HistoryField::PageExistence {
                    uuid: payload.page_uuid,
                },
            ];
            if let Some(parent_uuid) = payload.parent_uuid {
                fields.push(HistoryField::BlockExistence { uuid: parent_uuid });
            }
            fields
        }
        OpKind::BlockSetMarkdown(payload) => vec![
            HistoryField::BlockMarkdown { uuid: payload.uuid },
            HistoryField::BlockExistence { uuid: payload.uuid },
        ],
        OpKind::BlockSetStyle(payload) => vec![
            HistoryField::BlockStyle { uuid: payload.uuid },
            HistoryField::BlockExistence { uuid: payload.uuid },
        ],
        OpKind::BlockMove(payload) => {
            let mut fields = vec![
                HistoryField::BlockStructure { uuid: payload.uuid },
                HistoryField::BlockExistence { uuid: payload.uuid },
                HistoryField::PageExistence {
                    uuid: payload.page_uuid,
                },
            ];
            if let Some(parent_uuid) = payload.parent_uuid {
                fields.push(HistoryField::BlockExistence { uuid: parent_uuid });
            }
            fields
        }
        OpKind::BlockDelete(payload) => vec![
            HistoryField::BlockExistence { uuid: payload.uuid },
            HistoryField::BlockMarkdown { uuid: payload.uuid },
            HistoryField::BlockStyle { uuid: payload.uuid },
            HistoryField::BlockStructure { uuid: payload.uuid },
            HistoryField::PageExistence {
                uuid: payload.page_uuid,
            },
        ],
        OpKind::AttachmentAdd(payload) => vec![
            HistoryField::Attachment {
                owner: payload.owner,
                blob_hash: payload.blob_hash,
            },
            owner_existence(payload.owner),
        ],
        OpKind::AttachmentRemove(payload) => vec![
            HistoryField::Attachment {
                owner: payload.owner,
                blob_hash: payload.blob_hash,
            },
            owner_existence(payload.owner),
        ],
    }
}

fn owner_existence(owner: AttachmentOwner) -> HistoryField {
    match owner {
        AttachmentOwner::Page(uuid) => HistoryField::PageExistence { uuid },
        AttachmentOwner::Block(uuid) => HistoryField::BlockExistence { uuid },
    }
}

fn current_field_hlc(
    database: &rusqlite::Connection,
    field: &HistoryField,
) -> rusqlite::Result<Option<Hlc>> {
    let value = match field {
        HistoryField::PageExistence { uuid } => database.query_row(
            "SELECT MAX(value) FROM (
               SELECT existence_hlc AS value FROM pages WHERE uuid = ?1
               UNION ALL
               SELECT deleted_hlc AS value FROM tombstones
                WHERE uuid = ?1 AND object_kind = 'page'
             )",
            [uuid],
            |row| row.get::<_, Option<String>>(0),
        )?,
        HistoryField::PageTitle { uuid } => query_nullable_hlc(
            database,
            "SELECT title_hlc FROM pages WHERE uuid = ?1",
            [uuid],
        )?,
        HistoryField::PageLayout { uuid } => query_nullable_hlc(
            database,
            "SELECT layout_hlc FROM pages WHERE uuid = ?1",
            [uuid],
        )?,
        HistoryField::PageAlias { uuid, alias } => query_nullable_hlc(
            database,
            "SELECT hlc FROM page_alias_lww WHERE page_uuid = ?1 AND alias = ?2",
            rusqlite::params![uuid, alias],
        )?,
        HistoryField::BlockExistence { uuid } => database.query_row(
            "SELECT MAX(value) FROM (
               SELECT existence_hlc AS value FROM blocks WHERE uuid = ?1
               UNION ALL
               SELECT deleted_hlc AS value FROM tombstones
                WHERE uuid = ?1 AND object_kind = 'block'
             )",
            [uuid],
            |row| row.get::<_, Option<String>>(0),
        )?,
        HistoryField::BlockMarkdown { uuid } => query_nullable_hlc(
            database,
            "SELECT markdown_hlc FROM blocks WHERE uuid = ?1",
            [uuid],
        )?,
        HistoryField::BlockStyle { uuid } => query_nullable_hlc(
            database,
            "SELECT style_hlc FROM blocks WHERE uuid = ?1",
            [uuid],
        )?,
        HistoryField::BlockStructure { uuid } => query_nullable_hlc(
            database,
            "SELECT hlc FROM block_structure_lww WHERE block_uuid = ?1",
            [uuid],
        )?,
        HistoryField::Attachment { owner, blob_hash } => query_nullable_hlc(
            database,
            "SELECT hlc FROM attachment_lww
              WHERE owner_kind = ?1 AND owner_uuid = ?2 AND blob_hash = ?3",
            rusqlite::params![owner.kind(), owner.uuid(), blob_hash_bytes(blob_hash)],
        )?,
    };
    value.map(|value| operation::sql_hlc(value, 0)).transpose()
}

fn query_nullable_hlc<P: rusqlite::Params>(
    database: &rusqlite::Connection,
    sql: &str,
    params: P,
) -> rusqlite::Result<Option<String>> {
    Ok(database
        .query_row(sql, params, |row| row.get::<_, Option<String>>(0))
        .optional()?
        .flatten())
}

fn guards_match(
    database: &rusqlite::Connection,
    guards: &[HistoryGuard],
) -> rusqlite::Result<bool> {
    for guard in guards {
        if current_field_hlc(database, &guard.field)? != guard.expected_hlc {
            return Ok(false);
        }
    }
    Ok(true)
}

fn scope_guards_match(
    database: &rusqlite::Connection,
    guards: &[HistoryScopeGuard],
) -> rusqlite::Result<bool> {
    for guard in guards {
        let (current, expected) = match guard {
            HistoryScopeGuard::PageBlocks {
                page_uuid,
                expected_block_uuids,
            } => (
                page_block_uuids(database, *page_uuid)?,
                expected_block_uuids,
            ),
            HistoryScopeGuard::BlockDescendants {
                block_uuid,
                expected_block_uuids,
            } => (
                block_descendant_uuids(database, *block_uuid)?,
                expected_block_uuids,
            ),
        };
        let expected = expected.iter().copied().collect::<HashSet<_>>();
        if current.iter().any(|uuid| !expected.contains(uuid)) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn refresh_sibling_guards(
    transaction: &rusqlite::Transaction<'_>,
    next_guards: &[HistoryGuard],
) -> anyhow::Result<()> {
    // Linear undo intentionally adopts HLCs written by a successful neighboring move.
    // This can launder an earlier same-field change into the local history timeline;
    // scope guards stay fixed so newly introduced content is never adopted implicitly.
    let replacements = next_guards
        .iter()
        .map(|guard| (guard.field.clone(), guard.expected_hlc.clone()))
        .collect::<HashMap<_, _>>();
    if replacements.is_empty() {
        return Ok(());
    }
    for table in ["history_undo", "history_redo"] {
        let rows = {
            let mut statement = transaction.prepare(&format!(
                "SELECT id, forward_json, inverse_json FROM {table}"
            ))?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (id, forward_json, inverse_json) in rows {
            let mut forward = decode_history_payload(&forward_json)?;
            let mut inverse = decode_history_payload(&inverse_json)?;
            let changed = refresh_payload_guards(&mut forward, &replacements)
                | refresh_payload_guards(&mut inverse, &replacements);
            if changed {
                transaction.execute(
                    &format!(
                        "UPDATE {table} SET forward_json = ?1, inverse_json = ?2 WHERE id = ?3"
                    ),
                    rusqlite::params![
                        encode_history_payload(&forward)?,
                        encode_history_payload(&inverse)?,
                        id
                    ],
                )?;
            }
        }
    }
    Ok(())
}

fn refresh_payload_guards(
    payload: &mut GuardedHistoryPayload,
    replacements: &HashMap<HistoryField, Option<Hlc>>,
) -> bool {
    let mut changed = false;
    for guard in &mut payload.guards {
        if let Some(expected_hlc) = replacements.get(&guard.field)
            && guard.expected_hlc != *expected_hlc
        {
            guard.expected_hlc = expected_hlc.clone();
            changed = true;
        }
    }
    changed
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
    let inverse_guards = capture_history_guards(transaction, &inverse)?;
    let inverse_scope_guards = capture_history_scope_guards(transaction, &inverse)?;
    let forward_json = encode_history_payload(&GuardedHistoryPayload {
        history_format_version: HISTORY_PAYLOAD_VERSION,
        operations: forward,
        guards: Vec::new(),
        scope_guards: Vec::new(),
    })
    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let inverse_json = encode_history_payload(&GuardedHistoryPayload {
        history_format_version: HISTORY_PAYLOAD_VERSION,
        operations: inverse,
        guards: inverse_guards,
        scope_guards: inverse_scope_guards,
    })
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

pub async fn undo_history(conn: &Connection) -> Result<HistoryMoveResult> {
    move_history(conn, true).await
}

pub async fn redo_history(conn: &Connection) -> Result<HistoryMoveResult> {
    move_history(conn, false).await
}

async fn move_history(conn: &Connection, undo: bool) -> Result<HistoryMoveResult> {
    conn.call_domain(move |database| -> anyhow::Result<HistoryMoveResult> {
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
            return Ok(HistoryMoveResult::Empty);
        };
        let mut forward = decode_history_payload(&forward_json)?;
        let mut inverse = decode_history_payload(&inverse_json)?;
        let selected = if undo { &inverse } else { &forward };
        if !guards_match(&transaction, &selected.guards)?
            || !scope_guards_match(&transaction, &selected.scope_guards)?
        {
            transaction.execute(&format!("DELETE FROM {source} WHERE id = ?1"), [entry_id])?;
            transaction.commit()?;
            return Ok(HistoryMoveResult::Skipped);
        }
        let kinds = selected.operations.clone();
        operation::apply_local_kinds_in_transaction(&transaction, kinds.clone())?;
        let next_guards = capture_history_guards(&transaction, &kinds)?;
        if undo {
            forward.guards = next_guards;
            forward.scope_guards = capture_history_scope_guards(&transaction, &forward.operations)?;
        } else {
            inverse.guards = next_guards;
            inverse.scope_guards = capture_history_scope_guards(&transaction, &inverse.operations)?;
        }
        let forward_json = encode_history_payload(&forward)?;
        let inverse_json = encode_history_payload(&inverse)?;
        transaction.execute(&format!("DELETE FROM {source} WHERE id = ?1"), [entry_id])?;
        refresh_sibling_guards(
            &transaction,
            if undo {
                &forward.guards
            } else {
                &inverse.guards
            },
        )?;
        transaction.execute(
            &format!(
                "INSERT INTO {target}(action_uuid, action, forward_json, inverse_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, unixepoch())"
            ),
            rusqlite::params![action_uuid, action, forward_json, inverse_json],
        )?;
        transaction.commit()?;
        Ok(HistoryMoveResult::Applied)
    })
    .await
}

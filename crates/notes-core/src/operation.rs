//! Versioned, UUID-addressed page/block operations and the single apply boundary.

use crate::db::{blob_hash_bytes, row_blob_hash};
use crate::model::{
    AttachmentOwner, BlockStyle, ObjectKind, OrderKey, PageAlias, PageKind, PageLayout,
    journal_page_uuid,
};
use crate::{Connection, CoreError, CoreResult, Hlc};
use anyhow::{Context, Result};
use notes_blob::BlobHash;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub const FORMAT_VERSION: u32 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Local,
    Remote,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Op {
    pub op_id: uuid::Uuid,
    pub workspace_uuid: uuid::Uuid,
    pub device_id: uuid::Uuid,
    pub hlc: Hlc,
    pub format_version: u32,
    #[serde(flatten)]
    pub kind: OpKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum OpKind {
    PageCreate(PageCreate),
    PageAliasSet(PageAliasSet),
    PageSetTitle(PageSetTitle),
    PageSetLayout(PageSetLayout),
    PageDelete(PageDelete),
    BlockCreate(BlockCreate),
    BlockSetMarkdown(BlockSetMarkdown),
    BlockSetStyle(BlockSetStyle),
    BlockMove(BlockMove),
    BlockDelete(BlockDelete),
    AttachmentAdd(AttachmentAdd),
    AttachmentRemove(AttachmentRemove),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PageCreate {
    pub uuid: uuid::Uuid,
    pub kind: PageKind,
    pub title: Option<String>,
    pub layout: PageLayout,
    pub created_at: i64,
}

/// Sets the presence of one explicit page alias using operation HLC as its
/// LWW clock. A title is an implicit alias and does not use this operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PageAliasSet {
    pub uuid: uuid::Uuid,
    pub alias: PageAlias,
    pub present: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PageSetTitle {
    pub uuid: uuid::Uuid,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PageSetLayout {
    pub uuid: uuid::Uuid,
    pub layout: PageLayout,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PageDelete {
    pub uuid: uuid::Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlockCreate {
    pub uuid: uuid::Uuid,
    pub page_uuid: uuid::Uuid,
    pub parent_uuid: Option<uuid::Uuid>,
    pub order_key: OrderKey,
    pub style: BlockStyle,
    pub markdown: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlockSetMarkdown {
    pub uuid: uuid::Uuid,
    pub markdown: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlockSetStyle {
    pub uuid: uuid::Uuid,
    pub style: BlockStyle,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlockMove {
    pub uuid: uuid::Uuid,
    pub page_uuid: uuid::Uuid,
    pub parent_uuid: Option<uuid::Uuid>,
    pub order_key: OrderKey,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlockDelete {
    pub uuid: uuid::Uuid,
    pub page_uuid: uuid::Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttachmentAdd {
    pub owner: AttachmentOwner,
    pub blob_hash: BlobHash,
    pub filename: String,
    pub mime: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttachmentRemove {
    pub owner: AttachmentOwner,
    pub blob_hash: BlobHash,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyOutcome {
    pub applied: bool,
    pub affected_uuids: Vec<uuid::Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncSnapshot {
    pub format_version: u32,
    pub workspace_uuid: uuid::Uuid,
    pub seq: u64,
    pub page_identities: Vec<SnapshotPageIdentity>,
    pub pages: Vec<SnapshotPage>,
    pub page_aliases: Vec<SnapshotPageAlias>,
    pub blocks: Vec<SnapshotBlock>,
    pub structures: Vec<SnapshotBlockStructure>,
    pub tombstones: Vec<SnapshotTombstone>,
    pub attachments: Vec<SnapshotAttachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotPageAlias {
    pub page_uuid: uuid::Uuid,
    pub alias: PageAlias,
    pub hlc: Hlc,
    pub present: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotPageIdentity {
    pub uuid: uuid::Uuid,
    pub kind: PageKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotBlockStructure {
    pub block_uuid: uuid::Uuid,
    pub page_uuid: uuid::Uuid,
    pub parent_uuid: Option<uuid::Uuid>,
    pub order_key: OrderKey,
    pub hlc: Hlc,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotPage {
    pub uuid: uuid::Uuid,
    pub kind: PageKind,
    pub title: Option<String>,
    pub layout: PageLayout,
    pub title_hlc: Option<Hlc>,
    pub layout_hlc: Option<Hlc>,
    pub existence_hlc: Hlc,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotBlock {
    pub uuid: uuid::Uuid,
    pub page_uuid: uuid::Uuid,
    pub parent_uuid: Option<uuid::Uuid>,
    pub order_key: OrderKey,
    pub style: BlockStyle,
    pub markdown: String,
    pub markdown_hlc: Option<Hlc>,
    pub style_hlc: Option<Hlc>,
    pub structure_hlc: Option<Hlc>,
    pub existence_hlc: Hlc,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotTombstone {
    pub uuid: uuid::Uuid,
    pub object_kind: ObjectKind,
    pub deleted_hlc: Hlc,
    pub root_page_uuid: Option<uuid::Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotAttachment {
    pub owner: AttachmentOwner,
    pub blob_hash: BlobHash,
    pub attachment_uuid: uuid::Uuid,
    pub hlc: Hlc,
    pub present: bool,
    pub filename: Option<String>,
    pub mime: Option<String>,
    pub size: Option<u64>,
}

pub(crate) async fn apply_local_kinds(conn: &Connection, kinds: Vec<OpKind>) -> Result<()> {
    if kinds.is_empty() {
        return Ok(());
    }
    conn.call_domain(move |database| -> CoreResult<()> {
        let transaction = database.transaction()?;
        apply_local_kinds_in_transaction(&transaction, kinds)?;
        transaction.commit()?;
        Ok(())
    })
    .await
}

pub(crate) fn apply_local_kinds_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    kinds: Vec<OpKind>,
) -> CoreResult<()> {
    apply_local_kinds_with_policy(transaction, kinds, false)?;
    Ok(())
}

pub(crate) struct LocalBatchApply {
    pub operations: Vec<Op>,
    pub stats: DeferredApplyStats,
}

pub(crate) fn apply_local_kinds_deferred_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    kinds: Vec<OpKind>,
) -> CoreResult<LocalBatchApply> {
    apply_local_kinds_with_policy(transaction, kinds, true)
}

fn apply_local_kinds_with_policy(
    transaction: &rusqlite::Transaction<'_>,
    kinds: Vec<OpKind>,
    deferred: bool,
) -> CoreResult<LocalBatchApply> {
    // Validate the complete input before even creating the local device row.
    // This keeps the external-import boundary observably mutation-free when
    // any late element in a large batch is malformed.
    for kind in &kinds {
        validate_kind(kind)?;
    }
    let device_id = meta_or_insert_device_id(transaction)?;
    let workspace_uuid = crate::db::transaction_workspace_uuid(transaction)?;
    let previous = transaction
        .query_row(
            "SELECT value FROM sync_meta WHERE key = 'last_hlc'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .and_then(|value| value.parse::<Hlc>().ok());
    let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
    let mut operations = Vec::with_capacity(kinds.len());
    let mut clock = previous;
    for kind in kinds {
        let hlc = Hlc::send(clock.as_ref(), now, device_id);
        let operation = Op {
            op_id: uuid::Uuid::now_v7(),
            workspace_uuid,
            device_id,
            hlc: hlc.clone(),
            format_version: FORMAT_VERSION,
            kind,
        };
        validate(&operation)?;
        operations.push(operation);
        clock = Some(hlc);
    }
    let configured = sync_is_configured(transaction)?;
    let mut effects = if deferred {
        ApplyEffects::deferred()
    } else {
        ApplyEffects::immediate()
    };
    for operation in &operations {
        apply_one_with_effects(transaction, operation, &mut effects)?;
        transaction.execute(
            "INSERT INTO applied_ops(op_id, seq) VALUES (?1, NULL)",
            [operation.op_id],
        )?;
        if configured {
            transaction.execute(
                "INSERT INTO sync_outbox(op_id, envelope, created_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    operation.op_id,
                    serde_json::to_string(operation).map_err(|error| {
                        rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                    })?,
                    operation_timestamp(operation),
                ],
            )?;
        }
    }
    let stats = effects.finish(transaction)?;
    if let Some(last) = operations.last() {
        transaction.execute(
            "INSERT INTO sync_meta(key, value) VALUES ('last_hlc', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [last.hlc.to_string()],
        )?;
    }
    Ok(LocalBatchApply { operations, stats })
}

pub async fn apply(conn: &Connection, op: &Op, origin: Origin) -> Result<ApplyOutcome> {
    let mut outcomes = apply_batch(conn, std::slice::from_ref(op), origin).await?;
    Ok(outcomes.pop().unwrap_or_default())
}

pub async fn configure_sync(conn: &Connection, server_url: &str) -> Result<()> {
    let server_url = server_url.trim().to_string();
    conn.call(move |database| {
        database.execute(
            "INSERT INTO sync_meta(key, value) VALUES ('server_url', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [server_url],
        )?;
        Ok(())
    })
    .await
}

pub async fn sync_cursor(conn: &Connection) -> Result<u64> {
    let value = conn
        .call(|database| {
            database
                .query_row(
                    "SELECT value FROM sync_meta WHERE key = 'last_server_seq'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()
        })
        .await?;
    value.map_or(Ok(0), |value| {
        value
            .parse()
            .context("invalid last_server_seq in sync_meta")
    })
}

pub async fn pending_outbox(conn: &Connection, limit: u32) -> Result<Vec<Op>> {
    conn.call(move |database| {
        let mut statement =
            database.prepare("SELECT envelope FROM sync_outbox ORDER BY rowid LIMIT ?1")?;
        statement
            .query_map([limit], |row| row.get::<_, String>(0))?
            .map(|envelope| {
                serde_json::from_str(&envelope?).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
            })
            .collect()
    })
    .await
}

pub async fn apply_sequenced(conn: &Connection, seq: u64, op: &Op) -> Result<ApplyOutcome> {
    let mut outcomes = apply_sequenced_batch(conn, vec![(seq, op.clone())]).await?;
    Ok(outcomes.pop().unwrap_or_default())
}

pub async fn apply_sequenced_batch(
    conn: &Connection,
    operations: Vec<(u64, Op)>,
) -> Result<Vec<ApplyOutcome>> {
    for (_, operation) in &operations {
        validate(operation)?;
    }
    conn.call_domain(move |database| -> CoreResult<Vec<ApplyOutcome>> {
        let transaction = database.transaction()?;
        let mut cursor = transaction
            .query_row(
                "SELECT value FROM sync_meta WHERE key = 'last_server_seq'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| {
                value.parse::<u64>().map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
            })
            .transpose()?
            .unwrap_or_default();
        let mut outcomes = Vec::with_capacity(operations.len());
        let mut effects = ApplyEffects::deferred();
        for (seq, operation) in operations {
            if seq <= cursor {
                let operation_at_seq = transaction
                    .query_row(
                        "SELECT op_id FROM applied_ops WHERE seq = ?1",
                        [seq as i64],
                        |row| row.get::<_, uuid::Uuid>(0),
                    )
                    .optional()?;
                if operation_at_seq.is_some_and(|op_id| op_id != operation.op_id) {
                    return Err(CoreError::sync_conflict(format!(
                        "server sequence {seq} is assigned to multiple operations"
                    )));
                }
                outcomes.push(ApplyOutcome::default());
                continue;
            }
            if seq != cursor.saturating_add(1) {
                return Err(CoreError::sync_conflict(format!(
                    "sync sequence gap: expected {}, received {seq}",
                    cursor.saturating_add(1)
                )));
            }
            let existing = transaction
                .query_row(
                    "SELECT seq FROM applied_ops WHERE op_id = ?1",
                    [&operation.op_id],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .optional()?;
            if let Some(Some(existing_seq)) = existing
                && existing_seq as u64 != seq
            {
                return Err(CoreError::sync_conflict(format!(
                    "operation {} has server sequences {existing_seq} and {seq}",
                    operation.op_id
                )));
            }
            let outcome = if existing.is_some() {
                ApplyOutcome::default()
            } else {
                let affected_uuids =
                    apply_one_with_effects(&transaction, &operation, &mut effects)?;
                observe_hlc(&transaction, &operation.hlc)?;
                ApplyOutcome {
                    applied: true,
                    affected_uuids,
                }
            };
            transaction.execute(
                "INSERT INTO applied_ops(op_id, seq) VALUES (?1, ?2)
                 ON CONFLICT(op_id) DO UPDATE SET seq = excluded.seq",
                rusqlite::params![operation.op_id, seq as i64],
            )?;
            transaction.execute(
                "DELETE FROM sync_outbox WHERE op_id = ?1",
                [&operation.op_id],
            )?;
            cursor = cursor.max(seq);
            outcomes.push(outcome);
        }
        effects.finish(&transaction)?;
        transaction.execute(
            "INSERT INTO sync_meta(key, value) VALUES ('last_server_seq', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [cursor.to_string()],
        )?;
        transaction.commit()?;
        Ok(outcomes)
    })
    .await
}

pub async fn acknowledge_server_op(conn: &Connection, op_id: uuid::Uuid, seq: u64) -> Result<()> {
    acknowledge_server_ops(conn, vec![(op_id, seq)]).await
}

pub async fn acknowledge_server_ops(
    conn: &Connection,
    acknowledgements: Vec<(uuid::Uuid, u64)>,
) -> Result<()> {
    conn.call(move |database| {
        let transaction = database.transaction()?;
        for (op_id, seq) in acknowledgements {
            transaction.execute(
                "UPDATE applied_ops SET seq = ?2 WHERE op_id = ?1",
                rusqlite::params![op_id, seq as i64],
            )?;
            transaction.execute("DELETE FROM sync_outbox WHERE op_id = ?1", [op_id])?;
        }
        transaction.commit()?;
        Ok(())
    })
    .await
}

pub async fn export_sync_snapshot(conn: &Connection, seq: u64) -> Result<SyncSnapshot> {
    conn.call(move |database| {
        let workspace_uuid = database.query_row(
            "SELECT uuid FROM workspace WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        let page_identities = database
            .prepare(
                "SELECT page_uuid, page_kind, journal_date
                   FROM page_identities ORDER BY page_uuid",
            )?
            .query_map([], |row| {
                Ok(SnapshotPageIdentity {
                    uuid: row.get(0)?,
                    kind: page_kind_from_sql(row.get(1)?, row.get(2)?, 1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let pages = database
            .prepare(
                "SELECT uuid, title, layout, title_hlc, layout_hlc, existence_hlc,
                        created_at, updated_at,
                        (SELECT page_kind FROM page_identities WHERE page_uuid = pages.uuid),
                        (SELECT journal_date FROM page_identities WHERE page_uuid = pages.uuid)
                   FROM pages ORDER BY uuid",
            )?
            .query_map([], |row| {
                Ok(SnapshotPage {
                    uuid: row.get(0)?,
                    kind: page_kind_from_sql(row.get(8)?, row.get(9)?, 8)?,
                    title: row.get(1)?,
                    layout: row.get(2)?,
                    title_hlc: optional_sql_hlc(row.get(3)?, 3)?,
                    layout_hlc: optional_sql_hlc(row.get(4)?, 4)?,
                    existence_hlc: sql_hlc(row.get(5)?, 5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let page_aliases = database
            .prepare(
                "SELECT page_uuid, alias, hlc, present
                   FROM page_alias_lww ORDER BY page_uuid, alias",
            )?
            .query_map([], |row| {
                Ok(SnapshotPageAlias {
                    page_uuid: row.get(0)?,
                    alias: row.get(1)?,
                    hlc: sql_hlc(row.get(2)?, 2)?,
                    present: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let blocks = database
            .prepare(
                "SELECT uuid, page_uuid, parent_uuid, order_key, style, markdown,
                        markdown_hlc, style_hlc, structure_hlc, existence_hlc,
                        created_at, updated_at
                   FROM blocks ORDER BY uuid",
            )?
            .query_map([], |row| {
                Ok(SnapshotBlock {
                    uuid: row.get(0)?,
                    page_uuid: row.get(1)?,
                    parent_uuid: row.get(2)?,
                    order_key: row.get(3)?,
                    style: row.get(4)?,
                    markdown: row.get(5)?,
                    markdown_hlc: optional_sql_hlc(row.get(6)?, 6)?,
                    style_hlc: optional_sql_hlc(row.get(7)?, 7)?,
                    structure_hlc: optional_sql_hlc(row.get(8)?, 8)?,
                    existence_hlc: sql_hlc(row.get(9)?, 9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let tombstones = database
            .prepare(
                "SELECT uuid, object_kind, deleted_hlc, root_page_uuid
                   FROM tombstones ORDER BY uuid",
            )?
            .query_map([], |row| {
                Ok(SnapshotTombstone {
                    uuid: row.get(0)?,
                    object_kind: row.get(1)?,
                    deleted_hlc: sql_hlc(row.get(2)?, 2)?,
                    root_page_uuid: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let structures = database
            .prepare(
                "SELECT block_uuid, page_uuid, parent_uuid, order_key, hlc
                   FROM block_structure_lww ORDER BY block_uuid",
            )?
            .query_map([], |row| {
                Ok(SnapshotBlockStructure {
                    block_uuid: row.get(0)?,
                    page_uuid: row.get(1)?,
                    parent_uuid: row.get(2)?,
                    order_key: row.get(3)?,
                    hlc: sql_hlc(row.get(4)?, 4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let attachments = database
            .prepare(
                "SELECT owner_kind, owner_uuid, blob_hash, attachment_uuid, hlc,
                        present, filename, mime, size
                   FROM attachment_lww ORDER BY owner_kind, owner_uuid, blob_hash",
            )?
            .query_map([], snapshot_attachment_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(SyncSnapshot {
            format_version: FORMAT_VERSION,
            workspace_uuid,
            seq,
            page_identities,
            pages,
            page_aliases,
            blocks,
            structures,
            tombstones,
            attachments,
        })
    })
    .await
}

pub async fn import_sync_snapshot(conn: &Connection, snapshot: SyncSnapshot) -> Result<()> {
    if snapshot.format_version != FORMAT_VERSION {
        return Err(CoreError::invalid(format!(
            "unsupported snapshot format version {}",
            snapshot.format_version
        ))
        .into());
    }
    validate_snapshot(&snapshot)?;
    for page in &snapshot.pages {
        validate_title(page.title.as_deref())?;
    }
    for structure in &snapshot.structures {
        if structure.parent_uuid == Some(structure.block_uuid) {
            return Err(CoreError::invalid("a block cannot be its own parent").into());
        }
    }
    for attachment in &snapshot.attachments {
        if attachment.attachment_uuid != attachment_uuid(attachment.owner, &attachment.blob_hash) {
            return Err(
                CoreError::invalid("attachment UUID does not match its owner and hash").into(),
            );
        }
        if attachment.present {
            let filename = attachment
                .filename
                .as_deref()
                .ok_or_else(|| CoreError::invalid("present attachment is missing its filename"))?;
            validate_attachment_filename(filename)?;
            if attachment
                .mime
                .as_deref()
                .is_none_or(|mime| mime.trim().is_empty())
            {
                return Err(
                    CoreError::invalid("present attachment is missing its MIME type").into(),
                );
            }
            if attachment.size.is_none_or(|size| size > i64::MAX as u64) {
                return Err(CoreError::invalid("present attachment has an invalid size").into());
            }
        }
    }
    conn.call_domain(move |database| -> CoreResult<()> {
        let transaction = database.transaction()?;
        let current_workspace_uuid = crate::db::transaction_workspace_uuid(&transaction)?;
        if current_workspace_uuid != snapshot.workspace_uuid {
            let has_source_state: bool = transaction.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM pages
                    UNION ALL SELECT 1 FROM tombstones
                    UNION ALL SELECT 1 FROM page_identities
                 )",
                [],
                |row| row.get(0),
            )?;
            if has_source_state {
                return Err(CoreError::conflict(
                    "snapshot belongs to a different non-empty workspace",
                ));
            }
        }
        transaction.execute_batch(
            "DELETE FROM page_links;
             DELETE FROM block_refs;
             DELETE FROM attachments;
             DELETE FROM blocks;
             DELETE FROM pages;
             DELETE FROM page_identities;
             DELETE FROM page_alias_lww;
             DELETE FROM block_structure_lww;
             DELETE FROM attachment_lww;
             DELETE FROM tombstones;
             DELETE FROM applied_ops;
             DELETE FROM sync_outbox;
             DELETE FROM history_undo;
             DELETE FROM history_redo;",
        )?;
        if current_workspace_uuid != snapshot.workspace_uuid {
            transaction.execute("DELETE FROM external_import_receipts", [])?;
        }
        transaction.execute(
            "UPDATE workspace SET uuid = ?1 WHERE singleton = 1",
            [snapshot.workspace_uuid],
        )?;
        for identity in snapshot.page_identities {
            insert_page_identity(&transaction, identity.uuid, &identity.kind)?;
        }
        for page in snapshot.pages {
            let normalized_title = page.title.as_deref().map(crate::model::normalize_title);
            transaction.execute(
                "INSERT INTO pages(uuid, title, normalized_title, layout, title_hlc,
                                   layout_hlc, existence_hlc, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    page.uuid,
                    page.title,
                    normalized_title,
                    page.layout,
                    page.title_hlc.map(|value| value.to_string()),
                    page.layout_hlc.map(|value| value.to_string()),
                    page.existence_hlc.to_string(),
                    page.created_at,
                    page.updated_at,
                ],
            )?;
            materialize_page_kind(&transaction, page.uuid, &page.kind)?;
        }
        for alias in snapshot.page_aliases {
            transaction.execute(
                "INSERT INTO page_alias_lww(page_uuid, alias, hlc, present)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    alias.page_uuid,
                    alias.alias,
                    alias.hlc.to_string(),
                    alias.present,
                ],
            )?;
        }
        for block in &snapshot.blocks {
            transaction.execute(
                "INSERT INTO blocks(uuid, page_uuid, parent_uuid, order_key, style, markdown,
                                    body_stemmed, markdown_hlc, style_hlc, structure_hlc,
                                    existence_hlc, created_at, updated_at)
                 VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                rusqlite::params![
                    block.uuid,
                    block.page_uuid,
                    block.order_key,
                    block.style,
                    block.markdown,
                    crate::stem::stem(&block.markdown),
                    block.markdown_hlc.as_ref().map(ToString::to_string),
                    block.style_hlc.as_ref().map(ToString::to_string),
                    block.structure_hlc.as_ref().map(ToString::to_string),
                    block.existence_hlc.to_string(),
                    block.created_at,
                    block.updated_at,
                ],
            )?;
        }
        for structure in snapshot.structures {
            transaction.execute(
                "INSERT INTO block_structure_lww
                   (block_uuid, page_uuid, parent_uuid, order_key, hlc)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    structure.block_uuid,
                    structure.page_uuid,
                    structure.parent_uuid,
                    structure.order_key,
                    structure.hlc.to_string(),
                ],
            )?;
        }
        for tombstone in snapshot.tombstones {
            transaction.execute(
                "INSERT INTO tombstones(uuid, object_kind, deleted_hlc, root_page_uuid)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    tombstone.uuid,
                    tombstone.object_kind,
                    tombstone.deleted_hlc.to_string(),
                    tombstone.root_page_uuid,
                ],
            )?;
        }
        for attachment in snapshot.attachments {
            write_attachment_intent(&transaction, &attachment)?;
        }
        reconcile_structure(&transaction)?;
        let blocks = {
            let mut statement = transaction.prepare(
                "SELECT uuid, markdown, COALESCE(markdown_hlc, existence_hlc) FROM blocks",
            )?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, uuid::Uuid>(0)?,
                        row.get::<_, String>(1)?,
                        sql_hlc(row.get::<_, String>(2)?, 2)?.timestamp_seconds(),
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (uuid, markdown, timestamp) in blocks {
            replace_block_refs(&transaction, uuid, &markdown, timestamp)?;
        }
        reconcile_all_attachments(&transaction)?;
        observe_max_persisted_hlc(&transaction)?;
        transaction.execute(
            "INSERT INTO sync_meta(key, value) VALUES ('last_server_seq', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [snapshot.seq.to_string()],
        )?;
        transaction.commit()?;
        Ok(())
    })
    .await
}

fn validate_snapshot(snapshot: &SyncSnapshot) -> CoreResult<()> {
    if snapshot.workspace_uuid.is_nil() {
        return Err(CoreError::invalid("snapshot workspace UUID cannot be nil"));
    }
    let mut identities = HashMap::new();
    let mut journal_dates = HashSet::new();
    for identity in &snapshot.page_identities {
        validate_page_identity(snapshot.workspace_uuid, identity.uuid, &identity.kind)?;
        if identities.insert(identity.uuid, &identity.kind).is_some() {
            return Err(CoreError::invalid(format!(
                "snapshot contains duplicate page identity UUID {}",
                identity.uuid
            )));
        }
        if let PageKind::Journal { date } = &identity.kind
            && !journal_dates.insert(date)
        {
            return Err(CoreError::invalid(format!(
                "snapshot contains duplicate journal date {date}"
            )));
        }
    }
    let mut pages = HashMap::new();
    for page in &snapshot.pages {
        if identities
            .get(&page.uuid)
            .is_none_or(|kind| *kind != &page.kind)
        {
            return Err(CoreError::invalid(format!(
                "snapshot page {} is missing its matching immutable identity",
                page.uuid
            )));
        }
        if page.kind.is_journal() && page.title.is_some() {
            return Err(CoreError::invalid(format!(
                "snapshot journal page {} cannot have a stored title",
                page.uuid
            )));
        }
        if pages.insert(page.uuid, page).is_some() {
            return Err(CoreError::invalid(format!(
                "snapshot contains duplicate page UUID {}",
                page.uuid
            )));
        }
    }
    let mut aliases = HashSet::new();
    for alias in &snapshot.page_aliases {
        if !identities.contains_key(&alias.page_uuid) {
            return Err(CoreError::invalid(format!(
                "snapshot alias {} references unknown page identity {}",
                alias.alias, alias.page_uuid
            )));
        }
        if !aliases.insert((alias.page_uuid, alias.alias.as_str())) {
            return Err(CoreError::invalid(format!(
                "snapshot contains duplicate alias {} for page {}",
                alias.alias, alias.page_uuid
            )));
        }
    }
    let mut blocks = HashMap::new();
    for block in &snapshot.blocks {
        if identities.contains_key(&block.uuid) {
            return Err(CoreError::invalid(format!(
                "snapshot UUID {} is reserved by both a page identity and a block",
                block.uuid
            )));
        }
        if blocks.insert(block.uuid, block).is_some() {
            return Err(CoreError::invalid(format!(
                "snapshot contains duplicate block UUID {}",
                block.uuid
            )));
        }
        if !pages.contains_key(&block.page_uuid) {
            return Err(CoreError::invalid(format!(
                "snapshot block {} references missing page {}",
                block.uuid, block.page_uuid
            )));
        }
    }
    let mut tombstones = HashMap::new();
    for tombstone in &snapshot.tombstones {
        if tombstones.insert(tombstone.uuid, tombstone).is_some() {
            return Err(CoreError::invalid(format!(
                "snapshot contains duplicate tombstone UUID {}",
                tombstone.uuid
            )));
        }
        let (live, opposite) = match tombstone.object_kind {
            ObjectKind::Page => (
                pages.contains_key(&tombstone.uuid),
                blocks.contains_key(&tombstone.uuid),
            ),
            ObjectKind::Block => (
                blocks.contains_key(&tombstone.uuid),
                identities.contains_key(&tombstone.uuid),
            ),
        };
        if live || opposite {
            return Err(CoreError::invalid(format!(
                "snapshot UUID {} is both live and tombstoned or changes object kind",
                tombstone.uuid
            )));
        }
        match tombstone.object_kind {
            ObjectKind::Page if !identities.contains_key(&tombstone.uuid) => {
                return Err(CoreError::invalid(
                    "page tombstone is missing its immutable page identity",
                ));
            }
            ObjectKind::Page if tombstone.root_page_uuid != Some(tombstone.uuid) => {
                return Err(CoreError::invalid(
                    "page tombstone root must equal the page UUID",
                ));
            }
            ObjectKind::Block if tombstone.root_page_uuid.is_none() => {
                return Err(CoreError::invalid(
                    "block tombstone must retain its root page UUID",
                ));
            }
            _ => {}
        }
    }
    for block in blocks.values() {
        if let Some(parent_uuid) = block.parent_uuid {
            let Some(parent) = blocks.get(&parent_uuid) else {
                return Err(CoreError::invalid(format!(
                    "snapshot block {} has missing materialized parent {parent_uuid}",
                    block.uuid
                )));
            };
            if parent.page_uuid != block.page_uuid {
                return Err(CoreError::invalid(
                    "snapshot materialized parent belongs to another page",
                ));
            }
        }
    }
    let mut structures = HashMap::new();
    for structure in &snapshot.structures {
        if structures.insert(structure.block_uuid, structure).is_some() {
            return Err(CoreError::invalid(format!(
                "snapshot contains duplicate structure intent for {}",
                structure.block_uuid
            )));
        }
        let Some(block) = blocks.get(&structure.block_uuid) else {
            return Err(CoreError::invalid(format!(
                "snapshot structure references missing block {}",
                structure.block_uuid
            )));
        };
        if structure.page_uuid != block.page_uuid || !pages.contains_key(&structure.page_uuid) {
            return Err(CoreError::invalid(
                "snapshot structure and block disagree on their page",
            ));
        }
        if block.structure_hlc.as_ref() != Some(&structure.hlc)
            || block.order_key != structure.order_key
        {
            return Err(CoreError::invalid(
                "snapshot block does not match its raw structure intent",
            ));
        }
        if let Some(parent_uuid) = structure.parent_uuid {
            if let Some(parent) = blocks.get(&parent_uuid) {
                if parent.page_uuid != structure.page_uuid {
                    return Err(CoreError::invalid(
                        "snapshot raw parent belongs to another page",
                    ));
                }
            } else if tombstones
                .get(&parent_uuid)
                .is_none_or(|tombstone| tombstone.object_kind != ObjectKind::Block)
            {
                return Err(CoreError::invalid(format!(
                    "snapshot raw parent {parent_uuid} is neither live nor tombstoned"
                )));
            }
        }
    }
    if let Some(block) = blocks
        .values()
        .find(|block| !structures.contains_key(&block.uuid))
    {
        return Err(CoreError::invalid(format!(
            "snapshot block {} has no raw structure intent",
            block.uuid
        )));
    }
    let mut attachment_keys = HashSet::new();
    let mut attachment_uuids = HashSet::new();
    for attachment in &snapshot.attachments {
        if !attachment_keys.insert((
            attachment.owner.kind(),
            attachment.owner.uuid(),
            attachment.blob_hash,
        )) || !attachment_uuids.insert(attachment.attachment_uuid)
        {
            return Err(CoreError::invalid(
                "snapshot contains duplicate attachment identity",
            ));
        }
        let owner_is_known = match attachment.owner {
            AttachmentOwner::Page(uuid) => {
                pages.contains_key(&uuid)
                    || tombstones
                        .get(&uuid)
                        .is_some_and(|row| row.object_kind == ObjectKind::Page)
            }
            AttachmentOwner::Block(uuid) => {
                blocks.contains_key(&uuid)
                    || tombstones
                        .get(&uuid)
                        .is_some_and(|row| row.object_kind == ObjectKind::Block)
            }
        };
        if !owner_is_known {
            return Err(CoreError::invalid(
                "snapshot attachment owner is neither live nor tombstoned",
            ));
        }
    }
    Ok(())
}

pub async fn apply_batch(
    conn: &Connection,
    operations: &[Op],
    origin: Origin,
) -> Result<Vec<ApplyOutcome>> {
    for operation in operations {
        validate(operation)?;
    }
    let operations = operations.to_vec();
    conn.call_domain(move |database| -> CoreResult<Vec<ApplyOutcome>> {
        let transaction = database.transaction()?;
        let configured = sync_is_configured(&transaction)?;
        let mut outcomes = Vec::with_capacity(operations.len());
        let mut effects = ApplyEffects::deferred();
        for operation in operations {
            let exists = transaction
                .query_row(
                    "SELECT 1 FROM applied_ops WHERE op_id = ?1",
                    [&operation.op_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if exists {
                outcomes.push(ApplyOutcome::default());
                continue;
            }
            let affected_uuids = apply_one_with_effects(&transaction, &operation, &mut effects)?;
            transaction.execute(
                "INSERT INTO applied_ops(op_id, seq) VALUES (?1, NULL)",
                [&operation.op_id],
            )?;
            if origin == Origin::Local && configured {
                transaction.execute(
                    "INSERT INTO sync_outbox(op_id, envelope, created_at) VALUES (?1, ?2, ?3)",
                    rusqlite::params![
                        operation.op_id,
                        serde_json::to_string(&operation).map_err(|error| {
                            rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                        })?,
                        chrono::Utc::now().timestamp(),
                    ],
                )?;
            }
            if origin == Origin::Remote {
                observe_hlc(&transaction, &operation.hlc)?;
            }
            outcomes.push(ApplyOutcome {
                applied: true,
                affected_uuids,
            });
        }
        effects.finish(&transaction)?;
        transaction.commit()?;
        Ok(outcomes)
    })
    .await
}

fn validate(operation: &Op) -> CoreResult<()> {
    if operation.workspace_uuid.is_nil() {
        return Err(CoreError::invalid("operation workspace UUID cannot be nil"));
    }
    if operation.format_version != FORMAT_VERSION {
        return Err(CoreError::invalid(format!(
            "unsupported operation format version {}",
            operation.format_version
        )));
    }
    validate_kind(&operation.kind)
}

pub(crate) fn validate_kind(kind: &OpKind) -> CoreResult<()> {
    match kind {
        OpKind::PageCreate(payload) => {
            validate_title(payload.title.as_deref())?;
            if payload.kind.is_journal() && payload.title.is_some() {
                return Err(CoreError::invalid(
                    "journal pages cannot have a stored title",
                ));
            }
            Ok(())
        }
        OpKind::PageAliasSet(_) => Ok(()),
        OpKind::PageSetTitle(payload) => validate_title(payload.title.as_deref()),
        OpKind::PageSetLayout(_) | OpKind::PageDelete(_) => Ok(()),
        OpKind::BlockCreate(payload) => {
            if payload.parent_uuid == Some(payload.uuid) {
                Err(CoreError::invalid("a block cannot be its own parent"))
            } else {
                Ok(())
            }
        }
        OpKind::BlockMove(payload) => {
            if payload.parent_uuid == Some(payload.uuid) {
                Err(CoreError::invalid("a block cannot be its own parent"))
            } else {
                Ok(())
            }
        }
        OpKind::BlockSetMarkdown(_) | OpKind::BlockSetStyle(_) | OpKind::BlockDelete(_) => Ok(()),
        OpKind::AttachmentAdd(payload) => {
            validate_attachment_filename(&payload.filename)?;
            if payload.mime.trim().is_empty() {
                return Err(CoreError::invalid("attachment MIME type is required"));
            }
            if payload.size > i64::MAX as u64 {
                return Err(CoreError::invalid("attachment size is too large"));
            }
            Ok(())
        }
        OpKind::AttachmentRemove(_) => Ok(()),
    }
}

fn validate_title(title: Option<&str>) -> CoreResult<()> {
    if title.is_some_and(|title| title.trim().is_empty()) {
        Err(CoreError::invalid("page title cannot be blank"))
    } else {
        Ok(())
    }
}

pub fn validate_attachment_filename(filename: &str) -> CoreResult<()> {
    if filename.trim().is_empty()
        || matches!(filename, "." | "..")
        || filename
            .chars()
            .any(|character| matches!(character, '/' | '\\' | '\0') || character.is_control())
    {
        return Err(CoreError::invalid(
            "attachment filename must be a portable basename",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DeferredApplyStats {
    pub structure_reconciliations: u32,
    pub reference_projections: u32,
}

struct ApplyEffects {
    deferred: bool,
    structure_dirty: bool,
    pending_block_refs: BTreeMap<uuid::Uuid, (String, i64)>,
    pending_page_links: BTreeSet<String>,
    stats: DeferredApplyStats,
}

impl ApplyEffects {
    fn immediate() -> Self {
        Self {
            deferred: false,
            structure_dirty: false,
            pending_block_refs: BTreeMap::new(),
            pending_page_links: BTreeSet::new(),
            stats: DeferredApplyStats::default(),
        }
    }

    fn deferred() -> Self {
        Self {
            deferred: true,
            ..Self::immediate()
        }
    }

    fn request_structure(
        &mut self,
        transaction: &rusqlite::Transaction<'_>,
    ) -> rusqlite::Result<()> {
        if self.deferred {
            self.structure_dirty = true;
        } else {
            reconcile_structure(transaction)?;
            self.stats.structure_reconciliations += 1;
        }
        Ok(())
    }

    fn request_block_refs(
        &mut self,
        transaction: &rusqlite::Transaction<'_>,
        block_uuid: uuid::Uuid,
        markdown: &str,
        timestamp: i64,
    ) -> rusqlite::Result<()> {
        if self.deferred {
            self.pending_block_refs
                .insert(block_uuid, (markdown.to_owned(), timestamp));
        } else {
            replace_block_refs(transaction, block_uuid, markdown, timestamp)?;
            self.stats.reference_projections += 1;
        }
        Ok(())
    }

    fn request_page_links(
        &mut self,
        transaction: &rusqlite::Transaction<'_>,
        alias: impl Into<String>,
    ) -> rusqlite::Result<()> {
        let alias = alias.into();
        if self.deferred {
            self.pending_page_links.insert(alias);
        } else {
            reconcile_page_links_for_alias(transaction, &alias)?;
            self.stats.reference_projections += 1;
        }
        Ok(())
    }

    fn discard_block_refs(&mut self, block_uuid: uuid::Uuid) {
        self.pending_block_refs.remove(&block_uuid);
    }

    fn finish(
        mut self,
        transaction: &rusqlite::Transaction<'_>,
    ) -> rusqlite::Result<DeferredApplyStats> {
        if self.structure_dirty {
            reconcile_structure(transaction)?;
            self.stats.structure_reconciliations += 1;
        }
        // First update already-materialized references for changed aliases.
        // Pending Markdown is projected afterwards against the final alias
        // state, avoiding an alias-by-reference second pass during imports.
        for alias in self.pending_page_links {
            reconcile_page_links_for_alias(transaction, &alias)?;
            self.stats.reference_projections += 1;
        }
        for (block_uuid, (markdown, timestamp)) in self.pending_block_refs {
            replace_block_refs(transaction, block_uuid, &markdown, timestamp)?;
            self.stats.reference_projections += 1;
        }
        Ok(self.stats)
    }
}

fn apply_one_with_effects(
    transaction: &rusqlite::Transaction<'_>,
    operation: &Op,
    effects: &mut ApplyEffects,
) -> CoreResult<Vec<uuid::Uuid>> {
    let workspace_uuid = crate::db::transaction_workspace_uuid(transaction)?;
    if operation.workspace_uuid != workspace_uuid {
        return Err(CoreError::conflict(format!(
            "operation belongs to workspace {}, but this replica is {}",
            operation.workspace_uuid, workspace_uuid
        )));
    }
    let timestamp = operation_timestamp(operation);
    match &operation.kind {
        OpKind::PageCreate(payload) => {
            ensure_object_kind(transaction, payload.uuid, ObjectKind::Page)?;
            ensure_page_identity(transaction, payload.uuid, &payload.kind)?;
            if tombstone_dominates(transaction, payload.uuid, &operation.hlc)? {
                return Ok(vec![payload.uuid]);
            }
            let previous_existence_hlc = transaction
                .query_row(
                    "SELECT existence_hlc FROM pages WHERE uuid = ?1",
                    [payload.uuid],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if payload.kind.is_journal()
                && let Some(current_existence_hlc) = previous_existence_hlc.as_deref()
            {
                // Concurrent offline `ensure_journal` operations target the same
                // deterministic UUID. The first generation uses the earliest
                // observed ensure HLC, independently of delivery order. Initial
                // title/layout clocks follow that floor only while they still
                // equal the old creation clock, so a real later mutation wins.
                if operation.hlc.to_string().as_str() < current_existence_hlc {
                    transaction.execute(
                        "UPDATE pages
                            SET existence_hlc = ?2,
                                title_hlc = CASE WHEN title_hlc = ?3 THEN ?2 ELSE title_hlc END,
                                layout_hlc = CASE WHEN layout_hlc = ?3 THEN ?2 ELSE layout_hlc END,
                                created_at = MIN(created_at, ?4),
                                updated_at = MIN(updated_at, ?5)
                          WHERE uuid = ?1",
                        rusqlite::params![
                            payload.uuid,
                            operation.hlc.to_string(),
                            current_existence_hlc,
                            payload.created_at,
                            timestamp,
                        ],
                    )?;
                }
                return Ok(vec![payload.uuid]);
            }
            let mut affected = vec![payload.uuid];
            if previous_existence_hlc
                .as_deref()
                .is_some_and(|current| current < operation.hlc.to_string().as_str())
            {
                let stale_blocks = {
                    let mut statement = transaction.prepare(
                        "SELECT uuid FROM blocks
                          WHERE page_uuid = ?1 AND existence_hlc < ?2",
                    )?;
                    statement
                        .query_map(
                            rusqlite::params![payload.uuid, operation.hlc.to_string()],
                            |row| row.get::<_, uuid::Uuid>(0),
                        )?
                        .collect::<rusqlite::Result<Vec<_>>>()?
                };
                for block_uuid in &stale_blocks {
                    effects.discard_block_refs(*block_uuid);
                    write_tombstone(
                        transaction,
                        *block_uuid,
                        ObjectKind::Block,
                        Some(payload.uuid),
                        &operation.hlc,
                    )?;
                    transaction.execute(
                        "DELETE FROM block_structure_lww WHERE block_uuid = ?1",
                        [block_uuid],
                    )?;
                    transaction.execute("DELETE FROM blocks WHERE uuid = ?1", [block_uuid])?;
                }
                affected.extend(stale_blocks);
            }
            // Block tombstones double as page-generation watermarks. Promoting them means a
            // delayed create from before this page recreation cannot reattach old content, no
            // matter whether the delete or recreate operation arrived first.
            transaction.execute(
                "UPDATE tombstones
                    SET deleted_hlc = ?2
                  WHERE object_kind = 'block'
                    AND root_page_uuid = ?1
                    AND deleted_hlc < ?2",
                rusqlite::params![payload.uuid, operation.hlc.to_string()],
            )?;
            transaction.execute(
                "INSERT INTO pages(uuid, title, normalized_title, layout, title_hlc, layout_hlc,
                                   existence_hlc, created_at, updated_at)
                 VALUES (?1, NULL, NULL, ?2, NULL, NULL, ?3, ?4, ?5)
                 ON CONFLICT(uuid) DO UPDATE SET
                   existence_hlc = excluded.existence_hlc,
                   created_at = excluded.created_at,
                   updated_at = MAX(pages.updated_at, excluded.updated_at)
                 WHERE pages.existence_hlc < excluded.existence_hlc",
                rusqlite::params![
                    payload.uuid,
                    payload.layout,
                    operation.hlc.to_string(),
                    payload.created_at,
                    timestamp,
                ],
            )?;
            transaction.execute(
                "DELETE FROM tombstones WHERE uuid = ?1 AND deleted_hlc < ?2",
                rusqlite::params![payload.uuid, operation.hlc.to_string()],
            )?;
            materialize_page_kind(transaction, payload.uuid, &payload.kind)?;
            apply_page_title(
                transaction,
                payload.uuid,
                payload.title.as_deref(),
                operation,
                timestamp,
                effects,
            )?;
            for alias in explicit_page_aliases(transaction, payload.uuid)? {
                effects.request_page_links(transaction, alias)?;
            }
            apply_page_layout(
                transaction,
                payload.uuid,
                payload.layout,
                operation,
                timestamp,
            )?;
            effects.request_structure(transaction)?;
            reconcile_attachments_for_owner(transaction, AttachmentOwner::Page(payload.uuid))?;
            Ok(affected)
        }
        OpKind::PageAliasSet(payload) => {
            ensure_object_kind(transaction, payload.uuid, ObjectKind::Page)?;
            if page_identity(transaction, payload.uuid)?.is_none() {
                return Err(CoreError::not_found(format!(
                    "page {} for alias {} was not found",
                    payload.uuid, payload.alias
                )));
            }
            let changed = transaction.execute(
                "INSERT INTO page_alias_lww(page_uuid, alias, hlc, present)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(page_uuid, alias) DO UPDATE SET
                   hlc = excluded.hlc,
                   present = excluded.present
                 WHERE page_alias_lww.hlc < excluded.hlc",
                rusqlite::params![
                    payload.uuid,
                    payload.alias,
                    operation.hlc.to_string(),
                    payload.present,
                ],
            )? > 0;
            if changed {
                effects.request_page_links(transaction, payload.alias.as_str())?;
            }
            Ok(vec![payload.uuid])
        }
        OpKind::PageSetTitle(payload) => {
            ensure_object_kind(transaction, payload.uuid, ObjectKind::Page)?;
            if page_identity(transaction, payload.uuid)?.is_some_and(|kind| kind.is_journal()) {
                return Err(CoreError::invalid(
                    "journal page titles are derived from their date",
                ));
            }
            if !is_tombstoned(transaction, payload.uuid)? {
                apply_page_title(
                    transaction,
                    payload.uuid,
                    payload.title.as_deref(),
                    operation,
                    timestamp,
                    effects,
                )?;
            }
            Ok(vec![payload.uuid])
        }
        OpKind::PageSetLayout(payload) => {
            ensure_object_kind(transaction, payload.uuid, ObjectKind::Page)?;
            if !is_tombstoned(transaction, payload.uuid)? {
                apply_page_layout(
                    transaction,
                    payload.uuid,
                    payload.layout,
                    operation,
                    timestamp,
                )?;
            }
            Ok(vec![payload.uuid])
        }
        OpKind::PageDelete(payload) => {
            ensure_object_kind(transaction, payload.uuid, ObjectKind::Page)?;
            let existence_hlc = transaction
                .query_row(
                    "SELECT existence_hlc FROM pages WHERE uuid = ?1",
                    [payload.uuid],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if existence_hlc.is_some_and(|hlc| hlc > operation.hlc.to_string()) {
                return Ok(vec![payload.uuid]);
            }
            let reference_aliases = page_reference_aliases(transaction, payload.uuid)?;
            let blocks = {
                let mut statement =
                    transaction.prepare("SELECT uuid FROM blocks WHERE page_uuid = ?1")?;
                statement
                    .query_map([payload.uuid], |row| row.get::<_, uuid::Uuid>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            write_tombstone(
                transaction,
                payload.uuid,
                ObjectKind::Page,
                Some(payload.uuid),
                &operation.hlc,
            )?;
            for block_uuid in &blocks {
                write_tombstone(
                    transaction,
                    *block_uuid,
                    ObjectKind::Block,
                    Some(payload.uuid),
                    &operation.hlc,
                )?;
                transaction.execute(
                    "DELETE FROM block_structure_lww WHERE block_uuid = ?1",
                    [block_uuid],
                )?;
            }
            if tombstone_equals(transaction, payload.uuid, &operation.hlc)? {
                for block_uuid in &blocks {
                    effects.discard_block_refs(*block_uuid);
                }
                transaction.execute("DELETE FROM pages WHERE uuid = ?1", [payload.uuid])?;
                for alias in reference_aliases {
                    effects.request_page_links(transaction, alias)?;
                }
            }
            let mut affected = vec![payload.uuid];
            affected.extend(blocks);
            Ok(affected)
        }
        OpKind::BlockCreate(payload) => {
            ensure_object_kind(transaction, payload.uuid, ObjectKind::Block)?;
            ensure_object_kind(transaction, payload.page_uuid, ObjectKind::Page)?;
            if let Some(parent_uuid) = payload.parent_uuid {
                ensure_object_kind(transaction, parent_uuid, ObjectKind::Block)?;
            }
            if tombstone_dominates(transaction, payload.uuid, &operation.hlc)? {
                return Ok(vec![payload.uuid]);
            }
            let page_deleted_hlc = transaction
                .query_row(
                    "SELECT deleted_hlc FROM tombstones
                      WHERE uuid = ?1 AND object_kind = 'page'",
                    [payload.page_uuid],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if let Some(page_deleted_hlc) = page_deleted_hlc {
                write_tombstone(
                    transaction,
                    payload.uuid,
                    ObjectKind::Block,
                    Some(payload.page_uuid),
                    &sql_hlc(page_deleted_hlc, 0)?,
                )?;
                return Ok(vec![payload.uuid, payload.page_uuid]);
            }
            let page_existence_hlc = transaction
                .query_row(
                    "SELECT existence_hlc FROM pages WHERE uuid = ?1",
                    [payload.page_uuid],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            let Some(page_existence_hlc) = page_existence_hlc else {
                return Err(CoreError::not_found(format!(
                    "page {} for block {} was not found",
                    payload.page_uuid, payload.uuid
                )));
            };
            // A recreated page starts a new content generation. A delayed block create from the
            // deleted generation must not attach itself to the new page merely because the page
            // UUID is live again. Causal local creation always observes the page HLC first.
            if page_existence_hlc > operation.hlc.to_string() {
                write_tombstone(
                    transaction,
                    payload.uuid,
                    ObjectKind::Block,
                    Some(payload.page_uuid),
                    &sql_hlc(page_existence_hlc, 0)?,
                )?;
                return Ok(vec![payload.uuid, payload.page_uuid]);
            }
            transaction.execute(
                "INSERT INTO blocks(uuid, page_uuid, parent_uuid, order_key, style, markdown,
                                    body_stemmed, markdown_hlc, style_hlc, structure_hlc,
                                    existence_hlc, created_at, updated_at)
                 VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, NULL, NULL, NULL, ?7, ?8, ?9)
                 ON CONFLICT(uuid) DO UPDATE SET
                   existence_hlc = excluded.existence_hlc,
                   created_at = excluded.created_at,
                   updated_at = MAX(blocks.updated_at, excluded.updated_at)
                 WHERE blocks.existence_hlc < excluded.existence_hlc",
                rusqlite::params![
                    payload.uuid,
                    payload.page_uuid,
                    payload.order_key,
                    payload.style,
                    payload.markdown,
                    crate::stem::stem(&payload.markdown),
                    operation.hlc.to_string(),
                    payload.created_at,
                    timestamp,
                ],
            )?;
            transaction.execute(
                "DELETE FROM tombstones WHERE uuid = ?1 AND deleted_hlc < ?2",
                rusqlite::params![payload.uuid, operation.hlc.to_string()],
            )?;
            write_structure_intent(
                transaction,
                payload.uuid,
                payload.page_uuid,
                payload.parent_uuid,
                &payload.order_key,
                &operation.hlc,
            )?;
            apply_block_markdown(
                transaction,
                payload.uuid,
                &payload.markdown,
                operation,
                timestamp,
                effects,
            )?;
            apply_block_style(
                transaction,
                payload.uuid,
                payload.style,
                operation,
                timestamp,
            )?;
            effects.request_structure(transaction)?;
            reconcile_attachments_for_owner(transaction, AttachmentOwner::Block(payload.uuid))?;
            Ok(vec![payload.uuid, payload.page_uuid])
        }
        OpKind::BlockSetMarkdown(payload) => {
            ensure_object_kind(transaction, payload.uuid, ObjectKind::Block)?;
            if !is_tombstoned(transaction, payload.uuid)? {
                apply_block_markdown(
                    transaction,
                    payload.uuid,
                    &payload.markdown,
                    operation,
                    timestamp,
                    effects,
                )?;
            }
            Ok(vec![payload.uuid])
        }
        OpKind::BlockSetStyle(payload) => {
            ensure_object_kind(transaction, payload.uuid, ObjectKind::Block)?;
            if !is_tombstoned(transaction, payload.uuid)? {
                apply_block_style(
                    transaction,
                    payload.uuid,
                    payload.style,
                    operation,
                    timestamp,
                )?;
            }
            Ok(vec![payload.uuid])
        }
        OpKind::BlockMove(payload) => {
            ensure_object_kind(transaction, payload.uuid, ObjectKind::Block)?;
            ensure_object_kind(transaction, payload.page_uuid, ObjectKind::Page)?;
            if let Some(parent_uuid) = payload.parent_uuid {
                ensure_object_kind(transaction, parent_uuid, ObjectKind::Block)?;
            }
            if !is_tombstoned(transaction, payload.uuid)? {
                write_structure_intent(
                    transaction,
                    payload.uuid,
                    payload.page_uuid,
                    payload.parent_uuid,
                    &payload.order_key,
                    &operation.hlc,
                )?;
                effects.request_structure(transaction)?;
                transaction.execute(
                    "UPDATE blocks SET updated_at = MAX(updated_at, ?2) WHERE uuid = ?1",
                    rusqlite::params![payload.uuid, timestamp],
                )?;
            }
            Ok(vec![payload.uuid, payload.page_uuid])
        }
        OpKind::BlockDelete(payload) => {
            ensure_object_kind(transaction, payload.uuid, ObjectKind::Block)?;
            ensure_object_kind(transaction, payload.page_uuid, ObjectKind::Page)?;
            let state = transaction
                .query_row(
                    "SELECT page_uuid, parent_uuid, existence_hlc FROM blocks WHERE uuid = ?1",
                    [payload.uuid],
                    |row| {
                        Ok((
                            row.get::<_, uuid::Uuid>(0)?,
                            row.get::<_, Option<uuid::Uuid>>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()?;
            if state
                .as_ref()
                .is_some_and(|state| state.2 > operation.hlc.to_string())
            {
                return Ok(vec![payload.uuid]);
            }
            write_tombstone(
                transaction,
                payload.uuid,
                ObjectKind::Block,
                Some(payload.page_uuid),
                &operation.hlc,
            )?;
            if tombstone_equals(transaction, payload.uuid, &operation.hlc)? {
                effects.discard_block_refs(payload.uuid);
                transaction.execute(
                    "DELETE FROM block_structure_lww WHERE block_uuid = ?1",
                    [payload.uuid],
                )?;
                if state.is_some() {
                    transaction.execute("DELETE FROM blocks WHERE uuid = ?1", [payload.uuid])?;
                }
                effects.request_structure(transaction)?;
                return Ok(vec![payload.uuid, payload.page_uuid]);
            }
            Ok(vec![payload.uuid])
        }
        OpKind::AttachmentAdd(payload) => {
            ensure_object_kind(transaction, payload.owner.uuid(), payload.owner.kind())?;
            let snapshot = SnapshotAttachment {
                owner: payload.owner,
                blob_hash: payload.blob_hash,
                attachment_uuid: attachment_uuid(payload.owner, &payload.blob_hash),
                hlc: operation.hlc.clone(),
                present: true,
                filename: Some(payload.filename.clone()),
                mime: Some(payload.mime.clone()),
                size: Some(payload.size),
            };
            write_attachment_intent(transaction, &snapshot)?;
            reconcile_attachment(transaction, payload.owner, &payload.blob_hash)?;
            Ok(vec![payload.owner.uuid(), snapshot.attachment_uuid])
        }
        OpKind::AttachmentRemove(payload) => {
            ensure_object_kind(transaction, payload.owner.uuid(), payload.owner.kind())?;
            let snapshot = SnapshotAttachment {
                owner: payload.owner,
                blob_hash: payload.blob_hash,
                attachment_uuid: attachment_uuid(payload.owner, &payload.blob_hash),
                hlc: operation.hlc.clone(),
                present: false,
                filename: None,
                mime: None,
                size: None,
            };
            write_attachment_intent(transaction, &snapshot)?;
            reconcile_attachment(transaction, payload.owner, &payload.blob_hash)?;
            Ok(vec![payload.owner.uuid(), snapshot.attachment_uuid])
        }
    }
}

fn apply_page_title(
    transaction: &rusqlite::Transaction<'_>,
    uuid: uuid::Uuid,
    title: Option<&str>,
    operation: &Op,
    timestamp: i64,
    effects: &mut ApplyEffects,
) -> rusqlite::Result<()> {
    let current = transaction
        .query_row(
            "SELECT title_hlc, normalized_title FROM pages WHERE uuid = ?1",
            [uuid],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .optional()?;
    let Some((current_hlc, previous_title)) = current else {
        return Ok(());
    };
    if hlc_wins(current_hlc.as_deref(), &operation.hlc) {
        let mut dirty_aliases = previous_title.into_iter().collect::<BTreeSet<_>>();
        if let Some(title) = title {
            let normalized_title = crate::model::normalize_title(title);
            dirty_aliases.insert(normalized_title.clone());
            let conflict = transaction
                .query_row(
                    "SELECT uuid, title_hlc FROM pages
                      WHERE normalized_title = ?1 AND uuid != ?2",
                    rusqlite::params![normalized_title, uuid],
                    |row| {
                        Ok((
                            row.get::<_, uuid::Uuid>(0)?,
                            row.get::<_, Option<String>>(1)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((conflict_uuid, conflict_hlc)) = conflict {
                if conflict_hlc
                    .as_deref()
                    .is_some_and(|hlc| hlc >= operation.hlc.to_string().as_str())
                {
                    transaction.execute(
                        "UPDATE pages
                            SET title = NULL, normalized_title = NULL, title_hlc = ?2,
                                updated_at = MAX(updated_at, ?3)
                          WHERE uuid = ?1",
                        rusqlite::params![uuid, operation.hlc.to_string(), timestamp],
                    )?;
                    for alias in dirty_aliases {
                        effects.request_page_links(transaction, alias)?;
                    }
                    return Ok(());
                }
                transaction.execute(
                    "UPDATE pages SET title = NULL, normalized_title = NULL,
                                      updated_at = MAX(updated_at, ?2)
                      WHERE uuid = ?1",
                    rusqlite::params![conflict_uuid, timestamp],
                )?;
            }
        }
        transaction.execute(
            "UPDATE pages SET title = ?2, normalized_title = ?3, title_hlc = ?4,
                              updated_at = MAX(updated_at, ?5)
              WHERE uuid = ?1",
            rusqlite::params![
                uuid,
                title,
                title.map(crate::model::normalize_title),
                operation.hlc.to_string(),
                timestamp,
            ],
        )?;
        for alias in dirty_aliases {
            effects.request_page_links(transaction, alias)?;
        }
    }
    Ok(())
}

fn apply_page_layout(
    transaction: &rusqlite::Transaction<'_>,
    uuid: uuid::Uuid,
    layout: PageLayout,
    operation: &Op,
    timestamp: i64,
) -> rusqlite::Result<()> {
    let current = transaction
        .query_row(
            "SELECT layout_hlc FROM pages WHERE uuid = ?1",
            [uuid],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    if hlc_wins(current.as_deref(), &operation.hlc) {
        transaction.execute(
            "UPDATE pages SET layout = ?2, layout_hlc = ?3,
                              updated_at = MAX(updated_at, ?4)
              WHERE uuid = ?1",
            rusqlite::params![uuid, layout, operation.hlc.to_string(), timestamp],
        )?;
    }
    Ok(())
}

fn apply_block_markdown(
    transaction: &rusqlite::Transaction<'_>,
    uuid: uuid::Uuid,
    markdown: &str,
    operation: &Op,
    timestamp: i64,
    effects: &mut ApplyEffects,
) -> rusqlite::Result<()> {
    let current = transaction
        .query_row(
            "SELECT markdown_hlc FROM blocks WHERE uuid = ?1",
            [uuid],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    if hlc_wins(current.as_deref(), &operation.hlc) {
        transaction.execute(
            "UPDATE blocks
                SET markdown = ?2, body_stemmed = ?3, markdown_hlc = ?4,
                    updated_at = MAX(updated_at, ?5)
              WHERE uuid = ?1",
            rusqlite::params![
                uuid,
                markdown,
                crate::stem::stem(markdown),
                operation.hlc.to_string(),
                timestamp,
            ],
        )?;
        effects.request_block_refs(transaction, uuid, markdown, timestamp)?;
    }
    Ok(())
}

fn apply_block_style(
    transaction: &rusqlite::Transaction<'_>,
    uuid: uuid::Uuid,
    style: BlockStyle,
    operation: &Op,
    timestamp: i64,
) -> rusqlite::Result<()> {
    let current = transaction
        .query_row(
            "SELECT style_hlc FROM blocks WHERE uuid = ?1",
            [uuid],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    if hlc_wins(current.as_deref(), &operation.hlc) {
        transaction.execute(
            "UPDATE blocks SET style = ?2, style_hlc = ?3,
                               updated_at = MAX(updated_at, ?4)
              WHERE uuid = ?1",
            rusqlite::params![uuid, style, operation.hlc.to_string(), timestamp],
        )?;
    }
    Ok(())
}

fn write_structure_intent(
    transaction: &rusqlite::Transaction<'_>,
    block_uuid: uuid::Uuid,
    page_uuid: uuid::Uuid,
    parent_uuid: Option<uuid::Uuid>,
    order_key: &OrderKey,
    hlc: &Hlc,
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO block_structure_lww(block_uuid, page_uuid, parent_uuid, order_key, hlc)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(block_uuid) DO UPDATE SET
           page_uuid = excluded.page_uuid,
           parent_uuid = excluded.parent_uuid,
           order_key = excluded.order_key,
           hlc = excluded.hlc
         WHERE block_structure_lww.hlc < excluded.hlc",
        rusqlite::params![
            block_uuid,
            page_uuid,
            parent_uuid,
            order_key,
            hlc.to_string()
        ],
    )?;
    Ok(())
}

#[derive(Clone)]
struct StructureIntent {
    page_uuid: uuid::Uuid,
    parent_uuid: Option<uuid::Uuid>,
    order_key: OrderKey,
    hlc: String,
}

fn reconcile_structure(transaction: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    let existing = {
        let mut statement = transaction.prepare("SELECT uuid, existence_hlc FROM blocks")?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, uuid::Uuid>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<HashMap<_, _>>>()?
    };
    let pages = {
        let mut statement = transaction.prepare("SELECT uuid FROM pages")?;
        statement
            .query_map([], |row| row.get::<_, uuid::Uuid>(0))?
            .collect::<rusqlite::Result<HashSet<_>>>()?
    };
    let mut intents = {
        let mut statement = transaction.prepare(
            "SELECT block_uuid, page_uuid, parent_uuid, order_key, hlc
               FROM block_structure_lww",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, uuid::Uuid>(0)?,
                    StructureIntent {
                        page_uuid: row.get(1)?,
                        parent_uuid: row.get(2)?,
                        order_key: row.get(3)?,
                        hlc: row.get(4)?,
                    },
                ))
            })?
            .filter_map(|result| match result {
                Ok((uuid, intent))
                    if existing.contains_key(&uuid) && pages.contains(&intent.page_uuid) =>
                {
                    Some(Ok((uuid, intent)))
                }
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<rusqlite::Result<HashMap<_, _>>>()?
    };
    let pages_by_block = intents
        .iter()
        .map(|(uuid, intent)| (*uuid, intent.page_uuid))
        .collect::<HashMap<_, _>>();
    for intent in intents.values_mut() {
        if intent.parent_uuid.is_some_and(|parent| {
            existing.get(&parent).is_none_or(|parent_existence_hlc| {
                pages_by_block.get(&parent) != Some(&intent.page_uuid)
                    || intent.hlc.as_str() < parent_existence_hlc.as_str()
            })
        }) {
            intent.parent_uuid = None;
        }
    }
    break_structure_cycles(&mut intents);
    for (uuid, intent) in intents {
        let updated_at = sql_hlc(intent.hlc.clone(), 4)?.timestamp_seconds();
        transaction.execute(
            "UPDATE blocks
                SET page_uuid = ?2, parent_uuid = ?3, order_key = ?4, structure_hlc = ?5,
                    updated_at = MAX(updated_at, ?6)
              WHERE uuid = ?1",
            rusqlite::params![
                uuid,
                intent.page_uuid,
                intent.parent_uuid,
                intent.order_key,
                intent.hlc,
                updated_at,
            ],
        )?;
    }
    Ok(())
}

fn break_structure_cycles(intents: &mut HashMap<uuid::Uuid, StructureIntent>) {
    let starts = intents.keys().copied().collect::<Vec<_>>();
    let mut proven_acyclic = HashSet::new();
    for start in starts {
        if proven_acyclic.contains(&start) {
            continue;
        }
        let mut path = Vec::new();
        let mut positions = HashMap::new();
        let mut cursor = Some(start);
        let mut cycle_start = None;
        while let Some(uuid) = cursor {
            if proven_acyclic.contains(&uuid) {
                break;
            }
            if let Some(index) = positions.get(&uuid).copied() {
                cycle_start = Some(index);
                break;
            }
            positions.insert(uuid, path.len());
            path.push(uuid);
            cursor = intents.get(&uuid).and_then(|intent| intent.parent_uuid);
        }
        if let Some(index) = cycle_start {
            let detach = path[index..]
                .iter()
                .copied()
                .max_by_key(|uuid| {
                    intents
                        .get(uuid)
                        .map(|intent| (intent.hlc.clone(), *uuid))
                        .expect("cycle member has an intent")
                })
                .expect("cycle is non-empty");
            if let Some(intent) = intents.get_mut(&detach) {
                intent.parent_uuid = None;
            }
        }
        proven_acyclic.extend(path);
    }
}

fn page_reference_aliases(
    transaction: &rusqlite::Transaction<'_>,
    page_uuid: uuid::Uuid,
) -> rusqlite::Result<BTreeSet<String>> {
    let mut aliases = explicit_page_aliases(transaction, page_uuid)?;
    if let Some(title) = transaction
        .query_row(
            "SELECT normalized_title FROM pages WHERE uuid = ?1",
            [page_uuid],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
    {
        aliases.insert(title);
    }
    Ok(aliases)
}

fn explicit_page_aliases(
    transaction: &rusqlite::Transaction<'_>,
    page_uuid: uuid::Uuid,
) -> rusqlite::Result<BTreeSet<String>> {
    transaction
        .prepare(
            "SELECT alias FROM page_alias_lww
              WHERE page_uuid = ?1 AND present = 1",
        )?
        .query_map([page_uuid], |row| row.get::<_, String>(0))?
        .collect()
}

pub(crate) fn resolve_page_alias(
    transaction: &rusqlite::Connection,
    alias: &str,
) -> rusqlite::Result<Option<uuid::Uuid>> {
    let candidates = transaction
        .prepare(
            "SELECT uuid FROM pages WHERE normalized_title = ?1
             UNION
             SELECT aliases.page_uuid
               FROM page_alias_lww aliases
               JOIN pages ON pages.uuid = aliases.page_uuid
              WHERE aliases.alias = ?1 AND aliases.present = 1
             ORDER BY 1 LIMIT 2",
        )?
        .query_map([alias], |row| row.get::<_, uuid::Uuid>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok((candidates.len() == 1).then(|| candidates[0]))
}

fn reconcile_page_links_for_alias(
    transaction: &rusqlite::Transaction<'_>,
    alias: &str,
) -> rusqlite::Result<()> {
    let target = resolve_page_alias(transaction, alias)?;
    transaction.execute(
        "UPDATE page_links SET target_page_uuid = ?2 WHERE target_title = ?1",
        rusqlite::params![alias, target],
    )?;
    Ok(())
}

fn replace_block_refs(
    transaction: &rusqlite::Transaction<'_>,
    block_uuid: uuid::Uuid,
    markdown: &str,
    now: i64,
) -> rusqlite::Result<()> {
    transaction.execute(
        "DELETE FROM page_links WHERE source_block_uuid = ?1",
        [block_uuid],
    )?;
    transaction.execute(
        "DELETE FROM block_refs WHERE source_block_uuid = ?1",
        [block_uuid],
    )?;
    let (page_titles, block_uuids) = parse_refs(markdown);
    for title in page_titles {
        let normalized_title = crate::model::normalize_title(&title);
        let target_page_uuid = resolve_page_alias(transaction, &normalized_title)?;
        transaction.execute(
            "INSERT OR IGNORE INTO page_links
               (source_block_uuid, target_title, target_page_uuid, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![block_uuid, normalized_title, target_page_uuid, now],
        )?;
    }
    for target in block_uuids {
        if target == block_uuid {
            continue;
        }
        transaction.execute(
            "INSERT OR IGNORE INTO block_refs(source_block_uuid, target_block_uuid, created_at)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![block_uuid, target, now],
        )?;
    }
    Ok(())
}

pub fn parse_refs(markdown: &str) -> (Vec<String>, Vec<uuid::Uuid>) {
    let pages = delimited(markdown, "[[", "]]");
    let blocks = delimited(markdown, "((", "))")
        .into_iter()
        .filter_map(|value| uuid::Uuid::parse_str(&value).ok())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    (pages, blocks)
}

pub fn content_references_changed(previous: &str, next: &str) -> bool {
    parse_refs(previous) != parse_refs(next)
}

pub(crate) fn page_kind_from_sql(
    kind: String,
    date: Option<crate::model::JournalDate>,
    column: usize,
) -> rusqlite::Result<PageKind> {
    match (kind.as_str(), date) {
        ("note", None) => Ok(PageKind::Note),
        ("journal", Some(date)) => Ok(PageKind::Journal { date }),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid page identity shape: {kind}"),
            )),
        )),
    }
}

pub(crate) fn validate_page_identity(
    workspace_uuid: uuid::Uuid,
    page_uuid: uuid::Uuid,
    kind: &PageKind,
) -> CoreResult<()> {
    if let PageKind::Journal { date } = kind {
        let expected = journal_page_uuid(workspace_uuid, date);
        if page_uuid != expected {
            return Err(CoreError::invalid(format!(
                "journal {date} must use deterministic UUID {expected}"
            )));
        }
    }
    Ok(())
}

fn page_identity(
    transaction: &rusqlite::Transaction<'_>,
    page_uuid: uuid::Uuid,
) -> rusqlite::Result<Option<PageKind>> {
    transaction
        .query_row(
            "SELECT page_kind, journal_date FROM page_identities WHERE page_uuid = ?1",
            [page_uuid],
            |row| page_kind_from_sql(row.get(0)?, row.get(1)?, 0),
        )
        .optional()
}

fn insert_page_identity(
    transaction: &rusqlite::Transaction<'_>,
    page_uuid: uuid::Uuid,
    kind: &PageKind,
) -> CoreResult<()> {
    let (kind_name, date) = match kind {
        PageKind::Note => ("note", None),
        PageKind::Journal { date } => ("journal", Some(date)),
    };
    transaction.execute(
        "INSERT INTO page_identities(page_uuid, page_kind, journal_date)
         VALUES (?1, ?2, ?3)",
        rusqlite::params![page_uuid, kind_name, date],
    )?;
    Ok(())
}

pub(crate) fn ensure_page_identity(
    transaction: &rusqlite::Transaction<'_>,
    page_uuid: uuid::Uuid,
    kind: &PageKind,
) -> CoreResult<()> {
    let workspace_uuid = crate::db::transaction_workspace_uuid(transaction)?;
    validate_page_identity(workspace_uuid, page_uuid, kind)?;
    if let Some(existing) = page_identity(transaction, page_uuid)? {
        if existing != *kind {
            return Err(CoreError::conflict(format!(
                "page UUID {page_uuid} is already reserved for a different page kind"
            )));
        }
        return Ok(());
    }
    insert_page_identity(transaction, page_uuid, kind)
}

fn materialize_page_kind(
    transaction: &rusqlite::Transaction<'_>,
    page_uuid: uuid::Uuid,
    kind: &PageKind,
) -> CoreResult<()> {
    match kind {
        PageKind::Note => {
            transaction.execute(
                "DELETE FROM journal_pages WHERE page_uuid = ?1",
                [page_uuid],
            )?;
        }
        PageKind::Journal { date } => {
            transaction.execute(
                "INSERT INTO journal_pages(page_uuid, journal_date) VALUES (?1, ?2)
                 ON CONFLICT(page_uuid) DO UPDATE SET journal_date = excluded.journal_date",
                rusqlite::params![page_uuid, date],
            )?;
        }
    }
    Ok(())
}

fn ensure_object_kind(
    transaction: &rusqlite::Transaction<'_>,
    uuid: uuid::Uuid,
    expected: ObjectKind,
) -> CoreResult<()> {
    let page_exists = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM page_identities WHERE page_uuid = ?1)",
        [uuid],
        |row| row.get::<_, bool>(0),
    )?;
    let block_exists = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM blocks WHERE uuid = ?1)",
        [uuid],
        |row| row.get::<_, bool>(0),
    )?;
    let tombstone_kind = transaction
        .query_row(
            "SELECT object_kind FROM tombstones WHERE uuid = ?1",
            [uuid],
            |row| row.get::<_, ObjectKind>(0),
        )
        .optional()?;
    let conflicts = match expected {
        ObjectKind::Page => block_exists || tombstone_kind == Some(ObjectKind::Block),
        ObjectKind::Block => page_exists || tombstone_kind == Some(ObjectKind::Page),
    };
    if conflicts {
        return Err(CoreError::conflict(format!(
            "UUID {uuid} is already reserved for a different object kind"
        )));
    }
    Ok(())
}

fn delimited(content: &str, open: &str, close: &str) -> Vec<String> {
    let mut values = BTreeSet::new();
    let mut remaining = content;
    while let Some(start) = remaining.find(open) {
        let value = &remaining[start + open.len()..];
        let Some(end) = value.find(close) else { break };
        let value = value[..end].trim();
        if !value.is_empty() {
            values.insert(value.to_owned());
        }
        remaining = &remaining[start + open.len() + end + close.len()..];
    }
    values.into_iter().collect()
}

fn write_tombstone(
    transaction: &rusqlite::Transaction<'_>,
    uuid: uuid::Uuid,
    object_kind: ObjectKind,
    root_page_uuid: Option<uuid::Uuid>,
    hlc: &Hlc,
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO tombstones(uuid, object_kind, deleted_hlc, root_page_uuid)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(uuid) DO UPDATE SET
           object_kind = excluded.object_kind,
           deleted_hlc = excluded.deleted_hlc,
           root_page_uuid = excluded.root_page_uuid
         WHERE tombstones.deleted_hlc < excluded.deleted_hlc",
        rusqlite::params![uuid, object_kind, hlc.to_string(), root_page_uuid],
    )?;
    Ok(())
}

fn tombstone_dominates(
    transaction: &rusqlite::Transaction<'_>,
    uuid: uuid::Uuid,
    hlc: &Hlc,
) -> rusqlite::Result<bool> {
    Ok(transaction
        .query_row(
            "SELECT deleted_hlc FROM tombstones WHERE uuid = ?1",
            [uuid],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .is_some_and(|deleted| deleted >= hlc.to_string()))
}

fn is_tombstoned(
    transaction: &rusqlite::Transaction<'_>,
    uuid: uuid::Uuid,
) -> rusqlite::Result<bool> {
    transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM tombstones WHERE uuid = ?1)",
        [uuid],
        |row| row.get(0),
    )
}

fn tombstone_equals(
    transaction: &rusqlite::Transaction<'_>,
    uuid: uuid::Uuid,
    hlc: &Hlc,
) -> rusqlite::Result<bool> {
    Ok(transaction
        .query_row(
            "SELECT deleted_hlc FROM tombstones WHERE uuid = ?1",
            [uuid],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .is_some_and(|deleted| deleted == hlc.to_string()))
}

fn hlc_wins(current: Option<&str>, incoming: &Hlc) -> bool {
    current.is_none_or(|current| current < incoming.to_string().as_str())
}

fn write_attachment_intent(
    transaction: &rusqlite::Transaction<'_>,
    attachment: &SnapshotAttachment,
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO attachment_lww
           (owner_kind, owner_uuid, blob_hash, attachment_uuid, hlc, present,
            filename, mime, size)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(owner_kind, owner_uuid, blob_hash) DO UPDATE SET
           attachment_uuid = excluded.attachment_uuid,
           hlc = excluded.hlc,
           present = excluded.present,
           filename = excluded.filename,
           mime = excluded.mime,
           size = excluded.size
         WHERE attachment_lww.hlc < excluded.hlc",
        rusqlite::params![
            attachment.owner.kind(),
            attachment.owner.uuid(),
            blob_hash_bytes(&attachment.blob_hash),
            attachment.attachment_uuid,
            attachment.hlc.to_string(),
            attachment.present,
            attachment.filename,
            attachment.mime,
            attachment.size.and_then(|size| i64::try_from(size).ok()),
        ],
    )?;
    Ok(())
}

fn reconcile_all_attachments(transaction: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    let keys = {
        let mut statement =
            transaction.prepare("SELECT owner_kind, owner_uuid, blob_hash FROM attachment_lww")?;
        statement
            .query_map([], |row| {
                let kind = row.get::<_, ObjectKind>(0)?;
                let uuid = row.get::<_, uuid::Uuid>(1)?;
                let owner = match kind {
                    ObjectKind::Page => AttachmentOwner::Page(uuid),
                    ObjectKind::Block => AttachmentOwner::Block(uuid),
                };
                Ok((owner, row_blob_hash(row, 2)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (owner, hash) in keys {
        reconcile_attachment(transaction, owner, &hash)?;
    }
    Ok(())
}

fn reconcile_attachments_for_owner(
    transaction: &rusqlite::Transaction<'_>,
    owner: AttachmentOwner,
) -> rusqlite::Result<()> {
    let hashes = {
        let mut statement = transaction.prepare(
            "SELECT blob_hash FROM attachment_lww WHERE owner_kind = ?1 AND owner_uuid = ?2",
        )?;
        statement
            .query_map(rusqlite::params![owner.kind(), owner.uuid()], |row| {
                row_blob_hash(row, 0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for hash in hashes {
        reconcile_attachment(transaction, owner, &hash)?;
    }
    Ok(())
}

fn reconcile_attachment(
    transaction: &rusqlite::Transaction<'_>,
    owner: AttachmentOwner,
    blob_hash: &BlobHash,
) -> rusqlite::Result<()> {
    let intent = transaction
        .query_row(
            "SELECT attachment_uuid, hlc, present, filename, mime, size
               FROM attachment_lww
              WHERE owner_kind = ?1 AND owner_uuid = ?2 AND blob_hash = ?3",
            rusqlite::params![owner.kind(), owner.uuid(), blob_hash_bytes(blob_hash)],
            |row| {
                Ok((
                    row.get::<_, uuid::Uuid>(0)?,
                    sql_hlc(row.get::<_, String>(1)?, 1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((uuid, hlc, present, filename, mime, size)) = intent else {
        return Ok(());
    };
    let owner_exists = match owner {
        AttachmentOwner::Page(uuid) => transaction
            .query_row("SELECT 1 FROM pages WHERE uuid = ?1", [uuid], |_| Ok(()))
            .optional()?
            .is_some(),
        AttachmentOwner::Block(uuid) => transaction
            .query_row("SELECT 1 FROM blocks WHERE uuid = ?1", [uuid], |_| Ok(()))
            .optional()?
            .is_some(),
    };
    if !present || !owner_exists {
        transaction.execute("DELETE FROM attachments WHERE uuid = ?1", [uuid])?;
        return Ok(());
    }
    let (Some(filename), Some(mime), Some(size)) = (filename, mime, size) else {
        return Ok(());
    };
    let (page_uuid, block_uuid) = match owner {
        AttachmentOwner::Page(uuid) => (Some(uuid), None),
        AttachmentOwner::Block(uuid) => (None, Some(uuid)),
    };
    transaction.execute(
        "INSERT INTO attachments
           (uuid, page_uuid, block_uuid, blob_hash, filename, mime, size, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(uuid) DO UPDATE SET
           page_uuid = excluded.page_uuid,
           block_uuid = excluded.block_uuid,
           filename = excluded.filename,
           mime = excluded.mime,
           size = excluded.size",
        rusqlite::params![
            uuid,
            page_uuid,
            block_uuid,
            blob_hash_bytes(blob_hash),
            filename,
            mime,
            size,
            hlc.timestamp_seconds(),
        ],
    )?;
    Ok(())
}

pub fn attachment_uuid(owner: AttachmentOwner, blob_hash: &BlobHash) -> uuid::Uuid {
    uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_OID,
        format!(
            "notes-rs:attachment:{}:{}:{blob_hash}",
            owner.kind(),
            owner.uuid()
        )
        .as_bytes(),
    )
}

fn snapshot_attachment_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SnapshotAttachment> {
    let kind = row.get::<_, ObjectKind>(0)?;
    let uuid = row.get::<_, uuid::Uuid>(1)?;
    let owner = match kind {
        ObjectKind::Page => AttachmentOwner::Page(uuid),
        ObjectKind::Block => AttachmentOwner::Block(uuid),
    };
    Ok(SnapshotAttachment {
        owner,
        blob_hash: row_blob_hash(row, 2)?,
        attachment_uuid: row.get(3)?,
        hlc: sql_hlc(row.get(4)?, 4)?,
        present: row.get(5)?,
        filename: row.get(6)?,
        mime: row.get(7)?,
        size: row
            .get::<_, Option<i64>>(8)?
            .and_then(|size| u64::try_from(size).ok()),
    })
}

pub(crate) fn sql_hlc(value: String, index: usize) -> rusqlite::Result<Hlc> {
    value.parse::<Hlc>().map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error.to_string(),
            )),
        )
    })
}

fn optional_sql_hlc(value: Option<String>, index: usize) -> rusqlite::Result<Option<Hlc>> {
    value.map(|value| sql_hlc(value, index)).transpose()
}

fn sync_is_configured(transaction: &rusqlite::Transaction<'_>) -> rusqlite::Result<bool> {
    Ok(transaction
        .query_row(
            "SELECT value FROM sync_meta WHERE key = 'server_url'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .is_some_and(|value| !value.trim().is_empty()))
}

fn meta_or_insert_device_id(
    transaction: &rusqlite::Transaction<'_>,
) -> rusqlite::Result<uuid::Uuid> {
    if let Some(device_id) = transaction
        .query_row(
            "SELECT device_id FROM local_device WHERE singleton = 1",
            [],
            |row| row.get::<_, uuid::Uuid>(0),
        )
        .optional()?
    {
        return Ok(device_id);
    }
    let device_id = uuid::Uuid::now_v7();
    transaction.execute(
        "INSERT INTO local_device(singleton, device_id) VALUES (1, ?1)",
        [device_id],
    )?;
    Ok(device_id)
}

fn observe_hlc(transaction: &rusqlite::Transaction<'_>, incoming: &Hlc) -> rusqlite::Result<()> {
    let previous = transaction
        .query_row(
            "SELECT value FROM sync_meta WHERE key = 'last_hlc'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .and_then(|value| value.parse::<Hlc>().ok());
    let device_id = meta_or_insert_device_id(transaction)?;
    let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
    let observed = Hlc::receive(previous.as_ref(), incoming, now, device_id);
    transaction.execute(
        "INSERT INTO sync_meta(key, value) VALUES ('last_hlc', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [observed.to_string()],
    )?;
    Ok(())
}

fn observe_max_persisted_hlc(transaction: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    let value = transaction.query_row(
        "SELECT MAX(value) FROM (
           SELECT title_hlc AS value FROM pages
           UNION ALL SELECT layout_hlc FROM pages
           UNION ALL SELECT existence_hlc FROM pages
           UNION ALL SELECT markdown_hlc FROM blocks
           UNION ALL SELECT style_hlc FROM blocks
           UNION ALL SELECT structure_hlc FROM blocks
           UNION ALL SELECT existence_hlc FROM blocks
           UNION ALL SELECT deleted_hlc FROM tombstones
           UNION ALL SELECT hlc FROM page_alias_lww
           UNION ALL SELECT hlc FROM block_structure_lww
           UNION ALL SELECT hlc FROM attachment_lww
         ) WHERE value IS NOT NULL",
        [],
        |row| row.get::<_, Option<String>>(0),
    )?;
    if let Some(value) = value {
        observe_hlc(transaction, &sql_hlc(value, 0)?)?;
    }
    Ok(())
}

fn operation_timestamp(operation: &Op) -> i64 {
    operation.hlc.timestamp_seconds()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_json_uses_typed_page_variant() {
        let operation = Op {
            op_id: uuid::Uuid::from_u128(2),
            workspace_uuid: uuid::Uuid::from_u128(1),
            device_id: uuid::Uuid::from_u128(3),
            hlc: Hlc::new(1, 0, uuid::Uuid::from_u128(3)),
            format_version: FORMAT_VERSION,
            kind: OpKind::PageDelete(PageDelete {
                uuid: uuid::Uuid::from_u128(1),
            }),
        };
        let json = serde_json::to_value(&operation).expect("serialize operation");
        assert_eq!(json["kind"], "page_delete");
        assert_eq!(
            serde_json::from_value::<Op>(json).expect("deserialize operation"),
            operation
        );
    }

    #[test]
    fn reference_parser_deduplicates_and_validates_targets() {
        let block = uuid::Uuid::now_v7();
        assert_eq!(
            parse_refs(&format!("[[ Roadmap ]] [[Roadmap]] (({block})) ((bad))")),
            (vec!["Roadmap".into()], vec![block])
        );
        assert!(!content_references_changed(
            "Before [[Roadmap]]",
            "After [[Roadmap]]"
        ));
        assert!(content_references_changed(
            "Before [[Roadmap]]",
            "After [[Release]]"
        ));
    }

    #[test]
    fn cycle_break_is_deterministic() {
        let first = uuid::Uuid::from_u128(1);
        let second = uuid::Uuid::from_u128(2);
        let page = uuid::Uuid::from_u128(3);
        let mut intents = HashMap::from([
            (
                first,
                StructureIntent {
                    page_uuid: page,
                    parent_uuid: Some(second),
                    order_key: OrderKey::first(),
                    hlc: "1".into(),
                },
            ),
            (
                second,
                StructureIntent {
                    page_uuid: page,
                    parent_uuid: Some(first),
                    order_key: OrderKey::first(),
                    hlc: "2".into(),
                },
            ),
        ]);
        break_structure_cycles(&mut intents);
        assert_eq!(intents[&first].parent_uuid, Some(second));
        assert_eq!(intents[&second].parent_uuid, None);
    }

    #[test]
    fn attachment_identity_includes_the_typed_owner_kind() {
        let uuid = uuid::Uuid::now_v7();
        let hash = BlobHash::from_bytes([0xaa; 32]);
        let page_attachment = attachment_uuid(AttachmentOwner::Page(uuid), &hash);
        assert_eq!(
            page_attachment,
            attachment_uuid(AttachmentOwner::Page(uuid), &hash)
        );
        assert_eq!(page_attachment.get_version_num(), 5);
        assert_ne!(
            page_attachment,
            attachment_uuid(AttachmentOwner::Block(uuid), &hash)
        );
    }
}

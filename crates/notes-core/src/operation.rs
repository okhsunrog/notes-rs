//! Versioned, UUID-addressed source operations and the single apply boundary.

use crate::{Connection, CoreError, CoreResult, Hlc, NodeKind, db};
use anyhow::{Context, Result};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Local,
    Remote,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Op {
    pub op_id: uuid::Uuid,
    pub device_id: uuid::Uuid,
    pub hlc: Hlc,
    pub format_version: u32,
    #[serde(flatten)]
    pub kind: OpKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum OpKind {
    NodeCreate(NodeCreate),
    NodeSetContent(NodeSetContent),
    NodeSetTitle(NodeSetTitle),
    NodeMove(NodeMove),
    NodeDelete(NodeDelete),
    EdgeAdd(EdgeAdd),
    EdgeRemove(EdgeRemove),
    AttachmentAdd(AttachmentAdd),
    AttachmentRemove(AttachmentRemove),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeCreate {
    pub uuid: uuid::Uuid,
    pub node_kind: NodeKind,
    pub title: Option<String>,
    pub content: String,
    pub content_json: Option<String>,
    pub parent_uuid: Option<uuid::Uuid>,
    pub position: Option<f64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeSetContent {
    pub uuid: uuid::Uuid,
    pub content: String,
    pub content_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeSetTitle {
    pub uuid: uuid::Uuid,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeMove {
    pub uuid: uuid::Uuid,
    pub parent_uuid: Option<uuid::Uuid>,
    pub position: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeDelete {
    pub uuid: uuid::Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EdgeAdd {
    pub src_uuid: uuid::Uuid,
    pub dst_uuid: uuid::Uuid,
    pub edge_kind: String,
    pub weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EdgeRemove {
    pub src_uuid: uuid::Uuid,
    pub dst_uuid: uuid::Uuid,
    pub edge_kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttachmentAdd {
    pub node_uuid: uuid::Uuid,
    pub blob_hash: String,
    pub filename: String,
    pub mime: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttachmentRemove {
    pub node_uuid: uuid::Uuid,
    pub blob_hash: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyOutcome {
    pub applied: bool,
    pub affected_uuids: Vec<uuid::Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncSnapshot {
    pub format_version: u32,
    pub seq: u64,
    pub nodes: Vec<SnapshotNode>,
    pub tombstones: Vec<SnapshotTombstone>,
    pub edges: Vec<SnapshotEdge>,
    pub attachments: Vec<SnapshotAttachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotNode {
    pub uuid: uuid::Uuid,
    pub kind: NodeKind,
    pub title: Option<String>,
    pub content: String,
    pub content_json: Option<String>,
    pub parent_uuid: Option<uuid::Uuid>,
    pub position: Option<f64>,
    pub content_hlc: Option<Hlc>,
    pub title_hlc: Option<Hlc>,
    pub structure_hlc: Option<Hlc>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotTombstone {
    pub uuid: uuid::Uuid,
    pub deleted_hlc: Hlc,
    pub root_uuid: Option<uuid::Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotEdge {
    pub src_uuid: uuid::Uuid,
    pub dst_uuid: uuid::Uuid,
    pub kind: String,
    pub hlc: Hlc,
    pub present: bool,
    pub weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotAttachment {
    pub node_uuid: uuid::Uuid,
    pub blob_hash: String,
    pub hlc: Hlc,
    pub present: bool,
    pub filename: Option<String>,
    pub mime: Option<String>,
    pub size: Option<u64>,
}

/// Allocate monotonically increasing local HLCs and wrap operation payloads.
pub async fn local_ops(conn: &Connection, kinds: Vec<OpKind>) -> Result<Vec<Op>> {
    if kinds.is_empty() {
        return Ok(Vec::new());
    }
    conn.call(move |database| {
        let transaction = database.transaction()?;
        let device_id = meta_or_insert_device_id(&transaction)?;
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
            operations.push(Op {
                op_id: uuid::Uuid::now_v7(),
                device_id,
                hlc: hlc.clone(),
                format_version: FORMAT_VERSION,
                kind,
            });
            clock = Some(hlc);
        }
        if let Some(last) = operations.last() {
            transaction.execute(
                "INSERT INTO sync_meta(key, value) VALUES ('last_hlc', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [&last.hlc.to_string()],
            )?;
        }
        transaction.commit()?;
        Ok(operations)
    })
    .await
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
        let envelopes = statement
            .query_map([limit], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        envelopes
            .into_iter()
            .map(|envelope| {
                serde_json::from_str(&envelope).map_err(|error| {
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

/// Apply an ordered server batch, acknowledge echoed local operations, and
/// advance the sync cursor in one SQLite transaction.
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
        for (seq, operation) in operations {
            if seq > cursor.saturating_add(1) {
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
                    "operation {} was assigned conflicting server sequences {existing_seq} and {seq}",
                    operation.op_id
                )));
            }
            let outcome = if existing.is_some() {
                ApplyOutcome::default()
            } else {
                let affected_uuids = apply_one(&transaction, &operation, Origin::Remote)?;
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
        let nodes = {
            let mut statement = database.prepare(
                "SELECT n.uuid, n.kind, n.title, n.content, n.content_json, parent.uuid,
                        n.position, n.content_hlc, n.title_hlc, n.structure_hlc,
                        n.created_at, n.updated_at
                   FROM nodes n LEFT JOIN nodes parent ON parent.id = n.parent_id
                  WHERE n.kind IN ('page', 'block') ORDER BY n.uuid",
            )?;
            statement
                .query_map([], |row| {
                    Ok(SnapshotNode {
                        uuid: row.get(0)?,
                        kind: row.get(1)?,
                        title: row.get(2)?,
                        content: row.get(3)?,
                        content_json: row.get(4)?,
                        parent_uuid: row.get(5)?,
                        position: row.get(6)?,
                        content_hlc: optional_sql_hlc(row.get(7)?, 7)?,
                        title_hlc: optional_sql_hlc(row.get(8)?, 8)?,
                        structure_hlc: optional_sql_hlc(row.get(9)?, 9)?,
                        created_at: row.get(10)?,
                        updated_at: row.get(11)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        let tombstones = {
            let mut statement = database
                .prepare("SELECT uuid, deleted_hlc, root_uuid FROM tombstones ORDER BY uuid")?;
            statement
                .query_map([], |row| {
                    Ok(SnapshotTombstone {
                        uuid: row.get(0)?,
                        deleted_hlc: sql_hlc(row.get(1)?, 1)?,
                        root_uuid: row.get(2)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        let edges = {
            let mut statement = database.prepare(
                "SELECT src_uuid, dst_uuid, kind, hlc, present, weight
                   FROM edge_lww ORDER BY src_uuid, dst_uuid, kind",
            )?;
            statement
                .query_map([], |row| {
                    Ok(SnapshotEdge {
                        src_uuid: row.get(0)?,
                        dst_uuid: row.get(1)?,
                        kind: row.get(2)?,
                        hlc: sql_hlc(row.get(3)?, 3)?,
                        present: row.get(4)?,
                        weight: row.get(5)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        let attachments = {
            let mut statement = database.prepare(
                "SELECT node_uuid, blob_hash, hlc, present, filename, mime, size
                   FROM attachment_lww ORDER BY node_uuid, blob_hash",
            )?;
            statement
                .query_map([], |row| {
                    let size = row.get::<_, Option<i64>>(6)?;
                    Ok(SnapshotAttachment {
                        node_uuid: row.get(0)?,
                        blob_hash: row.get(1)?,
                        hlc: sql_hlc(row.get(2)?, 2)?,
                        present: row.get(3)?,
                        filename: row.get(4)?,
                        mime: row.get(5)?,
                        size: size.and_then(|size| u64::try_from(size).ok()),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(SyncSnapshot {
            format_version: FORMAT_VERSION,
            seq,
            nodes,
            tombstones,
            edges,
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
    let mut clocks = snapshot
        .nodes
        .iter()
        .flat_map(|node| {
            [
                node.content_hlc.as_ref(),
                node.title_hlc.as_ref(),
                node.structure_hlc.as_ref(),
            ]
            .into_iter()
            .flatten()
        })
        .chain(snapshot.tombstones.iter().map(|item| &item.deleted_hlc))
        .chain(snapshot.edges.iter().map(|item| &item.hlc))
        .chain(snapshot.attachments.iter().map(|item| &item.hlc));
    let max_hlc = clocks.next().map(|first| {
        clocks
            .fold(first, |current, next| current.max(next))
            .to_string()
    });
    conn.call(move |database| {
        let transaction = database.transaction()?;
        transaction.execute_batch(
            "DELETE FROM nodes;
             DELETE FROM tombstones;
             DELETE FROM edge_lww;
             DELETE FROM attachment_lww;
             DELETE FROM applied_ops;
             DELETE FROM sync_outbox;",
        )?;
        for node in &snapshot.nodes {
            let id = db::stable_node_id(&transaction, &node.uuid)?;
            let body = crate::stem::stem(&format!(
                "{}\n{}",
                node.title.as_deref().unwrap_or(""),
                node.content
            ));
            transaction.execute(
                "INSERT INTO nodes
                   (id, uuid, kind, title, content, content_json, body_stemmed,
                    parent_id, position, content_hlc, title_hlc, structure_hlc,
                    created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?9, ?10, ?11, ?12, ?13)",
                rusqlite::params![
                    id,
                    node.uuid,
                    node.kind,
                    node.title,
                    node.content,
                    node.content_json,
                    body,
                    node.position,
                    node.content_hlc,
                    node.title_hlc,
                    node.structure_hlc,
                    node.created_at,
                    node.updated_at,
                ],
            )?;
        }
        for node in &snapshot.nodes {
            if let Some(parent_uuid) = &node.parent_uuid {
                transaction.execute(
                    "UPDATE nodes SET parent_id = (SELECT id FROM nodes WHERE uuid = ?2)
                     WHERE uuid = ?1",
                    rusqlite::params![node.uuid, parent_uuid],
                )?;
            }
        }
        for node in snapshot
            .nodes
            .iter()
            .filter(|node| node.kind == NodeKind::Block)
        {
            let id = db::stable_node_id(&transaction, &node.uuid)?;
            let (wikilinks, block_refs) = parse_refs(&node.content);
            db::replace_block_refs_tx_at(
                &transaction,
                id,
                &wikilinks,
                &block_refs,
                node.updated_at,
            )?;
        }
        for item in snapshot.tombstones {
            transaction.execute(
                "INSERT INTO tombstones(uuid, deleted_hlc, root_uuid) VALUES (?1, ?2, ?3)",
                rusqlite::params![item.uuid, item.deleted_hlc, item.root_uuid],
            )?;
        }
        for item in snapshot.edges {
            transaction.execute(
                "INSERT INTO edge_lww(src_uuid, dst_uuid, kind, hlc, present, weight)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    item.src_uuid,
                    item.dst_uuid,
                    item.kind,
                    item.hlc,
                    item.present,
                    item.weight,
                ],
            )?;
            reconcile_edge(&transaction, &item.src_uuid, &item.dst_uuid, &item.kind)?;
        }
        for item in snapshot.attachments {
            transaction.execute(
                "INSERT INTO attachment_lww
                   (node_uuid, blob_hash, hlc, present, filename, mime, size)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    item.node_uuid,
                    item.blob_hash,
                    item.hlc,
                    item.present,
                    item.filename,
                    item.mime,
                    item.size.and_then(|size| i64::try_from(size).ok()),
                ],
            )?;
            reconcile_attachment(&transaction, &item.node_uuid, &item.blob_hash)?;
        }
        transaction.execute(
            "INSERT INTO sync_meta(key, value) VALUES ('last_server_seq', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [snapshot.seq.to_string()],
        )?;
        if let Some(max_hlc) = max_hlc {
            transaction.execute(
                "INSERT INTO sync_meta(key, value) VALUES ('last_hlc', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [max_hlc],
            )?;
        }
        transaction.commit()?;
        Ok(())
    })
    .await
}

fn sql_hlc(value: String, index: usize) -> rusqlite::Result<Hlc> {
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

/// Apply a logical mutation batch atomically. Redelivery of any already
/// recorded `op_id` is a successful no-op.
pub async fn apply_batch(
    conn: &Connection,
    operations: &[Op],
    origin: Origin,
) -> Result<Vec<ApplyOutcome>> {
    for operation in operations {
        validate(operation)?;
    }
    let operations = operations.to_vec();
    let envelopes = operations
        .iter()
        .map(serde_json::to_string)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    conn.call(move |database| {
        let transaction = database.transaction()?;
        let mut outcomes = Vec::with_capacity(operations.len());
        for (operation, envelope) in operations.iter().zip(envelopes) {
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
            let affected_uuids = apply_one(&transaction, operation, origin)?;
            transaction.execute(
                "INSERT INTO applied_ops(op_id, seq) VALUES (?1, NULL)",
                [&operation.op_id],
            )?;
            if origin == Origin::Local && sync_is_configured(&transaction)? {
                transaction.execute(
                    "INSERT OR IGNORE INTO sync_outbox(op_id, envelope, created_at)
                     VALUES (?1, ?2, unixepoch())",
                    rusqlite::params![operation.op_id, envelope],
                )?;
            }
            observe_hlc(&transaction, &operation.hlc)?;
            outcomes.push(ApplyOutcome {
                applied: true,
                affected_uuids,
            });
        }
        transaction.commit()?;
        Ok(outcomes)
    })
    .await
}

fn validate(operation: &Op) -> CoreResult<()> {
    if operation.format_version != FORMAT_VERSION {
        return Err(CoreError::invalid(format!(
            "unsupported operation format version {}",
            operation.format_version
        )));
    }
    if operation.hlc.device_id() != operation.device_id {
        return Err(CoreError::invalid(
            "hybrid logical clock device suffix does not match device_id",
        ));
    }
    match &operation.kind {
        OpKind::NodeCreate(_) | OpKind::NodeSetContent(_) | OpKind::NodeSetTitle(_) => {}
        OpKind::NodeMove(payload) => {
            if !payload.position.is_finite() {
                return Err(CoreError::invalid("node position must be finite"));
            }
        }
        OpKind::NodeDelete(_) => {}
        OpKind::EdgeAdd(payload) => {
            require_edge(&payload.src_uuid, &payload.dst_uuid, &payload.edge_kind)?;
            if !payload.weight.is_finite() {
                return Err(CoreError::invalid("edge weight must be finite"));
            }
        }
        OpKind::EdgeRemove(payload) => {
            require_edge(&payload.src_uuid, &payload.dst_uuid, &payload.edge_kind)?;
        }
        OpKind::AttachmentAdd(payload) => {
            validate_blob_hash(&payload.blob_hash)?;
            let mut components = std::path::Path::new(&payload.filename).components();
            if payload.filename.trim().is_empty()
                || !matches!(components.next(), Some(std::path::Component::Normal(_)))
                || components.next().is_some()
            {
                return Err(CoreError::invalid(
                    "attachment filename must be one safe path component",
                ));
            }
        }
        OpKind::AttachmentRemove(payload) => {
            validate_blob_hash(&payload.blob_hash)?;
        }
    }
    Ok(())
}

pub fn validate_blob_hash(hash: &str) -> CoreResult<()> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(CoreError::invalid(
            "attachment blob hash must be 64 lowercase hexadecimal characters",
        ));
    }
    Ok(())
}

fn require_edge(src: &uuid::Uuid, dst: &uuid::Uuid, kind: &str) -> CoreResult<()> {
    if src == dst || kind.trim().is_empty() {
        return Err(CoreError::invalid(
            "edge requires distinct nodes and a kind",
        ));
    }
    Ok(())
}

fn apply_one(
    transaction: &rusqlite::Transaction<'_>,
    operation: &Op,
    _origin: Origin,
) -> rusqlite::Result<Vec<uuid::Uuid>> {
    let timestamp = operation_timestamp(operation);
    match &operation.kind {
        OpKind::NodeCreate(payload) => apply_node_create(transaction, operation, payload),
        OpKind::NodeSetContent(payload) => {
            let title = transaction
                .query_row(
                    "SELECT title FROM nodes WHERE uuid = ?1",
                    [&payload.uuid],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten();
            let changed = transaction.execute(
                "UPDATE nodes SET content = ?2, content_json = ?3,
                    body_stemmed = ?4, content_hlc = ?5, updated_at = ?6
                 WHERE uuid = ?1 AND (content_hlc IS NULL OR content_hlc < ?5)
                   AND NOT EXISTS (SELECT 1 FROM tombstones WHERE uuid = ?1)",
                rusqlite::params![
                    payload.uuid,
                    payload.content,
                    payload.content_json,
                    crate::stem::stem(&format!(
                        "{}\n{}",
                        title.as_deref().unwrap_or(""),
                        payload.content
                    )),
                    operation.hlc,
                    timestamp,
                ],
            )?;
            if changed > 0 {
                let (kind, id): (NodeKind, i64) = transaction.query_row(
                    "SELECT kind, id FROM nodes WHERE uuid = ?1",
                    [&payload.uuid],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                if kind == NodeKind::Block {
                    let (wikilinks, block_refs) = parse_refs(&payload.content);
                    db::replace_block_refs_tx_at(
                        transaction,
                        id,
                        &wikilinks,
                        &block_refs,
                        timestamp,
                    )?;
                }
            }
            Ok(vec![payload.uuid])
        }
        OpKind::NodeSetTitle(payload) => {
            let Some(content) = transaction
                .query_row(
                    "SELECT content FROM nodes WHERE uuid = ?1",
                    [&payload.uuid],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
            else {
                return Ok(vec![payload.uuid]);
            };
            transaction.execute(
                "UPDATE nodes SET title = ?2,
                    body_stemmed = ?3, title_hlc = ?4, updated_at = ?5
                 WHERE uuid = ?1 AND (title_hlc IS NULL OR title_hlc < ?4)
                   AND NOT EXISTS (SELECT 1 FROM tombstones WHERE uuid = ?1)",
                rusqlite::params![
                    payload.uuid,
                    payload.title,
                    crate::stem::stem(&format!(
                        "{}\n{content}",
                        payload.title.as_deref().unwrap_or("")
                    )),
                    operation.hlc,
                    timestamp,
                ],
            )?;
            Ok(vec![payload.uuid])
        }
        OpKind::NodeMove(payload) => {
            let changed = transaction.execute(
                "INSERT INTO node_structure_lww(node_uuid, parent_uuid, position, hlc)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(node_uuid) DO UPDATE SET
                   parent_uuid = excluded.parent_uuid,
                   position = excluded.position,
                   hlc = excluded.hlc
                 WHERE node_structure_lww.hlc < excluded.hlc",
                rusqlite::params![
                    payload.uuid,
                    payload.parent_uuid,
                    payload.position,
                    operation.hlc,
                ],
            )?;
            if changed > 0 {
                reconcile_node_structure(transaction, &payload.uuid)?;
                transaction.execute(
                    "UPDATE nodes SET updated_at = ?2 WHERE uuid = ?1",
                    rusqlite::params![payload.uuid, timestamp],
                )?;
            }
            Ok(vec![payload.uuid])
        }
        OpKind::NodeDelete(payload) => {
            let root_uuid = containing_page_uuid(transaction, &payload.uuid)?;
            let previous = transaction
                .query_row(
                    "SELECT deleted_hlc FROM tombstones WHERE uuid = ?1",
                    [&payload.uuid],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if previous
                .as_deref()
                .and_then(|clock| clock.parse::<Hlc>().ok())
                .as_ref()
                .is_none_or(|clock| clock < &operation.hlc)
            {
                let deleted_id = transaction
                    .query_row(
                        "SELECT id FROM nodes WHERE uuid = ?1",
                        [&payload.uuid],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?;
                if let Some(deleted_id) = deleted_id {
                    let root_id = root_uuid
                        .as_ref()
                        .filter(|root_uuid| **root_uuid != payload.uuid)
                        .map(|root_uuid| {
                            transaction
                                .query_row(
                                    "SELECT id FROM nodes WHERE uuid = ?1 AND kind = 'page'",
                                    [root_uuid],
                                    |row| row.get::<_, i64>(0),
                                )
                                .optional()
                        })
                        .transpose()?
                        .flatten();
                    // A subtree deletion carries one node_delete per existing
                    // descendant. Reparenting first preserves a concurrently
                    // created child that has no matching delete operation.
                    transaction.execute(
                        "UPDATE nodes SET parent_id = ?2 WHERE parent_id = ?1",
                        rusqlite::params![deleted_id, root_id],
                    )?;
                }
                transaction.execute("DELETE FROM nodes WHERE uuid = ?1", [&payload.uuid])?;
                transaction.execute(
                    "INSERT INTO tombstones(uuid, deleted_hlc, root_uuid) VALUES (?1, ?2, ?3)
                     ON CONFLICT(uuid) DO UPDATE SET
                       deleted_hlc = excluded.deleted_hlc,
                       root_uuid = COALESCE(excluded.root_uuid, tombstones.root_uuid)",
                    rusqlite::params![payload.uuid, operation.hlc, root_uuid],
                )?;
                reconcile_node_structure(transaction, &payload.uuid)?;
            }
            Ok(vec![payload.uuid])
        }
        OpKind::EdgeAdd(payload) => {
            write_edge_intent(transaction, payload, &operation.hlc, true)?;
            Ok(vec![payload.src_uuid, payload.dst_uuid])
        }
        OpKind::EdgeRemove(payload) => {
            write_edge_intent(
                transaction,
                &EdgeAdd {
                    src_uuid: payload.src_uuid,
                    dst_uuid: payload.dst_uuid,
                    edge_kind: payload.edge_kind.clone(),
                    weight: 0.0,
                },
                &operation.hlc,
                false,
            )?;
            Ok(vec![payload.src_uuid, payload.dst_uuid])
        }
        OpKind::AttachmentAdd(payload) => apply_attachment_add(transaction, operation, payload),
        OpKind::AttachmentRemove(payload) => {
            write_attachment_intent(transaction, payload, None, &operation.hlc, false)?;
            Ok(vec![payload.node_uuid])
        }
    }
}

fn apply_node_create(
    transaction: &rusqlite::Transaction<'_>,
    operation: &Op,
    payload: &NodeCreate,
) -> rusqlite::Result<Vec<uuid::Uuid>> {
    let tombstone = transaction
        .query_row(
            "SELECT deleted_hlc FROM tombstones WHERE uuid = ?1",
            [&payload.uuid],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(deleted_hlc) = tombstone {
        if deleted_hlc
            .parse::<Hlc>()
            .is_ok_and(|deleted_hlc| operation.hlc <= deleted_hlc)
        {
            return Ok(vec![payload.uuid]);
        }
        // A newer explicit node_create is the inverse operation used by Undo.
        // Ordinary edits still cannot resurrect a tombstoned node.
        transaction.execute("DELETE FROM tombstones WHERE uuid = ?1", [&payload.uuid])?;
    }
    let parent_id = resolve_parent(transaction, payload.parent_uuid.as_ref())?;
    let node_id = db::stable_node_id(transaction, &payload.uuid)?;
    let body = crate::stem::stem(&format!(
        "{}\n{}",
        payload.title.as_deref().unwrap_or(""),
        payload.content
    ));
    if payload.node_kind == NodeKind::Page
        && let Some(title) = payload.title.as_deref()
    {
        let stub_uuid = db::page_uuid(title);
        adopt_page_stub(transaction, &stub_uuid, &payload.uuid, title, node_id)?;
    }
    transaction.execute(
        "INSERT OR IGNORE INTO nodes
           (id, uuid, kind, title, content, content_json, body_stemmed, parent_id, position,
            content_hlc, title_hlc, structure_hlc, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?10, ?11, ?11)",
        rusqlite::params![
            node_id,
            payload.uuid,
            payload.node_kind,
            payload.title,
            payload.content,
            payload.content_json,
            body,
            parent_id,
            payload.position,
            operation.hlc,
            payload.created_at
        ],
    )?;
    transaction.execute(
        "UPDATE nodes SET title = ?2, content = ?3, content_json = ?4,
            body_stemmed = ?5, parent_id = ?6, position = ?7,
            content_hlc = ?8, title_hlc = ?8, structure_hlc = ?8,
            created_at = MIN(created_at, ?9), updated_at = ?11
         WHERE uuid = ?1 AND kind = ?10
           AND (content_hlc IS NULL OR content_hlc < ?8)",
        rusqlite::params![
            payload.uuid,
            payload.title,
            payload.content,
            payload.content_json,
            body,
            parent_id,
            payload.position,
            operation.hlc,
            payload.created_at,
            payload.node_kind,
            operation_timestamp(operation),
        ],
    )?;
    if payload.node_kind == NodeKind::Block {
        transaction.execute(
            "INSERT INTO node_structure_lww(node_uuid, parent_uuid, position, hlc)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(node_uuid) DO UPDATE SET
               parent_uuid = excluded.parent_uuid,
               position = excluded.position,
               hlc = excluded.hlc
             WHERE node_structure_lww.hlc < excluded.hlc",
            rusqlite::params![
                payload.uuid,
                payload.parent_uuid,
                payload.position.unwrap_or_default(),
                operation.hlc,
            ],
        )?;
        reconcile_node_structure(transaction, &payload.uuid)?;
    }
    if payload.node_kind == NodeKind::Block {
        let id = transaction.query_row(
            "SELECT id FROM nodes WHERE uuid = ?1",
            [&payload.uuid],
            |row| row.get::<_, i64>(0),
        )?;
        let (wikilinks, block_refs) = parse_refs(&payload.content);
        db::replace_block_refs_tx_at(
            transaction,
            id,
            &wikilinks,
            &block_refs,
            operation_timestamp(operation),
        )?;
    }
    reconcile_deferred_for_node(transaction, &payload.uuid)?;
    Ok(vec![payload.uuid])
}

fn adopt_page_stub(
    transaction: &rusqlite::Transaction<'_>,
    stub_uuid: &uuid::Uuid,
    canonical_uuid: &uuid::Uuid,
    title: &str,
    canonical_id: i64,
) -> rusqlite::Result<()> {
    let stub_id = transaction
        .query_row(
            "SELECT id FROM nodes
             WHERE uuid = ?1 AND kind = 'page' AND lower(title) = lower(?2)",
            rusqlite::params![stub_uuid, title],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    let Some(stub_id) = stub_id else {
        return Ok(());
    };
    if stub_id == canonical_id {
        transaction.execute(
            "UPDATE nodes SET uuid = ?2 WHERE id = ?1",
            rusqlite::params![stub_id, canonical_uuid],
        )?;
        return Ok(());
    }
    // All relevant foreign keys are repaired before commit. Deferral allows
    // the primary-key change and its references to be updated atomically.
    transaction.execute_batch("PRAGMA defer_foreign_keys = ON;")?;
    transaction.execute(
        "UPDATE nodes SET id = ?2, uuid = ?3 WHERE id = ?1",
        rusqlite::params![stub_id, canonical_id, canonical_uuid],
    )?;
    for sql in [
        "UPDATE nodes SET parent_id = ?2 WHERE parent_id = ?1",
        "UPDATE edges SET src = ?2 WHERE src = ?1",
        "UPDATE edges SET dst = ?2 WHERE dst = ?1",
    ] {
        transaction.execute(sql, rusqlite::params![stub_id, canonical_id])?;
    }
    transaction.execute(
        "UPDATE edge_lww SET src_uuid = ?2 WHERE src_uuid = ?1",
        rusqlite::params![stub_uuid, canonical_uuid],
    )?;
    transaction.execute(
        "UPDATE edge_lww SET dst_uuid = ?2 WHERE dst_uuid = ?1",
        rusqlite::params![stub_uuid, canonical_uuid],
    )?;
    transaction.execute(
        "UPDATE attachment_lww SET node_uuid = ?2 WHERE node_uuid = ?1",
        rusqlite::params![stub_uuid, canonical_uuid],
    )?;
    transaction.execute(
        "UPDATE tombstones SET root_uuid = ?2 WHERE root_uuid = ?1",
        rusqlite::params![stub_uuid, canonical_uuid],
    )?;
    Ok(())
}

fn apply_attachment_add(
    transaction: &rusqlite::Transaction<'_>,
    operation: &Op,
    payload: &AttachmentAdd,
) -> rusqlite::Result<Vec<uuid::Uuid>> {
    let attachment_uuid = attachment_uuid(&payload.node_uuid, &payload.blob_hash);
    write_attachment_intent(
        transaction,
        &AttachmentRemove {
            node_uuid: payload.node_uuid,
            blob_hash: payload.blob_hash.clone(),
        },
        Some(payload),
        &operation.hlc,
        true,
    )?;
    Ok(vec![payload.node_uuid, attachment_uuid])
}

fn write_edge_intent(
    transaction: &rusqlite::Transaction<'_>,
    payload: &EdgeAdd,
    hlc: &Hlc,
    present: bool,
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO edge_lww(src_uuid, dst_uuid, kind, hlc, present, weight)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(src_uuid, dst_uuid, kind) DO UPDATE SET
           hlc = excluded.hlc,
           present = excluded.present,
           weight = excluded.weight
         WHERE edge_lww.hlc < excluded.hlc",
        rusqlite::params![
            payload.src_uuid,
            payload.dst_uuid,
            payload.edge_kind,
            hlc,
            present,
            payload.weight
        ],
    )?;
    reconcile_edge(
        transaction,
        &payload.src_uuid,
        &payload.dst_uuid,
        &payload.edge_kind,
    )
}

fn reconcile_edge(
    transaction: &rusqlite::Transaction<'_>,
    src_uuid: &uuid::Uuid,
    dst_uuid: &uuid::Uuid,
    kind: &str,
) -> rusqlite::Result<()> {
    let intent = transaction
        .query_row(
            "SELECT present, weight, hlc FROM edge_lww
             WHERE src_uuid = ?1 AND dst_uuid = ?2 AND kind = ?3",
            rusqlite::params![src_uuid, dst_uuid, kind],
            |row| {
                Ok((
                    row.get::<_, bool>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    match intent {
        Some((true, weight, hlc)) => {
            let timestamp = hlc
                .parse::<Hlc>()
                .map_or(0, |clock| clock.timestamp_seconds());
            transaction.execute(
                "INSERT INTO edges(src, dst, kind, weight, created_at)
                 SELECT src.id, dst.id, ?3, ?4, ?5
                   FROM nodes src, nodes dst
                  WHERE src.uuid = ?1 AND dst.uuid = ?2
                 ON CONFLICT(src, dst, kind) DO UPDATE SET
                   weight = excluded.weight,
                   created_at = excluded.created_at",
                rusqlite::params![src_uuid, dst_uuid, kind, weight, timestamp],
            )?;
        }
        Some((false, _, _)) => {
            transaction.execute(
                "DELETE FROM edges WHERE kind = ?3
                   AND src = (SELECT id FROM nodes WHERE uuid = ?1)
                   AND dst = (SELECT id FROM nodes WHERE uuid = ?2)",
                rusqlite::params![src_uuid, dst_uuid, kind],
            )?;
        }
        None => {}
    }
    Ok(())
}

fn write_attachment_intent(
    transaction: &rusqlite::Transaction<'_>,
    key: &AttachmentRemove,
    add: Option<&AttachmentAdd>,
    hlc: &Hlc,
    present: bool,
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO attachment_lww
           (node_uuid, blob_hash, hlc, present, filename, mime, size)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(node_uuid, blob_hash) DO UPDATE SET
           hlc = excluded.hlc,
           present = excluded.present,
           filename = COALESCE(excluded.filename, attachment_lww.filename),
           mime = COALESCE(excluded.mime, attachment_lww.mime),
           size = COALESCE(excluded.size, attachment_lww.size)
         WHERE attachment_lww.hlc < excluded.hlc",
        rusqlite::params![
            key.node_uuid,
            key.blob_hash,
            hlc,
            present,
            add.map(|payload| payload.filename.as_str()),
            add.map(|payload| payload.mime.as_str()),
            add.map(|payload| payload.size as i64),
        ],
    )?;
    reconcile_attachment(transaction, &key.node_uuid, &key.blob_hash)
}

fn reconcile_attachment(
    transaction: &rusqlite::Transaction<'_>,
    node_uuid: &uuid::Uuid,
    blob_hash: &str,
) -> rusqlite::Result<()> {
    let intent = transaction
        .query_row(
            "SELECT present, filename, mime, size, hlc FROM attachment_lww
             WHERE node_uuid = ?1 AND blob_hash = ?2",
            rusqlite::params![node_uuid, blob_hash],
            |row| {
                Ok((
                    row.get::<_, bool>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?;
    let attachment_uuid = attachment_uuid(node_uuid, blob_hash);
    let attachment_id = db::stable_node_id(transaction, &attachment_uuid)?;
    let Some((present, filename, mime, size, hlc)) = intent else {
        return Ok(());
    };
    let timestamp = hlc
        .parse::<Hlc>()
        .map_or(0, |clock| clock.timestamp_seconds());
    if !present {
        transaction.execute("DELETE FROM nodes WHERE uuid = ?1", [&attachment_uuid])?;
        return Ok(());
    }
    let Some(filename) = filename else {
        return Ok(());
    };
    let Some(parent_id) = transaction
        .query_row(
            "SELECT id FROM nodes WHERE uuid = ?1 AND kind IN ('page', 'block')",
            [node_uuid],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
    else {
        return Ok(());
    };
    let relative_path = format!("attachments/{blob_hash}/{filename}");
    let metadata = serde_json::json!({
        "blobHash": blob_hash,
        "mime": mime.unwrap_or_else(|| "application/octet-stream".into()),
        "size": size.unwrap_or_default(),
    })
    .to_string();
    transaction.execute(
        "INSERT INTO nodes
           (id, uuid, kind, title, content, content_json, body_stemmed,
            content_hlc, title_hlc, structure_hlc, created_at, updated_at)
         VALUES (?1, ?2, 'attachment', ?3, ?4, ?5, ?6, ?7, ?7, ?7, ?8, ?8)
         ON CONFLICT(uuid) DO UPDATE SET
           title = excluded.title,
           content = excluded.content,
           content_json = excluded.content_json,
           body_stemmed = excluded.body_stemmed,
           content_hlc = excluded.content_hlc,
           title_hlc = excluded.title_hlc,
           updated_at = excluded.updated_at",
        rusqlite::params![
            attachment_id,
            attachment_uuid,
            filename,
            relative_path,
            metadata,
            crate::stem::stem(&filename),
            hlc,
            timestamp,
        ],
    )?;
    transaction.execute(
        "INSERT OR IGNORE INTO edges(src, dst, kind, weight, created_at)
         SELECT ?1, id, 'attachment', 1.0, ?3 FROM nodes WHERE uuid = ?2",
        rusqlite::params![parent_id, attachment_uuid, timestamp],
    )?;
    Ok(())
}

fn resolve_parent(
    transaction: &rusqlite::Transaction<'_>,
    parent_uuid: Option<&uuid::Uuid>,
) -> rusqlite::Result<Option<i64>> {
    let Some(parent_uuid) = parent_uuid else {
        return Ok(None);
    };
    if let Some(parent_id) = transaction
        .query_row(
            "SELECT id FROM nodes WHERE uuid = ?1 AND kind IN ('page', 'block')",
            [parent_uuid],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
    {
        return Ok(Some(parent_id));
    }
    let root_uuid = transaction
        .query_row(
            "SELECT root_uuid FROM tombstones WHERE uuid = ?1",
            [parent_uuid],
            |row| row.get::<_, Option<uuid::Uuid>>(0),
        )
        .optional()?
        .flatten();
    root_uuid
        .map(|root_uuid| {
            transaction
                .query_row(
                    "SELECT id FROM nodes WHERE uuid = ?1 AND kind = 'page'",
                    [root_uuid],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
        })
        .transpose()
        .map(Option::flatten)
}

fn containing_page_uuid(
    transaction: &rusqlite::Transaction<'_>,
    node_uuid: &uuid::Uuid,
) -> rusqlite::Result<Option<uuid::Uuid>> {
    transaction
        .query_row(
            "WITH RECURSIVE ancestors(id, uuid, kind, parent_id, depth) AS (
               SELECT id, uuid, kind, parent_id, 0 FROM nodes WHERE uuid = ?1
               UNION ALL
               SELECT n.id, n.uuid, n.kind, n.parent_id, ancestors.depth + 1
                 FROM nodes n JOIN ancestors ON n.id = ancestors.parent_id
             )
             SELECT uuid FROM ancestors WHERE kind = 'page' ORDER BY depth LIMIT 1",
            [node_uuid],
            |row| row.get::<_, uuid::Uuid>(0),
        )
        .optional()
}

fn reconcile_node_structure(
    transaction: &rusqlite::Transaction<'_>,
    changed_uuid: &uuid::Uuid,
) -> rusqlite::Result<()> {
    let intents = {
        let mut statement = transaction.prepare(
            "WITH RECURSIVE component(uuid) AS (
               VALUES (?1)
               UNION
               SELECT intent.parent_uuid FROM node_structure_lww intent
                 JOIN component ON intent.node_uuid = component.uuid
                WHERE intent.parent_uuid IS NOT NULL
               UNION
               SELECT intent.node_uuid FROM node_structure_lww intent
                 JOIN component ON intent.parent_uuid = component.uuid
             )
             SELECT node_uuid, parent_uuid, position, hlc
               FROM node_structure_lww
              WHERE node_uuid IN (SELECT uuid FROM component)
              ORDER BY node_uuid",
        )?;
        statement
            .query_map([changed_uuid], |row| {
                Ok((
                    row.get::<_, uuid::Uuid>(0)?,
                    row.get::<_, Option<uuid::Uuid>>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };

    // Re-materialize the connected intent component. Cycle breaking happens
    // only in parent_id, so a later change in this component can reproduce all
    // winning intents without scanning unrelated workspaces.
    for (node_uuid, parent_uuid, position, hlc) in &intents {
        let parent_id = resolve_parent(transaction, parent_uuid.as_ref())?;
        transaction.execute(
            "UPDATE nodes SET parent_id = ?2, position = ?3, structure_hlc = ?4
             WHERE uuid = ?1 AND kind = 'block'
               AND NOT EXISTS (SELECT 1 FROM tombstones WHERE uuid = ?1)",
            rusqlite::params![node_uuid, parent_id, position, hlc],
        )?;
    }

    let root_id = transaction
        .query_row(
            "SELECT id FROM nodes WHERE kind = 'page' ORDER BY uuid LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    loop {
        let nodes = {
            let mut statement = transaction.prepare(
                "WITH RECURSIVE component(uuid) AS (
                   VALUES (?1)
                   UNION
                   SELECT intent.parent_uuid FROM node_structure_lww intent
                     JOIN component ON intent.node_uuid = component.uuid
                    WHERE intent.parent_uuid IS NOT NULL
                   UNION
                   SELECT intent.node_uuid FROM node_structure_lww intent
                     JOIN component ON intent.parent_uuid = component.uuid
                 )
                 SELECT id, uuid, parent_id, COALESCE(structure_hlc, '')
                   FROM nodes
                  WHERE kind = 'block' AND uuid IN (SELECT uuid FROM component)
                  ORDER BY uuid",
            )?;
            statement
                .query_map([changed_uuid], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, uuid::Uuid>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        let by_id = nodes
            .iter()
            .map(|node| (node.0, node))
            .collect::<std::collections::HashMap<_, _>>();
        let mut cycle_to_break = None;
        for start in &nodes {
            let mut chain = Vec::new();
            let mut next = Some(start.0);
            while let Some(id) = next {
                if let Some(cycle_start) = chain.iter().position(|seen| *seen == id) {
                    cycle_to_break = chain[cycle_start..]
                        .iter()
                        .filter_map(|id| by_id.get(id).copied())
                        .min_by(|left, right| (&left.3, left.1).cmp(&(&right.3, right.1)))
                        .map(|node| node.0);
                    break;
                }
                chain.push(id);
                next = by_id.get(&id).and_then(|node| node.2);
            }
            if cycle_to_break.is_some() {
                break;
            }
        }
        let Some(break_id) = cycle_to_break else {
            return Ok(());
        };
        transaction.execute(
            "UPDATE nodes SET parent_id = ?2 WHERE id = ?1",
            rusqlite::params![break_id, root_id],
        )?;
    }
}

fn reconcile_deferred_for_node(
    transaction: &rusqlite::Transaction<'_>,
    uuid: &uuid::Uuid,
) -> rusqlite::Result<()> {
    let edges = {
        let mut statement = transaction.prepare(
            "SELECT src_uuid, dst_uuid, kind FROM edge_lww
             WHERE src_uuid = ?1 OR dst_uuid = ?1",
        )?;
        statement
            .query_map([uuid], |row| {
                Ok((
                    row.get::<_, uuid::Uuid>(0)?,
                    row.get::<_, uuid::Uuid>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    for (src_uuid, dst_uuid, kind) in edges {
        reconcile_edge(transaction, &src_uuid, &dst_uuid, &kind)?;
    }
    let blobs = {
        let mut statement =
            transaction.prepare("SELECT blob_hash FROM attachment_lww WHERE node_uuid = ?1")?;
        statement
            .query_map([uuid], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    for blob_hash in blobs {
        reconcile_attachment(transaction, uuid, &blob_hash)?;
    }
    Ok(())
}

fn sync_is_configured(transaction: &rusqlite::Transaction<'_>) -> rusqlite::Result<bool> {
    transaction.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM sync_meta WHERE key = 'server_url' AND trim(value) <> ''
         )",
        [],
        |row| row.get(0),
    )
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
    let device_id = uuid::Uuid::new_v4();
    transaction.execute(
        "INSERT INTO local_device(singleton, device_id) VALUES (1, ?1)",
        [device_id],
    )?;
    Ok(device_id)
}

fn observe_hlc(transaction: &rusqlite::Transaction<'_>, incoming: &Hlc) -> rusqlite::Result<()> {
    let current = transaction
        .query_row(
            "SELECT value FROM sync_meta WHERE key = 'last_hlc'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let current = current.and_then(|value| value.parse::<Hlc>().ok());
    let device_id = meta_or_insert_device_id(transaction)?;
    let observed = if incoming.device_id() == device_id {
        current
            .clone()
            .filter(|clock| clock >= incoming)
            .unwrap_or_else(|| incoming.clone())
    } else {
        Hlc::receive(
            current.as_ref(),
            incoming,
            chrono::Utc::now().timestamp_millis().max(0) as u64,
            device_id,
        )
    };
    if current.as_ref() != Some(&observed) {
        transaction.execute(
            "INSERT INTO sync_meta(key, value) VALUES ('last_hlc', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [observed.to_string()],
        )?;
    }
    Ok(())
}

fn operation_timestamp(operation: &Op) -> i64 {
    operation.hlc.timestamp_seconds()
}

pub(crate) fn parse_refs(content: &str) -> (Vec<String>, Vec<String>) {
    (
        delimited(content, "[[", "]]"),
        delimited(content, "((", "))"),
    )
}

/// Returns whether changing a block from `previous` to `next` changes its
/// deterministic wikilink or block-reference edges.
pub fn content_references_changed(previous: &str, next: &str) -> bool {
    parse_refs(previous) != parse_refs(next)
}

fn delimited(content: &str, open: &str, close: &str) -> Vec<String> {
    let mut remaining = content;
    let mut values = Vec::new();
    while let Some(start) = remaining.find(open) {
        remaining = &remaining[start + open.len()..];
        let Some(end) = remaining.find(close) else {
            break;
        };
        let value = remaining[..end].trim();
        if !value.is_empty() && !values.iter().any(|existing| existing == value) {
            values.push(value.to_string());
        }
        remaining = &remaining[end + close.len()..];
    }
    values
}

fn attachment_uuid(node_uuid: &uuid::Uuid, blob_hash: &str) -> uuid::Uuid {
    uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_OID,
        format!("notes-rs:attachment:{node_uuid}:{blob_hash}").as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn database() -> (tempfile::TempDir, Connection) {
        let directory = tempfile::tempdir().expect("temporary directory");
        let connection = db::open(directory.path().join("notes.db"))
            .await
            .expect("open test database");
        (directory, connection)
    }

    fn remote_op(device: u128, wall_ms: u64, counter: u32, kind: OpKind) -> Op {
        let device_id = uuid::Uuid::from_u128(device);
        Op {
            op_id: uuid::Uuid::new_v4(),
            device_id,
            hlc: Hlc::new(wall_ms, counter, device_id),
            format_version: FORMAT_VERSION,
            kind,
        }
    }

    fn create_node_op(
        device: u128,
        wall_ms: u64,
        uuid: &uuid::Uuid,
        kind: &str,
        title: Option<&str>,
        parent_uuid: Option<&uuid::Uuid>,
        position: Option<f64>,
    ) -> Op {
        remote_op(
            device,
            wall_ms,
            0,
            OpKind::NodeCreate(NodeCreate {
                uuid: *uuid,
                node_kind: kind.parse().expect("valid test node kind"),
                title: title.map(str::to_owned),
                content: String::new(),
                content_json: None,
                parent_uuid: parent_uuid.copied(),
                position,
                created_at: (wall_ms / 1_000) as i64,
            }),
        )
    }

    #[tokio::test]
    async fn uuid_derived_local_id_collision_fails_without_losing_a_node() {
        let (_directory, connection) = database().await;
        let first_uuid = uuid::Uuid::from_u128(1);
        let colliding_uuid = uuid::Uuid::from_u128(1_u128 << 64);
        let first = create_node_op(1, 1_000, &first_uuid, "page", Some("First"), None, None);
        let second = create_node_op(
            1,
            1_001,
            &colliding_uuid,
            "page",
            Some("Second"),
            None,
            None,
        );

        apply(&connection, &first, Origin::Remote)
            .await
            .expect("first node");
        let error = apply(&connection, &second, Origin::Remote)
            .await
            .expect_err("colliding local ID must be rejected");
        assert!(error.to_string().contains("node ID collision"));
        assert!(
            db::get_node_by_uuid(&connection, first_uuid)
                .await
                .expect("first node query")
                .is_some()
        );
        assert!(
            db::get_node_by_uuid(&connection, colliding_uuid)
                .await
                .expect("colliding node query")
                .is_none()
        );
    }

    #[tokio::test]
    async fn sequenced_batch_is_atomic_when_it_contains_a_gap() {
        let (_directory, connection) = database().await;
        let first_uuid = uuid::Uuid::new_v4();
        let second_uuid = uuid::Uuid::new_v4();
        let first = create_node_op(1, 1_000, &first_uuid, "page", Some("First"), None, None);
        let second = create_node_op(1, 1_001, &second_uuid, "page", Some("Second"), None, None);

        let error = apply_sequenced_batch(&connection, vec![(1, first), (3, second)])
            .await
            .expect_err("sequence gap must reject the whole batch");
        assert!(error.to_string().contains("sync sequence gap"));
        assert_eq!(sync_cursor(&connection).await.expect("cursor"), 0);
        assert!(
            db::get_node_by_uuid(&connection, first_uuid)
                .await
                .expect("first node query")
                .is_none()
        );
        assert!(
            db::get_node_by_uuid(&connection, second_uuid)
                .await
                .expect("second node query")
                .is_none()
        );
    }

    #[tokio::test]
    async fn sequenced_batch_applies_and_advances_cursor_once() {
        let (_directory, connection) = database().await;
        let first_uuid = uuid::Uuid::new_v4();
        let second_uuid = uuid::Uuid::new_v4();
        let operations = vec![
            (
                1,
                create_node_op(1, 1_000, &first_uuid, "page", Some("First"), None, None),
            ),
            (
                2,
                create_node_op(1, 1_001, &second_uuid, "page", Some("Second"), None, None),
            ),
        ];

        let outcomes = apply_sequenced_batch(&connection, operations)
            .await
            .expect("apply batch");
        assert_eq!(outcomes.len(), 2);
        assert!(outcomes.iter().all(|outcome| outcome.applied));
        assert_eq!(sync_cursor(&connection).await.expect("cursor"), 2);
        assert!(
            db::get_node_by_uuid(&connection, first_uuid)
                .await
                .expect("first node query")
                .is_some()
        );
        assert!(
            db::get_node_by_uuid(&connection, second_uuid)
                .await
                .expect("second node query")
                .is_some()
        );
    }

    #[test]
    fn envelope_round_trips_with_flat_kind_and_payload() {
        let device_id = uuid::Uuid::new_v4();
        let operation = Op {
            op_id: uuid::Uuid::new_v4(),
            device_id,
            hlc: Hlc::new(42, 3, device_id),
            format_version: FORMAT_VERSION,
            kind: OpKind::NodeDelete(NodeDelete {
                uuid: uuid::Uuid::from_u128(1),
            }),
        };
        let json = serde_json::to_value(&operation).expect("serialize operation");
        assert_eq!(json["kind"], "node_delete");
        assert_eq!(
            json["payload"]["uuid"],
            uuid::Uuid::from_u128(1).to_string()
        );
        assert_eq!(
            serde_json::from_value::<Op>(json).expect("deserialize operation"),
            operation
        );
    }

    #[test]
    fn reference_parser_deduplicates_and_trims_targets() {
        assert_eq!(
            parse_refs("[[ Roadmap ]] [[Roadmap]] ((abc))"),
            (vec!["Roadmap".into()], vec!["abc".into()])
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

    #[tokio::test]
    async fn apply_is_idempotent_and_outbox_contains_one_envelope() {
        let (_directory, connection) = database().await;
        connection
            .call(|database| {
                database.execute(
                    "INSERT INTO sync_meta(key, value) VALUES ('server_url', 'https://sync.test')",
                    [],
                )?;
                Ok(())
            })
            .await
            .expect("configure sync");
        let node_uuid = uuid::Uuid::new_v4();
        let operation = local_ops(
            &connection,
            vec![OpKind::NodeCreate(NodeCreate {
                uuid: node_uuid,
                node_kind: NodeKind::Page,
                title: Some("Idempotent".into()),
                content: String::new(),
                content_json: None,
                parent_uuid: None,
                position: None,
                created_at: 1,
            })],
        )
        .await
        .expect("allocate operation")
        .remove(0);

        assert!(
            apply(&connection, &operation, Origin::Local)
                .await
                .expect("first apply")
                .applied
        );
        assert!(
            !apply(&connection, &operation, Origin::Local)
                .await
                .expect("redelivery")
                .applied
        );
        let counts = connection
            .call(|database| {
                database.query_row(
                    "SELECT
                       (SELECT COUNT(*) FROM nodes WHERE title = 'Idempotent'),
                       (SELECT COUNT(*) FROM applied_ops),
                       (SELECT COUNT(*) FROM sync_outbox)",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
            })
            .await
            .expect("read counts");
        assert_eq!(counts, (1, 1, 1));
    }

    #[tokio::test]
    async fn older_content_operation_cannot_overwrite_newer_content() {
        let (_directory, connection) = database().await;
        let node_uuid = uuid::Uuid::new_v4();
        let create = local_ops(
            &connection,
            vec![OpKind::NodeCreate(NodeCreate {
                uuid: node_uuid,
                node_kind: NodeKind::Page,
                title: Some("LWW".into()),
                content: "initial".into(),
                content_json: None,
                parent_uuid: None,
                position: None,
                created_at: 1,
            })],
        )
        .await
        .expect("create operation")
        .remove(0);
        apply(&connection, &create, Origin::Remote)
            .await
            .expect("apply create");
        let updates = local_ops(
            &connection,
            vec![
                OpKind::NodeSetContent(NodeSetContent {
                    uuid: node_uuid,
                    content: "older".into(),
                    content_json: None,
                }),
                OpKind::NodeSetContent(NodeSetContent {
                    uuid: node_uuid,
                    content: "newer".into(),
                    content_json: None,
                }),
            ],
        )
        .await
        .expect("update operations");
        apply(&connection, &updates[1], Origin::Remote)
            .await
            .expect("newer update");
        apply(&connection, &updates[0], Origin::Remote)
            .await
            .expect("older update");
        let content: String = connection
            .call(move |database| {
                database.query_row(
                    "SELECT content FROM nodes WHERE uuid = ?1",
                    [node_uuid],
                    |row| row.get(0),
                )
            })
            .await
            .expect("read content");
        assert_eq!(content, "newer");
    }

    #[tokio::test]
    async fn edits_cannot_resurrect_a_tombstoned_node() {
        let (_directory, connection) = database().await;
        let node_uuid = uuid::Uuid::new_v4();
        let mut operations = local_ops(
            &connection,
            vec![
                OpKind::NodeCreate(NodeCreate {
                    uuid: node_uuid,
                    node_kind: NodeKind::Page,
                    title: Some("Deleted".into()),
                    content: String::new(),
                    content_json: None,
                    parent_uuid: None,
                    position: None,
                    created_at: 1,
                }),
                OpKind::NodeDelete(NodeDelete { uuid: node_uuid }),
                OpKind::NodeSetContent(NodeSetContent {
                    uuid: node_uuid,
                    content: "must not return".into(),
                    content_json: None,
                }),
            ],
        )
        .await
        .expect("operations");
        let edit = operations.pop().expect("edit");
        let delete = operations.pop().expect("delete");
        let create = operations.pop().expect("create");
        apply(&connection, &create, Origin::Remote)
            .await
            .expect("create");
        apply(&connection, &delete, Origin::Remote)
            .await
            .expect("delete");
        apply(&connection, &edit, Origin::Remote)
            .await
            .expect("edit after delete");
        let counts = connection
            .call(move |database| {
                database.query_row(
                    "SELECT
                       (SELECT COUNT(*) FROM nodes WHERE uuid = ?1),
                       (SELECT COUNT(*) FROM tombstones WHERE uuid = ?1)",
                    [node_uuid],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
            })
            .await
            .expect("read state");
        assert_eq!(counts, (0, 1));
    }

    #[tokio::test]
    async fn canonical_page_create_adopts_an_existing_wikilink_stub() {
        let (_directory, connection) = database().await;
        let (_direct_directory, direct) = database().await;
        let block_uuid = uuid::Uuid::new_v4();
        let block = local_ops(
            &connection,
            vec![OpKind::NodeCreate(NodeCreate {
                uuid: block_uuid,
                node_kind: NodeKind::Block,
                title: None,
                content: "See [[Roadmap]]".into(),
                content_json: None,
                parent_uuid: None,
                position: Some(1024.0),
                created_at: 1,
            })],
        )
        .await
        .expect("block operation")
        .remove(0);
        apply(&connection, &block, Origin::Remote)
            .await
            .expect("materialize stub");
        let stub_uuid = db::get_page_by_title(&connection, "Roadmap".into())
            .await
            .expect("query stub")
            .expect("stub")
            .uuid;

        let canonical_uuid = uuid::Uuid::new_v4();
        let page = local_ops(
            &connection,
            vec![OpKind::NodeCreate(NodeCreate {
                uuid: canonical_uuid,
                node_kind: NodeKind::Page,
                title: Some("Roadmap".into()),
                content: String::new(),
                content_json: None,
                parent_uuid: None,
                position: None,
                created_at: 2,
            })],
        )
        .await
        .expect("page operation")
        .remove(0);
        apply(&connection, &page, Origin::Remote)
            .await
            .expect("adopt stub");
        apply(&direct, &page, Origin::Remote)
            .await
            .expect("create canonical page first");
        apply(&direct, &block, Origin::Remote)
            .await
            .expect("materialize direct ref");
        let adopted = db::get_page_by_title(&connection, "Roadmap".into())
            .await
            .expect("query adopted page")
            .expect("adopted page");
        assert_ne!(canonical_uuid, stub_uuid);
        assert_eq!(adopted.uuid, canonical_uuid);
        let direct_page = db::get_page_by_title(&direct, "Roadmap".into())
            .await
            .expect("query direct page")
            .expect("direct page");
        assert_eq!(adopted.id, direct_page.id);
        let refs: i64 = connection
            .call(move |database| {
                database.query_row(
                    "SELECT COUNT(*) FROM edges e
                     JOIN nodes src ON src.id = e.src
                     JOIN nodes dst ON dst.id = e.dst
                     WHERE src.uuid = ?1 AND dst.uuid = ?2 AND e.kind = 'refs'",
                    rusqlite::params![block_uuid, canonical_uuid],
                    |row| row.get(0),
                )
            })
            .await
            .expect("count preserved refs");
        assert_eq!(refs, 1);
    }

    #[tokio::test]
    async fn edge_add_remove_converges_when_delivered_in_reverse_order() {
        let (_left_dir, left) = database().await;
        let (_right_dir, right) = database().await;
        let src = uuid::Uuid::new_v4();
        let dst = uuid::Uuid::new_v4();
        let initial = vec![
            create_node_op(1, 1_000, &src, "page", Some("Source"), None, None),
            create_node_op(1, 1_001, &dst, "page", Some("Target"), None, None),
        ];
        for connection in [&left, &right] {
            apply_batch(connection, &initial, Origin::Remote)
                .await
                .expect("initial nodes");
        }
        let add = remote_op(
            1,
            2_000,
            0,
            OpKind::EdgeAdd(EdgeAdd {
                src_uuid: src,
                dst_uuid: dst,
                edge_kind: "relates_to".into(),
                weight: 0.8,
            }),
        );
        let remove = remote_op(
            2,
            2_000,
            1,
            OpKind::EdgeRemove(EdgeRemove {
                src_uuid: src,
                dst_uuid: dst,
                edge_kind: "relates_to".into(),
            }),
        );
        apply_batch(&left, &[add.clone(), remove.clone()], Origin::Remote)
            .await
            .expect("left delivery");
        apply_batch(&right, &[remove, add], Origin::Remote)
            .await
            .expect("right delivery");
        for connection in [&left, &right] {
            let count: i64 = connection
                .call(|database| {
                    database.query_row(
                        "SELECT COUNT(*) FROM edges WHERE kind = 'relates_to'",
                        [],
                        |row| row.get(0),
                    )
                })
                .await
                .expect("edge count");
            assert_eq!(count, 0);
        }
    }

    #[tokio::test]
    async fn concurrent_moves_forming_a_cycle_break_identically() {
        let (_left_dir, left) = database().await;
        let (_right_dir, right) = database().await;
        let page = uuid::Uuid::new_v4();
        let first = uuid::Uuid::new_v4();
        let second = uuid::Uuid::new_v4();
        let initial = vec![
            create_node_op(1, 1_000, &page, "page", Some("Root"), None, None),
            create_node_op(1, 1_001, &first, "block", None, Some(&page), Some(1.0)),
            create_node_op(1, 1_002, &second, "block", None, Some(&page), Some(2.0)),
        ];
        for connection in [&left, &right] {
            apply_batch(connection, &initial, Origin::Remote)
                .await
                .expect("initial tree");
        }
        let first_move = remote_op(
            1,
            2_000,
            0,
            OpKind::NodeMove(NodeMove {
                uuid: first,
                parent_uuid: Some(second),
                position: 1.0,
            }),
        );
        let second_move = remote_op(
            2,
            2_000,
            0,
            OpKind::NodeMove(NodeMove {
                uuid: second,
                parent_uuid: Some(first),
                position: 1.0,
            }),
        );
        apply_batch(
            &left,
            &[first_move.clone(), second_move.clone()],
            Origin::Remote,
        )
        .await
        .expect("left moves");
        apply_batch(&right, &[second_move, first_move], Origin::Remote)
            .await
            .expect("right moves");
        let parents = |connection: &Connection| {
            let connection = connection.clone();
            async move {
                connection
                    .call(|database| {
                        let mut statement = database.prepare(
                            "SELECT child.uuid, parent.uuid FROM nodes child
                             LEFT JOIN nodes parent ON parent.id = child.parent_id
                             WHERE child.kind = 'block' ORDER BY child.uuid",
                        )?;
                        statement
                            .query_map([], |row| {
                                Ok((
                                    row.get::<_, uuid::Uuid>(0)?,
                                    row.get::<_, Option<uuid::Uuid>>(1)?,
                                ))
                            })?
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .await
                    .expect("parent state")
            }
        };
        assert_eq!(parents(&left).await, parents(&right).await);
    }

    #[tokio::test]
    async fn concurrent_child_survives_parent_delete_at_page_root() {
        let (_left_dir, left) = database().await;
        let (_right_dir, right) = database().await;
        let page = uuid::Uuid::new_v4();
        let parent = uuid::Uuid::new_v4();
        let child = uuid::Uuid::new_v4();
        let initial = vec![
            create_node_op(1, 1_000, &page, "page", Some("Root"), None, None),
            create_node_op(1, 1_001, &parent, "block", None, Some(&page), Some(1.0)),
        ];
        for connection in [&left, &right] {
            apply_batch(connection, &initial, Origin::Remote)
                .await
                .expect("initial parent");
        }
        let delete = remote_op(1, 2_000, 0, OpKind::NodeDelete(NodeDelete { uuid: parent }));
        let create_child =
            create_node_op(2, 2_000, &child, "block", None, Some(&parent), Some(1.0));
        apply_batch(
            &left,
            &[delete.clone(), create_child.clone()],
            Origin::Remote,
        )
        .await
        .expect("delete first");
        apply_batch(&right, &[create_child, delete], Origin::Remote)
            .await
            .expect("create first");
        for connection in [&left, &right] {
            let parent_uuid: Option<uuid::Uuid> = connection
                .call({
                    move |database| {
                        database.query_row(
                            "SELECT parent.uuid FROM nodes child
                             LEFT JOIN nodes parent ON parent.id = child.parent_id
                             WHERE child.uuid = ?1",
                            [child],
                            |row| row.get(0),
                        )
                    }
                })
                .await
                .expect("child parent");
            assert_eq!(parent_uuid, Some(page));
        }
    }

    #[tokio::test]
    async fn equal_sibling_positions_are_ordered_by_uuid() {
        let (_directory, connection) = database().await;
        let page = uuid::Uuid::new_v4();
        let low = uuid::Uuid::from_u128(10);
        let high = uuid::Uuid::from_u128(20);
        let operations = vec![
            create_node_op(1, 1_000, &page, "page", Some("Root"), None, None),
            create_node_op(2, 1_001, &high, "block", None, Some(&page), Some(1.0)),
            create_node_op(3, 1_002, &low, "block", None, Some(&page), Some(1.0)),
        ];
        apply_batch(&connection, &operations, Origin::Remote)
            .await
            .expect("equal-position siblings");
        let page_id = db::get_node_by_uuid(&connection, page)
            .await
            .expect("query page")
            .expect("page")
            .id;
        let children = db::list_block_children(&connection, page_id)
            .await
            .expect("children");
        assert_eq!(
            children
                .into_iter()
                .map(|node| node.uuid)
                .collect::<Vec<_>>(),
            vec![low, high]
        );
    }

    #[tokio::test]
    async fn attachment_add_remove_is_lww_and_order_independent() {
        let (_left_dir, left) = database().await;
        let (_right_dir, right) = database().await;
        let page = uuid::Uuid::new_v4();
        let create_page = create_node_op(1, 1_000, &page, "page", Some("Root"), None, None);
        for connection in [&left, &right] {
            apply(connection, &create_page, Origin::Remote)
                .await
                .expect("create page");
        }
        let add = remote_op(
            1,
            2_000,
            0,
            OpKind::AttachmentAdd(AttachmentAdd {
                node_uuid: page,
                blob_hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .into(),
                filename: "file.txt".into(),
                mime: "text/plain".into(),
                size: 4,
            }),
        );
        let remove = remote_op(
            2,
            2_001,
            0,
            OpKind::AttachmentRemove(AttachmentRemove {
                node_uuid: page,
                blob_hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .into(),
            }),
        );
        apply_batch(&left, &[add.clone(), remove.clone()], Origin::Remote)
            .await
            .expect("left attachment order");
        apply_batch(&right, &[remove, add], Origin::Remote)
            .await
            .expect("right attachment order");
        for connection in [&left, &right] {
            let state: (i64, bool) = connection
                .call(|database| {
                    database.query_row(
                        "SELECT
                           (SELECT COUNT(*) FROM nodes WHERE kind = 'attachment'),
                           (SELECT present FROM attachment_lww
                             WHERE blob_hash = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa')",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                })
                .await
                .expect("attachment state");
            assert_eq!(state, (0, false));
        }
    }
}

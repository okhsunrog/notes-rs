//! Versioned, UUID-addressed source operations and the single apply boundary.

use crate::{Connection, db};
use anyhow::{Context, Result, bail};
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
    pub op_id: String,
    pub device_id: String,
    pub hlc: String,
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
    pub uuid: String,
    pub node_kind: String,
    pub title: Option<String>,
    pub content: String,
    pub content_json: Option<String>,
    pub parent_uuid: Option<String>,
    pub position: Option<f64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeSetContent {
    pub uuid: String,
    pub content: String,
    pub content_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeSetTitle {
    pub uuid: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeMove {
    pub uuid: String,
    pub parent_uuid: Option<String>,
    pub position: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeDelete {
    pub uuid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EdgeAdd {
    pub src_uuid: String,
    pub dst_uuid: String,
    pub edge_kind: String,
    pub weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EdgeRemove {
    pub src_uuid: String,
    pub dst_uuid: String,
    pub edge_kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttachmentAdd {
    pub node_uuid: String,
    pub blob_hash: String,
    pub filename: String,
    pub mime: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttachmentRemove {
    pub node_uuid: String,
    pub blob_hash: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyOutcome {
    pub applied: bool,
    pub affected_uuids: Vec<String>,
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
            .and_then(|value| parse_hlc(&value).map(|(wall, counter, _)| (wall, counter)));
        let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
        let (wall, mut counter) = previous.map_or((now, 0), |(wall, counter)| {
            if wall >= now {
                (wall, counter.saturating_add(1))
            } else {
                (now, 0)
            }
        });
        let mut operations = Vec::with_capacity(kinds.len());
        for kind in kinds {
            let hlc = format_hlc(wall, counter, &device_id);
            operations.push(Op {
                op_id: uuid::Uuid::new_v4().to_string(),
                device_id: device_id.clone(),
                hlc,
                format_version: FORMAT_VERSION,
                kind,
            });
            counter = counter.saturating_add(1);
        }
        if let Some(last) = operations.last() {
            transaction.execute(
                "INSERT INTO sync_meta(key, value) VALUES ('last_hlc', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [&last.hlc],
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

fn validate(operation: &Op) -> Result<()> {
    if operation.format_version != FORMAT_VERSION {
        bail!(
            "unsupported operation format version {}",
            operation.format_version
        );
    }
    uuid::Uuid::parse_str(&operation.op_id).context("invalid operation ID")?;
    uuid::Uuid::parse_str(&operation.device_id).context("invalid device ID")?;
    let Some((_, _, suffix)) = parse_hlc(&operation.hlc) else {
        bail!("invalid hybrid logical clock");
    };
    if suffix != operation.device_id.replace('-', "").to_lowercase() {
        bail!("hybrid logical clock device suffix does not match device_id");
    }
    match &operation.kind {
        OpKind::NodeCreate(payload) => {
            if payload.uuid.trim().is_empty() || payload.node_kind.trim().is_empty() {
                bail!("node_create requires uuid and node_kind");
            }
        }
        OpKind::NodeSetContent(payload) => require_uuid(&payload.uuid)?,
        OpKind::NodeSetTitle(payload) => require_uuid(&payload.uuid)?,
        OpKind::NodeMove(payload) => {
            require_uuid(&payload.uuid)?;
            if !payload.position.is_finite() {
                bail!("node position must be finite");
            }
        }
        OpKind::NodeDelete(payload) => require_uuid(&payload.uuid)?,
        OpKind::EdgeAdd(payload) => {
            require_edge(&payload.src_uuid, &payload.dst_uuid, &payload.edge_kind)?;
            if !payload.weight.is_finite() {
                bail!("edge weight must be finite");
            }
        }
        OpKind::EdgeRemove(payload) => {
            require_edge(&payload.src_uuid, &payload.dst_uuid, &payload.edge_kind)?;
        }
        OpKind::AttachmentAdd(payload) => {
            require_uuid(&payload.node_uuid)?;
            if payload.blob_hash.trim().is_empty() || payload.filename.trim().is_empty() {
                bail!("attachment_add requires blob_hash and filename");
            }
        }
        OpKind::AttachmentRemove(payload) => {
            require_uuid(&payload.node_uuid)?;
            if payload.blob_hash.trim().is_empty() {
                bail!("attachment_remove requires blob_hash");
            }
        }
    }
    Ok(())
}

fn require_uuid(uuid: &str) -> Result<()> {
    if uuid.trim().is_empty() {
        bail!("operation requires a node UUID");
    }
    Ok(())
}

fn require_edge(src: &str, dst: &str, kind: &str) -> Result<()> {
    require_uuid(src)?;
    require_uuid(dst)?;
    if src == dst || kind.trim().is_empty() {
        bail!("edge requires distinct nodes and a kind");
    }
    Ok(())
}

fn apply_one(
    transaction: &rusqlite::Transaction<'_>,
    operation: &Op,
    _origin: Origin,
) -> rusqlite::Result<Vec<String>> {
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
                let (kind, id): (String, i64) = transaction.query_row(
                    "SELECT kind, id FROM nodes WHERE uuid = ?1",
                    [&payload.uuid],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                if kind == "block" {
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
            Ok(vec![payload.uuid.clone()])
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
                return Ok(vec![payload.uuid.clone()]);
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
            Ok(vec![payload.uuid.clone()])
        }
        OpKind::NodeMove(payload) => {
            let parent_id = resolve_parent(transaction, payload.parent_uuid.as_deref())?;
            transaction.execute(
                "UPDATE nodes SET parent_id = ?2, position = ?3,
                    structure_hlc = ?4, updated_at = ?5
                 WHERE uuid = ?1 AND kind = 'block'
                   AND (structure_hlc IS NULL OR structure_hlc < ?4)
                   AND NOT EXISTS (SELECT 1 FROM tombstones WHERE uuid = ?1)",
                rusqlite::params![
                    payload.uuid,
                    parent_id,
                    payload.position,
                    operation.hlc,
                    timestamp
                ],
            )?;
            Ok(vec![payload.uuid.clone()])
        }
        OpKind::NodeDelete(payload) => {
            let previous = transaction
                .query_row(
                    "SELECT deleted_hlc FROM tombstones WHERE uuid = ?1",
                    [&payload.uuid],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if previous
                .as_deref()
                .is_none_or(|clock| clock < operation.hlc.as_str())
            {
                transaction.execute("DELETE FROM nodes WHERE uuid = ?1", [&payload.uuid])?;
                transaction.execute(
                    "INSERT INTO tombstones(uuid, deleted_hlc) VALUES (?1, ?2)
                     ON CONFLICT(uuid) DO UPDATE SET deleted_hlc = excluded.deleted_hlc",
                    rusqlite::params![payload.uuid, operation.hlc],
                )?;
            }
            Ok(vec![payload.uuid.clone()])
        }
        OpKind::EdgeAdd(payload) => {
            transaction.execute(
                "INSERT INTO edges(src, dst, kind, weight, created_at)
                 SELECT src.id, dst.id, ?3, ?4, ?5
                   FROM nodes src, nodes dst
                  WHERE src.uuid = ?1 AND dst.uuid = ?2
                 ON CONFLICT(src, dst, kind) DO UPDATE SET weight = excluded.weight",
                rusqlite::params![
                    payload.src_uuid,
                    payload.dst_uuid,
                    payload.edge_kind,
                    payload.weight,
                    timestamp,
                ],
            )?;
            Ok(vec![payload.src_uuid.clone(), payload.dst_uuid.clone()])
        }
        OpKind::EdgeRemove(payload) => {
            transaction.execute(
                "DELETE FROM edges WHERE kind = ?3
                   AND src = (SELECT id FROM nodes WHERE uuid = ?1)
                   AND dst = (SELECT id FROM nodes WHERE uuid = ?2)",
                rusqlite::params![payload.src_uuid, payload.dst_uuid, payload.edge_kind],
            )?;
            Ok(vec![payload.src_uuid.clone(), payload.dst_uuid.clone()])
        }
        OpKind::AttachmentAdd(payload) => {
            apply_attachment_add(transaction, operation, payload, timestamp)
        }
        OpKind::AttachmentRemove(payload) => {
            transaction.execute(
                "DELETE FROM nodes WHERE kind = 'attachment'
                   AND json_extract(content_json, '$.blobHash') = ?2
                   AND id IN (
                     SELECT e.dst FROM edges e JOIN nodes parent ON parent.id = e.src
                      WHERE parent.uuid = ?1 AND e.kind = 'attachment'
                   )",
                rusqlite::params![payload.node_uuid, payload.blob_hash],
            )?;
            Ok(vec![payload.node_uuid.clone()])
        }
    }
}

fn apply_node_create(
    transaction: &rusqlite::Transaction<'_>,
    operation: &Op,
    payload: &NodeCreate,
) -> rusqlite::Result<Vec<String>> {
    let tombstone = transaction
        .query_row(
            "SELECT deleted_hlc FROM tombstones WHERE uuid = ?1",
            [&payload.uuid],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(deleted_hlc) = tombstone {
        if operation.hlc <= deleted_hlc {
            return Ok(vec![payload.uuid.clone()]);
        }
        // A newer explicit node_create is the inverse operation used by Undo.
        // Ordinary edits still cannot resurrect a tombstoned node.
        transaction.execute("DELETE FROM tombstones WHERE uuid = ?1", [&payload.uuid])?;
    }
    let parent_id = resolve_parent(transaction, payload.parent_uuid.as_deref())?;
    let body = crate::stem::stem(&format!(
        "{}\n{}",
        payload.title.as_deref().unwrap_or(""),
        payload.content
    ));
    if payload.node_kind == "page"
        && let Some(title) = payload.title.as_deref()
    {
        let stub_uuid = db::page_uuid(title);
        transaction.execute(
            "UPDATE nodes SET uuid = ?2
             WHERE uuid = ?1 AND kind = 'page' AND lower(title) = lower(?3)",
            rusqlite::params![stub_uuid, payload.uuid, title],
        )?;
    }
    transaction.execute(
        "INSERT OR IGNORE INTO nodes
           (uuid, kind, title, content, content_json, body_stemmed, parent_id, position,
            content_hlc, title_hlc, structure_hlc, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?9, ?10, ?10)",
        rusqlite::params![
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
    if payload.node_kind == "block" {
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
    Ok(vec![payload.uuid.clone()])
}

fn apply_attachment_add(
    transaction: &rusqlite::Transaction<'_>,
    operation: &Op,
    payload: &AttachmentAdd,
    timestamp: i64,
) -> rusqlite::Result<Vec<String>> {
    let parent_id = transaction.query_row(
        "SELECT id FROM nodes WHERE uuid = ?1 AND kind IN ('page', 'block')",
        [&payload.node_uuid],
        |row| row.get::<_, i64>(0),
    )?;
    let attachment_uuid = attachment_uuid(&payload.node_uuid, &payload.blob_hash);
    let relative_path = format!("attachments/{}/{}", payload.blob_hash, payload.filename);
    let metadata = serde_json::json!({
        "blobHash": payload.blob_hash,
        "mime": payload.mime,
        "size": payload.size,
    })
    .to_string();
    transaction.execute(
        "INSERT OR IGNORE INTO nodes
           (uuid, kind, title, content, content_json, body_stemmed,
            content_hlc, title_hlc, structure_hlc, created_at, updated_at)
         VALUES (?1, 'attachment', ?2, ?3, ?4, ?5, ?6, ?6, ?6, ?7, ?7)",
        rusqlite::params![
            attachment_uuid,
            payload.filename,
            relative_path,
            metadata,
            crate::stem::stem(&payload.filename),
            operation.hlc,
            timestamp,
        ],
    )?;
    transaction.execute(
        "INSERT OR IGNORE INTO edges(src, dst, kind, weight, created_at)
         SELECT ?1, id, 'attachment', 1.0, ?3 FROM nodes WHERE uuid = ?2",
        rusqlite::params![parent_id, attachment_uuid, timestamp],
    )?;
    Ok(vec![payload.node_uuid.clone(), attachment_uuid])
}

fn resolve_parent(
    transaction: &rusqlite::Transaction<'_>,
    parent_uuid: Option<&str>,
) -> rusqlite::Result<Option<i64>> {
    parent_uuid
        .map(|uuid| {
            transaction
                .query_row(
                    "SELECT id FROM nodes WHERE uuid = ?1 AND kind IN ('page', 'block')",
                    [uuid],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
        })
        .transpose()
        .map(Option::flatten)
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

fn meta_or_insert_device_id(transaction: &rusqlite::Transaction<'_>) -> rusqlite::Result<String> {
    if let Some(device_id) = transaction
        .query_row(
            "SELECT value FROM sync_meta WHERE key = 'device_id'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        return Ok(device_id);
    }
    let device_id = uuid::Uuid::new_v4().to_string();
    transaction.execute(
        "INSERT INTO sync_meta(key, value) VALUES ('device_id', ?1)",
        [&device_id],
    )?;
    Ok(device_id)
}

fn observe_hlc(transaction: &rusqlite::Transaction<'_>, incoming: &str) -> rusqlite::Result<()> {
    let current = transaction
        .query_row(
            "SELECT value FROM sync_meta WHERE key = 'last_hlc'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if current.as_deref().is_none_or(|value| value < incoming) {
        transaction.execute(
            "INSERT INTO sync_meta(key, value) VALUES ('last_hlc', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [incoming],
        )?;
    }
    Ok(())
}

fn format_hlc(wall_ms: u64, counter: u32, device_id: &str) -> String {
    format!(
        "{wall_ms:016x}-{counter:08x}-{}",
        device_id.replace('-', "").to_lowercase()
    )
}

fn parse_hlc(value: &str) -> Option<(u64, u32, &str)> {
    let mut parts = value.splitn(3, '-');
    let wall = u64::from_str_radix(parts.next()?, 16).ok()?;
    let counter = u32::from_str_radix(parts.next()?, 16).ok()?;
    let device = parts.next()?;
    if device.len() != 32 || !device.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some((wall, counter, device))
}

fn operation_timestamp(operation: &Op) -> i64 {
    parse_hlc(&operation.hlc)
        .map(|(wall_ms, _, _)| (wall_ms / 1_000).min(i64::MAX as u64) as i64)
        .unwrap_or_default()
}

fn parse_refs(content: &str) -> (Vec<String>, Vec<String>) {
    (
        delimited(content, "[[", "]]"),
        delimited(content, "((", "))"),
    )
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

fn attachment_uuid(node_uuid: &str, blob_hash: &str) -> String {
    uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_OID,
        format!("notes-rs:attachment:{node_uuid}:{blob_hash}").as_bytes(),
    )
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn database() -> (tempfile::TempDir, Connection) {
        let directory = tempfile::tempdir().expect("temporary directory");
        let connection = db::open(directory.path().join("notes.db"), "test", 8)
            .await
            .expect("open test database");
        (directory, connection)
    }

    #[test]
    fn envelope_round_trips_with_flat_kind_and_payload() {
        let operation = Op {
            op_id: uuid::Uuid::new_v4().to_string(),
            device_id: uuid::Uuid::new_v4().to_string(),
            hlc: format_hlc(42, 3, &uuid::Uuid::new_v4().to_string()),
            format_version: FORMAT_VERSION,
            kind: OpKind::NodeDelete(NodeDelete {
                uuid: "node".into(),
            }),
        };
        let json = serde_json::to_value(&operation).expect("serialize operation");
        assert_eq!(json["kind"], "node_delete");
        assert_eq!(json["payload"]["uuid"], "node");
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
        let node_uuid = uuid::Uuid::new_v4().to_string();
        let operation = local_ops(
            &connection,
            vec![OpKind::NodeCreate(NodeCreate {
                uuid: node_uuid,
                node_kind: "page".into(),
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
        let node_uuid = uuid::Uuid::new_v4().to_string();
        let create = local_ops(
            &connection,
            vec![OpKind::NodeCreate(NodeCreate {
                uuid: node_uuid.clone(),
                node_kind: "page".into(),
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
                    uuid: node_uuid.clone(),
                    content: "older".into(),
                    content_json: None,
                }),
                OpKind::NodeSetContent(NodeSetContent {
                    uuid: node_uuid.clone(),
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
        let node_uuid = uuid::Uuid::new_v4().to_string();
        let mut operations = local_ops(
            &connection,
            vec![
                OpKind::NodeCreate(NodeCreate {
                    uuid: node_uuid.clone(),
                    node_kind: "page".into(),
                    title: Some("Deleted".into()),
                    content: String::new(),
                    content_json: None,
                    parent_uuid: None,
                    position: None,
                    created_at: 1,
                }),
                OpKind::NodeDelete(NodeDelete {
                    uuid: node_uuid.clone(),
                }),
                OpKind::NodeSetContent(NodeSetContent {
                    uuid: node_uuid.clone(),
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
        let block_uuid = uuid::Uuid::new_v4().to_string();
        let block = local_ops(
            &connection,
            vec![OpKind::NodeCreate(NodeCreate {
                uuid: block_uuid.clone(),
                node_kind: "block".into(),
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

        let canonical_uuid = uuid::Uuid::new_v4().to_string();
        let page = local_ops(
            &connection,
            vec![OpKind::NodeCreate(NodeCreate {
                uuid: canonical_uuid.clone(),
                node_kind: "page".into(),
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
        let adopted = db::get_page_by_title(&connection, "Roadmap".into())
            .await
            .expect("query adopted page")
            .expect("adopted page");
        assert_ne!(canonical_uuid, stub_uuid);
        assert_eq!(adopted.uuid, canonical_uuid);
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
}

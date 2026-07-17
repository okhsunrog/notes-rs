use super::*;
use std::collections::{HashMap, HashSet};

type HistoryRow = (i64, uuid::Uuid, String, String, String);

pub(crate) async fn capture_inverse_kinds(
    conn: &Connection,
    forward: &[OpKind],
) -> Result<Vec<OpKind>> {
    let forward = forward.to_vec();
    conn.call(move |database| {
        let mut inverse = Vec::new();
        for kind in &forward {
            capture_inverse(database, kind, &mut inverse)?;
        }
        inverse.reverse();
        Ok(order_restored_nodes(inverse))
    })
    .await
}

fn capture_inverse(
    database: &rusqlite::Connection,
    kind: &OpKind,
    inverse: &mut Vec<OpKind>,
) -> rusqlite::Result<()> {
    match kind {
        OpKind::NodeCreate(payload) => {
            inverse.push(OpKind::NodeDelete(NodeDelete { uuid: payload.uuid }))
        }
        OpKind::NodeSetContent(payload) => {
            if let Some((content, content_json)) = database
                .query_row(
                    "SELECT content, content_json FROM nodes WHERE uuid = ?1",
                    [payload.uuid],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?
            {
                inverse.push(OpKind::NodeSetContent(NodeSetContent {
                    uuid: payload.uuid,
                    content,
                    content_json,
                }));
            }
        }
        OpKind::NodeSetTitle(payload) => {
            if let Some(title) = database
                .query_row(
                    "SELECT title FROM nodes WHERE uuid = ?1",
                    [payload.uuid],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
            {
                inverse.push(OpKind::NodeSetTitle(NodeSetTitle {
                    uuid: payload.uuid,
                    title,
                }));
            }
        }
        OpKind::NodeMove(payload) => {
            if let Some((parent_uuid, position)) = database
                .query_row(
                    "SELECT parent.uuid, COALESCE(node.position, 0.0)
                       FROM nodes node LEFT JOIN nodes parent ON parent.id = node.parent_id
                      WHERE node.uuid = ?1",
                    [payload.uuid],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?
            {
                inverse.push(OpKind::NodeMove(NodeMove {
                    uuid: payload.uuid,
                    parent_uuid,
                    position,
                }));
            }
        }
        OpKind::NodeDelete(payload) => capture_deleted_node(database, payload.uuid, inverse)?,
        OpKind::EdgeAdd(payload) => {
            let previous = edge_intent(database, payload)?;
            inverse.push(previous.map_or_else(
                || {
                    OpKind::EdgeRemove(EdgeRemove {
                        src_uuid: payload.src_uuid,
                        dst_uuid: payload.dst_uuid,
                        edge_kind: payload.edge_kind.clone(),
                    })
                },
                OpKind::EdgeAdd,
            ));
        }
        OpKind::EdgeRemove(payload) => {
            let probe = EdgeAdd {
                src_uuid: payload.src_uuid,
                dst_uuid: payload.dst_uuid,
                edge_kind: payload.edge_kind.clone(),
                weight: 0.0,
            };
            if let Some(previous) = edge_intent(database, &probe)? {
                inverse.push(OpKind::EdgeAdd(previous));
            }
        }
        OpKind::AttachmentAdd(payload) => {
            if let Some(previous) =
                attachment_intent(database, &payload.node_uuid, &payload.blob_hash)?
            {
                inverse.push(OpKind::AttachmentAdd(previous));
            } else {
                inverse.push(OpKind::AttachmentRemove(AttachmentRemove {
                    node_uuid: payload.node_uuid,
                    blob_hash: payload.blob_hash.clone(),
                }));
            }
        }
        OpKind::AttachmentRemove(payload) => {
            if let Some(previous) =
                attachment_intent(database, &payload.node_uuid, &payload.blob_hash)?
            {
                inverse.push(OpKind::AttachmentAdd(previous));
            }
        }
    }
    Ok(())
}

fn capture_deleted_node(
    database: &rusqlite::Connection,
    uuid: uuid::Uuid,
    inverse: &mut Vec<OpKind>,
) -> rusqlite::Result<()> {
    let node = database
        .query_row(
            "SELECT node.kind, node.title, node.content, node.content_json,
                    parent.uuid, node.position, node.created_at
               FROM nodes node LEFT JOIN nodes parent ON parent.id = node.parent_id
              WHERE node.uuid = ?1",
            [uuid],
            |row| {
                Ok((
                    row.get::<_, NodeKind>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<uuid::Uuid>>(4)?,
                    row.get::<_, Option<f64>>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            },
        )
        .optional()?;
    let Some((kind, title, content, content_json, parent_uuid, position, created_at)) = node else {
        return Ok(());
    };
    if kind == NodeKind::Attachment
        && let Some(metadata) = content_json
            .as_deref()
            .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
        && let (Some(node_uuid), Some(blob_hash)) = (
            attachment_parent(database, uuid)?,
            metadata.get("blobHash").and_then(|value| value.as_str()),
        )
    {
        inverse.push(OpKind::AttachmentAdd(AttachmentAdd {
            node_uuid,
            blob_hash: blob_hash.to_owned(),
            filename: title.unwrap_or_else(|| "attachment".into()),
            mime: metadata
                .get("mime")
                .and_then(|value| value.as_str())
                .unwrap_or("application/octet-stream")
                .to_owned(),
            size: metadata
                .get("size")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
        }));
        return Ok(());
    }
    inverse.push(OpKind::NodeCreate(NodeCreate {
        uuid,
        node_kind: kind,
        title,
        content,
        content_json,
        parent_uuid,
        position,
        created_at,
    }));
    Ok(())
}

fn edge_intent(
    database: &rusqlite::Connection,
    edge: &EdgeAdd,
) -> rusqlite::Result<Option<EdgeAdd>> {
    database
        .query_row(
            "SELECT weight FROM edge_lww
              WHERE src_uuid = ?1 AND dst_uuid = ?2 AND kind = ?3 AND present = 1",
            rusqlite::params![edge.src_uuid, edge.dst_uuid, edge.edge_kind],
            |row| {
                Ok(EdgeAdd {
                    src_uuid: edge.src_uuid,
                    dst_uuid: edge.dst_uuid,
                    edge_kind: edge.edge_kind.clone(),
                    weight: row.get(0)?,
                })
            },
        )
        .optional()
}

fn attachment_intent(
    database: &rusqlite::Connection,
    node_uuid: &uuid::Uuid,
    blob_hash: &str,
) -> rusqlite::Result<Option<AttachmentAdd>> {
    database
        .query_row(
            "SELECT filename, mime, size FROM attachment_lww
              WHERE node_uuid = ?1 AND blob_hash = ?2 AND present = 1",
            rusqlite::params![node_uuid, blob_hash],
            |row| {
                Ok(AttachmentAdd {
                    node_uuid: *node_uuid,
                    blob_hash: blob_hash.to_owned(),
                    filename: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    mime: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    size: row.get::<_, Option<i64>>(2)?.unwrap_or_default().max(0) as u64,
                })
            },
        )
        .optional()
}

fn attachment_parent(
    database: &rusqlite::Connection,
    attachment_uuid: uuid::Uuid,
) -> rusqlite::Result<Option<uuid::Uuid>> {
    database
        .query_row(
            "SELECT parent.uuid FROM nodes attachment
               JOIN edges edge ON edge.dst = attachment.id AND edge.kind = 'attachment'
               JOIN nodes parent ON parent.id = edge.src
              WHERE attachment.uuid = ?1 LIMIT 1",
            [attachment_uuid],
            |row| row.get(0),
        )
        .optional()
}

fn order_restored_nodes(kinds: Vec<OpKind>) -> Vec<OpKind> {
    let mut creates = HashMap::new();
    let mut remaining = Vec::new();
    for kind in kinds {
        match kind {
            OpKind::NodeCreate(node) => {
                creates.insert(node.uuid, node);
            }
            other => remaining.push(other),
        }
    }
    if creates.is_empty() {
        return remaining;
    }
    let mut ordered = Vec::with_capacity(creates.len() + remaining.len());
    let mut restored = HashSet::new();
    while !creates.is_empty() {
        let ready = creates
            .iter()
            .filter_map(|(uuid, node)| {
                node.parent_uuid
                    .is_none_or(|parent| {
                        restored.contains(&parent) || !creates.contains_key(&parent)
                    })
                    .then_some(*uuid)
            })
            .collect::<Vec<_>>();
        if ready.is_empty() {
            ordered.extend(creates.drain().map(|(_, node)| OpKind::NodeCreate(node)));
            break;
        }
        for uuid in ready {
            let node = creates
                .remove(&uuid)
                .expect("ready restore node must exist");
            restored.insert(uuid);
            ordered.push(OpKind::NodeCreate(node));
        }
    }
    ordered.extend(remaining);
    ordered
}

pub(crate) async fn record_action(
    conn: &Connection,
    action: &str,
    forward: Vec<OpKind>,
    inverse: Vec<OpKind>,
) -> Result<()> {
    if inverse.is_empty() {
        return Ok(());
    }
    let action_uuid = uuid::Uuid::now_v7();
    let action = action.to_owned();
    let forward_json = serde_json::to_string(&forward)?;
    let inverse_json = serde_json::to_string(&inverse)?;
    conn.call(move |database| -> rusqlite::Result<()> {
        let transaction = database.transaction()?;
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
        transaction.commit()?;
        Ok(())
    })
    .await
}

pub async fn history_status(conn: &Connection) -> Result<(i64, i64)> {
    conn.call(|database| {
        database.query_row(
            "SELECT
               (SELECT COUNT(*) FROM history_undo),
               (SELECT COUNT(*) FROM history_redo)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
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
    let source = if undo { "history_undo" } else { "history_redo" };
    let target = if undo { "history_redo" } else { "history_undo" };
    let entry = conn
        .call(move |database| -> rusqlite::Result<Option<HistoryRow>> {
            database
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
                .optional()
        })
        .await?;
    let Some((entry_id, action_uuid, action, forward_json, inverse_json)) = entry else {
        return Ok(false);
    };
    let kinds: Vec<OpKind> =
        serde_json::from_str(if undo { &inverse_json } else { &forward_json })?;
    apply_local(conn, kinds).await?;
    conn.call(move |database| -> rusqlite::Result<()> {
        let transaction = database.transaction()?;
        transaction.execute(&format!("DELETE FROM {source} WHERE id = ?1"), [entry_id])?;
        transaction.execute(
            &format!(
                "INSERT INTO {target}(action_uuid, action, forward_json, inverse_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, unixepoch())"
            ),
            rusqlite::params![action_uuid, action, forward_json, inverse_json],
        )?;
        transaction.commit()?;
        Ok(())
    })
    .await?;
    Ok(true)
}

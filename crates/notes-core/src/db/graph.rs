use super::*;

pub async fn link_nodes(
    conn: &Connection,
    src: i64,
    dst: i64,
    kind: String,
    weight: f64,
) -> Result<()> {
    let (src_uuid, dst_uuid) = require_node_uuids(conn, src, dst).await?;
    apply_local(
        conn,
        vec![OpKind::EdgeAdd(EdgeAdd {
            src_uuid,
            dst_uuid,
            edge_kind: kind,
            weight,
        })],
    )
    .await?;
    Ok(())
}

/// Replace all outgoing reference edges for `block_id` in one transaction.
///
/// Strategy: delete every edge `(src=block_id, kind='refs')`, then re-insert
/// one edge per wikilink target (eagerly materializing missing pages) and one
/// per block-ref target (silently skipping broken `((uuid))` refs).
///
/// This is the "re-emit and cleanup on every save" approach from the plan —
/// inefficient but correct. Returns the number of broken block-ref UUIDs so
/// the frontend can surface them later if desired.
pub(crate) fn replace_block_refs_tx_at(
    tx: &rusqlite::Transaction<'_>,
    block_id: i64,
    wikilink_titles: &[String],
    block_uuids: &[String],
    now: i64,
) -> rusqlite::Result<u32> {
    tx.execute(
        "DELETE FROM edges WHERE src = ?1 AND kind = 'refs'",
        [block_id],
    )?;
    for raw_title in wikilink_titles {
        let title = raw_title.trim();
        if title.is_empty() {
            continue;
        }
        let existing = tx
            .query_row(
                "SELECT id FROM nodes
                 WHERE kind = 'page' AND lower(title) = lower(?1)
                 LIMIT 1",
                [title],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let page_id = match existing {
            Some(id) => id,
            None => {
                let uuid = page_uuid(title);
                let id = stable_node_id(tx, &uuid)?;
                tx.execute(
                    "INSERT INTO nodes (id, uuid, kind, title, content, content_json,
                                        body_stemmed, parent_id, position,
                                        created_at, updated_at)
                     VALUES (?1, ?2, 'page', ?3, '', NULL, '', NULL, NULL, ?4, ?4)",
                    rusqlite::params![id, uuid, title, now],
                )?;
                id
            }
        };
        if page_id != block_id {
            tx.execute(
                "INSERT OR IGNORE INTO edges (src, dst, kind, weight, created_at)
                 VALUES (?1, ?2, 'refs', 1.0, ?3)",
                rusqlite::params![block_id, page_id, now],
            )?;
        }
    }

    let mut broken = 0;
    for raw_uuid in block_uuids {
        let uuid = raw_uuid.trim();
        if uuid.is_empty() {
            continue;
        }
        let Ok(uuid) = uuid::Uuid::parse_str(uuid) else {
            broken += 1;
            continue;
        };
        let target = tx
            .query_row("SELECT id FROM nodes WHERE uuid = ?1", [uuid], |row| {
                row.get::<_, i64>(0)
            })
            .optional()?;
        match target {
            Some(target_id) if target_id != block_id => {
                tx.execute(
                    "INSERT OR IGNORE INTO edges (src, dst, kind, weight, created_at)
                     VALUES (?1, ?2, 'refs', 1.0, ?3)",
                    rusqlite::params![block_id, target_id, now],
                )?;
            }
            Some(_) => {}
            None => broken += 1,
        }
    }
    Ok(broken)
}

pub(crate) fn page_uuid(title: &str) -> uuid::Uuid {
    uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_OID,
        format!("notes-rs:page:{}", title.trim().to_lowercase()).as_bytes(),
    )
}

pub(crate) fn stable_node_id(
    transaction: &rusqlite::Transaction<'_>,
    uuid: &uuid::Uuid,
) -> rusqlite::Result<i64> {
    let mut high = [0_u8; 8];
    let mut low = [0_u8; 8];
    high.copy_from_slice(&uuid.as_bytes()[..8]);
    low.copy_from_slice(&uuid.as_bytes()[8..]);
    // Tauri sends IDs through JavaScript, so keep them inside Number's exact
    // integer range while retaining 53 bits of UUID-derived entropy.
    let value = (u64::from_be_bytes(high) ^ u64::from_be_bytes(low)) & ((1_u64 << 53) - 1);
    let id = value.max(1) as i64;
    let occupied = transaction
        .query_row("SELECT uuid FROM nodes WHERE id = ?1", [id], |row| {
            row.get::<_, uuid::Uuid>(0)
        })
        .optional()?;
    if occupied.is_some_and(|occupied| occupied != *uuid) {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "UUID-derived node ID collision for {uuid} at local ID {id}"
        )));
    }
    Ok(id)
}

pub async fn neighbors(conn: &Connection, node_id: i64, depth: u32) -> Result<Vec<Node>> {
    let nodes = conn
        .call(move |c| -> rusqlite::Result<Vec<Node>> {
            let mut stmt = c.prepare(
                "WITH RECURSIVE reachable(id, d) AS (
                   SELECT ?1, 0
                   UNION
                   SELECT e.dst, r.d + 1 FROM edges e
                     JOIN reachable r ON e.src = r.id
                     WHERE r.d < ?2
                   UNION
                   SELECT e.src, r.d + 1 FROM edges e
                     JOIN reachable r ON e.dst = r.id
                     WHERE r.d < ?2
                 )
                 SELECT n.id, n.uuid, n.kind, n.title, n.content, n.content_json, n.parent_id, n.position, n.created_at, n.updated_at
                 FROM nodes n JOIN reachable r ON n.id = r.id
                 WHERE n.id != ?1",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![node_id, depth as i64], row_to_node)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(nodes)
}

/// Nodes that link *to* `node_id` via an outgoing edge. Unlike `neighbors`
/// (which is undirected), this is one-directional: it answers "who points
/// at me?". Optional `kind` filter narrows to e.g. only `refs` for explicit
/// wikilinks/block-refs, or `mentions` for entity links.
pub async fn find_backlinks(
    conn: &Connection,
    node_id: i64,
    kind: Option<String>,
) -> Result<Vec<Node>> {
    let rows = conn
        .call(move |c| -> rusqlite::Result<Vec<Node>> {
            let sql = if kind.is_some() {
                format!(
                    "SELECT {NODE_COLUMNS_N} FROM nodes n
                     JOIN edges e ON e.src = n.id
                     WHERE e.dst = ?1 AND e.kind = ?2
                     ORDER BY n.updated_at DESC"
                )
            } else {
                format!(
                    "SELECT {NODE_COLUMNS_N} FROM nodes n
                     JOIN edges e ON e.src = n.id
                     WHERE e.dst = ?1
                     ORDER BY n.updated_at DESC"
                )
            };
            let mut stmt = c.prepare(&sql)?;
            let rows = match kind {
                Some(k) => stmt
                    .query_map(rusqlite::params![node_id, k], row_to_node)?
                    .collect::<Result<Vec<_>, _>>()?,
                None => stmt
                    .query_map([node_id], row_to_node)?
                    .collect::<Result<Vec<_>, _>>()?,
            };
            Ok(rows)
        })
        .await?;
    Ok(rows)
}

/// Walk the parent chain from `node_id` up to the root. Returns root-first so
/// the LLM reads it like a breadcrumb. The starting node itself is included
/// as the last element.
pub async fn read_ancestors(conn: &Connection, node_id: i64) -> Result<Vec<Node>> {
    let rows = conn
        .call(move |c| -> rusqlite::Result<Vec<Node>> {
            let sql = format!(
                "WITH RECURSIVE up(id, parent_id, depth) AS (
                   SELECT id, parent_id, 0 FROM nodes WHERE id = ?1
                   UNION ALL
                   SELECT n.id, n.parent_id, u.depth + 1
                     FROM up u JOIN nodes n ON n.id = u.parent_id
                 )
                 SELECT {NODE_COLUMNS_N} FROM up u
                 JOIN nodes n ON n.id = u.id
                 ORDER BY u.depth DESC"
            );
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt
                .query_map([node_id], row_to_node)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(rows)
}

/// All descendants of `node_id` up to `depth` levels. Returned in pre-order
/// DFS via a lexicographic sort path so the result reads like an outline:
/// each block immediately followed by its own children. Excludes `node_id`
/// itself.
pub async fn read_subtree(conn: &Connection, node_id: i64, depth: u32) -> Result<Vec<Node>> {
    let rows = conn
        .call(move |c| -> rusqlite::Result<Vec<Node>> {
            // sort_path is zero-padded floats joined by '/', so lexicographic
            // sort matches the outline order. We assume non-negative
            // positions, which holds for blocks created via create_block /
            // move_block.
            let sql = format!(
                "WITH RECURSIVE down(id, sort_path, depth) AS (
                   SELECT id, '', 0 FROM nodes WHERE id = ?1
                   UNION ALL
                   SELECT n.id,
                          d.sort_path || '/' ||
                            printf('%015.6f', COALESCE(n.position, 0.0)),
                          d.depth + 1
                     FROM down d JOIN nodes n ON n.parent_id = d.id
                     WHERE d.depth < ?2
                 )
                 SELECT {NODE_COLUMNS_N} FROM down d
                 JOIN nodes n ON n.id = d.id
                 WHERE n.id != ?1
                 ORDER BY d.sort_path"
            );
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt
                .query_map(rusqlite::params![node_id, depth as i64], row_to_node)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(rows)
}

/// Nodes that reference the given tag/entity by its (case-insensitive) title.
/// Matches both `kind='tag'` and `kind='entity'` since the extractor emits
/// entities, while user-typed `#tags` would become tag rows when that path
/// is added.
pub async fn find_tagged(conn: &Connection, title: String) -> Result<Vec<Node>> {
    let rows = conn
        .call(move |c| -> rusqlite::Result<Vec<Node>> {
            let sql = format!(
                "SELECT {NODE_COLUMNS_N} FROM nodes n
                 WHERE EXISTS (
                   SELECT 1 FROM edges e
                   JOIN nodes t ON t.id = e.dst
                   WHERE e.src = n.id
                     AND e.kind IN ('mentions', 'refs')
                     AND t.kind IN ('tag', 'entity')
                     AND lower(t.title) = lower(?1)
                 )
                 ORDER BY n.updated_at DESC"
            );
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt
                .query_map([title], row_to_node)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(rows)
}

pub async fn graph_snapshot(conn: &Connection, focus_id: Option<i64>) -> Result<GraphSnapshot> {
    conn.call(move |database| -> rusqlite::Result<GraphSnapshot> {
        let nodes = if let Some(focus_id) = focus_id {
            let sql = format!(
                "SELECT DISTINCT {NODE_COLUMNS} FROM nodes n
                 WHERE n.id = ?1
                    OR n.id IN (SELECT src FROM edges WHERE dst = ?1)
                    OR n.id IN (SELECT dst FROM edges WHERE src = ?1)
                    OR n.parent_id = ?1
                    OR n.id = (SELECT parent_id FROM nodes WHERE id = ?1)
                 ORDER BY n.kind, n.title, n.id
                 LIMIT 100"
            );
            let mut statement = database.prepare(&sql)?;
            statement
                .query_map([focus_id], row_to_node)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            let sql = format!(
                "SELECT {NODE_COLUMNS} FROM nodes
                 WHERE kind IN ('page', 'entity')
                 ORDER BY updated_at DESC LIMIT 100"
            );
            let mut statement = database.prepare(&sql)?;
            statement
                .query_map([], row_to_node)?
                .collect::<Result<Vec<_>, _>>()?
        };
        let ids = nodes.iter().map(|node| node.id).collect::<Vec<_>>();
        let edges = if ids.is_empty() {
            Vec::new()
        } else {
            let placeholders = std::iter::repeat_n("?", ids.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT src, dst, kind, weight, created_at FROM edges
                 WHERE src IN ({placeholders}) AND dst IN ({placeholders})
                 ORDER BY created_at DESC LIMIT 250"
            );
            let parameters = ids.iter().chain(ids.iter());
            let mut statement = database.prepare(&sql)?;
            statement
                .query_map(rusqlite::params_from_iter(parameters), |row| {
                    Ok(Edge {
                        src: row.get(0)?,
                        dst: row.get(1)?,
                        kind: row.get(2)?,
                        weight: row.get(3)?,
                        created_at: row.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(GraphSnapshot { nodes, edges })
    })
    .await
}

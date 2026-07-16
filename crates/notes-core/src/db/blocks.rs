use super::*;

pub async fn create_block(
    conn: &Connection,
    parent_id: Option<i64>,
    position: Option<f64>,
    content: String,
    content_json: Option<String>,
) -> Result<Node> {
    let uuid = uuid::Uuid::now_v7();
    let now = chrono::Utc::now().timestamp();
    let (parent_uuid, position) = conn
        .call(
            move |database| -> rusqlite::Result<(Option<uuid::Uuid>, f64)> {
                let parent_uuid = parent_id
                    .map(|parent_id| {
                        database.query_row(
                            "SELECT uuid FROM nodes WHERE id = ?1 AND kind IN ('page', 'block')",
                            [parent_id],
                            |row| row.get::<_, uuid::Uuid>(0),
                        )
                    })
                    .transpose()?;
                let position = match position {
                    Some(position) => position,
                    None => match parent_id {
                        Some(parent_id) => database.query_row(
                            "SELECT COALESCE(MAX(position), 0.0) + 1024.0
                         FROM nodes WHERE parent_id = ?1",
                            [parent_id],
                            |row| row.get(0),
                        )?,
                        None => 1024.0,
                    },
                };
                Ok((parent_uuid, position))
            },
        )
        .await?;
    apply_local(
        conn,
        vec![OpKind::NodeCreate(NodeCreate {
            uuid,
            node_kind: NodeKind::Block,
            title: None,
            content,
            content_json,
            parent_uuid,
            position: Some(position),
            created_at: now,
        })],
    )
    .await?;
    get_node_by_uuid(conn, uuid)
        .await?
        .context("created block was not materialized")
}

/// Move a block under a new parent. If `new_position` is `None`, appends to
/// the end of the new parent's children (MAX(position) + 1.0). Returns the
/// updated node so the frontend can maintain its ordering without a re-fetch.
pub async fn move_block(
    conn: &Connection,
    id: i64,
    new_parent_id: Option<i64>,
    new_position: Option<f64>,
) -> Result<Node> {
    let (uuid, parent_uuid, position) = conn
        .call(
            move |database| -> rusqlite::Result<(uuid::Uuid, Option<uuid::Uuid>, f64)> {
                let (uuid, kind): (uuid::Uuid, NodeKind) = database.query_row(
                    "SELECT uuid, kind FROM nodes WHERE id = ?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                if kind != NodeKind::Block {
                    return Err(rusqlite::Error::InvalidParameterName(
                        "only block nodes can be moved".into(),
                    ));
                }
                if new_parent_id == Some(id) {
                    return Err(rusqlite::Error::InvalidParameterName(
                        "a block cannot be its own parent".into(),
                    ));
                }
                let parent_uuid = new_parent_id
                    .map(|parent_id| {
                        let creates_cycle: bool = database.query_row(
                            "WITH RECURSIVE descendants(id) AS (
                           SELECT id FROM nodes WHERE parent_id = ?1
                           UNION ALL
                           SELECT n.id FROM nodes n JOIN descendants d ON n.parent_id = d.id
                         )
                         SELECT EXISTS(SELECT 1 FROM descendants WHERE id = ?2)",
                            rusqlite::params![id, parent_id],
                            |row| row.get(0),
                        )?;
                        if creates_cycle {
                            return Err(rusqlite::Error::InvalidParameterName(
                                "a block cannot be moved under one of its descendants".into(),
                            ));
                        }
                        database.query_row(
                            "SELECT uuid FROM nodes WHERE id = ?1 AND kind IN ('page', 'block')",
                            [parent_id],
                            |row| row.get::<_, uuid::Uuid>(0),
                        )
                    })
                    .transpose()?;
                let position = match new_position {
                    Some(position) => position,
                    None => match new_parent_id {
                        Some(parent_id) => database.query_row(
                            "SELECT COALESCE(MAX(position), 0.0) + 1024.0
                         FROM nodes WHERE parent_id = ?1",
                            [parent_id],
                            |row| row.get(0),
                        )?,
                        None => 1024.0,
                    },
                };
                Ok((uuid, parent_uuid, position))
            },
        )
        .await?;
    apply_local(
        conn,
        vec![OpKind::NodeMove(NodeMove {
            uuid,
            parent_uuid,
            position,
        })],
    )
    .await?;
    get_node_by_uuid(conn, uuid)
        .await?
        .context("moved block disappeared")
}

pub async fn reorder_block(
    conn: &Connection,
    id: i64,
    direction: ReorderDirection,
) -> Result<Node> {
    let (uuid, parent_uuid, moves) = conn.call(move |database| -> rusqlite::Result<ReorderPlan> {
        let parent_id: Option<i64> = database.query_row(
            "SELECT parent_id FROM nodes WHERE id = ?1 AND kind = 'block'",
            [id],
            |row| row.get(0),
        )?;
        let mut siblings = {
            let mut statement = database
                .prepare("SELECT uuid, position FROM nodes WHERE parent_id IS ?1 ORDER BY position, uuid")?;
            statement
                .query_map([parent_id], |row| Ok((row.get::<_, uuid::Uuid>(0)?, row.get::<_, f64>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?
        };
        let uuid: uuid::Uuid = database.query_row("SELECT uuid FROM nodes WHERE id = ?1", [id], |row| row.get(0))?;
        let index = siblings
            .iter()
            .position(|(sibling_uuid, _)| *sibling_uuid == uuid)
            .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        let target = match direction {
            ReorderDirection::Up if index > 0 => Some(index - 1),
            ReorderDirection::Down if index + 1 < siblings.len() => Some(index + 1),
            ReorderDirection::Up | ReorderDirection::Down => None,
        };
        let mut moves = Vec::new();
        if let Some(target) = target {
            siblings.swap(index, target);
            for (new_index, (sibling_uuid, _)) in siblings.into_iter().enumerate() {
                moves.push((sibling_uuid, (new_index as f64 + 1.0) * 1024.0));
            }
        }
        let parent_uuid = parent_id.map(|parent_id| database.query_row("SELECT uuid FROM nodes WHERE id = ?1", [parent_id], |row| row.get::<_, uuid::Uuid>(0))).transpose()?;
        Ok((uuid, parent_uuid, moves))
    }).await?;
    let kinds = moves
        .into_iter()
        .map(|(uuid, position)| {
            OpKind::NodeMove(NodeMove {
                uuid,
                parent_uuid,
                position,
            })
        })
        .collect();
    apply_local(conn, kinds).await?;
    get_node_by_uuid(conn, uuid)
        .await?
        .context("reordered block disappeared")
}

/// Delete a block. Refuses (returns `false`) if the block has children, so
/// callers can show feedback instead of silently cascading. Returns `true` on
/// successful delete.
pub async fn delete_block(conn: &Connection, id: i64) -> Result<bool> {
    let uuid = conn
        .call(move |c| -> rusqlite::Result<Option<uuid::Uuid>> {
            let kids: i64 = c.query_row(
                "SELECT COUNT(*) FROM nodes WHERE parent_id = ?1",
                [id],
                |r| r.get(0),
            )?;
            if kids > 0 {
                return Ok(None);
            }
            c.query_row(
                "SELECT uuid FROM nodes WHERE id = ?1 AND kind = 'block'",
                [id],
                |row| row.get(0),
            )
            .optional()
        })
        .await?;
    let Some(uuid) = uuid else {
        return Ok(false);
    };
    apply_local(conn, vec![OpKind::NodeDelete(NodeDelete { uuid })]).await?;
    Ok(true)
}

/// Find a page by case-insensitive title without creating one. Used by hover
/// previews / autocomplete so we don't spawn stubs on every hover.
pub async fn get_page_by_title(conn: &Connection, title: String) -> Result<Option<Node>> {
    let trimmed = title.trim().to_string();
    if trimmed.is_empty() {
        return Ok(None);
    }
    conn.call(move |c| -> rusqlite::Result<Option<Node>> {
        let sql = format!(
            "SELECT {NODE_COLUMNS} FROM nodes
             WHERE kind = 'page' AND lower(title) = lower(?1)
             LIMIT 1"
        );
        let mut stmt = c.prepare(&sql)?;
        let mut rows = stmt.query([&trimmed])?;
        if let Some(r) = rows.next()? {
            Ok(Some(row_to_node(r)?))
        } else {
            Ok(None)
        }
    })
    .await
}

pub async fn get_node_by_uuid(conn: &Connection, uuid: uuid::Uuid) -> Result<Option<Node>> {
    conn.call(move |c| -> rusqlite::Result<Option<Node>> {
        let sql = format!("SELECT {NODE_COLUMNS} FROM nodes WHERE uuid = ?1 LIMIT 1");
        let mut stmt = c.prepare(&sql)?;
        let mut rows = stmt.query([uuid])?;
        if let Some(r) = rows.next()? {
            Ok(Some(row_to_node(r)?))
        } else {
            Ok(None)
        }
    })
    .await
}

/// Find a page by case-insensitive title or create one. Used to eagerly
/// materialize `[[Wikilink]]` targets so backlinks work the moment the link
/// is typed.
pub async fn get_or_create_page_by_title(conn: &Connection, title: String) -> Result<Node> {
    let trimmed = title.trim().to_string();
    if trimmed.is_empty() {
        anyhow::bail!("page title is empty");
    }
    let found = conn
        .call({
            let t = trimmed.clone();
            move |c| -> rusqlite::Result<Option<Node>> {
                let sql = format!(
                    "SELECT {NODE_COLUMNS} FROM nodes
                     WHERE kind = 'page' AND lower(title) = lower(?1)
                     LIMIT 1"
                );
                let mut stmt = c.prepare(&sql)?;
                let mut rows = stmt.query([&t])?;
                if let Some(r) = rows.next()? {
                    Ok(Some(row_to_node(r)?))
                } else {
                    Ok(None)
                }
            }
        })
        .await?;
    if let Some(n) = found {
        return Ok(n);
    }
    create_node(conn, NodeKind::Page, Some(trimmed), String::new(), None).await
}

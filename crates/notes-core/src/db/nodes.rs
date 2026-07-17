use super::*;

pub async fn create_node(
    conn: &Connection,
    kind: NodeKind,
    title: Option<String>,
    content: String,
    content_json: Option<String>,
) -> Result<Node> {
    if kind == NodeKind::Page
        && let Some(title) = title.as_deref()
        && let Some(existing) = get_page_by_title(conn, title.to_string()).await?
    {
        return Ok(existing);
    }
    let uuid = uuid::Uuid::now_v7();
    let now = chrono::Utc::now().timestamp();
    apply_local(
        conn,
        vec![OpKind::NodeCreate(NodeCreate {
            uuid,
            node_kind: kind,
            title,
            content,
            content_json,
            parent_uuid: None,
            position: None,
            created_at: now,
        })],
    )
    .await?;
    get_node_by_uuid(conn, uuid)
        .await?
        .context("created node was not materialized")
}

pub async fn update_node(
    conn: &Connection,
    id: i64,
    title: Option<String>,
    content: String,
    content_json: Option<String>,
) -> Result<()> {
    let current = get_node(conn, id)
        .await?
        .ok_or_else(|| crate::CoreError::not_found("node not found"))?;
    let mut kinds = Vec::with_capacity(2);
    if current.title != title {
        kinds.push(OpKind::NodeSetTitle(NodeSetTitle {
            uuid: current.uuid,
            title,
        }));
    }
    if current.content != content || current.content_json != content_json {
        kinds.push(OpKind::NodeSetContent(NodeSetContent {
            uuid: current.uuid,
            content,
            content_json,
        }));
    }
    apply_local(conn, kinds).await?;
    Ok(())
}

pub async fn rename_page(
    conn: &Connection,
    uuid: uuid::Uuid,
    title: Option<String>,
) -> Result<Node> {
    let page = get_node_by_uuid(conn, uuid)
        .await?
        .filter(|node| node.kind == NodeKind::Page)
        .ok_or_else(|| crate::CoreError::not_found("page not found"))?;
    if page.title != title {
        apply_local(
            conn,
            vec![OpKind::NodeSetTitle(NodeSetTitle { uuid, title })],
        )
        .await?;
    }
    get_node_by_uuid(conn, uuid)
        .await?
        .context("renamed page disappeared")
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CreatedNote {
    pub page: Node,
    pub initial_block: Node,
}

/// Create the page and its first editable block in one operation batch. An
/// untitled page uses NULL title identity, so concurrent note creation cannot
/// collide on the case-insensitive page-title index.
pub async fn create_note(conn: &Connection) -> Result<CreatedNote> {
    let page_uuid = uuid::Uuid::now_v7();
    let block_uuid = uuid::Uuid::now_v7();
    let now = chrono::Utc::now().timestamp();
    apply_local_action(
        conn,
        "create note",
        vec![
            OpKind::NodeCreate(NodeCreate {
                uuid: page_uuid,
                node_kind: NodeKind::Page,
                title: None,
                content: String::new(),
                content_json: None,
                parent_uuid: None,
                position: None,
                created_at: now,
            }),
            OpKind::NodeCreate(NodeCreate {
                uuid: block_uuid,
                node_kind: NodeKind::Block,
                title: None,
                content: String::new(),
                content_json: None,
                parent_uuid: Some(page_uuid),
                position: Some(1024.0),
                created_at: now,
            }),
        ],
    )
    .await?;
    let page = get_node_by_uuid(conn, page_uuid)
        .await?
        .context("created page was not materialized")?;
    let initial_block = get_node_by_uuid(conn, block_uuid)
        .await?
        .context("created initial block was not materialized")?;
    Ok(CreatedNote {
        page,
        initial_block,
    })
}

#[derive(Debug, Clone, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct BlockContent {
    pub content: String,
    pub block_uuids: Vec<String>,
}

pub async fn update_block_with_refs(
    conn: &Connection,
    id: i64,
    block: BlockContent,
) -> Result<(Node, u32, bool)> {
    let node = get_node(conn, id)
        .await?
        .filter(|node| node.kind == NodeKind::Block)
        .context("atomic block save requires a block node")?;
    let previous_refs = crate::operation::parse_refs(&node.content);
    let next_refs = crate::operation::parse_refs(&block.content);
    let graph_changed = previous_refs != next_refs;
    apply_local(
        conn,
        vec![OpKind::NodeSetContent(NodeSetContent {
            uuid: node.uuid,
            content: block.content,
            content_json: None,
        })],
    )
    .await?;
    let broken = count_missing_block_refs(conn, block.block_uuids).await?;
    let updated = get_node_by_uuid(conn, node.uuid)
        .await?
        .context("updated block disappeared")?;
    Ok((updated, broken, graph_changed))
}

pub async fn set_block_content(
    conn: &Connection,
    uuid: uuid::Uuid,
    block: BlockContent,
) -> Result<(Node, u32, bool)> {
    let id = get_node_by_uuid(conn, uuid)
        .await?
        .filter(|node| node.kind == NodeKind::Block)
        .ok_or_else(|| crate::CoreError::not_found("block was not found"))?
        .id;
    update_block_with_refs(conn, id, block).await
}

/// Split one block into ordered siblings as a single transaction. Sibling
/// positions are normalized to wide integer gaps, preventing fractional
/// indexing from converging after repeated inserts.
pub async fn split_block(
    conn: &Connection,
    id: i64,
    parts: Vec<BlockContent>,
) -> Result<Vec<Node>> {
    if parts.is_empty() {
        return Err(crate::CoreError::invalid("split requires at least one part").into());
    }
    let (source_uuid, parent_uuid, mut siblings) = conn
        .call_domain(
            move |database| -> crate::CoreResult<(uuid::Uuid, uuid::Uuid, Vec<uuid::Uuid>)> {
                let (source_uuid, parent_id, kind): (uuid::Uuid, i64, NodeKind) = database
                    .query_row(
                        "SELECT uuid, parent_id, kind FROM nodes WHERE id = ?1",
                        [id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )?;
                if kind != NodeKind::Block {
                    return Err(crate::CoreError::invalid("only a block can be split"));
                }
                let parent_uuid = database.query_row(
                    "SELECT uuid FROM nodes WHERE id = ?1",
                    [parent_id],
                    |row| row.get(0),
                )?;
                let siblings = {
                    let mut statement = database.prepare(
                        "SELECT uuid FROM nodes WHERE parent_id = ?1 ORDER BY position, uuid",
                    )?;
                    statement
                        .query_map([parent_id], |row| row.get::<_, uuid::Uuid>(0))?
                        .collect::<Result<Vec<_>, _>>()?
                };
                Ok((source_uuid, parent_uuid, siblings))
            },
        )
        .await?;
    let insertion_index = siblings
        .iter()
        .position(|uuid| uuid == &source_uuid)
        .context("split source is missing from its siblings")?;
    let now = chrono::Utc::now().timestamp();
    let mut result_uuids = vec![source_uuid];
    let mut kinds = vec![OpKind::NodeSetContent(NodeSetContent {
        uuid: source_uuid,
        content: parts[0].content.clone(),
        content_json: None,
    })];
    for part in parts.into_iter().skip(1) {
        let uuid = uuid::Uuid::now_v7();
        result_uuids.push(uuid);
        kinds.push(OpKind::NodeCreate(NodeCreate {
            uuid,
            node_kind: NodeKind::Block,
            title: None,
            content: part.content,
            content_json: None,
            parent_uuid: Some(parent_uuid),
            position: Some(0.0),
            created_at: now,
        }));
    }
    siblings.splice(
        insertion_index + 1..insertion_index + 1,
        result_uuids[1..].iter().cloned(),
    );
    kinds.extend(siblings.into_iter().enumerate().map(|(index, uuid)| {
        OpKind::NodeMove(NodeMove {
            uuid,
            parent_uuid: Some(parent_uuid),
            position: (index as f64 + 1.0) * 1024.0,
        })
    }));
    apply_local_action(conn, "split block", kinds).await?;
    let mut nodes = Vec::with_capacity(result_uuids.len());
    for uuid in result_uuids {
        nodes.push(
            get_node_by_uuid(conn, uuid)
                .await?
                .context("split result disappeared")?,
        );
    }
    Ok(nodes)
}

pub async fn get_node(conn: &Connection, id: i64) -> Result<Option<Node>> {
    let node = conn
        .call(move |c| -> rusqlite::Result<Option<Node>> {
            let sql = format!("SELECT {NODE_COLUMNS} FROM nodes WHERE id = ?1");
            let mut stmt = c.prepare(&sql)?;
            let mut rows = stmt.query([id])?;
            if let Some(r) = rows.next()? {
                Ok(Some(row_to_node(r)?))
            } else {
                Ok(None)
            }
        })
        .await?;
    Ok(node)
}

pub async fn node_uuids_for_ids(
    conn: &Connection,
    ids: impl IntoIterator<Item = i64>,
) -> Result<Vec<uuid::Uuid>> {
    let mut ids = ids.into_iter().collect::<Vec<_>>();
    ids.sort_unstable();
    ids.dedup();
    conn.call(move |database| {
        let mut uuids = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(uuid) = database
                .query_row("SELECT uuid FROM nodes WHERE id = ?1", [id], |row| {
                    row.get::<_, uuid::Uuid>(0)
                })
                .optional()?
            {
                uuids.push(uuid);
            }
        }
        Ok(uuids)
    })
    .await
}

pub async fn get_containing_page(conn: &Connection, id: i64) -> Result<Option<Node>> {
    conn.call(move |database| -> rusqlite::Result<Option<Node>> {
        let sql = format!(
            "WITH RECURSIVE ancestors(id, parent_id, kind) AS (
               SELECT id, parent_id, kind FROM nodes WHERE id = ?1
               UNION
               SELECT n.id, n.parent_id, n.kind
                 FROM nodes n JOIN ancestors a ON n.id = a.parent_id
             )
             SELECT {NODE_COLUMNS} FROM nodes
             WHERE id IN (SELECT id FROM ancestors WHERE kind = 'page')
             LIMIT 1"
        );
        let mut statement = database.prepare(&sql)?;
        let mut rows = statement.query([id])?;
        rows.next()?.map(row_to_node).transpose()
    })
    .await
}

pub async fn list_entities(conn: &Connection, limit: u32) -> Result<Vec<Node>> {
    let rows = conn
        .call(move |c| -> rusqlite::Result<Vec<Node>> {
            let mut stmt = c.prepare(
                "SELECT n.id, n.uuid, n.kind, n.title, n.content, n.content_json, n.parent_id, n.position, n.created_at, n.updated_at,
                        (SELECT COUNT(*) FROM edges e WHERE e.dst = n.id AND e.kind = 'mentions') AS mc
                 FROM nodes n
                 WHERE n.kind = 'entity'
                 ORDER BY mc DESC, n.updated_at DESC
                 LIMIT ?1",
            )?;
            let rows = stmt
                .query_map([limit as i64], row_to_node)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(rows)
}

pub async fn list_pages(conn: &Connection, limit: u32) -> Result<Vec<Node>> {
    let rows = conn
        .call(move |c| -> rusqlite::Result<Vec<Node>> {
            let sql = format!(
                "SELECT {NODE_COLUMNS} FROM nodes
                 WHERE kind = 'page'
                 ORDER BY updated_at DESC
                 LIMIT ?1"
            );
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt
                .query_map([limit as i64], row_to_node)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(rows)
}

/// Case-insensitive substring match on page titles, shortest-first so exact
/// matches surface above the long ones. Used by the `[[` autocomplete.
pub async fn list_block_children(conn: &Connection, parent_id: i64) -> Result<Vec<Node>> {
    let rows = conn
        .call(move |c| -> rusqlite::Result<Vec<Node>> {
            let sql = format!(
                "SELECT {NODE_COLUMNS} FROM nodes
                 WHERE parent_id = ?1
                 ORDER BY position ASC, uuid ASC"
            );
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt
                .query_map([parent_id], row_to_node)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(rows)
}

pub struct DeletedPage {
    pub node_uuids: Vec<uuid::Uuid>,
    pub attachments: Vec<Node>,
}

pub async fn delete_page(conn: &Connection, id: i64) -> Result<Option<DeletedPage>> {
    let Some((uuids, attachments)) = conn
        .call(
            move |database| -> rusqlite::Result<Option<(Vec<uuid::Uuid>, Vec<Node>)>> {
                let exists = database
                    .query_row(
                        "SELECT 1 FROM nodes WHERE id = ?1 AND kind = 'page'",
                        [id],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
                if !exists {
                    return Ok(None);
                }
                let uuids = {
                    let mut statement = database.prepare(
                        "WITH RECURSIVE subtree(id) AS (
                       SELECT ?1
                       UNION ALL
                       SELECT n.id FROM nodes n JOIN subtree s ON n.parent_id = s.id
                     )
                     SELECT uuid FROM nodes WHERE id IN (SELECT id FROM subtree)
                     ORDER BY id DESC",
                    )?;
                    statement
                        .query_map([id], |row| row.get::<_, uuid::Uuid>(0))?
                        .collect::<Result<Vec<_>, _>>()?
                };
                let attachments = {
                    let sql = format!(
                        "WITH RECURSIVE subtree(id) AS (
                       SELECT ?1
                       UNION ALL
                       SELECT n.id FROM nodes n JOIN subtree s ON n.parent_id = s.id
                     )
                     SELECT DISTINCT {NODE_COLUMNS_N} FROM nodes n
                     JOIN edges e ON e.dst = n.id
                     WHERE e.kind = 'attachment' AND e.src IN (SELECT id FROM subtree)"
                    );
                    let mut statement = database.prepare(&sql)?;
                    statement
                        .query_map([id], row_to_node)?
                        .collect::<Result<Vec<_>, _>>()?
                };
                Ok(Some((uuids, attachments)))
            },
        )
        .await?
    else {
        return Ok(None);
    };
    let mut kinds = attachments
        .iter()
        .map(|attachment| {
            OpKind::NodeDelete(NodeDelete {
                uuid: attachment.uuid,
            })
        })
        .collect::<Vec<_>>();
    kinds.extend(
        uuids
            .iter()
            .copied()
            .map(|uuid| OpKind::NodeDelete(NodeDelete { uuid })),
    );
    apply_local_action(conn, "delete page", kinds).await?;
    Ok(Some(DeletedPage {
        node_uuids: uuids,
        attachments,
    }))
}

pub async fn create_page(conn: &Connection, title: String) -> Result<Node> {
    let title = title.trim().to_owned();
    if title.is_empty() {
        return Err(crate::CoreError::invalid("title is required").into());
    }
    if let Some(existing) = get_page_by_title(conn, title.clone()).await? {
        return Ok(existing);
    }
    let uuid = uuid::Uuid::now_v7();
    apply_local_action(
        conn,
        "create page",
        vec![OpKind::NodeCreate(NodeCreate {
            uuid,
            node_kind: NodeKind::Page,
            title: Some(title),
            content: String::new(),
            content_json: None,
            parent_uuid: None,
            position: None,
            created_at: chrono::Utc::now().timestamp(),
        })],
    )
    .await?;
    get_node_by_uuid(conn, uuid)
        .await?
        .context("created page was not materialized")
}

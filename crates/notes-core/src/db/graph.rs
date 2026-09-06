use super::*;

pub async fn neighbors(conn: &Connection, uuid: uuid::Uuid, depth: u32) -> Result<Vec<Content>> {
    let uuids = conn
        .call(move |database| {
            let mut statement = database.prepare(
                "WITH RECURSIVE adjacent(source, target) AS (
                   SELECT source_block_uuid, target_page_uuid FROM page_links
                    WHERE target_page_uuid IS NOT NULL
                   UNION SELECT target_page_uuid, source_block_uuid FROM page_links
                    WHERE target_page_uuid IS NOT NULL
                   UNION SELECT source_block_uuid, target_block_uuid FROM block_refs
                   UNION SELECT target_block_uuid, source_block_uuid FROM block_refs
                   UNION SELECT uuid, page_uuid FROM blocks WHERE parent_uuid IS NULL
                   UNION SELECT page_uuid, uuid FROM blocks WHERE parent_uuid IS NULL
                   UNION SELECT uuid, parent_uuid FROM blocks WHERE parent_uuid IS NOT NULL
                   UNION SELECT parent_uuid, uuid FROM blocks WHERE parent_uuid IS NOT NULL
                 ), reachable(uuid, depth) AS (
                   SELECT ?1, 0
                   UNION
                   SELECT adjacent.target, reachable.depth + 1
                     FROM reachable JOIN adjacent ON adjacent.source = reachable.uuid
                    WHERE reachable.depth < ?2
                 )
                 SELECT DISTINCT uuid FROM reachable WHERE uuid != ?1 ORDER BY depth, uuid",
            )?;
            statement
                .query_map(rusqlite::params![uuid, depth], |row| {
                    row.get::<_, uuid::Uuid>(0)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .await?;
    get_contents(conn, uuids).await
}

pub async fn find_backlinks(conn: &Connection, uuid: uuid::Uuid) -> Result<Vec<Content>> {
    let source_uuids = conn
        .call(move |database| {
            let mut statement = database.prepare(
                "SELECT source_block_uuid FROM page_links WHERE target_page_uuid = ?1
                 UNION
                 SELECT source_block_uuid FROM block_refs WHERE target_block_uuid = ?1
                 ORDER BY 1",
            )?;
            statement
                .query_map([uuid], |row| row.get::<_, uuid::Uuid>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .await?;
    get_contents(conn, source_uuids).await
}

pub async fn read_ancestors(conn: &Connection, uuid: uuid::Uuid) -> Result<Vec<Content>> {
    let block = get_block(conn, uuid).await?;
    let Some(block) = block else {
        return Ok(get_page(conn, uuid)
            .await?
            .map(Content::Page)
            .into_iter()
            .collect());
    };
    let ancestor_uuids = conn
        .call(move |database| {
            let mut statement = database.prepare(
                "WITH RECURSIVE ancestors(uuid, parent_uuid, depth) AS (
                   SELECT uuid, parent_uuid, 0 FROM blocks WHERE uuid = ?1
                   UNION ALL
                   SELECT parent.uuid, parent.parent_uuid, ancestors.depth + 1
                     FROM ancestors JOIN blocks parent ON parent.uuid = ancestors.parent_uuid
                 )
                 SELECT uuid FROM ancestors ORDER BY depth DESC",
            )?;
            statement
                .query_map([uuid], |row| row.get::<_, uuid::Uuid>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .await?;
    let mut records = Vec::with_capacity(ancestor_uuids.len() + 1);
    if let Some(page) = get_page(conn, block.page_uuid).await? {
        records.push(Content::Page(page));
    }
    for ancestor in ancestor_uuids {
        if let Some(block) = get_block(conn, ancestor).await? {
            records.push(Content::Block(block));
        }
    }
    Ok(records)
}

pub async fn read_subtree(conn: &Connection, uuid: uuid::Uuid, depth: u32) -> Result<Vec<Content>> {
    let (page_uuid, parent_uuid) = if get_page(conn, uuid).await?.is_some() {
        (uuid, None)
    } else if let Some(block) = get_block(conn, uuid).await? {
        (block.page_uuid, Some(uuid))
    } else {
        return Ok(Vec::new());
    };
    let blocks = conn
        .call(move |database| {
            let sql = format!(
                "WITH RECURSIVE subtree(uuid, path, depth) AS (
                   SELECT block.uuid, block.order_key, 1
                     FROM blocks block
                    WHERE block.page_uuid = ?1 AND block.parent_uuid IS ?2
                   UNION ALL
                   SELECT child.uuid, subtree.path || '/' || child.order_key, subtree.depth + 1
                     FROM subtree JOIN blocks child ON child.parent_uuid = subtree.uuid
                    WHERE subtree.depth < ?3
                 )
                 SELECT {QUALIFIED_BLOCK_COLUMNS} FROM subtree
                   JOIN blocks ON blocks.uuid = subtree.uuid
                  ORDER BY subtree.path"
            );
            database
                .prepare(&sql)?
                .query_map(
                    rusqlite::params![page_uuid, parent_uuid, depth],
                    row_to_block,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .await?;
    Ok(blocks.into_iter().map(Content::Block).collect())
}

pub async fn graph_snapshot(
    conn: &Connection,
    focus_uuid: Option<uuid::Uuid>,
) -> Result<GraphSnapshot> {
    let page_level = focus_uuid.is_none();
    let content = match focus_uuid {
        Some(uuid) => {
            let mut records = vec![];
            if let Some(record) = get_content(conn, uuid).await? {
                records.push(record);
            }
            records.extend(neighbors(conn, uuid, 1).await?);
            records
        }
        None => list_pages(conn, u32::MAX)
            .await?
            .into_iter()
            .map(Content::Page)
            .collect(),
    };
    let selected = content
        .iter()
        .map(Content::uuid)
        .collect::<std::collections::HashSet<_>>();
    let items = content
        .into_iter()
        .map(|content| match content {
            Content::Page(page) => GraphItem {
                uuid: page.uuid,
                kind: ObjectKind::Page,
                label: match page.kind {
                    PageKind::Note | PageKind::Handwriting => {
                        page.title.unwrap_or_else(|| "Untitled".into())
                    }
                    PageKind::Journal { date } => date.to_string(),
                },
            },
            Content::Block(block) => GraphItem {
                uuid: block.uuid,
                kind: ObjectKind::Block,
                label: block.markdown.chars().take(80).collect(),
            },
        })
        .collect();
    let edges = conn
        .call(move |database| {
            if page_level {
                return page_level_edges(database, &selected);
            }

            let mut edges = Vec::new();
            {
                let mut statement = database.prepare(
                    "SELECT source_block_uuid, target_page_uuid FROM page_links
                      WHERE target_page_uuid IS NOT NULL",
                )?;
                for row in statement.query_map([], |row| {
                    Ok((row.get::<_, uuid::Uuid>(0)?, row.get::<_, uuid::Uuid>(1)?))
                })? {
                    let (source_uuid, target_uuid) = row?;
                    if selected.contains(&source_uuid) && selected.contains(&target_uuid) {
                        edges.push(GraphEdge {
                            source_uuid,
                            target_uuid,
                            relation: GraphRelation::PageLink,
                        });
                    }
                }
            }
            {
                let mut statement = database
                    .prepare("SELECT source_block_uuid, target_block_uuid FROM block_refs")?;
                for row in statement.query_map([], |row| {
                    Ok((row.get::<_, uuid::Uuid>(0)?, row.get::<_, uuid::Uuid>(1)?))
                })? {
                    let (source_uuid, target_uuid) = row?;
                    if selected.contains(&source_uuid) && selected.contains(&target_uuid) {
                        edges.push(GraphEdge {
                            source_uuid,
                            target_uuid,
                            relation: GraphRelation::BlockReference,
                        });
                    }
                }
            }
            {
                let mut statement = database
                    .prepare("SELECT uuid, COALESCE(parent_uuid, page_uuid) FROM blocks")?;
                for row in statement.query_map([], |row| {
                    Ok((row.get::<_, uuid::Uuid>(0)?, row.get::<_, uuid::Uuid>(1)?))
                })? {
                    let (source_uuid, target_uuid) = row?;
                    if selected.contains(&source_uuid) && selected.contains(&target_uuid) {
                        edges.push(GraphEdge {
                            source_uuid: target_uuid,
                            target_uuid: source_uuid,
                            relation: GraphRelation::Contains,
                        });
                    }
                }
            }
            Ok(edges)
        })
        .await?;
    Ok(GraphSnapshot { items, edges })
}

/// Project content-level references onto their owning pages for the workspace graph.
///
/// Page links always have a concrete page target here. Block references are included
/// only once the target block exists and its owning page can be determined. `DISTINCT`
/// intentionally collapses any number of block-level references between the same two
/// pages into one edge of each relation kind.
fn page_level_edges(
    database: &rusqlite::Connection,
    selected: &std::collections::HashSet<uuid::Uuid>,
) -> rusqlite::Result<Vec<GraphEdge>> {
    let mut edges = Vec::new();
    {
        let mut statement = database.prepare(
            "SELECT DISTINCT source.page_uuid, link.target_page_uuid
               FROM page_links link
               JOIN blocks source ON source.uuid = link.source_block_uuid
              WHERE link.target_page_uuid IS NOT NULL
              ORDER BY source.page_uuid, link.target_page_uuid",
        )?;
        for row in statement.query_map([], |row| {
            Ok((row.get::<_, uuid::Uuid>(0)?, row.get::<_, uuid::Uuid>(1)?))
        })? {
            let (source_uuid, target_uuid) = row?;
            if selected.contains(&source_uuid) && selected.contains(&target_uuid) {
                edges.push(GraphEdge {
                    source_uuid,
                    target_uuid,
                    relation: GraphRelation::PageLink,
                });
            }
        }
    }
    {
        let mut statement = database.prepare(
            "SELECT DISTINCT source.page_uuid, target.page_uuid
               FROM block_refs reference
               JOIN blocks source ON source.uuid = reference.source_block_uuid
               JOIN blocks target ON target.uuid = reference.target_block_uuid
              ORDER BY source.page_uuid, target.page_uuid",
        )?;
        for row in statement.query_map([], |row| {
            Ok((row.get::<_, uuid::Uuid>(0)?, row.get::<_, uuid::Uuid>(1)?))
        })? {
            let (source_uuid, target_uuid) = row?;
            if selected.contains(&source_uuid) && selected.contains(&target_uuid) {
                edges.push(GraphEdge {
                    source_uuid,
                    target_uuid,
                    relation: GraphRelation::BlockReference,
                });
            }
        }
    }
    Ok(edges)
}

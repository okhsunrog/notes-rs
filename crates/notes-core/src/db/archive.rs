use super::*;

pub async fn export_archive(conn: &Connection) -> Result<DataArchive> {
    conn.call(|database| -> rusqlite::Result<DataArchive> {
        let nodes = {
            let sql = format!("SELECT {NODE_COLUMNS} FROM nodes ORDER BY id");
            let mut statement = database.prepare(&sql)?;
            statement
                .query_map([], row_to_node)?
                .collect::<Result<Vec<_>, _>>()?
        };
        let edges = {
            let mut statement = database
                .prepare("SELECT src, dst, kind, weight, created_at FROM edges ORDER BY id")?;
            statement
                .query_map([], |row| {
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
        let entity_descriptions = {
            let mut statement = database.prepare(
                "SELECT source_node_id, entity_node_id, description, created_at
                   FROM entity_descriptions
                  ORDER BY source_node_id, entity_node_id",
            )?;
            statement
                .query_map([], |row| {
                    Ok(EntityDescriptionRecord {
                        source_node_id: row.get(0)?,
                        entity_node_id: row.get(1)?,
                        description: row.get(2)?,
                        created_at: row.get(3)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(DataArchive {
            format: "notes-rs".into(),
            version: 1,
            exported_at: chrono::Utc::now().timestamp(),
            nodes,
            edges,
            entity_descriptions,
            files: std::collections::BTreeMap::new(),
        })
    })
    .await
}

pub async fn import_archive(conn: &Connection, archive: DataArchive) -> Result<()> {
    if archive.format != "notes-rs" || archive.version != 1 {
        anyhow::bail!("unsupported notes-rs archive format or version");
    }
    let current = export_archive(conn).await?;
    let kinds = archive_transition_kinds(&current, &archive)?;
    apply_local(conn, kinds).await?;

    let target_nodes = archive
        .nodes
        .iter()
        .map(|node| (node.id, node.uuid.clone()))
        .collect::<std::collections::HashMap<_, _>>();
    let descriptions = archive
        .entity_descriptions
        .into_iter()
        .filter_map(|description| {
            Some((
                target_nodes.get(&description.source_node_id)?.clone(),
                target_nodes.get(&description.entity_node_id)?.clone(),
                description.description,
                description.created_at,
            ))
        })
        .collect::<Vec<_>>();
    conn.call(move |database| -> rusqlite::Result<()> {
        let transaction = database.transaction()?;
        transaction.execute("DELETE FROM entity_descriptions", [])?;
        for (source_uuid, entity_uuid, description, created_at) in descriptions {
            transaction.execute(
                "INSERT OR IGNORE INTO entity_descriptions
                   (source_node_id, entity_node_id, description, created_at)
                 SELECT source.id, entity.id, ?3, ?4 FROM nodes source, nodes entity
                  WHERE source.uuid = ?1 AND entity.uuid = ?2",
                rusqlite::params![source_uuid, entity_uuid, description, created_at],
            )?;
        }
        transaction.execute_batch(
            "DELETE FROM extracted_edge_sources;
             UPDATE nodes SET last_extracted_hash = NULL
               WHERE kind IN ('page', 'block');
             INSERT OR REPLACE INTO extract_queue(node_id, enqueued_at, retry_count, last_attempt)
               SELECT id, unixepoch(), 0, NULL FROM nodes WHERE kind IN ('page', 'block');
             INSERT OR REPLACE INTO embed_queue(node_id, enqueued_at, retry_count, last_attempt)
               SELECT id, unixepoch(), 0, NULL FROM nodes;",
        )?;
        transaction.commit()?;
        Ok(())
    })
    .await
}

fn archive_transition_kinds(current: &DataArchive, target: &DataArchive) -> Result<Vec<OpKind>> {
    use std::collections::{BTreeMap, BTreeSet, HashMap};

    let current_by_uuid = current
        .nodes
        .iter()
        .map(|node| (node.uuid.as_str(), node))
        .collect::<BTreeMap<_, _>>();
    let target_by_uuid = target
        .nodes
        .iter()
        .map(|node| (node.uuid.as_str(), node))
        .collect::<BTreeMap<_, _>>();
    let target_id_to_uuid = target
        .nodes
        .iter()
        .map(|node| (node.id, node.uuid.as_str()))
        .collect::<HashMap<_, _>>();
    let current_id_to_uuid = current
        .nodes
        .iter()
        .map(|node| (node.id, node.uuid.as_str()))
        .collect::<HashMap<_, _>>();
    let mut kinds = Vec::new();

    for uuid in current_by_uuid.keys().rev() {
        if !target_by_uuid.contains_key(uuid) {
            kinds.push(OpKind::NodeDelete(NodeDelete {
                uuid: (*uuid).to_string(),
            }));
        }
    }
    for (uuid, target_node) in &target_by_uuid {
        match current_by_uuid.get(uuid) {
            None => kinds.push(OpKind::NodeCreate(NodeCreate {
                uuid: (*uuid).to_string(),
                node_kind: target_node.kind,
                title: target_node.title.clone(),
                content: target_node.content.clone(),
                content_json: target_node.content_json.clone(),
                parent_uuid: None,
                position: target_node.position,
                created_at: target_node.created_at,
            })),
            Some(current_node) => {
                if current_node.title != target_node.title {
                    kinds.push(OpKind::NodeSetTitle(NodeSetTitle {
                        uuid: (*uuid).to_string(),
                        title: target_node.title.clone(),
                    }));
                }
                if current_node.content != target_node.content
                    || current_node.content_json != target_node.content_json
                {
                    kinds.push(OpKind::NodeSetContent(NodeSetContent {
                        uuid: (*uuid).to_string(),
                        content: target_node.content.clone(),
                        content_json: target_node.content_json.clone(),
                    }));
                }
            }
        }
    }
    for (uuid, target_node) in &target_by_uuid {
        if target_node.kind != NodeKind::Block {
            continue;
        }
        let parent_uuid = target_node
            .parent_id
            .and_then(|parent_id| target_id_to_uuid.get(&parent_id))
            .map(|uuid| (*uuid).to_string());
        let current_structure = current_by_uuid.get(uuid).map(|node| {
            (
                node.parent_id
                    .and_then(|parent_id| current_id_to_uuid.get(&parent_id))
                    .copied(),
                node.position,
            )
        });
        if current_structure != Some((parent_uuid.as_deref(), target_node.position)) {
            kinds.push(OpKind::NodeMove(NodeMove {
                uuid: (*uuid).to_string(),
                parent_uuid,
                position: target_node.position.unwrap_or(1024.0),
            }));
        }
    }

    let edge_map = |archive: &DataArchive, ids: &HashMap<i64, &str>| {
        archive
            .edges
            .iter()
            .filter_map(|edge| {
                Some((
                    (
                        (*ids.get(&edge.src)?).to_string(),
                        (*ids.get(&edge.dst)?).to_string(),
                        edge.kind.clone(),
                    ),
                    edge.weight,
                ))
            })
            .collect::<BTreeMap<_, _>>()
    };
    let current_edges = edge_map(current, &current_id_to_uuid);
    let target_edges = edge_map(target, &target_id_to_uuid);
    let current_keys = current_edges.keys().cloned().collect::<BTreeSet<_>>();
    let target_keys = target_edges.keys().cloned().collect::<BTreeSet<_>>();
    for (src_uuid, dst_uuid, edge_kind) in current_keys.difference(&target_keys) {
        kinds.push(OpKind::EdgeRemove(operation::EdgeRemove {
            src_uuid: src_uuid.clone(),
            dst_uuid: dst_uuid.clone(),
            edge_kind: edge_kind.clone(),
        }));
    }
    for (src_uuid, dst_uuid, edge_kind) in target_keys.difference(&current_keys) {
        kinds.push(OpKind::EdgeAdd(EdgeAdd {
            src_uuid: src_uuid.clone(),
            dst_uuid: dst_uuid.clone(),
            edge_kind: edge_kind.clone(),
            weight: target_edges[&(src_uuid.clone(), dst_uuid.clone(), edge_kind.clone())],
        }));
    }
    Ok(kinds)
}

pub async fn checkpoint_history(conn: &Connection, action: &str) -> Result<()> {
    let archive = export_archive(conn).await?;
    let json = serde_json::to_string(&archive)?;
    let action = action.to_string();
    conn.call(move |database| -> rusqlite::Result<()> {
        let transaction = database.transaction()?;
        transaction.execute(
            "INSERT INTO history_undo(action, archive_json, created_at)
             VALUES (?1, ?2, unixepoch())",
            rusqlite::params![action, json],
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
        .call(
            move |database| -> rusqlite::Result<Option<(i64, String, String)>> {
                let sql = format!(
                    "SELECT id, action, archive_json FROM {source} ORDER BY id DESC LIMIT 1"
                );
                database
                    .query_row(&sql, [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                    .optional()
            },
        )
        .await?;
    let Some((entry_id, action, json)) = entry else {
        return Ok(false);
    };
    let destination_archive = export_archive(conn).await?;
    let destination_json = serde_json::to_string(&destination_archive)?;
    let archive: DataArchive = serde_json::from_str(&json)?;
    import_archive(conn, archive).await?;
    conn.call(move |database| -> rusqlite::Result<()> {
        let transaction = database.transaction()?;
        transaction.execute(&format!("DELETE FROM {source} WHERE id = ?1"), [entry_id])?;
        transaction.execute(
            &format!(
                "INSERT INTO {target}(action, archive_json, created_at)
                 VALUES (?1, ?2, unixepoch())"
            ),
            rusqlite::params![action, destination_json],
        )?;
        transaction.commit()?;
        Ok(())
    })
    .await?;
    Ok(true)
}

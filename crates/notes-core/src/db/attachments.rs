use super::*;

pub async fn create_attachment(
    conn: &Connection,
    parent_id: i64,
    blob_hash: String,
    title: String,
    mime: String,
    size: u64,
) -> Result<Node> {
    let parent_uuid = require_node_uuid(conn, parent_id).await?;
    apply_local(
        conn,
        vec![OpKind::AttachmentAdd(AttachmentAdd {
            node_uuid: parent_uuid,
            blob_hash: blob_hash.clone(),
            filename: title,
            mime,
            size,
        })],
    )
    .await?;
    conn.call(move |database| {
        let sql = format!(
            "SELECT {NODE_COLUMNS} FROM nodes
             WHERE kind = 'attachment'
               AND json_extract(content_json, '$.blobHash') = ?1"
        );
        database.query_row(&sql, [&blob_hash], row_to_node)
    })
    .await
}

pub async fn list_attachments(conn: &Connection, parent_id: i64) -> Result<Vec<Node>> {
    conn.call(move |database| -> rusqlite::Result<Vec<Node>> {
        let sql = format!(
            "SELECT {NODE_COLUMNS_N} FROM nodes n
             JOIN edges e ON e.dst = n.id
             WHERE e.src = ?1 AND e.kind = 'attachment' AND n.kind = 'attachment'
             ORDER BY n.created_at, n.id"
        );
        let mut statement = database.prepare(&sql)?;
        statement
            .query_map([parent_id], row_to_node)?
            .collect::<Result<Vec<_>, _>>()
    })
    .await
}

pub async fn attachment_path_ref_count(conn: &Connection, relative_path: String) -> Result<i64> {
    conn.call(move |database| {
        database.query_row(
            "SELECT COUNT(*) FROM nodes WHERE kind = 'attachment' AND content = ?1",
            [relative_path],
            |row| row.get(0),
        )
    })
    .await
}

pub async fn delete_attachment(conn: &Connection, id: i64) -> Result<Option<Node>> {
    let record = conn
        .call(
            move |database| -> rusqlite::Result<Option<AttachmentDeleteRecord>> {
                let sql = format!(
                    "SELECT {NODE_COLUMNS} FROM nodes WHERE id = ?1 AND kind = 'attachment'"
                );
                let Some(node) = database.query_row(&sql, [id], row_to_node).optional()? else {
                    return Ok(None);
                };
                let parent_uuid = database
                    .query_row(
                        "SELECT parent.uuid FROM edges e JOIN nodes parent ON parent.id = e.src
             WHERE e.dst = ?1 AND e.kind = 'attachment' LIMIT 1",
                        [id],
                        |row| row.get::<_, uuid::Uuid>(0),
                    )
                    .optional()?;
                let blob_hash = node
                    .content_json
                    .as_deref()
                    .and_then(|metadata| serde_json::from_str::<serde_json::Value>(metadata).ok())
                    .and_then(|metadata| {
                        metadata
                            .get("blobHash")
                            .and_then(|value| value.as_str())
                            .map(str::to_owned)
                    });
                Ok(Some((node, parent_uuid, blob_hash)))
            },
        )
        .await?;
    let Some((node, parent_uuid, blob_hash)) = record else {
        return Ok(None);
    };
    let kind = if let (Some(node_uuid), Some(blob_hash)) = (parent_uuid, blob_hash) {
        OpKind::AttachmentRemove(AttachmentRemove {
            node_uuid,
            blob_hash,
        })
    } else {
        OpKind::NodeDelete(NodeDelete { uuid: node.uuid })
    };
    apply_local(conn, vec![kind]).await?;
    Ok(Some(node))
}

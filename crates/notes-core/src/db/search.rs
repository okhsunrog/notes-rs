use super::*;

pub async fn search_pages_by_title(
    conn: &Connection,
    query: String,
    limit: u32,
) -> Result<Vec<Node>> {
    let trimmed = query.trim().to_string();
    if trimmed.is_empty() {
        return list_pages(conn, limit).await;
    }
    let pattern = format!("%{trimmed}%");
    conn.call(move |c| -> rusqlite::Result<Vec<Node>> {
        let sql = format!(
            "SELECT {NODE_COLUMNS} FROM nodes
             WHERE kind = 'page' AND title IS NOT NULL AND lower(title) LIKE lower(?1)
             ORDER BY length(title) ASC, lower(title) ASC
             LIMIT ?2"
        );
        let mut stmt = c.prepare(&sql)?;
        stmt.query_map(rusqlite::params![pattern, limit as i64], row_to_node)?
            .collect::<Result<Vec<_>, _>>()
    })
    .await
}

/// FTS5 over blocks only. Used by block-reference autocomplete.
pub async fn search_blocks_fts(conn: &Connection, query: String, limit: u32) -> Result<Vec<Node>> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let stemmed_query = crate::stem::stem_search_query(&query);
    if stemmed_query.is_empty() {
        return Ok(Vec::new());
    }
    conn.call(move |c| -> rusqlite::Result<Vec<Node>> {
        let sql = format!(
            "SELECT {} FROM nodes_fts
             JOIN nodes n ON n.id = nodes_fts.rowid
             WHERE nodes_fts MATCH ?1 AND n.kind = 'block'
             ORDER BY bm25(nodes_fts)
             LIMIT ?2",
            NODE_COLUMNS
                .split(',')
                .map(|column| format!("n.{}", column.trim()))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let mut statement = c.prepare(&sql)?;
        statement
            .query_map(rusqlite::params![stemmed_query, limit as i64], row_to_node)?
            .collect::<Result<Vec<_>, _>>()
    })
    .await
}

pub async fn search_fts(conn: &Connection, query: String, limit: u32) -> Result<Vec<SearchHit>> {
    let stemmed_query = crate::stem::stem_search_query(&query);
    if stemmed_query.is_empty() {
        return Ok(Vec::new());
    }
    conn.call(move |connection| {
        let mut statement = connection.prepare(
            "SELECT n.id, n.uuid, n.kind, n.title, n.content, n.content_json,
                    n.parent_id, n.position, n.created_at, n.updated_at,
                    bm25(nodes_fts) AS score
             FROM nodes_fts
             JOIN nodes n ON n.id = nodes_fts.rowid
             WHERE nodes_fts MATCH ?1
             ORDER BY score
             LIMIT ?2",
        )?;
        statement
            .query_map(rusqlite::params![stemmed_query, limit as i64], |row| {
                Ok(SearchHit {
                    node: row_to_node(row)?,
                    score: row.get(10)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
    })
    .await
}

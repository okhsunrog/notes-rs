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
        let rows = stmt
            .query_map(rusqlite::params![pattern, limit as i64], row_to_node)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
    .await
}

/// FTS5 over blocks only. Used by the `((` autocomplete — no rerank for
/// keystroke-time speed. Empty query returns nothing.
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
                .map(|s| format!("n.{}", s.trim()))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let mut stmt = c.prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params![stemmed_query, limit as i64], row_to_node)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
    .await
}

pub async fn search_fts(conn: &Connection, query: String, limit: u32) -> Result<Vec<SearchHit>> {
    // The UI supplies natural language, so never interpret it as FTS5 syntax.
    let stemmed_query = crate::stem::stem_search_query(&query);
    if stemmed_query.is_empty() {
        return Ok(Vec::new());
    }
    let hits = conn
        .call(move |c| -> rusqlite::Result<Vec<SearchHit>> {
            let mut stmt = c.prepare(
                "SELECT n.id, n.uuid, n.kind, n.title, n.content, n.content_json, n.parent_id, n.position, n.created_at, n.updated_at,
                        bm25(nodes_fts) AS score
                 FROM nodes_fts
                 JOIN nodes n ON n.id = nodes_fts.rowid
                 WHERE nodes_fts MATCH ?1
                 ORDER BY score
                 LIMIT ?2",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![&stemmed_query, limit as i64], |r| {
                    Ok(SearchHit {
                        node: row_to_node(r)?,
                        score: r.get::<_, f64>(10)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(hits)
}

pub async fn search_vec(
    conn: &Connection,
    embedding: Vec<f32>,
    limit: u32,
) -> Result<Vec<SearchHit>> {
    let blob: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
    let hits = conn
        .call(move |c| -> rusqlite::Result<Vec<SearchHit>> {
            let mut stmt = c.prepare(
                "SELECT n.id, n.uuid, n.kind, n.title, n.content, n.content_json, n.parent_id, n.position, n.created_at, n.updated_at,
                        v.distance
                 FROM vec_nodes v
                 JOIN nodes n ON n.id = v.rowid
                 WHERE v.embedding MATCH ?1 AND k = ?2
                 ORDER BY v.distance",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![&blob, limit as i64], |r| {
                    Ok(SearchHit {
                        node: row_to_node(r)?,
                        score: r.get::<_, f64>(10)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(hits)
}

pub async fn search_hybrid(
    conn: &Connection,
    query: String,
    embedding: Vec<f32>,
    limit: u32,
) -> Result<Vec<SearchHit>> {
    let fts = search_fts(conn, query, limit * 4).await?;
    let vec = search_vec(conn, embedding, limit * 4).await?;
    // Reciprocal rank fusion (k=60)
    use std::collections::HashMap;
    let mut scores: HashMap<i64, f64> = HashMap::new();
    let mut nodes: HashMap<i64, Node> = HashMap::new();
    for (rank, h) in fts.into_iter().enumerate() {
        let s = scores.entry(h.node.id).or_default();
        *s += 1.0 / (60.0 + rank as f64 + 1.0);
        nodes.entry(h.node.id).or_insert(h.node);
    }
    for (rank, h) in vec.into_iter().enumerate() {
        let s = scores.entry(h.node.id).or_default();
        *s += 1.0 / (60.0 + rank as f64 + 1.0);
        nodes.entry(h.node.id).or_insert(h.node);
    }
    // Backlink boost: blocks/pages that are linked to from elsewhere in the
    // graph are usually more central. Add a tiny term proportional to
    // log(1 + incoming_edges) so well-connected nodes break ties in their
    // favor without overwhelming relevance.
    if !scores.is_empty() {
        let ids: Vec<i64> = scores.keys().copied().collect();
        let counts = incoming_link_counts(conn, &ids).await?;
        for (id, c) in counts {
            if let Some(s) = scores.get_mut(&id) {
                *s += BACKLINK_BOOST * (1.0 + c as f64).ln();
            }
        }
    }
    let mut out: Vec<SearchHit> = scores
        .into_iter()
        .map(|(id, score)| SearchHit {
            node: nodes.remove(&id).unwrap(),
            score,
        })
        .collect();
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    out.truncate(limit as usize);
    Ok(out)
}

/// Small enough that a top-RRF item with zero backlinks still beats a poorly-
/// ranked item with many. At BOOST=0.002 a node with 100 incoming edges gains
/// ~0.009 — roughly equivalent to moving up 30 ranks.
const BACKLINK_BOOST: f64 = 0.002;

async fn incoming_link_counts(conn: &Connection, ids: &[i64]) -> Result<Vec<(i64, i64)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids = ids.to_vec();
    let rows = conn
        .call(move |c| -> rusqlite::Result<Vec<(i64, i64)>> {
            let placeholders = std::iter::repeat_n("?", ids.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT dst, COUNT(*) FROM edges
                 WHERE kind IN ('refs', 'mentions') AND dst IN ({placeholders})
                 GROUP BY dst"
            );
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(ids.iter()), |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(rows)
}

use super::*;

pub async fn search_pages_by_title(
    conn: &Connection,
    query: String,
    limit: u32,
) -> Result<Vec<Page>> {
    let query = crate::model::normalize_title(&query);
    if query.is_empty() {
        return Ok(Vec::new());
    }
    conn.call(move |database| {
        let sql = format!(
            "SELECT {PAGE_COLUMNS} FROM pages
              WHERE normalized_title IS NOT NULL AND instr(normalized_title, ?1) > 0
              ORDER BY CASE WHEN normalized_title = ?1 THEN 0 ELSE 1 END,
                       length(title), title
              LIMIT ?2"
        );
        database
            .prepare(&sql)?
            .query_map(rusqlite::params![query, limit], row_to_page)?
            .collect()
    })
    .await
}

pub async fn search_blocks_fts(
    conn: &Connection,
    query: String,
    limit: u32,
    token_mode: crate::stem::SearchTokenMode,
) -> Result<Vec<Block>> {
    let query = crate::stem::stem_search_query_with_mode(&query, token_mode);
    if query.is_empty() {
        return Ok(Vec::new());
    }
    conn.call(move |database| {
        let sql = format!(
            "SELECT {BLOCK_COLUMNS} FROM blocks
              JOIN blocks_fts ON blocks_fts.rowid = blocks.id
             WHERE blocks_fts MATCH ?1
             ORDER BY bm25(blocks_fts), blocks.updated_at DESC
             LIMIT ?2"
        );
        database
            .prepare(&sql)?
            .query_map(rusqlite::params![query, limit], row_to_block)?
            .collect()
    })
    .await
}

pub async fn search_fts(conn: &Connection, query: String, limit: u32) -> Result<Vec<SearchHit>> {
    let pages = search_pages_by_title(conn, query.clone(), limit).await?;
    let blocks = search_blocks_fts(conn, query, limit, crate::stem::SearchTokenMode::Plain).await?;
    let mut hits = pages
        .into_iter()
        .enumerate()
        .map(|(index, page)| SearchHit {
            content: Content::Page(page),
            score: 1.0 / (index as f64 + 1.0),
        })
        .chain(
            blocks
                .into_iter()
                .enumerate()
                .map(|(index, block)| SearchHit {
                    content: Content::Block(block),
                    score: 0.9 / (index as f64 + 1.0),
                }),
        )
        .collect::<Vec<_>>();
    hits.sort_by(|left, right| right.score.total_cmp(&left.score));
    hits.truncate(limit as usize);
    Ok(hits)
}

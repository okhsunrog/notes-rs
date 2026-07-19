use super::*;

pub async fn search_pages_by_title(
    conn: &Connection,
    query: String,
    limit: u32,
) -> Result<Vec<Page>> {
    let normalized_query = crate::model::normalize_title(&query);
    if normalized_query.is_empty() {
        return Ok(Vec::new());
    }
    let strict_query =
        crate::stem::stem_search_query_with_mode(&query, crate::stem::SearchTokenMode::Prefix);
    let relaxed_query = crate::stem::relaxed_stem_prefix_search_query(&query);
    conn.call(move |database| {
        let mut pages = if strict_query.is_empty() {
            Vec::new()
        } else {
            query_pages_fts(database, &strict_query, &normalized_query, limit)?
        };
        if pages.is_empty() {
            pages = query_pages_by_substring(database, &normalized_query, limit)?;
        }
        if pages.is_empty() && !relaxed_query.is_empty() {
            pages = query_pages_fts(database, &relaxed_query, &normalized_query, limit)?;
        }
        let mut seen = std::collections::HashSet::new();
        pages.retain(|page| seen.insert(page.uuid));
        Ok(pages)
    })
    .await
}

fn query_pages_fts(
    database: &rusqlite::Connection,
    query: &str,
    normalized_query: &str,
    limit: u32,
) -> rusqlite::Result<Vec<Page>> {
    let sql = format!(
        "SELECT {PAGE_COLUMNS} FROM pages
           JOIN pages_fts ON pages_fts.rowid = pages.id
          WHERE pages_fts MATCH ?1
          ORDER BY CASE WHEN pages.normalized_title = ?2 THEN 0 ELSE 1 END,
                   bm25(pages_fts), length(pages.title), pages.title
          LIMIT ?3"
    );
    database
        .prepare(&sql)?
        .query_map(
            rusqlite::params![query, normalized_query, limit],
            row_to_page,
        )?
        .collect()
}

fn query_pages_by_substring(
    database: &rusqlite::Connection,
    normalized_query: &str,
    limit: u32,
) -> rusqlite::Result<Vec<Page>> {
    let sql = format!(
        "SELECT {PAGE_COLUMNS} FROM pages
          WHERE normalized_title IS NOT NULL AND instr(normalized_title, ?1) > 0
          ORDER BY CASE WHEN normalized_title = ?1 THEN 0 ELSE 1 END,
                   length(title), title
          LIMIT ?2"
    );
    database
        .prepare(&sql)?
        .query_map(rusqlite::params![normalized_query, limit], row_to_page)?
        .collect()
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

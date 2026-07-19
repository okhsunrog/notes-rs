use super::*;

struct RankedPage {
    page: Page,
    bm25: Option<f64>,
    exact_title: bool,
}

struct RankedBlock {
    block: Block,
    bm25: f64,
    snippet: String,
}

pub async fn search_pages_by_title(
    conn: &Connection,
    query: String,
    limit: u32,
) -> Result<Vec<Page>> {
    Ok(search_pages_ranked(conn, query, limit)
        .await?
        .into_iter()
        .map(|ranked| ranked.page)
        .collect())
}

async fn search_pages_ranked(
    conn: &Connection,
    query: String,
    limit: u32,
) -> Result<Vec<RankedPage>> {
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
        pages.retain(|ranked| seen.insert(ranked.page.uuid));
        Ok(pages)
    })
    .await
}

fn query_pages_fts(
    database: &rusqlite::Connection,
    query: &str,
    normalized_query: &str,
    limit: u32,
) -> rusqlite::Result<Vec<RankedPage>> {
    let sql = format!(
        "SELECT {PAGE_COLUMNS}, bm25(pages_fts), pages.normalized_title = ?2 FROM pages
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
            row_to_ranked_page,
        )?
        .collect()
}

fn query_pages_by_substring(
    database: &rusqlite::Connection,
    normalized_query: &str,
    limit: u32,
) -> rusqlite::Result<Vec<RankedPage>> {
    let sql = format!(
        "SELECT {PAGE_COLUMNS}, NULL, normalized_title = ?1 FROM pages
          WHERE normalized_title IS NOT NULL AND instr(normalized_title, ?1) > 0
          ORDER BY CASE WHEN normalized_title = ?1 THEN 0 ELSE 1 END,
                   length(title), title
          LIMIT ?2"
    );
    database
        .prepare(&sql)?
        .query_map(
            rusqlite::params![normalized_query, limit],
            row_to_ranked_page,
        )?
        .collect()
}

fn row_to_ranked_page(row: &rusqlite::Row<'_>) -> rusqlite::Result<RankedPage> {
    Ok(RankedPage {
        page: row_to_page(row)?,
        bm25: row.get(8)?,
        exact_title: row.get(9)?,
    })
}

pub async fn search_blocks_fts(
    conn: &Connection,
    query: String,
    limit: u32,
    token_mode: crate::stem::SearchTokenMode,
) -> Result<Vec<Block>> {
    Ok(search_blocks_ranked(conn, query, limit, token_mode)
        .await?
        .into_iter()
        .map(|ranked| ranked.block)
        .collect())
}

async fn search_blocks_ranked(
    conn: &Connection,
    query: String,
    limit: u32,
    token_mode: crate::stem::SearchTokenMode,
) -> Result<Vec<RankedBlock>> {
    let display_query = query.clone();
    let fts_query = crate::stem::stem_search_query_with_mode(&query, token_mode);
    if fts_query.is_empty() {
        return Ok(Vec::new());
    }
    conn.call(move |database| {
        let sql = format!(
            "SELECT {QUALIFIED_BLOCK_COLUMNS}, bm25(blocks_fts)
               FROM blocks
              JOIN blocks_fts ON blocks_fts.rowid = blocks.id
             WHERE blocks_fts MATCH ?1
             ORDER BY bm25(blocks_fts), blocks.updated_at DESC
             LIMIT ?2"
        );
        database
            .prepare(&sql)?
            .query_map(rusqlite::params![fts_query, limit], |row| {
                let block = row_to_block(row)?;
                Ok(RankedBlock {
                    snippet: raw_markdown_snippet(&block.markdown, &display_query),
                    block,
                    bm25: row.get(9)?,
                })
            })?
            .collect()
    })
    .await
}

fn raw_markdown_snippet(markdown: &str, query: &str) -> String {
    const CONTEXT_CHARS: usize = 60;

    let query_stems = token_spans(query)
        .into_iter()
        .map(|(start, end)| crate::stem::stem(&query[start..end]))
        .collect::<std::collections::HashSet<_>>();
    let tokens = token_spans(markdown);
    let matches = tokens
        .iter()
        .map(|&(start, end)| query_stems.contains(&crate::stem::stem(&markdown[start..end])))
        .collect::<Vec<_>>();
    let Some(first_match) = matches.iter().position(|matched| *matched) else {
        return markdown.to_string();
    };

    let match_start = tokens[first_match].0;
    let match_end = tokens[first_match].1;
    let mut first_token = first_match;
    while first_token > 0
        && markdown[tokens[first_token - 1].0..match_start]
            .chars()
            .count()
            <= CONTEXT_CHARS
    {
        first_token -= 1;
    }
    let mut last_token = first_match;
    while last_token + 1 < tokens.len()
        && markdown[match_end..tokens[last_token + 1].1]
            .chars()
            .count()
            <= CONTEXT_CHARS
    {
        last_token += 1;
    }

    let window_start = tokens[first_token].0;
    let window_end = tokens[last_token].1;
    let mut snippet = String::new();
    if window_start > 0 {
        snippet.push('…');
    }
    let mut cursor = window_start;
    for (index, &(start, end)) in tokens
        .iter()
        .enumerate()
        .take(last_token + 1)
        .skip(first_token)
    {
        snippet.push_str(&markdown[cursor..start]);
        if matches[index] {
            snippet.push_str("<mark>");
            snippet.push_str(&markdown[start..end]);
            snippet.push_str("</mark>");
        } else {
            snippet.push_str(&markdown[start..end]);
        }
        cursor = end;
    }
    snippet.push_str(&markdown[cursor..window_end]);
    if window_end < markdown.len() {
        snippet.push('…');
    }
    snippet
}

fn token_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = None;
    for (index, character) in text.char_indices() {
        if character.is_alphanumeric() {
            start.get_or_insert(index);
        } else if let Some(start) = start.take() {
            spans.push((start, index));
        }
    }
    if let Some(start) = start {
        spans.push((start, text.len()));
    }
    spans
}

pub async fn search_fts(conn: &Connection, query: String, limit: u32) -> Result<Vec<SearchHit>> {
    let pages = search_pages_ranked(conn, query.clone(), limit).await?;
    let blocks =
        search_blocks_ranked(conn, query, limit, crate::stem::SearchTokenMode::Plain).await?;

    let (exact_pages, pages): (Vec<_>, Vec<_>) =
        pages.into_iter().partition(|ranked| ranked.exact_title);
    let mut hits = exact_pages
        .into_iter()
        .map(page_search_hit)
        .collect::<Vec<_>>();
    let mut pages = pages.into_iter();
    let mut blocks = blocks.into_iter();
    // The bm25 scales of two independent FTS tables are not directly comparable.
    // Preserve each list's bm25 order and interleave by rank, starting with blocks
    // so only an exact title gets cross-source priority over a stronger body hit.
    while hits.len() < limit as usize {
        let mut advanced = false;
        if let Some(block) = blocks.next() {
            hits.push(block_search_hit(block));
            advanced = true;
        }
        if hits.len() < limit as usize
            && let Some(page) = pages.next()
        {
            hits.push(page_search_hit(page));
            advanced = true;
        }
        if !advanced {
            break;
        }
    }
    Ok(hits)
}

fn page_search_hit(ranked: RankedPage) -> SearchHit {
    SearchHit {
        content: Content::Page(ranked.page),
        score: ranked.bm25.map_or(0.0, |score| -score),
        snippet: None,
    }
}

fn block_search_hit(ranked: RankedBlock) -> SearchHit {
    SearchHit {
        content: Content::Block(ranked.block),
        score: -ranked.bm25,
        snippet: Some(ranked.snippet),
    }
}

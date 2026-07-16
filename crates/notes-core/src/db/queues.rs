use super::*;

/// A queued background job that has failed at least once.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundFailure {
    pub queue: BackgroundQueue,
    pub node_id: i64,
    pub node_title: Option<String>,
    pub retry_count: i64,
    pub last_attempt: Option<i64>,
    pub failure_kind: FailureKind,
    pub last_error: String,
    pub terminal: bool,
}

/// Pull the next batch from the embed queue, packaging each row with its
/// ancestor-chain context.
type EmbedChainRow = (i64, i32, Option<String>, String);

pub const EMBED_MAX_ATTEMPTS: i64 = 8;
pub const EMBED_BACKOFF_BASE_SECS: i64 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueStatus {
    pub embeddings_pending: i64,
    pub embeddings_failed: i64,
    pub extractions_pending: i64,
    pub extractions_failed: i64,
    pub failures: Vec<BackgroundFailure>,
}

pub async fn take_pending_embeddings(conn: &Connection, batch: u32) -> Result<Vec<(i64, String)>> {
    let rows = conn
        .call(move |c| -> rusqlite::Result<Vec<EmbedChainRow>> {
            let mut stmt = c.prepare(
                "WITH batch(node_id) AS (
                   SELECT node_id FROM embed_queue
                   WHERE terminal = 0 AND retry_count < ?2
                     AND (last_attempt IS NULL
                          OR unixepoch() - last_attempt >= ?3 * (1 << retry_count))
                   ORDER BY enqueued_at ASC LIMIT ?1
                 ),
                 chain(qid, id, parent_id, title, content, depth) AS (
                   SELECT b.node_id, n.id, n.parent_id, n.title, n.content, 0
                     FROM batch b JOIN nodes n ON n.id = b.node_id
                   UNION ALL
                   SELECT c.qid, p.id, p.parent_id, p.title, p.content, c.depth + 1
                     FROM chain c JOIN nodes p ON p.id = c.parent_id
                 )
                 SELECT qid, depth, title, content FROM chain
                 ORDER BY qid, depth",
            )?;
            let rows = stmt
                .query_map(
                    rusqlite::params![batch as i64, EMBED_MAX_ATTEMPTS, EMBED_BACKOFF_BASE_SECS],
                    |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, i32>(1)?,
                            r.get::<_, Option<String>>(2)?,
                            r.get::<_, String>(3)?,
                        ))
                    },
                )?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;

    // Group by qid, then compose each block's embedding text.
    let mut out: Vec<(i64, String)> = Vec::new();
    let mut current_qid: Option<i64> = None;
    let mut chain: Vec<(i32, Option<String>, String)> = Vec::new();
    for (qid, depth, title, content) in rows {
        if Some(qid) != current_qid {
            if let Some(prev) = current_qid {
                out.push((prev, compose_embed_text(&chain)));
                chain.clear();
            }
            current_qid = Some(qid);
        }
        chain.push((depth, title, content));
    }
    if let Some(qid) = current_qid {
        out.push((qid, compose_embed_text(&chain)));
    }
    Ok(out)
}

pub async fn record_embedding_failure(
    conn: &Connection,
    node_ids: Vec<i64>,
    failure_kind: FailureKind,
    error: &str,
    terminal: bool,
) -> Result<()> {
    if node_ids.is_empty() {
        return Ok(());
    }
    let error: String = error.chars().take(2_000).collect();
    conn.call(move |c| -> rusqlite::Result<()> {
        let transaction = c.transaction()?;
        for node_id in node_ids {
            transaction.execute(
                "UPDATE embed_queue
                   SET retry_count = retry_count + 1,
                       last_attempt = unixepoch(),
                       failure_kind = ?2,
                       last_error = ?3,
                       terminal = ?4
                   WHERE node_id = ?1",
                rusqlite::params![node_id, failure_kind, error, terminal],
            )?;
        }
        transaction.commit()?;
        Ok(())
    })
    .await
}

pub async fn queue_status(conn: &Connection) -> Result<QueueStatus> {
    conn.call(|database| -> rusqlite::Result<QueueStatus> {
        let counts = database.query_row(
            "SELECT
               (SELECT COUNT(*) FROM embed_queue),
               (SELECT COUNT(*) FROM embed_queue WHERE retry_count > 0),
               (SELECT COUNT(*) FROM extract_queue),
               (SELECT COUNT(*) FROM extract_queue WHERE retry_count > 0)",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )?;
        let mut statement = database.prepare(
            "SELECT queue, node_id, title, retry_count, last_attempt,
                    COALESCE(failure_kind, 'unknown'), COALESCE(last_error, ''), terminal
             FROM (
               SELECT 'embedding' AS queue, q.node_id, n.title, q.retry_count,
                      q.last_attempt, q.failure_kind, q.last_error, q.terminal
                 FROM embed_queue q JOIN nodes n ON n.id = q.node_id
                WHERE q.retry_count > 0
               UNION ALL
               SELECT 'extraction', q.node_id, n.title, q.retry_count,
                      q.last_attempt, q.failure_kind, q.last_error, q.terminal
                 FROM extract_queue q JOIN nodes n ON n.id = q.node_id
                WHERE q.retry_count > 0
             )
             ORDER BY terminal DESC, last_attempt DESC, queue, node_id
             LIMIT 50",
        )?;
        let failures = statement
            .query_map([], |row| {
                Ok(BackgroundFailure {
                    queue: row.get(0)?,
                    node_id: row.get(1)?,
                    node_title: row.get(2)?,
                    retry_count: row.get(3)?,
                    last_attempt: row.get(4)?,
                    failure_kind: row.get(5)?,
                    last_error: row.get(6)?,
                    terminal: row.get::<_, i64>(7)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(QueueStatus {
            embeddings_pending: counts.0,
            embeddings_failed: counts.1,
            extractions_pending: counts.2,
            extractions_failed: counts.3,
            failures,
        })
    })
    .await
}

pub async fn retry_background_jobs(conn: &Connection) -> Result<()> {
    conn.call(|database| -> rusqlite::Result<()> {
        database.execute_batch(
            "UPDATE embed_queue
                SET retry_count = 0, last_attempt = NULL, last_error = NULL,
                    failure_kind = NULL, terminal = 0;
             UPDATE extract_queue
                SET retry_count = 0, last_attempt = NULL, last_error = NULL,
                    failure_kind = NULL, terminal = 0;",
        )
    })
    .await
}

pub async fn clear_background_jobs(conn: &Connection) -> Result<()> {
    conn.call(|database| -> rusqlite::Result<()> {
        database.execute_batch("DELETE FROM embed_queue; DELETE FROM extract_queue;")
    })
    .await
}

/// Compose ancestor-aware embedding text. `chain` is ordered depth-ascending
/// (depth=0 is the node itself). Layout:
///
/// ```text
/// {ancestor titles joined by " > " — outermost first}
/// {direct parent's content, truncated}
/// {self title}
/// {self content}
/// ```
///
/// Empty sections are dropped. Pages (no ancestors) collapse to just title +
/// content, matching the previous behavior.
fn compose_embed_text(chain: &[(i32, Option<String>, String)]) -> String {
    const PARENT_EXCERPT_MAX: usize = 200;
    let mut parts: Vec<String> = Vec::new();
    // Ancestor titles — depth descending (root first) so the breadcrumb reads
    // top-down like the user sees the outline.
    let mut titles: Vec<(i32, &str)> = chain
        .iter()
        .filter(|(d, t, _)| *d > 0 && t.as_deref().is_some_and(|s| !s.trim().is_empty()))
        .map(|(d, t, _)| (*d, t.as_deref().unwrap()))
        .collect();
    titles.sort_by_key(|item| std::cmp::Reverse(item.0));
    if !titles.is_empty() {
        let joined: Vec<&str> = titles.iter().map(|(_, t)| *t).collect();
        parts.push(joined.join(" > "));
    }
    // Direct parent's content (depth = 1), truncated, newlines flattened.
    if let Some((_, _, parent_content)) = chain.iter().find(|(d, _, _)| *d == 1) {
        let trimmed = parent_content.trim();
        if !trimmed.is_empty() {
            let flat: String = trimmed
                .chars()
                .map(|c| if c == '\n' { ' ' } else { c })
                .collect();
            let cut = flat.char_indices().nth(PARENT_EXCERPT_MAX).map(|(i, _)| i);
            let excerpt = match cut {
                Some(i) => format!("{}…", &flat[..i]),
                None => flat,
            };
            parts.push(excerpt);
        }
    }
    // Self.
    if let Some((_, title, content)) = chain.iter().find(|(d, _, _)| *d == 0) {
        if let Some(t) = title.as_deref().filter(|s| !s.trim().is_empty()) {
            parts.push(t.to_string());
        }
        if !content.trim().is_empty() {
            parts.push(content.clone());
        }
    }
    parts.join("\n")
}

/// Max extraction attempts before a node is left in the queue and skipped.
/// A subsequent content edit re-enqueues it with retry_count reset by the
/// `INSERT OR REPLACE` trigger.
pub const EXTRACT_MAX_ATTEMPTS: i64 = 5;
/// Base seconds for exponential backoff: wait = BASE * 2^retry_count.
/// At retry_count=0 we wait 0s (haven't tried), then 30s, 60s, 120s, 240s.
pub const EXTRACT_BACKOFF_BASE_SECS: i64 = 30;

pub async fn take_pending_extractions(
    conn: &Connection,
    batch: u32,
) -> Result<Vec<(i64, Option<String>, String)>> {
    let rows = conn
        .call(
            move |c| -> rusqlite::Result<Vec<(i64, Option<String>, String)>> {
                // Eligible: under retry cap AND (never attempted OR backoff elapsed).
                let mut stmt = c.prepare(
                    "SELECT n.id, n.title, n.content
                 FROM extract_queue q JOIN nodes n ON n.id = q.node_id
                 WHERE q.terminal = 0 AND q.retry_count < ?2
                   AND (q.last_attempt IS NULL
                        OR unixepoch() - q.last_attempt >= ?3 * (1 << q.retry_count))
                 ORDER BY q.enqueued_at ASC
                 LIMIT ?1",
                )?;
                let rows = stmt
                    .query_map(
                        rusqlite::params![
                            batch as i64,
                            EXTRACT_MAX_ATTEMPTS,
                            EXTRACT_BACKOFF_BASE_SECS,
                        ],
                        |r| {
                            Ok((
                                r.get::<_, i64>(0)?,
                                r.get::<_, Option<String>>(1)?,
                                r.get::<_, String>(2)?,
                            ))
                        },
                    )?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            },
        )
        .await?;
    Ok(rows)
}

pub async fn finish_extraction(conn: &Connection, node_id: i64) -> Result<()> {
    conn.call(move |c| -> rusqlite::Result<()> {
        c.execute("DELETE FROM extract_queue WHERE node_id = ?1", [node_id])?;
        Ok(())
    })
    .await?;
    Ok(())
}

/// Read the hash recorded at the node's last successful extraction. `None`
/// when the node has never been extracted or the row is missing.
pub async fn get_last_extracted_hash(conn: &Connection, node_id: i64) -> Result<Option<String>> {
    let h = conn
        .call(move |c| -> rusqlite::Result<Option<String>> {
            c.query_row(
                "SELECT last_extracted_hash FROM nodes WHERE id = ?1",
                [node_id],
                |r| r.get::<_, Option<String>>(0),
            )
        })
        .await?;
    Ok(h)
}

/// Stamp the hash that was just successfully extracted, so future queue ticks
/// can skip identical content.
pub async fn set_last_extracted_hash(conn: &Connection, node_id: i64, hash: String) -> Result<()> {
    conn.call(move |c| -> rusqlite::Result<()> {
        c.execute(
            "UPDATE nodes SET last_extracted_hash = ?2 WHERE id = ?1",
            rusqlite::params![node_id, hash],
        )?;
        Ok(())
    })
    .await?;
    Ok(())
}

/// Mark an extraction attempt as failed: bump retry_count and stamp
/// last_attempt. The row stays in the queue; backoff governs the next try.
pub async fn record_extraction_failure(
    conn: &Connection,
    node_id: i64,
    failure_kind: FailureKind,
    error: &str,
    terminal: bool,
) -> Result<()> {
    let error: String = error.chars().take(2_000).collect();
    conn.call(move |c| -> rusqlite::Result<()> {
        c.execute(
            "UPDATE extract_queue
             SET retry_count = retry_count + 1,
                 last_attempt = unixepoch(),
                 failure_kind = ?2,
                 last_error = ?3,
                 terminal = CASE
                   WHEN ?4 THEN 1
                   WHEN ?2 = 'schema' AND retry_count + 1 >= 2 THEN 1
                   ELSE 0
                 END
             WHERE node_id = ?1",
            rusqlite::params![node_id, failure_kind, error, terminal],
        )?;
        Ok(())
    })
    .await?;
    Ok(())
}

pub async fn write_embeddings(conn: &Connection, items: Vec<(i64, Vec<f32>)>) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    conn.call(move |c| -> rusqlite::Result<()> {
        let tx = c.transaction()?;
        for (id, emb) in &items {
            let still_pending = tx
                .query_row(
                    "SELECT 1 FROM embed_queue q JOIN nodes n ON n.id = q.node_id
                     WHERE q.node_id = ?1",
                    [id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !still_pending {
                continue;
            }
            let blob: Vec<u8> = emb.iter().flat_map(|f| f.to_le_bytes()).collect();
            tx.execute("DELETE FROM vec_nodes WHERE rowid = ?1", [id])?;
            tx.execute(
                "INSERT INTO vec_nodes(rowid, embedding) VALUES (?1, ?2)",
                rusqlite::params![id, &blob],
            )?;
            tx.execute("DELETE FROM embed_queue WHERE node_id = ?1", [id])?;
        }
        tx.commit()?;
        Ok(())
    })
    .await?;
    Ok(())
}

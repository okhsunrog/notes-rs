use anyhow::{Context, Result};
use rusqlite::ffi::sqlite3_auto_extension;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Once;
use tokio_rusqlite::Connection;

static VEC_INIT: Once = Once::new();

fn register_sqlite_vec() {
    VEC_INIT.call_once(|| unsafe {
        sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

pub async fn open(
    path: impl AsRef<Path>,
    embedder_id: &str,
    ndims: usize,
) -> Result<Connection> {
    register_sqlite_vec();
    let conn = Connection::open(path.as_ref())
        .await
        .context("opening sqlite database")?;
    conn.call(|c| -> rusqlite::Result<()> {
        c.pragma_update(None, "journal_mode", "WAL")?;
        c.pragma_update(None, "synchronous", "NORMAL")?;
        c.pragma_update(None, "foreign_keys", "ON")?;
        c.pragma_update(None, "temp_store", "MEMORY")?;
        Ok(())
    })
    .await?;
    migrate(&conn, ndims).await?;
    check_embedder_compat(&conn, embedder_id, ndims).await?;
    Ok(conn)
}

const CURRENT_SCHEMA_VERSION: i64 = 2;

async fn migrate(conn: &Connection, ndims: usize) -> Result<()> {
    let schema = SCHEMA_V1.replace("{NDIMS}", &ndims.to_string());
    conn.call(move |c| -> rusqlite::Result<()> {
        c.execute_batch(&schema)?;
        // idempotent ALTERs for in-place upgrades
        let has_content_json: bool = c
            .prepare("SELECT 1 FROM pragma_table_info('nodes') WHERE name = 'content_json'")?
            .exists([])?;
        if !has_content_json {
            c.execute_batch("ALTER TABLE nodes ADD COLUMN content_json TEXT;")?;
        }
        let has_body_stemmed: bool = c
            .prepare("SELECT 1 FROM pragma_table_info('nodes') WHERE name = 'body_stemmed'")?
            .exists([])?;
        if !has_body_stemmed {
            c.execute_batch(
                "ALTER TABLE nodes ADD COLUMN body_stemmed TEXT NOT NULL DEFAULT '';",
            )?;
        }
        let has_parent_id: bool = c
            .prepare("SELECT 1 FROM pragma_table_info('nodes') WHERE name = 'parent_id'")?
            .exists([])?;
        if !has_parent_id {
            c.execute_batch(
                "ALTER TABLE nodes ADD COLUMN parent_id INTEGER REFERENCES nodes(id) ON DELETE CASCADE;
                 CREATE INDEX IF NOT EXISTS idx_nodes_parent ON nodes(parent_id, position);",
            )?;
        }
        let has_position: bool = c
            .prepare("SELECT 1 FROM pragma_table_info('nodes') WHERE name = 'position'")?
            .exists([])?;
        if !has_position {
            c.execute_batch("ALTER TABLE nodes ADD COLUMN position REAL;")?;
        }
        // Drop dead node_properties (never written to). Reintroduce when we
        // have a real properties write path.
        c.execute_batch("DROP TABLE IF EXISTS node_properties;")?;
        let has_retry_count: bool = c
            .prepare("SELECT 1 FROM pragma_table_info('extract_queue') WHERE name = 'retry_count'")?
            .exists([])?;
        if !has_retry_count {
            c.execute_batch(
                "ALTER TABLE extract_queue ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE extract_queue ADD COLUMN last_attempt INTEGER;",
            )?;
        }
        // Detect old FTS schema (had `title, content` cols instead of just `body_stemmed`).
        let fts_old: bool = c
            .prepare(
                "SELECT 1 FROM pragma_table_info('nodes_fts') WHERE name = 'content'",
            )?
            .exists([])?;
        if fts_old {
            c.execute_batch(
                "DROP TRIGGER IF EXISTS nodes_ai;
                 DROP TRIGGER IF EXISTS nodes_ad;
                 DROP TRIGGER IF EXISTS nodes_au;
                 DROP TABLE IF EXISTS nodes_fts;
                 CREATE VIRTUAL TABLE nodes_fts USING fts5(
                   body_stemmed,
                   content='nodes', content_rowid='id',
                   tokenize='unicode61 remove_diacritics 2'
                 );
                 CREATE TRIGGER nodes_ai AFTER INSERT ON nodes BEGIN
                   INSERT INTO nodes_fts(rowid, body_stemmed) VALUES (new.id, new.body_stemmed);
                 END;
                 CREATE TRIGGER nodes_ad AFTER DELETE ON nodes BEGIN
                   INSERT INTO nodes_fts(nodes_fts, rowid, body_stemmed) VALUES('delete', old.id, old.body_stemmed);
                 END;
                 CREATE TRIGGER nodes_au AFTER UPDATE ON nodes BEGIN
                   INSERT INTO nodes_fts(nodes_fts, rowid, body_stemmed) VALUES('delete', old.id, old.body_stemmed);
                   INSERT INTO nodes_fts(rowid, body_stemmed) VALUES (new.id, new.body_stemmed);
                 END;",
            )?;
        }
        Ok(())
    })
    .await
    .context("running migrations")?;
    backfill_stemmed(conn).await?;
    set_schema_version(conn, CURRENT_SCHEMA_VERSION).await?;
    Ok(())
}

async fn set_schema_version(conn: &Connection, version: i64) -> Result<()> {
    conn.call(move |c| -> rusqlite::Result<()> {
        c.execute("DELETE FROM schema_version", [])?;
        c.execute(
            "INSERT INTO schema_version(version) VALUES (?1)",
            [version],
        )?;
        Ok(())
    })
    .await?;
    Ok(())
}

async fn backfill_stemmed(conn: &Connection) -> Result<()> {
    // Stem rows where body_stemmed is empty but content/title isn't.
    let rows = conn
        .call(|c| -> rusqlite::Result<Vec<(i64, Option<String>, String)>> {
            let mut stmt = c.prepare(
                "SELECT id, title, content FROM nodes
                 WHERE body_stemmed = '' AND (content != '' OR title IS NOT NULL)",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    if rows.is_empty() {
        return Ok(());
    }
    let stemmed: Vec<(i64, String)> = rows
        .into_iter()
        .map(|(id, title, content)| {
            let combined = format!("{}\n{}", title.as_deref().unwrap_or(""), content);
            (id, crate::stem::stem(&combined))
        })
        .collect();
    let n = stemmed.len();
    conn.call(move |c| -> rusqlite::Result<()> {
        let tx = c.transaction()?;
        for (id, body) in &stemmed {
            tx.execute(
                "UPDATE nodes SET body_stemmed = ?2 WHERE id = ?1",
                rusqlite::params![id, body],
            )?;
        }
        tx.execute_batch("INSERT INTO nodes_fts(nodes_fts) VALUES('rebuild');")?;
        tx.commit()?;
        Ok(())
    })
    .await?;
    tracing::info!("backfilled stemmed FTS for {n} rows");
    Ok(())
}

async fn check_embedder_compat(conn: &Connection, embedder_id: &str, ndims: usize) -> Result<()> {
    let id = embedder_id.to_string();
    let stored = conn
        .call(move |c| -> rusqlite::Result<Option<(String, i64)>> {
            let mut stmt = c.prepare("SELECT provider, ndims FROM embed_meta WHERE id = 1")?;
            let mut rows = stmt.query([])?;
            if let Some(r) = rows.next()? {
                Ok(Some((r.get(0)?, r.get(1)?)))
            } else {
                Ok(None)
            }
        })
        .await?;
    match stored {
        None => {
            let id2 = id.clone();
            conn.call(move |c| -> rusqlite::Result<()> {
                c.execute(
                    "INSERT INTO embed_meta(id, provider, ndims) VALUES (1, ?1, ?2)",
                    rusqlite::params![id2, ndims as i64],
                )?;
                Ok(())
            })
            .await?;
            Ok(())
        }
        Some((p, n)) if p == id && n as usize == ndims => Ok(()),
        Some((p, n)) => Err(anyhow::anyhow!(
            "embedder mismatch: db was created with {p} ({n} dims), current backend is {id} ({ndims} dims). \
             Delete the db or switch back to match."
        )),
    }
}

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY);
INSERT OR IGNORE INTO schema_version VALUES (1);

CREATE TABLE IF NOT EXISTS nodes (
  id INTEGER PRIMARY KEY,
  uuid TEXT UNIQUE NOT NULL,
  kind TEXT NOT NULL,
  title TEXT,
  content TEXT NOT NULL DEFAULT '',
  content_json TEXT,
  body_stemmed TEXT NOT NULL DEFAULT '',
  parent_id INTEGER REFERENCES nodes(id) ON DELETE CASCADE,
  position REAL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_nodes_kind ON nodes(kind);
CREATE INDEX IF NOT EXISTS idx_nodes_parent ON nodes(parent_id, position);

CREATE TABLE IF NOT EXISTS edges (
  id INTEGER PRIMARY KEY,
  src INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
  dst INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  weight REAL NOT NULL DEFAULT 1.0,
  created_at INTEGER NOT NULL,
  UNIQUE(src, dst, kind)
);
CREATE INDEX IF NOT EXISTS idx_edges_src ON edges(src, kind);
CREATE INDEX IF NOT EXISTS idx_edges_dst ON edges(dst, kind);

CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(
  body_stemmed,
  content='nodes', content_rowid='id',
  tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER IF NOT EXISTS nodes_ai AFTER INSERT ON nodes BEGIN
  INSERT INTO nodes_fts(rowid, body_stemmed) VALUES (new.id, new.body_stemmed);
END;
CREATE TRIGGER IF NOT EXISTS nodes_ad AFTER DELETE ON nodes BEGIN
  INSERT INTO nodes_fts(nodes_fts, rowid, body_stemmed) VALUES('delete', old.id, old.body_stemmed);
END;
CREATE TRIGGER IF NOT EXISTS nodes_au AFTER UPDATE ON nodes BEGIN
  INSERT INTO nodes_fts(nodes_fts, rowid, body_stemmed) VALUES('delete', old.id, old.body_stemmed);
  INSERT INTO nodes_fts(rowid, body_stemmed) VALUES (new.id, new.body_stemmed);
END;

CREATE VIRTUAL TABLE IF NOT EXISTS vec_nodes USING vec0(
  embedding float[{NDIMS}]
);

CREATE TABLE IF NOT EXISTS embed_meta (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  provider TEXT NOT NULL,
  ndims INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS embed_queue (
  node_id INTEGER PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
  enqueued_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS extract_queue (
  node_id INTEGER PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
  enqueued_at INTEGER NOT NULL,
  retry_count INTEGER NOT NULL DEFAULT 0,
  last_attempt INTEGER
);

CREATE TRIGGER IF NOT EXISTS nodes_ai_extract
  AFTER INSERT ON nodes
  WHEN new.kind IN ('block', 'page')
BEGIN
  INSERT OR REPLACE INTO extract_queue(node_id, enqueued_at)
  VALUES(new.id, unixepoch());
END;

CREATE TRIGGER IF NOT EXISTS nodes_au_extract
  AFTER UPDATE OF content, title ON nodes
  WHEN new.kind IN ('block', 'page')
BEGIN
  INSERT OR REPLACE INTO extract_queue(node_id, enqueued_at)
  VALUES(new.id, unixepoch());
END;

CREATE UNIQUE INDEX IF NOT EXISTS idx_entity_title
  ON nodes(kind, lower(title))
  WHERE kind = 'entity' AND title IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_page_title
  ON nodes(kind, lower(title))
  WHERE kind = 'page' AND title IS NOT NULL;

CREATE TRIGGER IF NOT EXISTS nodes_ai_embed AFTER INSERT ON nodes BEGIN
  INSERT OR REPLACE INTO embed_queue(node_id, enqueued_at) VALUES(new.id, unixepoch());
END;
CREATE TRIGGER IF NOT EXISTS nodes_au_embed
  AFTER UPDATE OF content, title ON nodes
BEGIN
  INSERT OR REPLACE INTO embed_queue(node_id, enqueued_at) VALUES(new.id, unixepoch());
END;
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: i64,
    pub uuid: String,
    pub kind: String,
    pub title: Option<String>,
    pub content: String,
    pub content_json: Option<String>,
    pub parent_id: Option<i64>,
    pub position: Option<f64>,
    pub created_at: i64,
    pub updated_at: i64,
}

const NODE_COLUMNS: &str =
    "id, uuid, kind, title, content, content_json, parent_id, position, created_at, updated_at";

fn row_to_node(r: &rusqlite::Row<'_>) -> rusqlite::Result<Node> {
    Ok(Node {
        id: r.get(0)?,
        uuid: r.get(1)?,
        kind: r.get(2)?,
        title: r.get(3)?,
        content: r.get(4)?,
        content_json: r.get(5)?,
        parent_id: r.get(6)?,
        position: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub node: Node,
    pub score: f64,
}

pub async fn create_node(
    conn: &Connection,
    kind: String,
    title: Option<String>,
    content: String,
    content_json: Option<String>,
) -> Result<Node> {
    let uuid = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();
    let body_stemmed = crate::stem::stem(&format!(
        "{}\n{content}",
        title.as_deref().unwrap_or("")
    ));
    let node = conn
        .call(move |c| -> rusqlite::Result<Node> {
            c.execute(
                "INSERT INTO nodes (uuid, kind, title, content, content_json, body_stemmed, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                rusqlite::params![&uuid, &kind, &title, &content, &content_json, &body_stemmed, now],
            )?;
            let id = c.last_insert_rowid();
            Ok(Node {
                id,
                uuid,
                kind,
                title,
                content,
                content_json,
                parent_id: None,
                position: None,
                created_at: now,
                updated_at: now,
            })
        })
        .await?;
    Ok(node)
}

pub async fn update_node(
    conn: &Connection,
    id: i64,
    title: Option<String>,
    content: String,
    content_json: Option<String>,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    let body_stemmed = crate::stem::stem(&format!(
        "{}\n{content}",
        title.as_deref().unwrap_or("")
    ));
    conn.call(move |c| -> rusqlite::Result<()> {
        c.execute(
            "UPDATE nodes
             SET title = ?2, content = ?3, content_json = ?4, body_stemmed = ?5, updated_at = ?6
             WHERE id = ?1",
            rusqlite::params![id, &title, &content, &content_json, &body_stemmed, now],
        )?;
        Ok(())
    })
    .await?;
    Ok(())
}

pub async fn link_nodes(
    conn: &Connection,
    src: i64,
    dst: i64,
    kind: String,
    weight: f64,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    conn.call(move |c| -> rusqlite::Result<()> {
        c.execute(
            "INSERT OR IGNORE INTO edges (src, dst, kind, weight, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![src, dst, &kind, weight, now],
        )?;
        Ok(())
    })
    .await?;
    Ok(())
}

/// Replace all outgoing reference edges for `block_id` in one transaction.
///
/// Strategy: delete every edge `(src=block_id, kind='refs')`, then re-insert
/// one edge per wikilink target (eagerly materializing missing pages) and one
/// per block-ref target (silently skipping broken `((uuid))` refs).
///
/// This is the "re-emit and cleanup on every save" approach from the plan —
/// inefficient but correct. Returns the number of broken block-ref UUIDs so
/// the frontend can surface them later if desired.
pub async fn replace_block_refs(
    conn: &Connection,
    block_id: i64,
    wikilink_titles: Vec<String>,
    block_uuids: Vec<String>,
) -> Result<u32> {
    let broken = conn
        .call(move |c| -> rusqlite::Result<u32> {
            let tx = c.transaction()?;
            tx.execute(
                "DELETE FROM edges WHERE src = ?1 AND kind = 'refs'",
                [block_id],
            )?;
            let now = chrono::Utc::now().timestamp();

            // Wikilinks: get-or-create page row, then link.
            for raw_title in &wikilink_titles {
                let title = raw_title.trim();
                if title.is_empty() {
                    continue;
                }
                let existing: Option<i64> = tx
                    .query_row(
                        "SELECT id FROM nodes
                         WHERE kind = 'page' AND lower(title) = lower(?1)
                         LIMIT 1",
                        [title],
                        |r| r.get(0),
                    )
                    .ok();
                let page_id = match existing {
                    Some(id) => id,
                    None => {
                        let uuid = uuid::Uuid::new_v4().to_string();
                        let body_stemmed = crate::stem::stem("");
                        tx.execute(
                            "INSERT INTO nodes (uuid, kind, title, content, content_json,
                                                body_stemmed, parent_id, position,
                                                created_at, updated_at)
                             VALUES (?1, 'page', ?2, '', NULL, ?3, NULL, NULL, ?4, ?4)",
                            rusqlite::params![&uuid, title, &body_stemmed, now],
                        )?;
                        tx.last_insert_rowid()
                    }
                };
                if page_id != block_id {
                    tx.execute(
                        "INSERT OR IGNORE INTO edges (src, dst, kind, weight, created_at)
                         VALUES (?1, ?2, 'refs', 1.0, ?3)",
                        rusqlite::params![block_id, page_id, now],
                    )?;
                }
            }

            // Block refs: look up target by uuid; skip broken silently.
            let mut broken: u32 = 0;
            for raw_uuid in &block_uuids {
                let uuid = raw_uuid.trim();
                if uuid.is_empty() {
                    continue;
                }
                let target: Option<i64> = tx
                    .query_row("SELECT id FROM nodes WHERE uuid = ?1", [uuid], |r| {
                        r.get(0)
                    })
                    .ok();
                match target {
                    Some(target_id) if target_id != block_id => {
                        tx.execute(
                            "INSERT OR IGNORE INTO edges (src, dst, kind, weight, created_at)
                             VALUES (?1, ?2, 'refs', 1.0, ?3)",
                            rusqlite::params![block_id, target_id, now],
                        )?;
                    }
                    Some(_) => {}
                    None => broken += 1,
                }
            }

            tx.commit()?;
            Ok(broken)
        })
        .await?;
    Ok(broken)
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

pub async fn neighbors(
    conn: &Connection,
    node_id: i64,
    depth: u32,
) -> Result<Vec<Node>> {
    let nodes = conn
        .call(move |c| -> rusqlite::Result<Vec<Node>> {
            let mut stmt = c.prepare(
                "WITH RECURSIVE reachable(id, d) AS (
                   SELECT ?1, 0
                   UNION
                   SELECT e.dst, r.d + 1 FROM edges e
                     JOIN reachable r ON e.src = r.id
                     WHERE r.d < ?2
                   UNION
                   SELECT e.src, r.d + 1 FROM edges e
                     JOIN reachable r ON e.dst = r.id
                     WHERE r.d < ?2
                 )
                 SELECT n.id, n.uuid, n.kind, n.title, n.content, n.content_json, n.parent_id, n.position, n.created_at, n.updated_at
                 FROM nodes n JOIN reachable r ON n.id = r.id
                 WHERE n.id != ?1",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![node_id, depth as i64], row_to_node)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(nodes)
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

pub async fn search_fts(conn: &Connection, query: String, limit: u32) -> Result<Vec<SearchHit>> {
    // Stem each term; preserve FTS5 operators (AND/OR/NOT/NEAR) and metacharacters.
    let stemmed_query = crate::stem::stem_query(&query);
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

pub async fn list_block_children(conn: &Connection, parent_id: i64) -> Result<Vec<Node>> {
    let rows = conn
        .call(move |c| -> rusqlite::Result<Vec<Node>> {
            let sql = format!(
                "SELECT {NODE_COLUMNS} FROM nodes
                 WHERE parent_id = ?1
                 ORDER BY position ASC, id ASC"
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

/// Create a block. If `position` is `None`, append at end of parent's children
/// (MAX(position) + 1.0). `parent_id = None` creates an orphan root block —
/// rare; usually a block has a parent page.
pub async fn create_block(
    conn: &Connection,
    parent_id: Option<i64>,
    position: Option<f64>,
    content: String,
    content_json: Option<String>,
) -> Result<Node> {
    let uuid = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();
    let body_stemmed = crate::stem::stem(&content);
    let node = conn
        .call(move |c| -> rusqlite::Result<Node> {
            let pos: f64 = match position {
                Some(p) => p,
                None => match parent_id {
                    Some(pid) => c.query_row(
                        "SELECT COALESCE(MAX(position), 0.0) + 1.0 FROM nodes WHERE parent_id = ?1",
                        [pid],
                        |r| r.get::<_, f64>(0),
                    )?,
                    None => 1.0,
                },
            };
            c.execute(
                "INSERT INTO nodes (uuid, kind, title, content, content_json, body_stemmed,
                                    parent_id, position, created_at, updated_at)
                 VALUES (?1, 'block', NULL, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                rusqlite::params![&uuid, &content, &content_json, &body_stemmed, parent_id, pos, now],
            )?;
            let id = c.last_insert_rowid();
            Ok(Node {
                id,
                uuid,
                kind: "block".into(),
                title: None,
                content,
                content_json,
                parent_id,
                position: Some(pos),
                created_at: now,
                updated_at: now,
            })
        })
        .await?;
    Ok(node)
}

/// Move a block under a new parent. If `new_position` is `None`, appends to
/// the end of the new parent's children (MAX(position) + 1.0). Returns the
/// updated node so the frontend can maintain its ordering without a re-fetch.
pub async fn move_block(
    conn: &Connection,
    id: i64,
    new_parent_id: Option<i64>,
    new_position: Option<f64>,
) -> Result<Node> {
    let now = chrono::Utc::now().timestamp();
    let node = conn
        .call(move |c| -> rusqlite::Result<Node> {
            let pos: f64 = match new_position {
                Some(p) => p,
                None => match new_parent_id {
                    Some(pid) => c.query_row(
                        "SELECT COALESCE(MAX(position), 0.0) + 1.0 FROM nodes WHERE parent_id = ?1",
                        [pid],
                        |r| r.get::<_, f64>(0),
                    )?,
                    None => 1.0,
                },
            };
            c.execute(
                "UPDATE nodes SET parent_id = ?2, position = ?3, updated_at = ?4 WHERE id = ?1",
                rusqlite::params![id, new_parent_id, pos, now],
            )?;
            let sql = format!("SELECT {NODE_COLUMNS} FROM nodes WHERE id = ?1");
            let mut stmt = c.prepare(&sql)?;
            let mut rows = stmt.query([id])?;
            let row = rows
                .next()?
                .ok_or_else(|| rusqlite::Error::QueryReturnedNoRows)?;
            row_to_node(row)
        })
        .await?;
    Ok(node)
}

/// Delete a block. Refuses (returns `false`) if the block has children, so
/// callers can show feedback instead of silently cascading. Returns `true` on
/// successful delete.
pub async fn delete_block(conn: &Connection, id: i64) -> Result<bool> {
    let deleted = conn
        .call(move |c| -> rusqlite::Result<bool> {
            let kids: i64 = c.query_row(
                "SELECT COUNT(*) FROM nodes WHERE parent_id = ?1",
                [id],
                |r| r.get(0),
            )?;
            if kids > 0 {
                return Ok(false);
            }
            c.execute("DELETE FROM nodes WHERE id = ?1", [id])?;
            Ok(true)
        })
        .await?;
    Ok(deleted)
}

/// Find a page by case-insensitive title or create one. Used to eagerly
/// materialize `[[Wikilink]]` targets so backlinks work the moment the link
/// is typed.
pub async fn get_or_create_page_by_title(conn: &Connection, title: String) -> Result<Node> {
    let trimmed = title.trim().to_string();
    if trimmed.is_empty() {
        anyhow::bail!("page title is empty");
    }
    let found = conn
        .call({
            let t = trimmed.clone();
            move |c| -> rusqlite::Result<Option<Node>> {
                let sql = format!(
                    "SELECT {NODE_COLUMNS} FROM nodes
                     WHERE kind = 'page' AND lower(title) = lower(?1)
                     LIMIT 1"
                );
                let mut stmt = c.prepare(&sql)?;
                let mut rows = stmt.query([&t])?;
                if let Some(r) = rows.next()? {
                    Ok(Some(row_to_node(r)?))
                } else {
                    Ok(None)
                }
            }
        })
        .await?;
    if let Some(n) = found {
        return Ok(n);
    }
    create_node(conn, "page".into(), Some(trimmed), String::new(), None).await
}

pub async fn take_pending_embeddings(
    conn: &Connection,
    batch: u32,
) -> Result<Vec<(i64, String)>> {
    let rows = conn
        .call(move |c| -> rusqlite::Result<Vec<(i64, String)>> {
            let mut stmt = c.prepare(
                "SELECT n.id, COALESCE(n.title, '') || char(10) || n.content
                 FROM embed_queue q JOIN nodes n ON n.id = q.node_id
                 ORDER BY q.enqueued_at ASC
                 LIMIT ?1",
            )?;
            let rows = stmt
                .query_map([batch as i64], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(rows)
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
        .call(move |c| -> rusqlite::Result<Vec<(i64, Option<String>, String)>> {
            // Eligible: under retry cap AND (never attempted OR backoff elapsed).
            let mut stmt = c.prepare(
                "SELECT n.id, n.title, n.content
                 FROM extract_queue q JOIN nodes n ON n.id = q.node_id
                 WHERE q.retry_count < ?2
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
        })
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

/// Mark an extraction attempt as failed: bump retry_count and stamp
/// last_attempt. The row stays in the queue; backoff governs the next try.
pub async fn record_extraction_failure(conn: &Connection, node_id: i64) -> Result<()> {
    conn.call(move |c| -> rusqlite::Result<()> {
        c.execute(
            "UPDATE extract_queue
             SET retry_count = retry_count + 1, last_attempt = unixepoch()
             WHERE node_id = ?1",
            [node_id],
        )?;
        Ok(())
    })
    .await?;
    Ok(())
}

/// Upsert an entity by (kind='entity', lower(title)). Returns the node id.
pub async fn upsert_entity(
    conn: &Connection,
    title: String,
    description: Option<String>,
) -> Result<i64> {
    let now = chrono::Utc::now().timestamp();
    let id = conn
        .call(move |c| -> rusqlite::Result<i64> {
            let mut stmt = c.prepare(
                "SELECT id FROM nodes
                 WHERE kind = 'entity' AND lower(title) = lower(?1)
                 LIMIT 1",
            )?;
            let mut rows = stmt.query([&title])?;
            if let Some(r) = rows.next()? {
                return r.get::<_, i64>(0);
            }
            drop(rows);
            drop(stmt);
            let uuid = uuid::Uuid::new_v4().to_string();
            let desc = description.as_deref().unwrap_or("");
            let body = crate::stem::stem(&format!("{title}\n{desc}"));
            c.execute(
                "INSERT INTO nodes (uuid, kind, title, content, body_stemmed, created_at, updated_at)
                 VALUES (?1, 'entity', ?2, ?3, ?4, ?5, ?5)",
                rusqlite::params![&uuid, &title, desc, &body, now],
            )?;
            Ok(c.last_insert_rowid())
        })
        .await?;
    Ok(id)
}

pub async fn write_embeddings(
    conn: &Connection,
    items: Vec<(i64, Vec<f32>)>,
) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    conn.call(move |c| -> rusqlite::Result<()> {
        let tx = c.transaction()?;
        for (id, emb) in &items {
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

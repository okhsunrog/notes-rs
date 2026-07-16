use crate::operation::{
    self, AttachmentAdd, AttachmentRemove, EdgeAdd, NodeCreate, NodeDelete, NodeMove,
    NodeSetContent, NodeSetTitle, OpKind, Origin,
};
use crate::sqlite::Connection;
use anyhow::{Context, Result};
use rusqlite::OptionalExtension;
use rusqlite::ffi::sqlite3_auto_extension;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Once;

static VEC_INIT: Once = Once::new();

fn register_sqlite_vec() {
    VEC_INIT.call_once(|| unsafe {
        type ExtensionEntry = unsafe extern "C" fn(
            *mut rusqlite::ffi::sqlite3,
            *mut *mut std::os::raw::c_char,
            *const rusqlite::ffi::sqlite3_api_routines,
        ) -> std::os::raw::c_int;
        sqlite3_auto_extension(Some(std::mem::transmute::<*const (), ExtensionEntry>(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

pub async fn open(path: impl AsRef<Path>, embedder_id: &str, ndims: usize) -> Result<Connection> {
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

const CURRENT_SCHEMA_VERSION: i64 = 7;

async fn migrate(conn: &Connection, ndims: usize) -> Result<()> {
    let schema = SCHEMA_V1.replace("{NDIMS}", &ndims.to_string());
    conn.call(move |c| -> rusqlite::Result<()> {
        // Bootstrap the two tables needed to inspect and upgrade legacy DBs.
        // The complete schema contains indexes and triggers that reference
        // newer columns, so it must only run after the idempotent ALTERs.
        c.execute_batch(NODES_BOOTSTRAP)?;
        let previous_version = c
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get::<_, Option<i64>>(0)
            })?
            .unwrap_or(1);

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
                "ALTER TABLE nodes ADD COLUMN parent_id INTEGER REFERENCES nodes(id) ON DELETE CASCADE;",
            )?;
        }
        let has_position: bool = c
            .prepare("SELECT 1 FROM pragma_table_info('nodes') WHERE name = 'position'")?
            .exists([])?;
        if !has_position {
            c.execute_batch("ALTER TABLE nodes ADD COLUMN position REAL;")?;
        }
        let has_last_extracted_hash: bool = c
            .prepare(
                "SELECT 1 FROM pragma_table_info('nodes') WHERE name = 'last_extracted_hash'",
            )?
            .exists([])?;
        if !has_last_extracted_hash {
            c.execute_batch("ALTER TABLE nodes ADD COLUMN last_extracted_hash TEXT;")?;
        }
        for column in ["content_hlc", "title_hlc", "structure_hlc"] {
            let exists: bool = c
                .prepare(&format!(
                    "SELECT 1 FROM pragma_table_info('nodes') WHERE name = '{column}'"
                ))?
                .exists([])?;
            if !exists {
                c.execute_batch(&format!("ALTER TABLE nodes ADD COLUMN {column} TEXT;"))?;
            }
        }
        let tombstones_exist: bool = c
            .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'tombstones'")?
            .exists([])?;
        let tombstones_have_root = tombstones_exist
            && c.prepare("SELECT 1 FROM pragma_table_info('tombstones') WHERE name = 'root_uuid'")?
                .exists([])?;
        if tombstones_exist && !tombstones_have_root {
            c.execute_batch("ALTER TABLE tombstones ADD COLUMN root_uuid TEXT;")?;
        }

        // All referenced node columns now exist, so indexes, triggers, queues,
        // FTS and vector tables can be created safely.
        c.execute_batch(&schema)?;

        if previous_version < 7 {
            c.execute_batch(
                "INSERT OR IGNORE INTO edge_lww(src_uuid, dst_uuid, kind, hlc, present, weight)
                 SELECT src.uuid, dst.uuid, edges.kind,
                        '0000000000000000-00000000-00000000000000000000000000000000',
                        1, edges.weight
                   FROM edges
                   JOIN nodes src ON src.id = edges.src
                   JOIN nodes dst ON dst.id = edges.dst
                  WHERE edges.kind NOT IN ('refs', 'mentions', 'attachment');",
            )?;
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
        for table in ["extract_queue", "embed_queue"] {
            let has_last_error: bool = c
                .prepare(&format!(
                    "SELECT 1 FROM pragma_table_info('{table}') WHERE name = 'last_error'"
                ))?
                .exists([])?;
            if !has_last_error {
                c.execute_batch(&format!(
                    "ALTER TABLE {table} ADD COLUMN last_error TEXT;
                     ALTER TABLE {table} ADD COLUMN failure_kind TEXT;
                     ALTER TABLE {table} ADD COLUMN terminal INTEGER NOT NULL DEFAULT 0;"
                ))?;
            }
        }
        let embed_has_retry_count: bool = c
            .prepare("SELECT 1 FROM pragma_table_info('embed_queue') WHERE name = 'retry_count'")?
            .exists([])?;
        if !embed_has_retry_count {
            c.execute_batch(
                "ALTER TABLE embed_queue ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE embed_queue ADD COLUMN last_attempt INTEGER;",
            )?;
        }
        if previous_version < 3 {
            // Earlier extraction runs had no provenance, so stale semantic
            // edges could not be removed safely. Rebuild only generated graph
            // edges and queue every source for a clean extraction pass.
            c.execute_batch(
                "DELETE FROM edges
                   WHERE kind = 'mentions'
                      OR (src IN (SELECT id FROM nodes WHERE kind = 'entity')
                          AND dst IN (SELECT id FROM nodes WHERE kind = 'entity'));
                 DELETE FROM extracted_edge_sources;
                 UPDATE nodes SET last_extracted_hash = NULL
                   WHERE kind IN ('page', 'block');
                 INSERT OR REPLACE INTO extract_queue(node_id, enqueued_at, retry_count, last_attempt)
                   SELECT id, unixepoch(), 0, NULL FROM nodes
                   WHERE kind IN ('page', 'block');",
            )?;
        }
        if previous_version < 4 {
            c.execute_batch(
                "UPDATE nodes SET last_extracted_hash = NULL
                   WHERE kind IN ('page', 'block');
                 INSERT OR REPLACE INTO extract_queue(node_id, enqueued_at, retry_count, last_attempt)
                   SELECT id, unixepoch(), 0, NULL FROM nodes
                   WHERE kind IN ('page', 'block');",
            )?;
        }
        // Ancestor-aware embedding requires re-embed on parent move; install
        // the trigger on existing DBs that predated it.
        c.execute_batch(
            "CREATE TRIGGER IF NOT EXISTS nodes_au_embed_parent
               AFTER UPDATE OF parent_id ON nodes
             BEGIN
               INSERT OR REPLACE INTO embed_queue(node_id, enqueued_at)
                 VALUES(new.id, unixepoch());
             END;",
        )?;
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

const NODES_BOOTSTRAP: &str = r#"
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
  last_extracted_hash TEXT,
  content_hlc TEXT,
  title_hlc TEXT,
  structure_hlc TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
"#;

async fn set_schema_version(conn: &Connection, version: i64) -> Result<()> {
    conn.call(move |c| -> rusqlite::Result<()> {
        c.execute("DELETE FROM schema_version", [])?;
        c.execute("INSERT INTO schema_version(version) VALUES (?1)", [version])?;
        Ok(())
    })
    .await?;
    Ok(())
}

async fn backfill_stemmed(conn: &Connection) -> Result<()> {
    // Stem rows where body_stemmed is empty but content/title isn't.
    let rows = conn
        .call(
            |c| -> rusqlite::Result<Vec<(i64, Option<String>, String)>> {
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
            },
        )
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
        Some((p, n)) => {
            tracing::warn!(
                old_provider = %p,
                old_ndims = n,
                new_provider = %id,
                new_ndims = ndims,
                "embedder changed — rebuilding vec_nodes and re-enqueueing all embeddings",
            );
            let id2 = id.clone();
            let ndims_u = ndims;
            conn.call(move |c| -> rusqlite::Result<()> {
                let tx = c.transaction()?;
                tx.execute_batch("DROP TABLE IF EXISTS vec_nodes;")?;
                tx.execute_batch(&format!(
                    "CREATE VIRTUAL TABLE vec_nodes USING vec0(embedding float[{ndims_u}]);"
                ))?;
                tx.execute("DELETE FROM embed_queue", [])?;
                tx.execute(
                    "INSERT INTO embed_queue(node_id, enqueued_at)
                     SELECT id, unixepoch() FROM nodes
                     WHERE kind IN ('block', 'page')
                       AND (content != '' OR title IS NOT NULL)",
                    [],
                )?;
                tx.execute(
                    "UPDATE embed_meta SET provider = ?1, ndims = ?2 WHERE id = 1",
                    rusqlite::params![id2, ndims_u as i64],
                )?;
                tx.commit()?;
                Ok(())
            })
            .await?;
            Ok(())
        }
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
  -- Hash of the (title, content) at the last *successful* extraction. The
  -- worker compares the current hash against this before invoking the LLM,
  -- so cosmetic re-saves (typo fixes, indent changes) don't burn credits.
  last_extracted_hash TEXT,
  content_hlc TEXT,
  title_hlc TEXT,
  structure_hlc TEXT,
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

-- Records which source note caused an extracted edge to exist. Relations
-- between two entities cannot otherwise be cleaned up when their source note
-- changes, because the edge itself does not point back to that note.
CREATE TABLE IF NOT EXISTS extracted_edge_sources (
  source_node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
  edge_id INTEGER NOT NULL REFERENCES edges(id) ON DELETE CASCADE,
  PRIMARY KEY(source_node_id, edge_id)
);

-- Entity descriptions are source-specific. Keeping provenance prevents the
-- last extracted note from silently overwriting every other description.
CREATE TABLE IF NOT EXISTS entity_descriptions (
  source_node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
  entity_node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
  description TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY(source_node_id, entity_node_id)
);
CREATE INDEX IF NOT EXISTS idx_entity_descriptions_entity
  ON entity_descriptions(entity_node_id, created_at DESC);

CREATE TABLE IF NOT EXISTS history_undo (
  id INTEGER PRIMARY KEY,
  action TEXT NOT NULL,
  archive_json TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS history_redo (
  id INTEGER PRIMARY KEY,
  action TEXT NOT NULL,
  archive_json TEXT NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sync_outbox (
  rowid INTEGER PRIMARY KEY,
  op_id TEXT NOT NULL UNIQUE,
  envelope TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS applied_ops (
  op_id TEXT PRIMARY KEY,
  seq INTEGER
);
CREATE TABLE IF NOT EXISTS tombstones (
  uuid TEXT PRIMARY KEY,
  deleted_hlc TEXT NOT NULL,
  root_uuid TEXT
);
CREATE TABLE IF NOT EXISTS edge_lww (
  src_uuid TEXT NOT NULL,
  dst_uuid TEXT NOT NULL,
  kind TEXT NOT NULL,
  hlc TEXT NOT NULL,
  present INTEGER NOT NULL,
  weight REAL NOT NULL,
  PRIMARY KEY(src_uuid, dst_uuid, kind)
);
CREATE TABLE IF NOT EXISTS attachment_lww (
  node_uuid TEXT NOT NULL,
  blob_hash TEXT NOT NULL,
  hlc TEXT NOT NULL,
  present INTEGER NOT NULL,
  filename TEXT,
  mime TEXT,
  size INTEGER,
  PRIMARY KEY(node_uuid, blob_hash)
);
CREATE TABLE IF NOT EXISTS sync_meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

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
  enqueued_at INTEGER NOT NULL,
  retry_count INTEGER NOT NULL DEFAULT 0,
  last_attempt INTEGER,
  last_error TEXT,
  failure_kind TEXT,
  terminal INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS extract_queue (
  node_id INTEGER PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
  enqueued_at INTEGER NOT NULL,
  retry_count INTEGER NOT NULL DEFAULT 0,
  last_attempt INTEGER,
  last_error TEXT,
  failure_kind TEXT,
  terminal INTEGER NOT NULL DEFAULT 0
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
-- Re-embed when a block's parent changes. The embedding text now includes
-- the ancestor chain, so a move invalidates it even when content didn't
-- change. Descendants of the moved block keep their stale chain until they
-- get edited (acceptable tradeoff in v1).
CREATE TRIGGER IF NOT EXISTS nodes_au_embed_parent
  AFTER UPDATE OF parent_id ON nodes
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
const NODE_COLUMNS_N: &str = "n.id, n.uuid, n.kind, n.title, n.content, n.content_json, n.parent_id, n.position, n.created_at, n.updated_at";

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

async fn apply_local(conn: &Connection, kinds: Vec<OpKind>) -> Result<()> {
    let operations = operation::local_ops(conn, kinds).await?;
    operation::apply_batch(conn, &operations, Origin::Local).await?;
    Ok(())
}

async fn require_node_uuid(conn: &Connection, id: i64) -> Result<String> {
    conn.call(move |database| {
        database.query_row("SELECT uuid FROM nodes WHERE id = ?1", [id], |row| {
            row.get(0)
        })
    })
    .await
    .with_context(|| format!("node {id} not found"))
}

async fn require_node_uuids(
    conn: &Connection,
    first: i64,
    second: i64,
) -> Result<(String, String)> {
    conn.call(move |database| {
        let first_uuid =
            database.query_row("SELECT uuid FROM nodes WHERE id = ?1", [first], |row| {
                row.get(0)
            })?;
        let second_uuid =
            database.query_row("SELECT uuid FROM nodes WHERE id = ?1", [second], |row| {
                row.get(0)
            })?;
        Ok((first_uuid, second_uuid))
    })
    .await
}

async fn count_missing_block_refs(conn: &Connection, block_uuids: Vec<String>) -> Result<u32> {
    conn.call(move |database| {
        let mut broken = 0;
        for uuid in block_uuids {
            if uuid.trim().is_empty() {
                continue;
            }
            let exists = database
                .query_row("SELECT 1 FROM nodes WHERE uuid = ?1", [&uuid], |_| Ok(()))
                .optional()?
                .is_some();
            if !exists {
                broken += 1;
            }
        }
        Ok(broken)
    })
    .await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub node: Node,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub src: i64,
    pub dst: i64,
    pub kind: String,
    pub weight: f64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataArchive {
    pub format: String,
    pub version: u32,
    pub exported_at: i64,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    #[serde(default)]
    pub entity_descriptions: Vec<EntityDescriptionRecord>,
    #[serde(default)]
    pub files: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityDescriptionRecord {
    pub source_node_id: i64,
    pub entity_node_id: i64,
    pub description: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphSnapshot {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

type AttachmentDeleteRecord = (Node, Option<String>, Option<String>);
type ReorderPlan = (String, Option<String>, Vec<(String, f64)>);

pub async fn create_node(
    conn: &Connection,
    kind: String,
    title: Option<String>,
    content: String,
    content_json: Option<String>,
) -> Result<Node> {
    if kind == "page"
        && let Some(title) = title.as_deref()
        && let Some(existing) = get_page_by_title(conn, title.to_string()).await?
    {
        return Ok(existing);
    }
    let uuid = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();
    apply_local(
        conn,
        vec![OpKind::NodeCreate(NodeCreate {
            uuid: uuid.clone(),
            node_kind: kind,
            title,
            content,
            content_json,
            parent_uuid: None,
            position: None,
            created_at: now,
        })],
    )
    .await?;
    get_node_by_uuid(conn, uuid)
        .await?
        .context("created node was not materialized")
}

pub async fn update_node(
    conn: &Connection,
    id: i64,
    title: Option<String>,
    content: String,
    content_json: Option<String>,
) -> Result<()> {
    let uuid = require_node_uuid(conn, id).await?;
    apply_local(
        conn,
        vec![
            OpKind::NodeSetTitle(NodeSetTitle {
                uuid: uuid.clone(),
                title,
            }),
            OpKind::NodeSetContent(NodeSetContent {
                uuid,
                content,
                content_json,
            }),
        ],
    )
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockContent {
    pub content: String,
    pub wikilink_titles: Vec<String>,
    pub block_uuids: Vec<String>,
}

pub async fn update_block_with_refs(
    conn: &Connection,
    id: i64,
    block: BlockContent,
) -> Result<(Node, u32)> {
    let node = get_node(conn, id)
        .await?
        .filter(|node| node.kind == "block")
        .context("atomic block save requires a block node")?;
    apply_local(
        conn,
        vec![OpKind::NodeSetContent(NodeSetContent {
            uuid: node.uuid.clone(),
            content: block.content,
            content_json: None,
        })],
    )
    .await?;
    let broken = count_missing_block_refs(conn, block.block_uuids).await?;
    let updated = get_node_by_uuid(conn, node.uuid)
        .await?
        .context("updated block disappeared")?;
    Ok((updated, broken))
}

/// Split one block into ordered siblings as a single transaction. Sibling
/// positions are normalized to wide integer gaps, preventing fractional
/// indexing from converging after repeated inserts.
pub async fn split_block(
    conn: &Connection,
    id: i64,
    parts: Vec<BlockContent>,
) -> Result<Vec<Node>> {
    if parts.is_empty() {
        anyhow::bail!("split requires at least one part");
    }
    let (source_uuid, parent_uuid, mut siblings) = conn
        .call(
            move |database| -> rusqlite::Result<(String, String, Vec<String>)> {
                let (source_uuid, parent_id, kind): (String, i64, String) = database.query_row(
                    "SELECT uuid, parent_id, kind FROM nodes WHERE id = ?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
                if kind != "block" {
                    return Err(rusqlite::Error::InvalidParameterName(
                        "only a block can be split".into(),
                    ));
                }
                let parent_uuid = database.query_row(
                    "SELECT uuid FROM nodes WHERE id = ?1",
                    [parent_id],
                    |row| row.get(0),
                )?;
                let siblings = {
                    let mut statement = database.prepare(
                        "SELECT uuid FROM nodes WHERE parent_id = ?1 ORDER BY position, uuid",
                    )?;
                    statement
                        .query_map([parent_id], |row| row.get::<_, String>(0))?
                        .collect::<Result<Vec<_>, _>>()?
                };
                Ok((source_uuid, parent_uuid, siblings))
            },
        )
        .await?;
    let insertion_index = siblings
        .iter()
        .position(|uuid| uuid == &source_uuid)
        .context("split source is missing from its siblings")?;
    let now = chrono::Utc::now().timestamp();
    let mut result_uuids = vec![source_uuid.clone()];
    let mut kinds = vec![OpKind::NodeSetContent(NodeSetContent {
        uuid: source_uuid,
        content: parts[0].content.clone(),
        content_json: None,
    })];
    for part in parts.into_iter().skip(1) {
        let uuid = uuid::Uuid::new_v4().to_string();
        result_uuids.push(uuid.clone());
        kinds.push(OpKind::NodeCreate(NodeCreate {
            uuid,
            node_kind: "block".into(),
            title: None,
            content: part.content,
            content_json: None,
            parent_uuid: Some(parent_uuid.clone()),
            position: Some(0.0),
            created_at: now,
        }));
    }
    siblings.splice(
        insertion_index + 1..insertion_index + 1,
        result_uuids[1..].iter().cloned(),
    );
    kinds.extend(siblings.into_iter().enumerate().map(|(index, uuid)| {
        OpKind::NodeMove(NodeMove {
            uuid,
            parent_uuid: Some(parent_uuid.clone()),
            position: (index as f64 + 1.0) * 1024.0,
        })
    }));
    apply_local(conn, kinds).await?;
    let mut nodes = Vec::with_capacity(result_uuids.len());
    for uuid in result_uuids {
        nodes.push(
            get_node_by_uuid(conn, uuid)
                .await?
                .context("split result disappeared")?,
        );
    }
    Ok(nodes)
}

pub async fn link_nodes(
    conn: &Connection,
    src: i64,
    dst: i64,
    kind: String,
    weight: f64,
) -> Result<()> {
    let (src_uuid, dst_uuid) = require_node_uuids(conn, src, dst).await?;
    apply_local(
        conn,
        vec![OpKind::EdgeAdd(EdgeAdd {
            src_uuid,
            dst_uuid,
            edge_kind: kind,
            weight,
        })],
    )
    .await?;
    Ok(())
}

/// Atomically replace every entity/relation edge produced from one source
/// note. `extracted_edge_sources` gives entity-to-entity relations provenance,
/// allowing later edits to remove relations that are no longer present.
pub async fn replace_extracted_edges(
    conn: &Connection,
    source_id: i64,
    expected_title: Option<String>,
    expected_content: String,
    entities: Vec<(String, Option<String>)>,
    relations: Vec<(String, String, String)>,
) -> Result<()> {
    conn.call(move |c| -> rusqlite::Result<()> {
        let tx = c.transaction()?;

        let source_unchanged = tx
            .query_row(
                "SELECT 1 FROM nodes
                 WHERE id = ?1 AND kind IN ('page', 'block')
                   AND title IS ?2 AND content = ?3",
                rusqlite::params![source_id, expected_title, expected_content],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !source_unchanged {
            return Err(rusqlite::Error::InvalidParameterName(
                "extraction source changed while the request was running".into(),
            ));
        }

        let mut affected_entity_ids = {
            let mut statement = tx.prepare(
                "SELECT DISTINCT e.dst
                   FROM extracted_edge_sources provenance
                   JOIN edges e ON e.id = provenance.edge_id
                  WHERE provenance.source_node_id = ?1 AND e.kind = 'mentions'",
            )?;
            statement
                .query_map([source_id], |row| row.get::<_, i64>(0))?
                .collect::<Result<std::collections::HashSet<_>, _>>()?
        };
        let old_edges = {
            let mut statement =
                tx.prepare("SELECT edge_id FROM extracted_edge_sources WHERE source_node_id = ?1")?;
            statement
                .query_map([source_id], |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        tx.execute(
            "DELETE FROM extracted_edge_sources WHERE source_node_id = ?1",
            [source_id],
        )?;
        tx.execute(
            "DELETE FROM entity_descriptions WHERE source_node_id = ?1",
            [source_id],
        )?;
        for edge_id in old_edges {
            tx.execute(
                "DELETE FROM edges
                   WHERE id = ?1
                     AND NOT EXISTS (
                       SELECT 1 FROM extracted_edge_sources WHERE edge_id = ?1
                     )",
                [edge_id],
            )?;
        }

        let now = chrono::Utc::now().timestamp();
        let mut entity_ids = std::collections::HashMap::new();
        for (raw_name, description) in entities {
            let name = raw_name.trim();
            if name.is_empty() {
                continue;
            }
            let existing = tx
                .query_row(
                    "SELECT id FROM nodes
                       WHERE kind = 'entity' AND lower(title) = lower(?1)
                       LIMIT 1",
                    [name],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?;
            let entity_id = if let Some(id) = existing {
                id
            } else {
                let uuid = uuid::Uuid::new_v4().to_string();
                let body = crate::stem::stem(name);
                tx.execute(
                    "INSERT INTO nodes
                       (uuid, kind, title, content, body_stemmed, created_at, updated_at)
                     VALUES (?1, 'entity', ?2, '', ?3, ?4, ?4)",
                    rusqlite::params![uuid, name, body, now],
                )?;
                tx.last_insert_rowid()
            };
            affected_entity_ids.insert(entity_id);
            if let Some(description) = description
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                tx.execute(
                    "INSERT INTO entity_descriptions
                       (source_node_id, entity_node_id, description, created_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(source_node_id, entity_node_id) DO UPDATE SET
                       description = excluded.description,
                       created_at = excluded.created_at",
                    rusqlite::params![source_id, entity_id, description, now],
                )?;
            }
            entity_ids.insert(name.to_lowercase(), entity_id);
            insert_extracted_edge(&tx, source_id, source_id, entity_id, "mentions", now)?;
        }

        for (raw_src, raw_dst, raw_kind) in relations {
            let Some(src) = entity_ids.get(&raw_src.trim().to_lowercase()).copied() else {
                continue;
            };
            let Some(dst) = entity_ids.get(&raw_dst.trim().to_lowercase()).copied() else {
                continue;
            };
            let kind = raw_kind.trim();
            if src != dst && !kind.is_empty() {
                insert_extracted_edge(&tx, source_id, src, dst, kind, now)?;
            }
        }

        for entity_id in affected_entity_ids {
            let title = tx.query_row(
                "SELECT title FROM nodes WHERE id = ?1",
                [entity_id],
                |row| row.get::<_, Option<String>>(0),
            )?;
            let description = tx
                .query_row(
                    "SELECT group_concat(description, '\n\n')
                       FROM (
                         SELECT description FROM entity_descriptions
                          WHERE entity_node_id = ?1
                          ORDER BY created_at DESC, source_node_id DESC
                          LIMIT 5
                       )",
                    [entity_id],
                    |row| row.get::<_, Option<String>>(0),
                )?
                .unwrap_or_default();
            let body = crate::stem::stem(&format!(
                "{}\n{description}",
                title.as_deref().unwrap_or("")
            ));
            tx.execute(
                "UPDATE nodes SET content = ?2, body_stemmed = ?3, updated_at = ?4
                   WHERE id = ?1",
                rusqlite::params![entity_id, description, body, now],
            )?;
        }

        tx.commit()?;
        Ok(())
    })
    .await
}

fn insert_extracted_edge(
    tx: &rusqlite::Transaction<'_>,
    source_id: i64,
    src: i64,
    dst: i64,
    kind: &str,
    now: i64,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO edges (src, dst, kind, weight, created_at)
         VALUES (?1, ?2, ?3, 1.0, ?4)
         ON CONFLICT(src, dst, kind) DO UPDATE SET weight = excluded.weight",
        rusqlite::params![src, dst, kind, now],
    )?;
    let edge_id = tx.query_row(
        "SELECT id FROM edges WHERE src = ?1 AND dst = ?2 AND kind = ?3",
        rusqlite::params![src, dst, kind],
        |row| row.get::<_, i64>(0),
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO extracted_edge_sources(source_node_id, edge_id)
         VALUES (?1, ?2)",
        rusqlite::params![source_id, edge_id],
    )?;
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
            let broken = replace_block_refs_tx(&tx, block_id, &wikilink_titles, &block_uuids)?;
            tx.commit()?;
            Ok(broken)
        })
        .await?;
    Ok(broken)
}

pub(crate) fn replace_block_refs_tx(
    tx: &rusqlite::Transaction<'_>,
    block_id: i64,
    wikilink_titles: &[String],
    block_uuids: &[String],
) -> rusqlite::Result<u32> {
    replace_block_refs_tx_at(
        tx,
        block_id,
        wikilink_titles,
        block_uuids,
        chrono::Utc::now().timestamp(),
    )
}

pub(crate) fn replace_block_refs_tx_at(
    tx: &rusqlite::Transaction<'_>,
    block_id: i64,
    wikilink_titles: &[String],
    block_uuids: &[String],
    now: i64,
) -> rusqlite::Result<u32> {
    tx.execute(
        "DELETE FROM edges WHERE src = ?1 AND kind = 'refs'",
        [block_id],
    )?;
    for raw_title in wikilink_titles {
        let title = raw_title.trim();
        if title.is_empty() {
            continue;
        }
        let existing = tx
            .query_row(
                "SELECT id FROM nodes
                 WHERE kind = 'page' AND lower(title) = lower(?1)
                 LIMIT 1",
                [title],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let page_id = match existing {
            Some(id) => id,
            None => {
                let uuid = page_uuid(title);
                let id = stable_node_id(&uuid)
                    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
                tx.execute(
                    "INSERT INTO nodes (id, uuid, kind, title, content, content_json,
                                        body_stemmed, parent_id, position,
                                        created_at, updated_at)
                     VALUES (?1, ?2, 'page', ?3, '', NULL, '', NULL, NULL, ?4, ?4)",
                    rusqlite::params![id, uuid, title, now],
                )?;
                id
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

    let mut broken = 0;
    for raw_uuid in block_uuids {
        let uuid = raw_uuid.trim();
        if uuid.is_empty() {
            continue;
        }
        let target = tx
            .query_row("SELECT id FROM nodes WHERE uuid = ?1", [uuid], |row| {
                row.get::<_, i64>(0)
            })
            .optional()?;
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
    Ok(broken)
}

pub(crate) fn page_uuid(title: &str) -> String {
    uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_OID,
        format!("notes-rs:page:{}", title.trim().to_lowercase()).as_bytes(),
    )
    .to_string()
}

pub(crate) fn stable_node_id(uuid: &str) -> Result<i64, uuid::Error> {
    let uuid = uuid::Uuid::parse_str(uuid)?;
    let mut high = [0_u8; 8];
    let mut low = [0_u8; 8];
    high.copy_from_slice(&uuid.as_bytes()[..8]);
    low.copy_from_slice(&uuid.as_bytes()[8..]);
    // Tauri sends IDs through JavaScript, so keep them inside Number's exact
    // integer range while retaining 53 bits of UUID-derived entropy.
    let value = (u64::from_be_bytes(high) ^ u64::from_be_bytes(low)) & ((1_u64 << 53) - 1);
    Ok(value.max(1) as i64)
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

pub async fn get_containing_page(conn: &Connection, id: i64) -> Result<Option<Node>> {
    conn.call(move |database| -> rusqlite::Result<Option<Node>> {
        let sql = format!(
            "WITH RECURSIVE ancestors(id, parent_id, kind) AS (
               SELECT id, parent_id, kind FROM nodes WHERE id = ?1
               UNION
               SELECT n.id, n.parent_id, n.kind
                 FROM nodes n JOIN ancestors a ON n.id = a.parent_id
             )
             SELECT {NODE_COLUMNS} FROM nodes
             WHERE id IN (SELECT id FROM ancestors WHERE kind = 'page')
             LIMIT 1"
        );
        let mut statement = database.prepare(&sql)?;
        let mut rows = statement.query([id])?;
        rows.next()?.map(row_to_node).transpose()
    })
    .await
}

pub async fn neighbors(conn: &Connection, node_id: i64, depth: u32) -> Result<Vec<Node>> {
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

/// Nodes that link *to* `node_id` via an outgoing edge. Unlike `neighbors`
/// (which is undirected), this is one-directional: it answers "who points
/// at me?". Optional `kind` filter narrows to e.g. only `refs` for explicit
/// wikilinks/block-refs, or `mentions` for entity links.
pub async fn find_backlinks(
    conn: &Connection,
    node_id: i64,
    kind: Option<String>,
) -> Result<Vec<Node>> {
    let rows = conn
        .call(move |c| -> rusqlite::Result<Vec<Node>> {
            let sql = if kind.is_some() {
                format!(
                    "SELECT {NODE_COLUMNS_N} FROM nodes n
                     JOIN edges e ON e.src = n.id
                     WHERE e.dst = ?1 AND e.kind = ?2
                     ORDER BY n.updated_at DESC"
                )
            } else {
                format!(
                    "SELECT {NODE_COLUMNS_N} FROM nodes n
                     JOIN edges e ON e.src = n.id
                     WHERE e.dst = ?1
                     ORDER BY n.updated_at DESC"
                )
            };
            let mut stmt = c.prepare(&sql)?;
            let rows = match kind {
                Some(k) => stmt
                    .query_map(rusqlite::params![node_id, k], row_to_node)?
                    .collect::<Result<Vec<_>, _>>()?,
                None => stmt
                    .query_map([node_id], row_to_node)?
                    .collect::<Result<Vec<_>, _>>()?,
            };
            Ok(rows)
        })
        .await?;
    Ok(rows)
}

/// Walk the parent chain from `node_id` up to the root. Returns root-first so
/// the LLM reads it like a breadcrumb. The starting node itself is included
/// as the last element.
pub async fn read_ancestors(conn: &Connection, node_id: i64) -> Result<Vec<Node>> {
    let rows = conn
        .call(move |c| -> rusqlite::Result<Vec<Node>> {
            let sql = format!(
                "WITH RECURSIVE up(id, parent_id, depth) AS (
                   SELECT id, parent_id, 0 FROM nodes WHERE id = ?1
                   UNION ALL
                   SELECT n.id, n.parent_id, u.depth + 1
                     FROM up u JOIN nodes n ON n.id = u.parent_id
                 )
                 SELECT {NODE_COLUMNS_N} FROM up u
                 JOIN nodes n ON n.id = u.id
                 ORDER BY u.depth DESC"
            );
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt
                .query_map([node_id], row_to_node)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(rows)
}

/// All descendants of `node_id` up to `depth` levels. Returned in pre-order
/// DFS via a lexicographic sort path so the result reads like an outline:
/// each block immediately followed by its own children. Excludes `node_id`
/// itself.
pub async fn read_subtree(conn: &Connection, node_id: i64, depth: u32) -> Result<Vec<Node>> {
    let rows = conn
        .call(move |c| -> rusqlite::Result<Vec<Node>> {
            // sort_path is zero-padded floats joined by '/', so lexicographic
            // sort matches the outline order. We assume non-negative
            // positions, which holds for blocks created via create_block /
            // move_block.
            let sql = format!(
                "WITH RECURSIVE down(id, sort_path, depth) AS (
                   SELECT id, '', 0 FROM nodes WHERE id = ?1
                   UNION ALL
                   SELECT n.id,
                          d.sort_path || '/' ||
                            printf('%015.6f', COALESCE(n.position, 0.0)),
                          d.depth + 1
                     FROM down d JOIN nodes n ON n.parent_id = d.id
                     WHERE d.depth < ?2
                 )
                 SELECT {NODE_COLUMNS_N} FROM down d
                 JOIN nodes n ON n.id = d.id
                 WHERE n.id != ?1
                 ORDER BY d.sort_path"
            );
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt
                .query_map(rusqlite::params![node_id, depth as i64], row_to_node)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(rows)
}

/// Nodes that reference the given tag/entity by its (case-insensitive) title.
/// Matches both `kind='tag'` and `kind='entity'` since the extractor emits
/// entities, while user-typed `#tags` would become tag rows when that path
/// is added.
pub async fn find_tagged(conn: &Connection, title: String) -> Result<Vec<Node>> {
    let rows = conn
        .call(move |c| -> rusqlite::Result<Vec<Node>> {
            let sql = format!(
                "SELECT {NODE_COLUMNS_N} FROM nodes n
                 WHERE EXISTS (
                   SELECT 1 FROM edges e
                   JOIN nodes t ON t.id = e.dst
                   WHERE e.src = n.id
                     AND e.kind IN ('mentions', 'refs')
                     AND t.kind IN ('tag', 'entity')
                     AND lower(t.title) = lower(?1)
                 )
                 ORDER BY n.updated_at DESC"
            );
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt
                .query_map([title], row_to_node)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(rows)
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

/// Case-insensitive substring match on page titles, shortest-first so exact
/// matches surface above the long ones. Used by the `[[` autocomplete.
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

pub async fn list_block_children(conn: &Connection, parent_id: i64) -> Result<Vec<Node>> {
    let rows = conn
        .call(move |c| -> rusqlite::Result<Vec<Node>> {
            let sql = format!(
                "SELECT {NODE_COLUMNS} FROM nodes
                 WHERE parent_id = ?1
                 ORDER BY position ASC, uuid ASC"
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

pub async fn delete_page(conn: &Connection, id: i64) -> Result<Option<Vec<Node>>> {
    let Some((uuids, attachments)) = conn
        .call(
            move |database| -> rusqlite::Result<Option<(Vec<String>, Vec<Node>)>> {
                let exists = database
                    .query_row(
                        "SELECT 1 FROM nodes WHERE id = ?1 AND kind = 'page'",
                        [id],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
                if !exists {
                    return Ok(None);
                }
                let uuids = {
                    let mut statement = database.prepare(
                        "WITH RECURSIVE subtree(id) AS (
                       SELECT ?1
                       UNION ALL
                       SELECT n.id FROM nodes n JOIN subtree s ON n.parent_id = s.id
                     )
                     SELECT uuid FROM nodes WHERE id IN (SELECT id FROM subtree)
                     ORDER BY id DESC",
                    )?;
                    statement
                        .query_map([id], |row| row.get::<_, String>(0))?
                        .collect::<Result<Vec<_>, _>>()?
                };
                let attachments = {
                    let sql = format!(
                        "WITH RECURSIVE subtree(id) AS (
                       SELECT ?1
                       UNION ALL
                       SELECT n.id FROM nodes n JOIN subtree s ON n.parent_id = s.id
                     )
                     SELECT DISTINCT {NODE_COLUMNS_N} FROM nodes n
                     JOIN edges e ON e.dst = n.id
                     WHERE e.kind = 'attachment' AND e.src IN (SELECT id FROM subtree)"
                    );
                    let mut statement = database.prepare(&sql)?;
                    statement
                        .query_map([id], row_to_node)?
                        .collect::<Result<Vec<_>, _>>()?
                };
                Ok(Some((uuids, attachments)))
            },
        )
        .await?
    else {
        return Ok(None);
    };
    let mut kinds = attachments
        .iter()
        .map(|attachment| {
            OpKind::NodeDelete(NodeDelete {
                uuid: attachment.uuid.clone(),
            })
        })
        .collect::<Vec<_>>();
    kinds.extend(
        uuids
            .into_iter()
            .map(|uuid| OpKind::NodeDelete(NodeDelete { uuid })),
    );
    apply_local(conn, kinds).await?;
    Ok(Some(attachments))
}

pub async fn graph_snapshot(conn: &Connection, focus_id: Option<i64>) -> Result<GraphSnapshot> {
    conn.call(move |database| -> rusqlite::Result<GraphSnapshot> {
        let nodes = if let Some(focus_id) = focus_id {
            let sql = format!(
                "SELECT DISTINCT {NODE_COLUMNS} FROM nodes n
                 WHERE n.id = ?1
                    OR n.id IN (SELECT src FROM edges WHERE dst = ?1)
                    OR n.id IN (SELECT dst FROM edges WHERE src = ?1)
                    OR n.parent_id = ?1
                    OR n.id = (SELECT parent_id FROM nodes WHERE id = ?1)
                 ORDER BY n.kind, n.title, n.id
                 LIMIT 100"
            );
            let mut statement = database.prepare(&sql)?;
            statement
                .query_map([focus_id], row_to_node)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            let sql = format!(
                "SELECT {NODE_COLUMNS} FROM nodes
                 WHERE kind IN ('page', 'entity')
                 ORDER BY updated_at DESC LIMIT 100"
            );
            let mut statement = database.prepare(&sql)?;
            statement
                .query_map([], row_to_node)?
                .collect::<Result<Vec<_>, _>>()?
        };
        let ids = nodes.iter().map(|node| node.id).collect::<Vec<_>>();
        let edges = if ids.is_empty() {
            Vec::new()
        } else {
            let placeholders = std::iter::repeat_n("?", ids.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT src, dst, kind, weight, created_at FROM edges
                 WHERE src IN ({placeholders}) AND dst IN ({placeholders})
                 ORDER BY created_at DESC LIMIT 250"
            );
            let parameters = ids.iter().chain(ids.iter());
            let mut statement = database.prepare(&sql)?;
            statement
                .query_map(rusqlite::params_from_iter(parameters), |row| {
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
        Ok(GraphSnapshot { nodes, edges })
    })
    .await
}

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
                        |row| row.get::<_, String>(0),
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
        OpKind::NodeDelete(NodeDelete {
            uuid: node.uuid.clone(),
        })
    };
    apply_local(conn, vec![kind]).await?;
    Ok(Some(node))
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
                node_kind: target_node.kind.clone(),
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
        if target_node.kind != "block" {
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
    let (parent_uuid, position) = conn
        .call(move |database| -> rusqlite::Result<(Option<String>, f64)> {
            let parent_uuid = parent_id
                .map(|parent_id| {
                    database.query_row(
                        "SELECT uuid FROM nodes WHERE id = ?1 AND kind IN ('page', 'block')",
                        [parent_id],
                        |row| row.get::<_, String>(0),
                    )
                })
                .transpose()?;
            let position = match position {
                Some(position) => position,
                None => match parent_id {
                    Some(parent_id) => database.query_row(
                        "SELECT COALESCE(MAX(position), 0.0) + 1024.0
                         FROM nodes WHERE parent_id = ?1",
                        [parent_id],
                        |row| row.get(0),
                    )?,
                    None => 1024.0,
                },
            };
            Ok((parent_uuid, position))
        })
        .await?;
    apply_local(
        conn,
        vec![OpKind::NodeCreate(NodeCreate {
            uuid: uuid.clone(),
            node_kind: "block".into(),
            title: None,
            content,
            content_json,
            parent_uuid,
            position: Some(position),
            created_at: now,
        })],
    )
    .await?;
    get_node_by_uuid(conn, uuid)
        .await?
        .context("created block was not materialized")
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
    let (uuid, parent_uuid, position) = conn
        .call(
            move |database| -> rusqlite::Result<(String, Option<String>, f64)> {
                let (uuid, kind): (String, String) = database.query_row(
                    "SELECT uuid, kind FROM nodes WHERE id = ?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                if kind != "block" {
                    return Err(rusqlite::Error::InvalidParameterName(
                        "only block nodes can be moved".into(),
                    ));
                }
                if new_parent_id == Some(id) {
                    return Err(rusqlite::Error::InvalidParameterName(
                        "a block cannot be its own parent".into(),
                    ));
                }
                let parent_uuid = new_parent_id
                    .map(|parent_id| {
                        let creates_cycle: bool = database.query_row(
                            "WITH RECURSIVE descendants(id) AS (
                           SELECT id FROM nodes WHERE parent_id = ?1
                           UNION ALL
                           SELECT n.id FROM nodes n JOIN descendants d ON n.parent_id = d.id
                         )
                         SELECT EXISTS(SELECT 1 FROM descendants WHERE id = ?2)",
                            rusqlite::params![id, parent_id],
                            |row| row.get(0),
                        )?;
                        if creates_cycle {
                            return Err(rusqlite::Error::InvalidParameterName(
                                "a block cannot be moved under one of its descendants".into(),
                            ));
                        }
                        database.query_row(
                            "SELECT uuid FROM nodes WHERE id = ?1 AND kind IN ('page', 'block')",
                            [parent_id],
                            |row| row.get::<_, String>(0),
                        )
                    })
                    .transpose()?;
                let position = match new_position {
                    Some(position) => position,
                    None => match new_parent_id {
                        Some(parent_id) => database.query_row(
                            "SELECT COALESCE(MAX(position), 0.0) + 1024.0
                         FROM nodes WHERE parent_id = ?1",
                            [parent_id],
                            |row| row.get(0),
                        )?,
                        None => 1024.0,
                    },
                };
                Ok((uuid, parent_uuid, position))
            },
        )
        .await?;
    apply_local(
        conn,
        vec![OpKind::NodeMove(NodeMove {
            uuid: uuid.clone(),
            parent_uuid,
            position,
        })],
    )
    .await?;
    get_node_by_uuid(conn, uuid)
        .await?
        .context("moved block disappeared")
}

pub async fn reorder_block(conn: &Connection, id: i64, direction: String) -> Result<Node> {
    let (uuid, parent_uuid, moves) = conn.call(move |database| -> rusqlite::Result<ReorderPlan> {
        let parent_id: Option<i64> = database.query_row(
            "SELECT parent_id FROM nodes WHERE id = ?1 AND kind = 'block'",
            [id],
            |row| row.get(0),
        )?;
        let mut siblings = {
            let mut statement = database
                .prepare("SELECT uuid, position FROM nodes WHERE parent_id IS ?1 ORDER BY position, uuid")?;
            statement
                .query_map([parent_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?
        };
        let uuid: String = database.query_row("SELECT uuid FROM nodes WHERE id = ?1", [id], |row| row.get(0))?;
        let index = siblings
            .iter()
            .position(|(sibling_uuid, _)| *sibling_uuid == uuid)
            .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        let target = match direction.as_str() {
            "up" if index > 0 => Some(index - 1),
            "down" if index + 1 < siblings.len() => Some(index + 1),
            "up" | "down" => None,
            _ => {
                return Err(rusqlite::Error::InvalidParameterName(
                    "direction must be up or down".into(),
                ));
            }
        };
        let mut moves = Vec::new();
        if let Some(target) = target {
            siblings.swap(index, target);
            for (new_index, (sibling_uuid, _)) in siblings.into_iter().enumerate() {
                moves.push((sibling_uuid, (new_index as f64 + 1.0) * 1024.0));
            }
        }
        let parent_uuid = parent_id.map(|parent_id| database.query_row("SELECT uuid FROM nodes WHERE id = ?1", [parent_id], |row| row.get::<_, String>(0))).transpose()?;
        Ok((uuid, parent_uuid, moves))
    }).await?;
    let kinds = moves
        .into_iter()
        .map(|(uuid, position)| {
            OpKind::NodeMove(NodeMove {
                uuid,
                parent_uuid: parent_uuid.clone(),
                position,
            })
        })
        .collect();
    apply_local(conn, kinds).await?;
    get_node_by_uuid(conn, uuid)
        .await?
        .context("reordered block disappeared")
}

/// Delete a block. Refuses (returns `false`) if the block has children, so
/// callers can show feedback instead of silently cascading. Returns `true` on
/// successful delete.
pub async fn delete_block(conn: &Connection, id: i64) -> Result<bool> {
    let uuid = conn
        .call(move |c| -> rusqlite::Result<Option<String>> {
            let kids: i64 = c.query_row(
                "SELECT COUNT(*) FROM nodes WHERE parent_id = ?1",
                [id],
                |r| r.get(0),
            )?;
            if kids > 0 {
                return Ok(None);
            }
            c.query_row(
                "SELECT uuid FROM nodes WHERE id = ?1 AND kind = 'block'",
                [id],
                |row| row.get(0),
            )
            .optional()
        })
        .await?;
    let Some(uuid) = uuid else {
        return Ok(false);
    };
    apply_local(conn, vec![OpKind::NodeDelete(NodeDelete { uuid })]).await?;
    Ok(true)
}

/// Find a page by case-insensitive title without creating one. Used by hover
/// previews / autocomplete so we don't spawn stubs on every hover.
pub async fn get_page_by_title(conn: &Connection, title: String) -> Result<Option<Node>> {
    let trimmed = title.trim().to_string();
    if trimmed.is_empty() {
        return Ok(None);
    }
    conn.call(move |c| -> rusqlite::Result<Option<Node>> {
        let sql = format!(
            "SELECT {NODE_COLUMNS} FROM nodes
             WHERE kind = 'page' AND lower(title) = lower(?1)
             LIMIT 1"
        );
        let mut stmt = c.prepare(&sql)?;
        let mut rows = stmt.query([&trimmed])?;
        if let Some(r) = rows.next()? {
            Ok(Some(row_to_node(r)?))
        } else {
            Ok(None)
        }
    })
    .await
}

pub async fn get_node_by_uuid(conn: &Connection, uuid: String) -> Result<Option<Node>> {
    let trimmed = uuid.trim().to_string();
    if trimmed.is_empty() {
        return Ok(None);
    }
    conn.call(move |c| -> rusqlite::Result<Option<Node>> {
        let sql = format!("SELECT {NODE_COLUMNS} FROM nodes WHERE uuid = ?1 LIMIT 1");
        let mut stmt = c.prepare(&sql)?;
        let mut rows = stmt.query([&trimmed])?;
        if let Some(r) = rows.next()? {
            Ok(Some(row_to_node(r)?))
        } else {
            Ok(None)
        }
    })
    .await
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

/// Pull the next batch from the embed queue, packaging each row with its
/// ancestor-chain context. A block embedded in isolation often loses meaning
/// ("yeah, that fits") — bundling the breadcrumb of ancestor titles plus the
/// direct parent's content gives the embedder enough signal to disambiguate.
type EmbedChainRow = (i64, i32, Option<String>, String);

pub const EMBED_MAX_ATTEMPTS: i64 = 8;
pub const EMBED_BACKOFF_BASE_SECS: i64 = 5;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundFailure {
    pub queue: String,
    pub node_id: i64,
    pub node_title: Option<String>,
    pub retry_count: i64,
    pub last_attempt: Option<i64>,
    pub failure_kind: String,
    pub last_error: String,
    pub terminal: bool,
}

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
    failure_kind: &str,
    error: &str,
    terminal: bool,
) -> Result<()> {
    if node_ids.is_empty() {
        return Ok(());
    }
    let failure_kind = failure_kind.to_string();
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
    failure_kind: &str,
    error: &str,
    terminal: bool,
) -> Result<()> {
    let failure_kind = failure_kind.to_string();
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn temporary_database() -> (tempfile::NamedTempFile, Connection) {
        let database = tempfile::NamedTempFile::new().expect("create temporary database");
        let connection = open(database.path(), "test:4d", 4)
            .await
            .expect("open temporary database");
        (database, connection)
    }

    #[tokio::test]
    async fn migrates_legacy_nodes_before_creating_new_indexes_and_triggers() {
        let database = tempfile::NamedTempFile::new().expect("create temporary database");
        let legacy = rusqlite::Connection::open(database.path()).expect("open legacy database");
        legacy
            .execute_batch(
                "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
                 INSERT INTO schema_version VALUES (1);
                 CREATE TABLE nodes (
                   id INTEGER PRIMARY KEY,
                   uuid TEXT UNIQUE NOT NULL,
                   kind TEXT NOT NULL,
                   title TEXT,
                   content TEXT NOT NULL DEFAULT '',
                   created_at INTEGER NOT NULL,
                   updated_at INTEGER NOT NULL
                 );",
            )
            .expect("create legacy schema");
        drop(legacy);

        let connection = open(database.path(), "test:4d", 4)
            .await
            .expect("migrate legacy database");
        let columns: Vec<String> = connection
            .call(|db| {
                let mut statement = db.prepare("SELECT name FROM pragma_table_info('nodes')")?;
                statement.query_map([], |row| row.get(0))?.collect()
            })
            .await
            .expect("read migrated columns");

        for expected in [
            "content_json",
            "body_stemmed",
            "parent_id",
            "position",
            "last_extracted_hash",
        ] {
            assert!(columns.iter().any(|column| column == expected));
        }
    }

    #[tokio::test]
    async fn refuses_to_move_a_block_into_its_descendant() {
        let (_database, connection) = temporary_database().await;
        let page = create_node(
            &connection,
            "page".into(),
            Some("Page".into()),
            String::new(),
            None,
        )
        .await
        .expect("create page");
        let parent = create_block(&connection, Some(page.id), None, "parent".into(), None)
            .await
            .expect("create parent");
        let child = create_block(&connection, Some(parent.id), None, "child".into(), None)
            .await
            .expect("create child");

        let error = move_block(&connection, parent.id, Some(child.id), None)
            .await
            .expect_err("cycle must be rejected");
        assert!(error.to_string().contains("descendants"));
    }

    #[tokio::test]
    async fn natural_language_fts_search_treats_punctuation_as_text() {
        let (_database, connection) = temporary_database().await;
        let block = create_node(
            &connection,
            "block".into(),
            None,
            "Offline-first Rust notebook".into(),
            None,
        )
        .await
        .expect("create searchable block");

        let hits = search_fts(&connection, "offline-first? Rust".into(), 10)
            .await
            .expect("punctuation must not become FTS syntax");
        assert!(hits.iter().any(|hit| hit.node.id == block.id));

        assert!(
            search_fts(&connection, "???".into(), 10)
                .await
                .expect("punctuation-only search")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn replacing_extraction_removes_stale_mentions_and_relations() {
        let (_database, connection) = temporary_database().await;
        let source = create_node(
            &connection,
            "page".into(),
            Some("Source".into()),
            String::new(),
            None,
        )
        .await
        .expect("create source");
        replace_extracted_edges(
            &connection,
            source.id,
            source.title.clone(),
            source.content.clone(),
            vec![
                ("Rust".into(), Some("language".into())),
                ("Tauri".into(), Some("framework".into())),
            ],
            vec![("Tauri".into(), "Rust".into(), "uses".into())],
        )
        .await
        .expect("apply extraction");

        replace_extracted_edges(
            &connection,
            source.id,
            source.title.clone(),
            source.content.clone(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("clear extraction");
        let generated_edges: i64 = connection
            .call(|database| {
                database.query_row(
                    "SELECT COUNT(*) FROM edges
                     WHERE kind = 'mentions' OR kind = 'uses'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("count generated edges");
        assert_eq!(generated_edges, 0);
    }

    #[tokio::test]
    async fn entity_descriptions_keep_source_provenance() {
        let (_database, connection) = temporary_database().await;
        let first = create_node(
            &connection,
            "page".into(),
            Some("First".into()),
            "Rust note".into(),
            None,
        )
        .await
        .expect("create first source");
        let second = create_node(
            &connection,
            "page".into(),
            Some("Second".into()),
            "Another Rust note".into(),
            None,
        )
        .await
        .expect("create second source");

        for (source, description) in [
            (&first, "A systems language"),
            (&second, "A memory-safe language"),
        ] {
            replace_extracted_edges(
                &connection,
                source.id,
                source.title.clone(),
                source.content.clone(),
                vec![("Rust".into(), Some(description.into()))],
                Vec::new(),
            )
            .await
            .expect("apply extraction");
        }

        let content = connection
            .call(|database| {
                database.query_row(
                    "SELECT content FROM nodes WHERE kind = 'entity' AND title = 'Rust'",
                    [],
                    |row| row.get::<_, String>(0),
                )
            })
            .await
            .expect("read merged entity description");
        assert!(content.contains("A systems language"));
        assert!(content.contains("A memory-safe language"));

        replace_extracted_edges(
            &connection,
            second.id,
            second.title.clone(),
            second.content.clone(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("clear second extraction");
        let content = connection
            .call(|database| {
                database.query_row(
                    "SELECT content FROM nodes WHERE kind = 'entity' AND title = 'Rust'",
                    [],
                    |row| row.get::<_, String>(0),
                )
            })
            .await
            .expect("read remaining entity description");
        assert_eq!(content, "A systems language");
    }

    #[tokio::test]
    async fn stale_extraction_cannot_attach_entities_to_a_changed_source() {
        let (_database, connection) = temporary_database().await;
        let source = create_node(
            &connection,
            "page".into(),
            Some("Before".into()),
            "old content".into(),
            None,
        )
        .await
        .expect("create source");
        update_node(
            &connection,
            source.id,
            Some("After".into()),
            "new content".into(),
            None,
        )
        .await
        .expect("change source");

        let error = replace_extracted_edges(
            &connection,
            source.id,
            source.title,
            source.content,
            vec![("Stale entity".into(), None)],
            Vec::new(),
        )
        .await
        .expect_err("stale result must be rejected");
        assert!(error.to_string().contains("source changed"));
        assert!(
            get_page_by_title(&connection, "Stale entity".into())
                .await
                .expect("query stale entity")
                .is_none()
        );
        let entity_count: i64 = connection
            .call(|database| {
                database.query_row(
                    "SELECT COUNT(*) FROM nodes WHERE kind = 'entity'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("count entities");
        assert_eq!(entity_count, 0);
    }

    #[tokio::test]
    async fn split_block_is_atomic_and_normalizes_positions() {
        let (_database, connection) = temporary_database().await;
        let page = create_node(
            &connection,
            "page".into(),
            Some("Page".into()),
            String::new(),
            None,
        )
        .await
        .expect("create page");
        let block = create_block(&connection, Some(page.id), None, "old".into(), None)
            .await
            .expect("create block");
        let parts = vec![
            BlockContent {
                content: "first [[Linked]]".into(),
                wikilink_titles: vec!["Linked".into()],
                block_uuids: Vec::new(),
            },
            BlockContent {
                content: "second".into(),
                wikilink_titles: Vec::new(),
                block_uuids: Vec::new(),
            },
        ];
        let changed = split_block(&connection, block.id, parts)
            .await
            .expect("split block");
        assert_eq!(changed.len(), 2);
        assert_eq!(changed[0].content, "first [[Linked]]");
        assert_eq!(changed[1].content, "second");
        assert_eq!(changed[0].position, Some(1024.0));
        assert_eq!(changed[1].position, Some(2048.0));

        let refs: i64 = connection
            .call(move |database| {
                database.query_row(
                    "SELECT COUNT(*) FROM edges WHERE src = ?1 AND kind = 'refs'",
                    [block.id],
                    |row| row.get(0),
                )
            })
            .await
            .expect("count refs");
        assert_eq!(refs, 1);
    }

    #[tokio::test]
    async fn reorder_block_swaps_siblings_without_fractional_positions() {
        let (_database, connection) = temporary_database().await;
        let page = create_node(
            &connection,
            "page".into(),
            Some("Page".into()),
            String::new(),
            None,
        )
        .await
        .expect("create page");
        let first = create_block(&connection, Some(page.id), None, "first".into(), None)
            .await
            .expect("create first");
        let second = create_block(&connection, Some(page.id), None, "second".into(), None)
            .await
            .expect("create second");

        reorder_block(&connection, second.id, "up".into())
            .await
            .expect("reorder");
        let siblings = list_block_children(&connection, page.id)
            .await
            .expect("list siblings");
        assert_eq!(
            siblings.iter().map(|node| node.id).collect::<Vec<_>>(),
            vec![second.id, first.id]
        );
        assert_eq!(siblings[0].position, Some(1024.0));
        assert_eq!(siblings[1].position, Some(2048.0));
    }

    #[tokio::test]
    async fn archive_round_trip_restores_nodes_edges_and_hierarchy() {
        let (_database, connection) = temporary_database().await;
        let page = create_node(
            &connection,
            "page".into(),
            Some("Original".into()),
            String::new(),
            None,
        )
        .await
        .expect("create page");
        let block = create_block(
            &connection,
            Some(page.id),
            None,
            "A [[Linked]] block".into(),
            None,
        )
        .await
        .expect("create block");
        replace_block_refs(&connection, block.id, vec!["Linked".into()], Vec::new())
            .await
            .expect("create reference");
        let archive = export_archive(&connection).await.expect("export archive");

        delete_page(&connection, page.id)
            .await
            .expect("delete page");
        import_archive(&connection, archive)
            .await
            .expect("restore archive");

        let restored = get_node_by_uuid(&connection, block.uuid.clone())
            .await
            .expect("query block")
            .expect("restored block");
        let restored_page = get_node_by_uuid(&connection, page.uuid)
            .await
            .expect("query restored page")
            .expect("restored page");
        assert_eq!(restored.parent_id, Some(restored_page.id));
        let linked = get_page_by_title(&connection, "Linked".into())
            .await
            .expect("query linked page")
            .expect("linked page restored");
        let backlinks = find_backlinks(&connection, linked.id, Some("refs".into()))
            .await
            .expect("query backlinks");
        assert_eq!(backlinks.len(), 1);
        assert_eq!(backlinks[0].uuid, block.uuid);
    }

    #[tokio::test]
    async fn explicit_page_creation_reuses_wikilink_stub_identity() {
        let (_database, connection) = temporary_database().await;
        let page = create_node(
            &connection,
            "page".into(),
            Some("Source".into()),
            String::new(),
            None,
        )
        .await
        .expect("create source page");
        let block = create_block(
            &connection,
            Some(page.id),
            None,
            "See [[Roadmap]]".into(),
            None,
        )
        .await
        .expect("create linked block");
        let stub = get_page_by_title(&connection, "Roadmap".into())
            .await
            .expect("query stub")
            .expect("stub page");
        let explicit = create_node(
            &connection,
            "page".into(),
            Some("Roadmap".into()),
            String::new(),
            None,
        )
        .await
        .expect("materialize explicit page");
        assert_eq!(explicit.uuid, stub.uuid);
        assert_eq!(
            find_backlinks(&connection, explicit.id, Some("refs".into()))
                .await
                .expect("backlinks")[0]
                .uuid,
            block.uuid
        );
    }

    #[tokio::test]
    async fn renamed_page_does_not_reserve_its_old_title_identity() {
        let (_database, connection) = temporary_database().await;
        let original = create_node(
            &connection,
            "page".into(),
            Some("Old title".into()),
            String::new(),
            None,
        )
        .await
        .expect("create original page");
        update_node(
            &connection,
            original.id,
            Some("New title".into()),
            String::new(),
            None,
        )
        .await
        .expect("rename page");
        let replacement = create_node(
            &connection,
            "page".into(),
            Some("Old title".into()),
            String::new(),
            None,
        )
        .await
        .expect("reuse old title");
        assert_ne!(replacement.uuid, original.uuid);
    }

    #[tokio::test]
    async fn attachments_follow_their_page_into_deletion() {
        let (_database, connection) = temporary_database().await;
        let page = create_node(
            &connection,
            "page".into(),
            Some("Page".into()),
            String::new(),
            None,
        )
        .await
        .expect("create page");
        let attachment = create_attachment(
            &connection,
            page.id,
            "attachment-hash".into(),
            "photo.png".into(),
            "image/png".into(),
            4,
        )
        .await
        .expect("create attachment");
        assert_eq!(
            list_attachments(&connection, page.id)
                .await
                .expect("list attachments")
                .len(),
            1
        );

        let removed = delete_page(&connection, page.id)
            .await
            .expect("delete page")
            .expect("page existed");
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].id, attachment.id);
        assert!(
            get_node(&connection, attachment.id)
                .await
                .expect("query attachment")
                .is_none()
        );
    }

    #[tokio::test]
    async fn background_queue_controls_report_retry_and_clear_work() {
        let (_database, connection) = temporary_database().await;
        let page = create_node(
            &connection,
            "page".into(),
            Some("Queued".into()),
            "content".into(),
            None,
        )
        .await
        .expect("create queued page");
        record_embedding_failure(
            &connection,
            vec![page.id],
            "network",
            "connection timed out",
            false,
        )
        .await
        .expect("record embedding failure");
        record_extraction_failure(
            &connection,
            page.id,
            "provider_request",
            "model unavailable",
            true,
        )
        .await
        .expect("record extraction failure");
        let status = queue_status(&connection).await.expect("read queue status");
        assert_eq!(status.embeddings_pending, 1);
        assert_eq!(status.embeddings_failed, 1);
        assert_eq!(status.extractions_pending, 1);
        assert_eq!(status.extractions_failed, 1);
        assert_eq!(status.failures.len(), 2);
        assert!(
            status
                .failures
                .iter()
                .any(|failure| failure.terminal && failure.last_error == "model unavailable")
        );

        retry_background_jobs(&connection)
            .await
            .expect("retry queues");
        let status = queue_status(&connection)
            .await
            .expect("read retried queues");
        assert_eq!(status.embeddings_pending, 1);
        assert_eq!(status.embeddings_failed, 0);
        assert_eq!(status.extractions_pending, 1);
        assert_eq!(status.extractions_failed, 0);
        assert!(status.failures.is_empty());

        record_extraction_failure(&connection, page.id, "schema", "invalid JSON", false)
            .await
            .expect("record first schema mismatch");
        record_extraction_failure(&connection, page.id, "schema", "invalid JSON again", false)
            .await
            .expect("record second schema mismatch");
        let status = queue_status(&connection)
            .await
            .expect("read terminal schema failure");
        assert!(status.failures[0].terminal);
        assert_eq!(status.failures[0].failure_kind, "schema");

        clear_background_jobs(&connection)
            .await
            .expect("clear queues");
        let status = queue_status(&connection)
            .await
            .expect("read cleared queues");
        assert_eq!(status.embeddings_pending, 0);
        assert_eq!(status.embeddings_failed, 0);
        assert_eq!(status.extractions_pending, 0);
        assert_eq!(status.extractions_failed, 0);
        assert!(status.failures.is_empty());
    }

    #[tokio::test]
    async fn structural_history_undoes_and_redoes_database_changes() {
        let (_database, connection) = temporary_database().await;
        let first = create_node(
            &connection,
            "page".into(),
            Some("First".into()),
            String::new(),
            None,
        )
        .await
        .expect("create first page");
        checkpoint_history(&connection, "create second")
            .await
            .expect("checkpoint");
        let second = create_node(
            &connection,
            "page".into(),
            Some("Second".into()),
            String::new(),
            None,
        )
        .await
        .expect("create second page");
        let before_undo: i64 = connection
            .call(|database| {
                database.query_row("SELECT COUNT(*) FROM applied_ops", [], |row| row.get(0))
            })
            .await
            .expect("count operations before undo");

        assert!(undo_history(&connection).await.expect("undo"));
        let after_undo: i64 = connection
            .call(|database| {
                database.query_row("SELECT COUNT(*) FROM applied_ops", [], |row| row.get(0))
            })
            .await
            .expect("count operations after undo");
        assert!(
            after_undo > before_undo,
            "undo must apply inverse operations"
        );
        assert!(
            get_node(&connection, first.id)
                .await
                .expect("first query")
                .is_some()
        );
        assert!(
            get_node(&connection, second.id)
                .await
                .expect("second query")
                .is_none()
        );
        assert_eq!(
            history_status(&connection).await.expect("undo status"),
            (0, 1)
        );

        assert!(redo_history(&connection).await.expect("redo"));
        let after_redo: i64 = connection
            .call(|database| {
                database.query_row("SELECT COUNT(*) FROM applied_ops", [], |row| row.get(0))
            })
            .await
            .expect("count operations after redo");
        assert!(
            after_redo > after_undo,
            "redo must apply inverse operations"
        );
        assert!(
            get_node(&connection, second.id)
                .await
                .expect("second query")
                .is_some()
        );
        assert_eq!(
            history_status(&connection).await.expect("redo status"),
            (1, 0)
        );
    }
}

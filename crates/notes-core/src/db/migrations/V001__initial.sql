CREATE TABLE nodes (
  id INTEGER PRIMARY KEY,
  uuid BLOB UNIQUE NOT NULL CHECK (length(uuid) = 16),
  kind TEXT NOT NULL CHECK (kind IN ('page', 'block', 'tag', 'entity', 'attachment')),
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
CREATE INDEX idx_nodes_kind ON nodes(kind);
CREATE INDEX idx_nodes_parent ON nodes(parent_id, position);

CREATE TABLE edges (
  id INTEGER PRIMARY KEY,
  src INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
  dst INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  weight REAL NOT NULL DEFAULT 1.0,
  created_at INTEGER NOT NULL,
  UNIQUE(src, dst, kind)
);
CREATE INDEX idx_edges_src ON edges(src, kind);
CREATE INDEX idx_edges_dst ON edges(dst, kind);

CREATE TABLE extracted_edge_sources (
  source_node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
  edge_id INTEGER NOT NULL REFERENCES edges(id) ON DELETE CASCADE,
  PRIMARY KEY(source_node_id, edge_id)
);
CREATE TABLE entity_descriptions (
  source_node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
  entity_node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
  description TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY(source_node_id, entity_node_id)
);
CREATE INDEX idx_entity_descriptions_entity
  ON entity_descriptions(entity_node_id, created_at DESC);

CREATE TABLE history_undo (
  id INTEGER PRIMARY KEY,
  action_uuid BLOB NOT NULL UNIQUE CHECK (length(action_uuid) = 16),
  action TEXT NOT NULL,
  forward_json TEXT NOT NULL,
  inverse_json TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE history_redo (
  id INTEGER PRIMARY KEY,
  action_uuid BLOB NOT NULL UNIQUE CHECK (length(action_uuid) = 16),
  action TEXT NOT NULL,
  forward_json TEXT NOT NULL,
  inverse_json TEXT NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE TABLE sync_outbox (
  rowid INTEGER PRIMARY KEY,
  op_id BLOB NOT NULL UNIQUE CHECK (length(op_id) = 16),
  envelope TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE applied_ops (
  op_id BLOB PRIMARY KEY CHECK (length(op_id) = 16),
  seq INTEGER
);
CREATE TABLE tombstones (
  uuid BLOB PRIMARY KEY CHECK (length(uuid) = 16),
  deleted_hlc TEXT NOT NULL,
  root_uuid BLOB CHECK (root_uuid IS NULL OR length(root_uuid) = 16)
);
CREATE TABLE edge_lww (
  src_uuid BLOB NOT NULL CHECK (length(src_uuid) = 16),
  dst_uuid BLOB NOT NULL CHECK (length(dst_uuid) = 16),
  kind TEXT NOT NULL,
  hlc TEXT NOT NULL,
  present INTEGER NOT NULL,
  weight REAL NOT NULL,
  PRIMARY KEY(src_uuid, dst_uuid, kind)
);
CREATE TABLE node_structure_lww (
  node_uuid BLOB PRIMARY KEY CHECK (length(node_uuid) = 16),
  parent_uuid BLOB CHECK (parent_uuid IS NULL OR length(parent_uuid) = 16),
  position REAL NOT NULL,
  hlc TEXT NOT NULL
);
CREATE TABLE attachment_lww (
  node_uuid BLOB NOT NULL CHECK (length(node_uuid) = 16),
  blob_hash TEXT NOT NULL,
  hlc TEXT NOT NULL,
  present INTEGER NOT NULL,
  filename TEXT,
  mime TEXT,
  size INTEGER,
  PRIMARY KEY(node_uuid, blob_hash)
);
CREATE TABLE sync_meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE local_device (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  device_id BLOB UNIQUE NOT NULL CHECK (length(device_id) = 16)
);

CREATE VIRTUAL TABLE nodes_fts USING fts5(
  body_stemmed,
  content='nodes', content_rowid='id',
  tokenize='unicode61 remove_diacritics 2'
);
CREATE TRIGGER nodes_ai AFTER INSERT ON nodes BEGIN
  INSERT INTO nodes_fts(rowid, body_stemmed) VALUES (new.id, new.body_stemmed);
END;
CREATE TRIGGER nodes_ad AFTER DELETE ON nodes BEGIN
  INSERT INTO nodes_fts(nodes_fts, rowid, body_stemmed)
  VALUES('delete', old.id, old.body_stemmed);
END;
CREATE TRIGGER nodes_au AFTER UPDATE ON nodes BEGIN
  INSERT INTO nodes_fts(nodes_fts, rowid, body_stemmed)
  VALUES('delete', old.id, old.body_stemmed);
  INSERT INTO nodes_fts(rowid, body_stemmed) VALUES (new.id, new.body_stemmed);
END;

CREATE TABLE embed_meta (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  provider TEXT NOT NULL,
  ndims INTEGER NOT NULL
);
CREATE TABLE embed_queue (
  node_id INTEGER PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
  enqueued_at INTEGER NOT NULL,
  retry_count INTEGER NOT NULL DEFAULT 0,
  last_attempt INTEGER,
  last_error TEXT,
  failure_kind TEXT,
  terminal INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE extract_queue (
  node_id INTEGER PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
  enqueued_at INTEGER NOT NULL,
  retry_count INTEGER NOT NULL DEFAULT 0,
  last_attempt INTEGER,
  last_error TEXT,
  failure_kind TEXT,
  terminal INTEGER NOT NULL DEFAULT 0
);

CREATE TRIGGER nodes_ai_extract
AFTER INSERT ON nodes WHEN new.kind IN ('block', 'page') BEGIN
  INSERT OR REPLACE INTO extract_queue(node_id, enqueued_at)
  VALUES(new.id, unixepoch());
END;
CREATE TRIGGER nodes_au_extract
AFTER UPDATE OF content, title ON nodes WHEN new.kind IN ('block', 'page') BEGIN
  INSERT OR REPLACE INTO extract_queue(node_id, enqueued_at)
  VALUES(new.id, unixepoch());
END;

CREATE UNIQUE INDEX idx_entity_title
  ON nodes(kind, lower(title))
  WHERE kind = 'entity' AND title IS NOT NULL;
CREATE UNIQUE INDEX idx_page_title
  ON nodes(kind, lower(title))
  WHERE kind = 'page' AND title IS NOT NULL;

CREATE TRIGGER nodes_ai_embed AFTER INSERT ON nodes BEGIN
  INSERT OR REPLACE INTO embed_queue(node_id, enqueued_at)
  VALUES(new.id, unixepoch());
END;
CREATE TRIGGER nodes_au_embed AFTER UPDATE OF content, title ON nodes BEGIN
  INSERT OR REPLACE INTO embed_queue(node_id, enqueued_at)
  VALUES(new.id, unixepoch());
END;
CREATE TRIGGER nodes_au_embed_parent AFTER UPDATE OF parent_id ON nodes BEGIN
  INSERT OR REPLACE INTO embed_queue(node_id, enqueued_at)
  VALUES(new.id, unixepoch());
END;

CREATE TABLE index_generations (
  id BLOB PRIMARY KEY CHECK (length(id) = 16),
  identity TEXT NOT NULL,
  dimensions INTEGER NOT NULL CHECK (dimensions > 0),
  table_name TEXT NOT NULL UNIQUE,
  status TEXT NOT NULL CHECK (status IN ('building', 'active', 'retired')),
  created_at INTEGER NOT NULL,
  activated_at INTEGER
);
CREATE UNIQUE INDEX one_active_generation
  ON index_generations(status) WHERE status = 'active';

CREATE TABLE index_control (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  automatic_embeddings INTEGER NOT NULL DEFAULT 1,
  entity_extraction INTEGER NOT NULL DEFAULT 1,
  query_rewriting INTEGER NOT NULL DEFAULT 1
);
INSERT INTO index_control(singleton) VALUES (1);

CREATE TABLE embedding_jobs (
  generation_id BLOB NOT NULL CHECK (length(generation_id) = 16),
  content_uuid BLOB NOT NULL CHECK (length(content_uuid) = 16),
  input_hash TEXT NOT NULL,
  input_text TEXT NOT NULL,
  source_seq INTEGER NOT NULL,
  enqueued_at INTEGER NOT NULL,
  retry_count INTEGER NOT NULL DEFAULT 0,
  last_attempt INTEGER,
  last_error TEXT,
  terminal INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(generation_id, content_uuid)
);
CREATE INDEX embedding_jobs_ready
  ON embedding_jobs(generation_id, terminal, retry_count, enqueued_at);

CREATE TABLE generation_vectors (
  generation_id BLOB NOT NULL CHECK (length(generation_id) = 16),
  content_uuid BLOB NOT NULL CHECK (length(content_uuid) = 16),
  vector_rowid INTEGER NOT NULL,
  input_hash TEXT NOT NULL,
  source_seq INTEGER NOT NULL,
  PRIMARY KEY(generation_id, content_uuid),
  UNIQUE(generation_id, vector_rowid)
);

CREATE TABLE extraction_jobs (
  content_uuid BLOB PRIMARY KEY CHECK (length(content_uuid) = 16),
  input_hash TEXT NOT NULL,
  input_text TEXT NOT NULL,
  source_seq INTEGER NOT NULL,
  enqueued_at INTEGER NOT NULL,
  retry_count INTEGER NOT NULL DEFAULT 0,
  last_attempt INTEGER,
  last_error TEXT,
  terminal INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE extraction_state (
  content_uuid BLOB PRIMARY KEY CHECK (length(content_uuid) = 16),
  input_hash TEXT NOT NULL,
  source_seq INTEGER NOT NULL
);
CREATE TABLE entities (
  uuid BLOB PRIMARY KEY CHECK (length(uuid) = 16),
  name TEXT NOT NULL,
  normalized_name TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL DEFAULT '',
  updated_at INTEGER NOT NULL
);
CREATE TABLE extraction_edges (
  source_uuid BLOB NOT NULL CHECK (length(source_uuid) = 16),
  src_uuid BLOB NOT NULL CHECK (length(src_uuid) = 16),
  dst_uuid BLOB NOT NULL CHECK (length(dst_uuid) = 16),
  kind TEXT NOT NULL,
  PRIMARY KEY(source_uuid, src_uuid, dst_uuid, kind)
);
CREATE INDEX extraction_edges_destination ON extraction_edges(dst_uuid, kind);

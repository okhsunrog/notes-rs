ALTER TABLE embedding_jobs
  ADD COLUMN input_header TEXT NOT NULL DEFAULT '';

ALTER TABLE generation_vectors RENAME TO generation_vectors_v1;

CREATE TABLE generation_vectors (
  generation_id BLOB NOT NULL CHECK (length(generation_id) = 16),
  content_uuid BLOB NOT NULL CHECK (length(content_uuid) = 16),
  chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
  chunk_text TEXT NOT NULL,
  vector_rowid INTEGER NOT NULL,
  input_hash TEXT NOT NULL,
  source_seq INTEGER NOT NULL,
  PRIMARY KEY(generation_id, content_uuid, chunk_index),
  UNIQUE(generation_id, vector_rowid)
);

INSERT INTO generation_vectors(
  generation_id, content_uuid, chunk_index, chunk_text,
  vector_rowid, input_hash, source_seq
)
SELECT generation_id, content_uuid, 0, '', vector_rowid, input_hash, source_seq
  FROM generation_vectors_v1;

DROP TABLE generation_vectors_v1;

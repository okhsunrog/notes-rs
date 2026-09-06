-- Keep existing page/journal identity constraints and foreign keys intact.
-- Handwriting is an immutable subtype of ordinary notes, not a text layout.
ALTER TABLE page_identities ADD COLUMN content_type TEXT NOT NULL DEFAULT 'text'
  CHECK (content_type = 'text' OR (content_type = 'ink' AND page_kind = 'note'));

CREATE TABLE ink_records (
  id BLOB PRIMARY KEY CHECK(length(id)=16), data BLOB NOT NULL,
  hash BLOB NOT NULL UNIQUE CHECK(length(hash)=32)
) WITHOUT ROWID;
CREATE TABLE ink_chunks (
  id BLOB PRIMARY KEY CHECK(length(id)=16), data BLOB NOT NULL,
  hash BLOB NOT NULL UNIQUE CHECK(length(hash)=32)
) WITHOUT ROWID;
CREATE TABLE ink_documents (
  page_uuid BLOB PRIMARY KEY REFERENCES page_identities(page_uuid),
  root_id BLOB REFERENCES ink_records(id),
  revision TEXT,
  cursor INTEGER NOT NULL DEFAULT 0,
  dirty INTEGER NOT NULL DEFAULT 0 CHECK(dirty IN (0,1)),
  publication_requested INTEGER NOT NULL DEFAULT 0 CHECK(publication_requested IN (0,1)),
  base_version BLOB CHECK(base_version IS NULL OR length(base_version)=16),
  CHECK ((root_id IS NULL) = (revision IS NULL))
) WITHOUT ROWID;
CREATE TABLE ink_history (
  page_uuid BLOB NOT NULL REFERENCES ink_documents(page_uuid) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  root_id BLOB NOT NULL REFERENCES ink_records(id),
  PRIMARY KEY(page_uuid,seq)
) WITHOUT ROWID;
CREATE TABLE ink_sealed_chunks (
  id BLOB PRIMARY KEY REFERENCES ink_chunks(id) ON DELETE CASCADE
) WITHOUT ROWID;
-- Derived membership, written atomically with each immutable root. GC uses this
-- index instead of parsing every note's CBOR on each gesture in another note.
CREATE TABLE ink_root_refs (
  root_id BLOB NOT NULL REFERENCES ink_records(id) ON DELETE CASCADE,
  kind INTEGER NOT NULL CHECK(kind IN (0,1)),
  object_id BLOB NOT NULL CHECK(length(object_id)=16),
  PRIMARY KEY(root_id,kind,object_id)
) WITHOUT ROWID;
CREATE INDEX ink_root_refs_object ON ink_root_refs(kind,object_id);

-- Version metadata can arrive before blob download. A materialized root pins
-- its local graph; an absent root never makes a partially downloaded note editable.
CREATE TABLE ink_versions (
  version_uuid BLOB PRIMARY KEY CHECK(length(version_uuid)=16),
  page_uuid BLOB NOT NULL REFERENCES page_identities(page_uuid),
  root_hash BLOB NOT NULL CHECK(length(root_hash)=32),
  root_id BLOB REFERENCES ink_records(id),
  device_name TEXT NOT NULL,
  device_id BLOB NOT NULL CHECK(length(device_id)=16),
  modified_hlc TEXT NOT NULL
) WITHOUT ROWID;
CREATE INDEX ink_versions_page ON ink_versions(page_uuid);
CREATE TABLE ink_version_parents (
  version_uuid BLOB NOT NULL REFERENCES ink_versions(version_uuid) ON DELETE CASCADE,
  parent_uuid BLOB NOT NULL CHECK(length(parent_uuid)=16),
  PRIMARY KEY(version_uuid,parent_uuid),
  CHECK(version_uuid != parent_uuid)
) WITHOUT ROWID;

-- Validated downloads remain pinned until their operation is applied.
CREATE TABLE ink_staged_roots (
  root_hash BLOB PRIMARY KEY CHECK(length(root_hash)=32),
  root_id BLOB NOT NULL REFERENCES ink_records(id)
) WITHOUT ROWID;

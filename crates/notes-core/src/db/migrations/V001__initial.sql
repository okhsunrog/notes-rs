CREATE TABLE workspace (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  uuid BLOB UNIQUE NOT NULL CHECK (length(uuid) = 16)
);

CREATE TABLE pages (
  id INTEGER PRIMARY KEY,
  uuid BLOB UNIQUE NOT NULL CHECK (length(uuid) = 16),
  title TEXT,
  normalized_title TEXT,
  layout TEXT NOT NULL DEFAULT 'outline'
    CHECK (layout IN ('outline', 'document')),
  title_hlc TEXT,
  layout_hlc TEXT,
  existence_hlc TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  FOREIGN KEY(uuid) REFERENCES page_identities(page_uuid)
);
CREATE UNIQUE INDEX idx_pages_normalized_title
  ON pages(normalized_title) WHERE normalized_title IS NOT NULL;
CREATE INDEX idx_pages_updated ON pages(updated_at DESC);

-- Page identity outlives the current materialized page so a delayed operation
-- cannot reuse one UUID with a different immutable kind after deletion.
CREATE TABLE page_identities (
  page_uuid BLOB PRIMARY KEY CHECK (length(page_uuid) = 16),
  page_kind TEXT NOT NULL CHECK (page_kind IN ('note', 'journal')),
  journal_date TEXT,
  CHECK ((page_kind = 'note' AND journal_date IS NULL)
      OR (page_kind = 'journal' AND journal_date IS NOT NULL))
) WITHOUT ROWID;
CREATE UNIQUE INDEX idx_page_identities_journal_date
  ON page_identities(journal_date) WHERE journal_date IS NOT NULL;
CREATE UNIQUE INDEX idx_page_identities_uuid_date
  ON page_identities(page_uuid, journal_date);

-- Explicit aliases are synced LWW intents. They deliberately reference the
-- immutable UUID without a foreign key so deletion can retain the intent and
-- a later valid page recreation can materialize it again.
CREATE TABLE page_alias_lww (
  page_uuid BLOB NOT NULL CHECK (length(page_uuid) = 16),
  alias TEXT NOT NULL CHECK (length(alias) > 0),
  hlc TEXT NOT NULL,
  present INTEGER NOT NULL CHECK (present IN (0, 1)),
  PRIMARY KEY(page_uuid, alias)
) WITHOUT ROWID;
CREATE INDEX idx_page_alias_lww_lookup
  ON page_alias_lww(alias, present, page_uuid);

CREATE TABLE journal_pages (
  page_uuid BLOB PRIMARY KEY REFERENCES pages(uuid) ON DELETE CASCADE,
  journal_date TEXT NOT NULL UNIQUE,
  FOREIGN KEY(page_uuid, journal_date)
    REFERENCES page_identities(page_uuid, journal_date)
) WITHOUT ROWID;

CREATE TABLE blocks (
  id INTEGER PRIMARY KEY,
  uuid BLOB UNIQUE NOT NULL CHECK (length(uuid) = 16),
  page_uuid BLOB NOT NULL REFERENCES pages(uuid) ON DELETE CASCADE,
  parent_uuid BLOB REFERENCES blocks(uuid) ON DELETE SET NULL,
  order_key TEXT NOT NULL
    CHECK (length(order_key) = 16 AND order_key NOT GLOB '*[^0-9A-F]*'),
  style TEXT NOT NULL DEFAULT 'paragraph'
    CHECK (style IN ('paragraph', 'bullet', 'numbered',
                     'task:todo', 'task:doing', 'task:now', 'task:later',
                     'task:done', 'task:waiting', 'task:cancelled',
                     'heading_1', 'heading_2', 'heading_3', 'quote',
                     'code', 'divider')),
  markdown TEXT NOT NULL DEFAULT '',
  body_stemmed TEXT NOT NULL DEFAULT '',
  markdown_hlc TEXT,
  style_hlc TEXT,
  structure_hlc TEXT,
  existence_hlc TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  CHECK (parent_uuid IS NULL OR parent_uuid != uuid)
);
CREATE INDEX idx_blocks_page_parent_order
  ON blocks(page_uuid, parent_uuid, order_key, uuid);
CREATE INDEX idx_blocks_parent ON blocks(parent_uuid);
CREATE INDEX idx_blocks_updated ON blocks(updated_at DESC);

CREATE TRIGGER pages_uuid_kind_guard BEFORE INSERT ON pages
WHEN EXISTS(SELECT 1 FROM blocks WHERE uuid = new.uuid)
BEGIN
  SELECT RAISE(ABORT, 'UUID is already used by a block');
END;
CREATE TRIGGER blocks_uuid_kind_guard BEFORE INSERT ON blocks
WHEN EXISTS(SELECT 1 FROM pages WHERE uuid = new.uuid)
  OR EXISTS(SELECT 1 FROM page_identities WHERE page_uuid = new.uuid)
BEGIN
  SELECT RAISE(ABORT, 'UUID is already used by a page');
END;

CREATE TABLE page_links (
  source_block_uuid BLOB NOT NULL REFERENCES blocks(uuid) ON DELETE CASCADE,
  target_title TEXT NOT NULL,
  target_page_uuid BLOB REFERENCES pages(uuid) ON DELETE SET NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY(source_block_uuid, target_title)
) WITHOUT ROWID;
CREATE INDEX idx_page_links_target ON page_links(target_page_uuid, source_block_uuid);
CREATE INDEX idx_page_links_title ON page_links(target_title, source_block_uuid);

CREATE TABLE block_refs (
  source_block_uuid BLOB NOT NULL REFERENCES blocks(uuid) ON DELETE CASCADE,
  target_block_uuid BLOB NOT NULL CHECK (length(target_block_uuid) = 16),
  created_at INTEGER NOT NULL,
  CHECK (source_block_uuid != target_block_uuid),
  PRIMARY KEY(source_block_uuid, target_block_uuid)
) WITHOUT ROWID;
CREATE INDEX idx_block_refs_target ON block_refs(target_block_uuid, source_block_uuid);

CREATE TABLE attachments (
  uuid BLOB PRIMARY KEY CHECK (length(uuid) = 16),
  page_uuid BLOB REFERENCES pages(uuid) ON DELETE CASCADE,
  block_uuid BLOB REFERENCES blocks(uuid) ON DELETE CASCADE,
  blob_hash BLOB NOT NULL
    CHECK (typeof(blob_hash) = 'blob' AND length(blob_hash) = 32),
  filename TEXT NOT NULL,
  mime TEXT NOT NULL,
  size INTEGER NOT NULL CHECK (size >= 0),
  created_at INTEGER NOT NULL,
  CHECK ((page_uuid IS NOT NULL) != (block_uuid IS NOT NULL)),
  UNIQUE(page_uuid, blob_hash),
  UNIQUE(block_uuid, blob_hash)
) WITHOUT ROWID;
CREATE INDEX idx_attachments_page ON attachments(page_uuid, created_at);
CREATE INDEX idx_attachments_block ON attachments(block_uuid, created_at);
CREATE INDEX idx_attachments_hash ON attachments(blob_hash);

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
) WITHOUT ROWID;
CREATE UNIQUE INDEX idx_applied_ops_server_seq ON applied_ops(seq) WHERE seq IS NOT NULL;
CREATE TABLE tombstones (
  uuid BLOB PRIMARY KEY CHECK (length(uuid) = 16),
  object_kind TEXT NOT NULL CHECK (object_kind IN ('page', 'block')),
  deleted_hlc TEXT NOT NULL,
  root_page_uuid BLOB CHECK (root_page_uuid IS NULL OR length(root_page_uuid) = 16)
) WITHOUT ROWID;
CREATE TABLE block_structure_lww (
  block_uuid BLOB PRIMARY KEY CHECK (length(block_uuid) = 16),
  page_uuid BLOB NOT NULL CHECK (length(page_uuid) = 16),
  parent_uuid BLOB CHECK (parent_uuid IS NULL OR length(parent_uuid) = 16),
  order_key TEXT NOT NULL
    CHECK (length(order_key) = 16 AND order_key NOT GLOB '*[^0-9A-F]*'),
  hlc TEXT NOT NULL
) WITHOUT ROWID;
CREATE TABLE attachment_lww (
  owner_kind TEXT NOT NULL CHECK (owner_kind IN ('page', 'block')),
  owner_uuid BLOB NOT NULL CHECK (length(owner_uuid) = 16),
  blob_hash BLOB NOT NULL
    CHECK (typeof(blob_hash) = 'blob' AND length(blob_hash) = 32),
  attachment_uuid BLOB NOT NULL CHECK (length(attachment_uuid) = 16),
  hlc TEXT NOT NULL,
  present INTEGER NOT NULL,
  filename TEXT,
  mime TEXT,
  size INTEGER,
  PRIMARY KEY(owner_kind, owner_uuid, blob_hash)
) WITHOUT ROWID;
CREATE TABLE sync_meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
) WITHOUT ROWID;
CREATE TABLE local_device (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  device_id BLOB UNIQUE NOT NULL CHECK (length(device_id) = 16)
);

-- External import receipts are local provenance, not user content. Imported
-- pages, blocks, and aliases still travel through ordinary synced operations.
CREATE TABLE external_import_receipts (
  receipt_uuid BLOB PRIMARY KEY CHECK (length(receipt_uuid) = 16),
  import_format TEXT NOT NULL UNIQUE CHECK (import_format IN ('logseq')),
  manifest_digest BLOB NOT NULL CHECK (length(manifest_digest) = 32),
  plan_digest BLOB NOT NULL CHECK (length(plan_digest) = 32),
  planner_version INTEGER NOT NULL CHECK (planner_version > 0),
  identity_workspace_uuid BLOB NOT NULL CHECK (length(identity_workspace_uuid) = 16),
  import_namespace_uuid BLOB NOT NULL CHECK (length(import_namespace_uuid) = 16),
  provenance_json TEXT NOT NULL,
  imported_at INTEGER NOT NULL
) WITHOUT ROWID;

CREATE VIRTUAL TABLE pages_fts USING fts5(
  title,
  content='pages', content_rowid='rowid',
  tokenize='unicode61 remove_diacritics 2'
);
CREATE TRIGGER pages_ai AFTER INSERT ON pages WHEN new.title IS NOT NULL BEGIN
  INSERT INTO pages_fts(rowid, title) VALUES (new.rowid, new.title);
END;
CREATE TRIGGER pages_ad AFTER DELETE ON pages WHEN old.title IS NOT NULL BEGIN
  INSERT INTO pages_fts(pages_fts, rowid, title) VALUES('delete', old.rowid, old.title);
END;
CREATE TRIGGER pages_au AFTER UPDATE OF title ON pages BEGIN
  INSERT INTO pages_fts(pages_fts, rowid, title)
  SELECT 'delete', old.rowid, old.title WHERE old.title IS NOT NULL;
  INSERT INTO pages_fts(rowid, title)
  SELECT new.rowid, new.title WHERE new.title IS NOT NULL;
END;

CREATE VIRTUAL TABLE blocks_fts USING fts5(
  body_stemmed,
  content='blocks', content_rowid='rowid',
  tokenize='unicode61 remove_diacritics 2'
);
CREATE TRIGGER blocks_ai AFTER INSERT ON blocks BEGIN
  INSERT INTO blocks_fts(rowid, body_stemmed) VALUES (new.rowid, new.body_stemmed);
END;
CREATE TRIGGER blocks_ad AFTER DELETE ON blocks BEGIN
  INSERT INTO blocks_fts(blocks_fts, rowid, body_stemmed)
  VALUES('delete', old.rowid, old.body_stemmed);
END;
CREATE TRIGGER blocks_au AFTER UPDATE OF body_stemmed ON blocks BEGIN
  INSERT INTO blocks_fts(blocks_fts, rowid, body_stemmed)
  VALUES('delete', old.rowid, old.body_stemmed);
  INSERT INTO blocks_fts(rowid, body_stemmed) VALUES (new.rowid, new.body_stemmed);
END;

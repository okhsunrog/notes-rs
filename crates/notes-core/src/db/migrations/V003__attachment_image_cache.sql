-- Local, reproducible rendering metadata. It is intentionally not synced: every
-- replica can rebuild it from its verified content-addressed blob.
CREATE TABLE attachment_image_cache (
  blob_hash BLOB PRIMARY KEY
    CHECK (typeof(blob_hash) = 'blob' AND length(blob_hash) = 32),
  format_version INTEGER NOT NULL CHECK (format_version > 0),
  byte_size INTEGER NOT NULL CHECK (byte_size > 0),
  mime TEXT NOT NULL,
  width INTEGER NOT NULL CHECK (width > 0),
  height INTEGER NOT NULL CHECK (height > 0),
  preview_hash BLOB
    CHECK (preview_hash IS NULL OR (typeof(preview_hash) = 'blob' AND length(preview_hash) = 32)),
  preview_size INTEGER CHECK (preview_size IS NULL OR preview_size > 0),
  preview_width INTEGER CHECK (preview_width IS NULL OR preview_width > 0),
  preview_height INTEGER CHECK (preview_height IS NULL OR preview_height > 0),
  CHECK ((preview_hash IS NULL) = (preview_size IS NULL)),
  CHECK ((preview_hash IS NULL) = (preview_width IS NULL)),
  CHECK ((preview_hash IS NULL) = (preview_height IS NULL))
) WITHOUT ROWID;

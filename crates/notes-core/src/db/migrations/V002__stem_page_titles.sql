ALTER TABLE pages ADD COLUMN title_stemmed TEXT NOT NULL DEFAULT '';

DROP TRIGGER pages_ai;
DROP TRIGGER pages_ad;
DROP TRIGGER pages_au;
DROP TABLE pages_fts;

CREATE VIRTUAL TABLE pages_fts USING fts5(
  title_stemmed,
  content='pages', content_rowid='rowid',
  tokenize='unicode61 remove_diacritics 2'
);
CREATE TRIGGER pages_ai AFTER INSERT ON pages WHEN new.title IS NOT NULL BEGIN
  INSERT INTO pages_fts(rowid, title_stemmed) VALUES (new.rowid, new.title_stemmed);
END;
CREATE TRIGGER pages_ad AFTER DELETE ON pages WHEN old.title IS NOT NULL BEGIN
  INSERT INTO pages_fts(pages_fts, rowid, title_stemmed)
  VALUES('delete', old.rowid, old.title_stemmed);
END;
CREATE TRIGGER pages_au AFTER UPDATE OF title_stemmed ON pages BEGIN
  INSERT INTO pages_fts(pages_fts, rowid, title_stemmed)
  SELECT 'delete', old.rowid, old.title_stemmed WHERE old.title IS NOT NULL;
  INSERT INTO pages_fts(rowid, title_stemmed)
  SELECT new.rowid, new.title_stemmed WHERE new.title IS NOT NULL;
END;

-- Seed one empty FTS row for each existing title so the Rust Snowball backfill
-- can update through the normal delete/insert trigger without a missing-row delete.
INSERT INTO pages_fts(rowid, title_stemmed)
SELECT rowid, title_stemmed FROM pages WHERE title IS NOT NULL;

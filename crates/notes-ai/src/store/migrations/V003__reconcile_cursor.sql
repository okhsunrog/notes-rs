ALTER TABLE index_generations
  ADD COLUMN last_reconciled_cursor TEXT;

ALTER TABLE index_generations
  ADD COLUMN source_documents INTEGER NOT NULL DEFAULT 0
    CHECK (source_documents >= 0);

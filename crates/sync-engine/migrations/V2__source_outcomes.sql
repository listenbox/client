ALTER TABLE source_items ADD COLUMN title TEXT NOT NULL DEFAULT '';
ALTER TABLE source_items ADD COLUMN duration_seconds INTEGER CHECK (duration_seconds > 0);
ALTER TABLE source_items ADD COLUMN phase TEXT NOT NULL DEFAULT 'queued'
    CHECK (phase IN ('queued', 'complete', 'skipped', 'failed'));
ALTER TABLE source_items ADD COLUMN reason TEXT;
ALTER TABLE source_items ADD COLUMN error TEXT;
UPDATE source_items SET title = source_url;

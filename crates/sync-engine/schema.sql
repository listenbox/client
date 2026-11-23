CREATE TABLE transfers (
    origin TEXT NOT NULL,
    show_slug TEXT NOT NULL,
    source_url TEXT NOT NULL,
    collection_url TEXT NOT NULL,
    operation_id TEXT NOT NULL UNIQUE,
    manifest TEXT,
    upload_session_id TEXT,
    PRIMARY KEY (origin, show_slug, source_url, collection_url)
);
CREATE TABLE parts (
    operation_id TEXT NOT NULL REFERENCES transfers(operation_id) ON DELETE CASCADE,
    object_index INTEGER NOT NULL CHECK (object_index >= 0),
    part_number INTEGER NOT NULL CHECK (part_number > 0),
    PRIMARY KEY (operation_id, object_index, part_number)
);
CREATE TABLE segments (
    operation_id TEXT NOT NULL REFERENCES transfers(operation_id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    PRIMARY KEY (operation_id, name)
);
CREATE TABLE downloads (
    operation_id TEXT NOT NULL REFERENCES transfers(operation_id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    identity TEXT NOT NULL,
    PRIMARY KEY (operation_id, name)
);
CREATE TABLE download_ranges (
    operation_id TEXT NOT NULL,
    name TEXT NOT NULL,
    start INTEGER NOT NULL CHECK (start >= 0),
    sha256 TEXT NOT NULL,
    PRIMARY KEY (operation_id, name, start),
    FOREIGN KEY (operation_id, name) REFERENCES downloads(operation_id, name) ON DELETE CASCADE
);
CREATE TABLE source_items (
    origin TEXT NOT NULL,
    show_slug TEXT NOT NULL,
    collection_url TEXT NOT NULL,
    source_url TEXT NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    title TEXT NOT NULL DEFAULT '',
    duration_seconds INTEGER CHECK (duration_seconds > 0),
    phase TEXT NOT NULL DEFAULT 'queued' CHECK (phase IN ('queued', 'complete', 'skipped', 'failed')),
    reason TEXT,
    error TEXT,
    PRIMARY KEY (origin, show_slug, source_url)
);
CREATE TABLE sync_completions (
    origin TEXT NOT NULL,
    show_slug TEXT NOT NULL,
    completed_at INTEGER NOT NULL CHECK (completed_at >= 0),
    PRIMARY KEY (origin, show_slug)
);
CREATE TABLE catalog_cache (
    origin TEXT NOT NULL PRIMARY KEY,
    credential TEXT NOT NULL,
    catalog TEXT NOT NULL
);
CREATE TABLE episode_cache (
    origin TEXT NOT NULL,
    show_slug TEXT NOT NULL,
    credential TEXT NOT NULL,
    episodes TEXT NOT NULL,
    PRIMARY KEY (origin, show_slug)
);

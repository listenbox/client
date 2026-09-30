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
    PRIMARY KEY (origin, show_slug, source_url)
);

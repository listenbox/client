CREATE TABLE sync_completions (
    origin TEXT NOT NULL,
    show_slug TEXT NOT NULL,
    completed_at INTEGER NOT NULL CHECK (completed_at >= 0),
    PRIMARY KEY (origin, show_slug)
);

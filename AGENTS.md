# Rust client

- CLI and desktop share `crates/sync-engine`; put HTTP, format selection,
  configuration, and sync behavior there.
- Debug builds default to `~/.cache/listenbox/dev/`. Release builds use
  `~/Library/Application Support/Listenbox/` on macOS, `%LOCALAPPDATA%\Listenbox\`
  on Windows, and `$XDG_DATA_HOME/listenbox/` (default `~/.local/share/listenbox/`)
  on Linux. `LISTENBOX_PROFILE_DIR` explicitly overrides the whole
  profile (config, credentials, YouTube cookies, SQLite, and downloads).
- `moon run desktop:dev` and debug binaries use the same worktree-independent
  profile. The dev SQLite database is **`~/.cache/listenbox/dev/sync.sqlite`**.
  Start debugging there, using read-only queries:
  `sqlite3 -readonly ~/.cache/listenbox/dev/sync.sqlite '.schema'` and
  `sqlite3 -readonly ~/.cache/listenbox/dev/sync.sqlite 'SELECT origin, show_slug, source_url, collection_url FROM transfers'`.
- Never repair a dev issue by editing the release profile or copying release
  credentials/state into it.
- Keep release authentication and SQLite in persistent application data.
  SQLite records unfinished uploads and resumable work, not just server data.
- Debug builds automatically log HTTP method, route, status, elapsed time, and
  trace ID to stderr. Inspect local traces from the parent repository
  with `aube node scripts/spaniel.ts <trace-id>`.
- Verify shared behavior through the parent repository's integrated API E2E
  targets; build the desktop with `moon run desktop:build`.

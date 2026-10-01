# Rust client

- Repository and submodule fetch/push remotes and `.gitmodules` URLs use SSH.
  GitHub Actions checkout may use HTTPS.

- Run `shellcheck` on every shell script you create or change. `moon run client:lint`
  checks the client's shell scripts with the installed ShellCheck binary.

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

## YouTube.js fork and upstream fixes

- `vendor/youtubejs` is our `listenbox/YouTube.js` fork, a nested Git
  submodule. Patch bugs there when the defect belongs to YouTube.js; keep the
  parser and runtime fixes in their canonical upstream implementation instead
  of adding application workarounds or host-runtime shims.
- Reproduce the observed response shape and failure with the real Rust client
  and embedded YouTube.js against local integrated E2E fixtures. Prove the
  regression fails before the fix and passes afterward, including the relevant
  incomplete-scan and deletion safeguards.
- Send a minimal PR to `LuanRT/YouTube.js` for each upstream bug fix, following
  [#1276](https://github.com/LuanRT/YouTube.js/pull/1276). Explain what we
  encountered (response shape, affected operation and actual exception), why
  the chosen change correctly handles it, and the verification performed.
  Keep the upstream diff limited to the bug fix; do not include Listenbox
  integration, dependency, lockfile, build, or workflow changes.
- Before GitHub writes, verify this submodule's own `origin` remote. Publish
  the fix to our fork, submit the PR against upstream's current default branch,
  and update the pinned commits in this client repository and its parent.

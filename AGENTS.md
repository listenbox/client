# Rust client

- Repository and submodule fetch/push remotes and `.gitmodules` URLs use SSH.
  GitHub Actions checkout may use HTTPS.

- Run `shellcheck` on every shell script you create or change. `moon run client:lint`
  checks the client's shell scripts with the installed ShellCheck binary.
- Write client command and build automation as simple Bash scripts checked by
  ShellCheck, never TypeScript or zx scripts. This does not apply to application
  code in the YouTube.js submodule.
- Run Cargo commands as `kache cargo -- ...` and standalone Rust compilation as
  `kache rustc --crate-name ... --out-dir ...`. Keep `.cargo/config.toml`'s
  wrapper enabled for tools that invoke Cargo internally. Registry installs
  must explicitly set `RUSTC_WRAPPER=kache`, `CARGO_INCREMENTAL=0`, and
  `KACHE_CONFIG` because Cargo ignores project configuration for those installs.
- Keep Cargo build directories independent per worktree, with compatible
  artifacts in kache's shared OS cache on the same filesystem. Install the
  pinned Dioxus CLI through `tools/dev-tools.sh`; its versioned installation
  is shared across worktrees and its temporary build artifacts are removed.

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
- All desktop builds persist operation, lock, media and HTTP diagnostics under
  `<profile>/diagnostics/`. Run the desktop executable with `--diagnostics` to
  read a bounded tail without opening the UI. During logout, inspect unmatched
  operation spans and drain warnings; a nonzero `dropped_log_lines` means missing
  events cannot establish task liveness. Preserve the distinction between
  cancellable admission and owned atomic writes that must drain before logout.
- Verify shared behavior through the parent repository's integrated API E2E
  targets; build the desktop with `moon run desktop:build`.

## Releases

- Standard CI runs on `master` pushes and pull requests. Successful CI for the
  current `master` prepares an automatic patch release. Superseded CI is skipped.
- `automatic-release.yaml` serializes version allocation. `release-next.sh`
  chooses the next patch after the highest reserved numeric tag, updates Cargo
  and its lockfile in a release commit whose parent is the CI source, and creates
  the immutable `vX.Y.Z` tag. It never pushes a version commit to `master`.
  Rerunning preparation reuses that source's existing release tag.
- Major/minor bumps are deliberate: set `[workspace.package].version` to a
  greater major/minor version, run `kache cargo -- update --workspace`, and merge.
  Automation uses that version before resuming automatic patch bumps.
- `release-dispatch.sh` pushes only the prepared tag and explicitly dispatches
  the native workflow. GitHub's built-in token does not trigger tag-push workflows.
  Native builds run on pull requests, `v*` tag pushes, and tag dispatches; never
  add a branch-push trigger, which would duplicate native builds.
- A newer tagged release cancels unfinished native build jobs. Publication
  remains serialized and is never canceled by a newer release. Reserved versions
  remain immutable, so cancellation may leave gaps in patch numbers. No manual
  deployment approval is required. Actions publishes all desktop installers and
  update feeds together after native builds and publication checks succeed.
  Branch/PR builds never publish releases; already published tags are skipped.

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

# Go client migration

The CLI and MyGo native desktop share the Go package in `crates/sync-engine`.
The existing directory names keep Moon project identities and parent E2E build
paths stable. `crates/youtubei` owns a single-threaded buke/quickjs-go runtime;
the vendored YouTube.js remains the provider protocol and parser implementation.

Mise combines configuration from parent directories with the current directory;
Git repository and submodule boundaries do not stop that search. The parent
`mise.toml` pins shared workspace tools. This repository repeats the required
pins so a standalone checkout also works, and owns the client-specific CGO
environment. Keep shared pins aligned. Each repository
commits its own `mise.lock`; neither config includes the other, so parent Go
services do not inherit the client's FFmpeg linker environment. Moon remains
the workspace dependency graph and owns every task. Run `moon run client:check`
from either checkout. Use `mise install --locked` to provision tools; no Mise
TOML or file tasks are allowed. CI selects its repository with `jdx/mise-action`'s
`working_directory` and installs the committed lockfile.

## Compatibility boundaries

- Preserve CLI commands, stdout contracts, cancellation exit code 130, YAML
  configuration, profile paths, `auth.json`, cookie generations/revisions, and
  the existing SQLite tables, migration history and prepared-package manifests.
- Use tailscale/sqlite with separate writer/read connections and cancellable
  process locks. Once a file write is admitted it must finish before shutdown.
- Keep resumable range validation, multipart acknowledgements and operation
  IDs. Reconcile authoritative inventory before retrying ambiguous publication.
  Incomplete playlist scans must never authorize removal.
- Use astiav with source-built pinned FFmpeg/OpenH264 for AAC conversion,
  progressive MP4 and HLS renditions, seek previews, bounded keyframe spacing,
  timestamps and language metadata. No ffmpeg subprocess in application code.
- Preserve desktop sign-in, team selection, import, automatic/manual sync,
  pause/stop, source issues, episodes, cookies, settings, tray and safe quit.
  Apply the existing native design to MyGo's Go UI toolkit.
- Preserve Sparkle and WinSparkle feeds/keys, macOS bundle identifier,
  Windows installer AppId/mutex/profile and release payload names. Port native
  bridges to Go and build with Go/CGO on the platform's native architecture.

## Implementation sequence

1. Go module, generated public API/config contracts, profile/auth/HTTP/events,
   compatible journal and cookies.
2. Embedded YouTube.js host and bindings, import admission, format selection,
   range downloads, astiav preparation, upload journal and reconciliation.
3. CLI commands and MyGo desktop using the same engine and operation lifetime.
4. Go build/lint/codegen targets, native packaging/release workflows, updated
   docs/notices; remove Rust source, Cargo files and obsolete Rust tools.
5. Commit the client and update the parent repository's submodule pin.

## Verification

Use existing integrated API E2E contracts through `api:test-e2e`, including CLI
configuration/auth/publishing, real embedded YouTube.js fixture traffic,
cookies, quota admission, source metadata/order/removal, format conformance,
resumable uploads/downloads, V1 migration, cancellation and process cleanup.
Port the existing parent-invoked journal/network fixture executables to Go.
Keep controllable clocks and direct event gates; do not weaken assertions,
serialize tests, extend timeouts or exercise production retry delays.
Run client builds, the common API golangci-lint policy and ShellCheck,
release publication E2E, then the
holistic Moon check/CI graph. macOS and Windows compile/package smoke checks
belong in the existing native workflows. Actual installed GPUI-to-MyGo upgrades
require those operating systems and signed release artifacts; Linux E2E cannot
establish their outcome.

The SDK is generated from `https://v1.listenbox.app/` using the OASmith version
pinned in `go.mod` and invoked with `go tool`. Generated SDK/configuration
packages are ignored by Git and lint. No private parent specification is needed
for the standalone client build.

The port retains all 80 named Rust tests, recorded in `rust-test-inventory.json`,
with one additional Atlas forward-schema test. The legacy artwork test retains
both request coalescing and refresh assertions. Integrated coverage also proves
the disk HTTP cache after restarts and explicit resumption after source/storage
HTTP failures. API server and transport errors keep their retry classification.

The one-off compatibility proof used the real pre-rewrite Rust 0.2.4 CLI from
client commit `98877ad278757a50eeadd96fb63365b89f2cc7ff`, SHA-256
`1d7043feb92b065bb40c15a7822af92910d012d6489614ab5655b9cc9991371b`.
Rust created an admitted, interrupted upload; the Go Atlas driver added and
removed a test-only table without changing its history, manifest or session;
the Go CLI resumed the same session without fetching the player again.
`TestClientRustJournalContinuesWithAtlasAndGoCLI` passed locally. Re-run it with
`CLIENT_MIGRATION_RUST_BINARY=/path/to/prebuilt/rust-cli moon run api:test-e2e -- -run '^TestClientRustJournal'`.
The executable is local temporary data and must never be committed or become a
CI dependency. Ongoing V1 upgrade, divergence, rollback and checkpoint coverage
uses SQLite fixtures and Go binaries.

# Listenbox client

A Rust monorepo for the Listenbox desktop app, CLI and shared synchronization engine.

| Crate | Responsibility |
| --- | --- |
| `crates/desktop` | Native GPUI Kit interface, live teams and podcasts, transfer progress |
| `crates/cli` | Commands, terminal progress and foreground watch service |
| `crates/sync-engine` | Authentication, generated public API, reconciliation, durable transfers, FFmpeg and SQLite |
| `crates/youtubei` | Embedded YouTube.js bindings, checked-in Rust source |

## Build

Install Rust via rustup, [Moon](https://moonrepo.dev), [pkgx](https://pkgx.sh), a C compiler, make and tar. Rust is pinned in `rust-toolchain.toml`. No JavaScript runtime or package manager is needed in this repository.

```sh
git clone https://github.com/listenbox/client.git
cd client
moon run client:build
moon ci
```

The app is `crates/desktop/dist/listenbox-desktop`; the terminal executable is `crates/cli/dist/listenbox`. `moon run client:package` packages release binaries and notices. `moon run client:dmg` builds the macOS application and DMG.

FFmpeg 9.0.2 is built from verified source by `tools/native-ffmpeg.rs`, linked with `ffmpeg-the-third`, and never run as a subprocess. The physical `youtubei` crate embeds a verified upstream bundle in QuickJS. Both applications are self-contained. See `THIRD-PARTY-NOTICES.txt` and the packaged FFmpeg source/license notice.

### Windows releases

Every push to `master` builds and publishes a Windows prerelease on [GitHub Releases](https://github.com/listenbox/client/releases). Its version is the desktop package version resolved by Cargo plus the first 12 characters of the commit SHA, such as `0.1.0+abcdef123456`, with release tag `desktop-v0.1.0+abcdef123456`. Choose `listenbox-desktop-windows-x64.zip` for Intel/AMD PCs or `listenbox-desktop-windows-arm64.zip` for native Windows ARM64, including Windows 11 ARM in VMware Fusion on Apple Silicon. Both archives include the executable and license notices; matching `.exe` assets are also available to run directly. Windows 11 ARM can also run the x64 version through emulation. The executables are unsigned; signing is not configured. Each build checks its architecture, runtime DLL dependencies and `--help` startup. Interactive Windows testing remains necessary.

The same builds can run on native Windows machines of the matching architecture with Rust, Moon, Visual Studio's C++ tools for that architecture and Windows SDK, host-native LLVM, and MSYS2 (`make`, `diffutils`, `tar`, `xz`, `openssl`). Set `MSYS2_LOCATION` to the MSYS2 installation directory and, if LLVM is not installed at `C:\Program Files\LLVM`, set `LIBCLANG_PATH` to the directory containing `libclang.dll`:

```powershell
$env:MSYS2_LOCATION = 'C:\msys64'
moon run desktop:build-release-windows-x64
# On Windows ARM64:
moon run desktop:build-release-windows-arm64
```

Outputs are in `crates/desktop/dist/`. FFmpeg and the MSVC runtime are linked statically. Each Windows target has its own FFmpeg cache, separate from the native macOS/Linux build. GPUI compiles its release shaders using the Windows SDK, so these tasks must run on Windows. The client profile is `%USERPROFILE%\.config\listenbox` on Windows; `LISTENBOX_PROFILE_DIR` overrides it on all platforms.

## Desktop development

In the parent Listenbox workspace, start the backend and dashboard in one terminal:

```sh
moonx dev
```

Then start the desktop watcher in another terminal, from either workspace:

```sh
moonx desktop:dev
```

This follows [Mazit's watchexec workflow](https://github.com/meoyawn/mazit/blob/main/Taskfile.yaml): source changes rebuild and restart the debug app after a 300 ms debounce. Changes to engine and YouTube code, migrations, assets, Cargo manifests, and local config are watched too. Failed builds leave the watcher running for the next edit. Quit Listenbox ends the watcher; closing the window keeps the app running. Ctrl-C stops the watcher and app. Restart signals use the app's normal cancel-and-drain path, with a five-second force-stop guard; the engine recovers interrupted work from its journal.

The watched crate directories come from `cargo metadata`, following the desktop's transitive local dependencies. There is no hand-maintained crate list, and CLI source edits do not restart the desktop. Cargo handles incremental compilation through `desktop:build`. Restart `moonx desktop:dev` after adding or removing a local crate dependency so it discovers the changed dependency graph.

`config/dev.yaml` points at `http://localhost:8080` (public API) and `http://localhost:5174` (dashboard sign-in), with API trace IDs enabled. The watcher sets `LISTENBOX_PROFILE_DIR` to this checkout's `.cache/dev`, isolating credentials and resumable work from the normal production profile. To use the same dev login from the CLI, run from the client repository:

```sh
env LISTENBOX_PROFILE_DIR="$PWD/.cache/dev" crates/cli/dist/listenbox --config config/dev.yaml shows list
```

The desktop task runs independently of the parent `scripts/dev.ts` and never starts or stops the backend services. For a standalone client checkout, start those services separately on the configured addresses.

## Install

macOS downloads are attached to tagged [GitHub releases](https://github.com/listenbox/client/releases). Open the DMG and drag Listenbox to Applications, or extract the command-line executable from the matching architecture archive. The release workflow builds Apple Silicon and Intel packages. Current bundles are ad-hoc signed; Developer ID signing and Apple notarization require distribution credentials. Local builds can set `LISTENBOX_SIGNING_IDENTITY` for a configured Developer ID certificate.

## Connect and sync

The desktop's sign-in button opens Listenbox in your browser. `listenbox login` uses the same authorization flow. Both save and read one private credential in `~/.config/listenbox/auth.json`; signing in through either authorizes both.

```sh
listenbox login
listenbox shows list
listenbox shows create --title "Field notes" --slug field-notes --type audio --language en
listenbox shows source --show field-notes --youtube 'https://www.youtube.com/playlist?list=PLAYLIST_ID'
listenbox shows sync youtube --show field-notes
listenbox shows sync youtube --show field-notes --watch
listenbox shows source --show field-notes --disconnect
```

An active paid audio or video plan is required for synchronization. Teams and accessible shows load live, including team membership and direct collaboration. A source cannot coexist with a YouTube publishing destination; PostgreSQL enforces both directions. The backend checks permissions and video allowance when admitting work.

The engine completes the entire playlist scan before reconciling canonical item URLs. New videos become episodes. Removed videos are deleted only when owned by the current playlist. Unavailable videos and episodes from other sources are preserved. Playlist order becomes RSS order; real publication dates stay intact. Reordering does not repeat media work.

For creator-managed podcasts, read or change order through the same ordering API:

```sh
listenbox shows order --show field-notes
listenbox shows order --show field-notes --episode ep_0123456789abcdef --episode ep_fedcba9876543210
```

Supplied episodes move to the front in sequence; other episodes retain their relative order. Disconnect a source before managing that podcast's order by hand.

## Interrupted work

Both interfaces use the engine's persistent sync path. It opens `~/.config/listenbox/sync.sqlite` only for sync work and keeps media under `~/.config/listenbox/transfers`. Refinery applies embedded SQL migrations with strict history validation. Neither interface accesses SQLite directly.

One writer connection serializes changes; pooled read-only connections read concurrently through WAL. FULL synchronous commits and macOS full-fsync preserve acknowledged checkpoints. Closing the desktop window leaves sync running. The menu-bar icon reopens it; Log out and Quit cancel active work and wait for admitted writes and processing to finish. A second ⌘Q press within one second or holding it for two seconds confirms keyboard quit; quitting waits for key release. Log out removes the shared credential, keeping resumable work.

Verified download ranges, prepared files, transfer UUIDs, upload sessions and acknowledged parts survive interruption. Restarting checks saved work against the live backend before resuming. An OS lock per server and show prevents simultaneous CLI and desktop work on that show. Slots adapt to measured throughput across podcasts. Pausing stops new admissions; stopping a sync preserves its work.

`--watch` scans immediately, then hourly, and handles SIGINT/SIGTERM. Desktop watches share one hourly clock. The CLI stays in the foreground, suitable for a user systemd service:

```ini
[Service]
ExecStart=%h/.local/bin/listenbox shows sync youtube --show field-notes --watch
Restart=on-failure
RestartSec=10
```

## Configuration and tests

Both applications read `~/.config/listenbox/config.yaml`, or accept `--config PATH`. `LISTENBOX_PROFILE_DIR` overrides the shared directory for config, credentials, and sync state; an empty override is rejected. `--config` selects the config file without changing that profile directory. Release defaults are:

```yaml
api_origin: https://v1.listenbox.app
dashboard_origin: https://web.listenbox.app
print_trace_ids: false
```

E2E tests use the same code with isolated homes and explicit local configuration. GPUI Kit tests interact with real controls in a headless window. The integrated suite boots the API, worker and local external fixtures; the desktop flow needs no Chrome process.

Generated API and configuration modules are committed in `sync-engine`, so this monorepo builds independently. The parent workspace regenerates them through Moon from `packages/openapi/spec/public.responsible.ts` and `client.responsible.ts` before client builds. Its integration command is `moon run api:test-e2e`; both workspaces use `moon ci`.

## Keeping private data out of the repository

Run `moon run client:secrets` before publishing changes. It uses [Gitleaks](https://github.com/gitleaks/gitleaks) to scan all locally available Git history, staged and unstaged edits, and new non-ignored files. `moon ci` always runs this check without caching. Findings are redacted. The additional file-content rules catch personal home paths and personal email addresses; Git author and committer identities remain public attribution.

Keep credentials, sync databases, downloaded media, and logs in the client profile, outside tracked source. The development profile already lives in ignored `.cache/dev`. Local environment files, credentials, SQLite journals, logs, and signing keys are also ignored. Use synthetic data for fixtures and screenshots, and review images manually: a text scanner cannot establish that an image contains no private information.

See `crates/desktop/DESIGN.md` for native tokens and component conventions, and [bench/README.md](bench/README.md) for the historical import benchmark.

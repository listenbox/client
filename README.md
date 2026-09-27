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

The debug app is `crates/desktop/dist/listenbox-desktop`; the terminal executable is `crates/cli/dist/listenbox`. `moon run desktop:build-release` builds the production desktop executable for the host architecture at `crates/desktop/dist/release/listenbox-desktop`. `moon run desktop:dmg` packages its Apple Silicon build as a DMG. `moon run client:package` packages release CLI and desktop binaries with notices.

FFmpeg 9.0.2 is built from verified source by `tools/native-ffmpeg.rs`, linked with `ffmpeg-the-third`, and never run as a subprocess. The physical `youtubei` crate embeds a verified upstream bundle in QuickJS. Both applications are self-contained. See `THIRD-PARTY-NOTICES.txt` and the packaged FFmpeg source/license notice.

### Worktree disk usage

Cargo downloads are shared through `registry` and `git` under `CARGO_HOME`
(normally `~/.cargo`). Build artifacts stay in this checkout's `target/` so
different worktrees can compile independently. Do not share `CARGO_TARGET_DIR`
or `build.build-dir`: Cargo's build-directory lock would queue those builds.

`.cargo/config.toml` disables incremental compilation to avoid its extra disk
state. Cargo still reuses unchanged dependencies and crate artifacts; editing a
crate can take longer to rebuild. The development profile already disables debug
information. Compiler wrappers such as `sccache` save compilation time but still
materialize artifacts in each worktree, so they do not eliminate disk duplication.

The policy applies to both direct Cargo commands and Moon tasks, in standalone
and parent checkouts. Existing artifacts are not automatically removed. After
stopping builds in a worktree, `cargo clean` from this directory reclaims its
build artifacts. FFmpeg's native build remains in this checkout's `.cache/ffmpeg`.

CI and release jobs restore Cargo downloads, unpacked sources, and compiled
artifacts together. Cargo fetches dependencies as the selected Moon tasks need
them; there is no workspace-wide prefetch step. Cache prefixes keep the platform,
Rust toolchain, and native build configuration fixed while allowing unchanged
dependencies to be reused after lockfile updates. Windows also keys on the MSVC
toolchain version.

Linux CI and macOS releases also retain Moon's content-hashed output archives.
Moon can restore a matching FFmpeg build into a fresh checkout instead of
rebuilding the restored libraries. Native preparation remains a prerequisite:
missing archives or changed task inputs still run the real build. Windows keeps
its separate FFmpeg cache and existing save-before-Rust-build behavior.

### Native media acceptance

`moon run client-engine:test-media` runs the production media preparation code
against local AAC, Opus, and AVC fixtures. It decodes M4A, MP4, and HLS output and
checks invalid input, cancellation before preparation, and native tool paths.
Tests have a hard 30-second timeout and no retries. See
[the build comparison](docs/ffmpeg-build.md) for why the existing minimal builder
remains in use.

### Desktop releases

Every push to `master` builds and publishes one prerelease on [GitHub Releases](https://github.com/listenbox/client/releases) containing an Apple Silicon DMG and Windows x64 and ARM64 builds. Its version is the desktop package version resolved by Cargo plus the first 12 characters of the commit SHA, such as `0.1.0+abcdef123456`, with release tag `desktop-v0.1.0+abcdef123456`. Choose the DMG for Apple Silicon Macs, `listenbox-desktop-windows-x64.zip` for Intel/AMD PCs or `listenbox-desktop-windows-arm64.zip` for native Windows ARM64, including Windows 11 ARM in VMware Fusion on Apple Silicon. Both Windows archives include the executable and license notices; matching `.exe` assets are also available to run directly. Windows 11 ARM can also run the x64 version through emulation. The Windows executables are unsigned; signing is not configured. Each Windows build checks its architecture, runtime DLL dependencies and `--help` startup. Interactive Windows testing remains necessary.

Pull requests build and package the same desktop applications on macOS ARM64, Windows x64, and Windows ARM64. Every runner executes release-linked media tests before packaging, including after native cache hits; only pushes to `master` publish releases. Linux retains the regular Moon checks. On Windows, FFmpeg is cached separately and saved before the Rust build, so subsequent attempts can reuse it even after a Rust compilation failure. Release runs also upload build logs, FFmpeg configuration diagnostics, and Moon reports for troubleshooting without a local Windows machine.

The same builds can run on native Windows machines of the matching architecture with Rust, Moon, Visual Studio's C++ tools for that architecture and Windows SDK, host-native LLVM, cargo-nextest, and MSYS2 (`make`, `diffutils`, `tar`, `xz`, `openssl`). Set `MSYS2_LOCATION` to the MSYS2 installation directory and, if LLVM is not installed at `C:\Program Files\LLVM`, set `LIBCLANG_PATH` to the directory containing `libclang.dll`:

```powershell
$env:MSYS2_LOCATION = 'C:\msys64'
moon run desktop:build-release-windows-x64
# On Windows ARM64:
moon run desktop:build-release-windows-arm64
```

Outputs are in `crates/desktop/dist/`. FFmpeg and the MSVC runtime are linked statically. Each Windows target has its own FFmpeg cache, separate from the native macOS/Linux build. GPUI compiles its release shaders using the Windows SDK, so these tasks must run on Windows. The client profile is `%USERPROFILE%\.config\listenbox` on Windows; `LISTENBOX_PROFILE_DIR` overrides it on all platforms.

Listenbox opens without a console window and keeps a system tray icon while running. Closing the window with X hides it and keeps synchronization running. Click the tray icon to reopen the window, or right-click it for Open Listenbox and Quit Listenbox. The macOS menu-bar icon offers the same two actions. Quit waits for active work to shut down cleanly. Windows may place the icon in the tray's hidden-icons overflow.

Desktop icon artwork lives in `crates/desktop/assets/icon.png`. macOS packages it directly; the desktop Cargo build generates a multi-resolution Windows ICO from the same image and embeds it as resource 1 for Explorer, the window, and the taskbar. `assets/tray.svg` adapts the shared web favicon's L-and-dot geometry without its tile background. The build renders a 36px macOS template (18pt at Retina scale) and a 32px Windows mark with a contrasting outline for light and dark taskbars. Generated ICO, PNG previews, and embedded RGBA pixels live in Cargo's build output directory; edit the source artwork to regenerate them.

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

The watched crate directories come from `cargo metadata`, following the desktop's transitive local dependencies. There is no hand-maintained crate list, and CLI source edits do not restart the desktop. Cargo reuses unchanged crate artifacts through `desktop:build`. Restart `moonx desktop:dev` after adding or removing a local crate dependency so it discovers the changed dependency graph.

`config/dev.yaml` points at `http://localhost:8080` (public API) and `http://localhost:5174` (dashboard sign-in), with API trace IDs enabled. The watcher sets `LISTENBOX_PROFILE_DIR` to this checkout's `.cache/dev`, isolating credentials and resumable work from the normal production profile. To use the same dev login from the CLI, run from the client repository:

```sh
env LISTENBOX_PROFILE_DIR="$PWD/.cache/dev" crates/cli/dist/listenbox --config config/dev.yaml shows list
```

The desktop task runs independently of the parent `scripts/dev.ts` and never starts or stops the backend services. For a standalone client checkout, start those services separately on the configured addresses.

## Install

Open the Apple Silicon DMG from the [desktop release](https://github.com/listenbox/client/releases) and drag Listenbox to Applications. Current bundles are ad-hoc signed; Developer ID signing and Apple notarization require distribution credentials. Local builds can set `LISTENBOX_SIGNING_IDENTITY` for a configured Developer ID certificate.

To build and reinstall the release locally, run this from either the parent monorepo or the client checkout:

```sh
moonx desktop:macos-reinstall
```

This reuses the release/DMG packaging, verifies the staged app, gracefully stops the copy running from `/Applications/Listenbox.app`, and atomically replaces that bundle. It also handles a first install. A failed build, copy, signature check, or shutdown leaves the installed bundle intact. The task never force-kills the app, changes its profile or credentials, or starts it afterward; open Listenbox from Applications when ready. Installation always runs, even with cached build outputs.

## Import and sync

The desktop's sign-in button opens Listenbox in your browser. `listenbox login` uses the same authorization flow. Both save and read one private credential in `~/.config/listenbox/auth.json`; signing in through either authorizes both.

```sh
listenbox login
listenbox shows list
listenbox import --slug field-notes 'https://www.youtube.com/playlist?list=PLAYLIST_ID'
listenbox shows sync youtube --show field-notes
listenbox shows sync youtube --show field-notes --watch
```

An active paid audio or video plan is required for synchronization. The desktop automatically syncs on startup and every hour while running. Sync now requests an immediate pass. Its library lists only existing YouTube imports, including accessible imports shared with you. Import creates a fresh podcast; its source is fixed. A URL is needed only for import. Sync and watch need just the slug. A source cannot coexist with a YouTube publishing destination; PostgreSQL enforces both directions. The backend checks permissions and video allowance when admitting work.

The engine requests a listing that includes hidden videos and follows every continuation before reconciling canonical item URLs. Each new video ID gets one playback check per sync, even when playlist metadata labels it unavailable. Playable videos become episodes; unavailable videos are skipped without retries until the next sync. A playlist with no playable videos can still be imported and populated later. Informational and warning alerts allow imports but prevent deleting missing episodes because YouTube may have hidden them. Removed videos are deleted only after an alert-free scan and only when imported into this podcast. Unavailable videos and manually uploaded episodes are preserved. Playlist order becomes RSS order; real publication dates stay intact. Reordering does not repeat media work.

For creator-managed podcasts, read or change order through the same ordering API:

```sh
listenbox shows order --show field-notes
listenbox shows order --show field-notes --episode ep_0123456789abcdef --episode ep_fedcba9876543210
```

Supplied episodes move to the front in sequence; other episodes retain their relative order. Sync restores playlist order for imported podcasts.

## Interrupted work

Both interfaces use the engine's persistent sync path. It opens `~/.config/listenbox/sync.sqlite` only for sync work and keeps media under `~/.config/listenbox/transfers`. Refinery applies embedded SQL migrations with strict history validation. Neither interface accesses SQLite directly.

One writer connection serializes changes; pooled read-only connections read concurrently through WAL. FULL synchronous commits and macOS full-fsync preserve acknowledged checkpoints. Closing the desktop window leaves sync running. The menu-bar icon offers Open Listenbox and Quit Listenbox. Quit cancels active work and waits for admitted writes and processing to finish. A second ⌘Q press within one second or holding it for two seconds confirms keyboard quit; quitting waits for key release. Log out from the account controls also drains active work and removes the shared credential, keeping resumable work.

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

See `crates/desktop/DESIGN.md` for native tokens and component conventions.

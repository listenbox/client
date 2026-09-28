# Listenbox client

A Rust monorepo for the Listenbox desktop app, CLI and shared synchronization engine.

| Crate | Responsibility |
| --- | --- |
| `crates/desktop` | Native GPUI Kit interface, live teams and podcasts, transfer progress |
| `crates/cli` | Commands, terminal progress and foreground watch service |
| `crates/sync-engine` | Authentication, generated public API, reconciliation, durable transfers, FFmpeg and SQLite |
| `crates/youtubei` | Embedded YouTube.js bindings, checked-in Rust source |

## Build

Install Rust via rustup, [Moon](https://moonrepo.dev), [pkgx](https://pkgx.sh), [kache 0.27.0](https://github.com/kunobi-ninja/kache/releases/tag/v0.27.0), a C compiler, make and tar. Rust is pinned in `rust-toolchain.toml`. Source builds also use Node.js 26 and [Aube](https://aube.sh); shipped applications need neither.

```sh
# Install once per machine, outside a checkout (or use the prebuilt release).
cargo install --locked kache --version 0.27.0

git clone --recurse-submodules https://github.com/listenbox/client.git
cd client
moon run client:build
moon ci
```

The debug app is `crates/desktop/dist/listenbox-desktop`; the terminal executable is `crates/cli/dist/listenbox`. `moon run desktop:build-release` builds the production desktop executable for the host architecture at `crates/desktop/dist/release/listenbox-desktop`. `moon run desktop:dmg` packages its Apple Silicon build as a DMG. `moon run client:package` packages release CLI and desktop binaries with notices.

FFmpeg 9.0.2 is built from verified source by `tools/native-ffmpeg.rs`, linked with `ffmpeg-the-third`, and never run as a subprocess. The `youtubei` crate embeds our pinned `vendor/youtubejs` source build in QuickJS. Moon installs locked JavaScript build dependencies and creates the bundle before Cargo; Cargo never downloads a prebuilt YouTube.js bundle. Both applications are self-contained. See `THIRD-PARTY-NOTICES.txt` and the packaged FFmpeg source/license notice.

### Moon and Cargo

The task setup follows the [Moon Rust handbook](https://moonrepo.dev/docs/guides/rust/handbook).
`.moon/toolchains.yml` enables Rust integration, and the parent Listenbox
workspace extends that same configuration. `rust-toolchain.toml` remains the
single toolchain version and component pin for both direct Cargo and Moon runs.
Moon reads Cargo manifests and the lockfile for dependency tracking and hashing,
and infers local crate relationships from Cargo path dependencies. Cargo owns
compilation and its build-directory lock; concurrent crate tasks may wait for it.

From either workspace:

```sh
moon run client:lint       # Format Rust, check shell scripts, and run Clippy
moon run client:test       # Run each crate's existing test runner
moon run client:check      # Secrets, lint, tests, and debug builds
```

Build, lint, and test tasks retain the native FFmpeg prerequisite.
In the parent workspace, it also waits for the OpenAPI client and configuration
generators; standalone checkouts use the committed generated Rust sources.
Moon caches deliverable binaries under `dist/`, never Cargo's `target/` directory.

### Worktree disk usage

Cargo downloads share `registry` and `git` under `CARGO_HOME` (normally
`~/.cargo`). `.cargo/config.toml` routes both direct Cargo commands and Moon tasks
through [kache](https://github.com/kunobi-ninja/kache). Install its pinned version
once on `PATH`; no global `kache init` or background service is needed for local
caching. The parent workspace uses this same configuration.

Each checkout keeps its own `target/` and Cargo build lock. Kache stores compatible
compiler artifacts once in the user-level content-addressed store:

| Platform | Shared store |
| --- | --- |
| macOS | `~/Library/Caches/kache` |
| Linux | `${XDG_CACHE_HOME:-~/.cache}/kache` |
| Windows | `%LOCALAPPDATA%\kache` |

Restores use copy-on-write clones or hardlinks when supported. Keep the store and
worktrees on the **same filesystem**; a cross-filesystem restore copies data and
loses the disk saving. `KACHE_CACHE_DIR` can select a shared location on that
filesystem. Never point it inside a worktree, and never share `CARGO_TARGET_DIR`
or `build.build-dir` between worktrees.

`.kache.toml` disables adaptive/preserved incremental state. Cargo also disables
incremental compilation, and the development profile omits debug information.
Cache retention uses kache's defaults or the machine owner's environment settings;
the project imposes no size budget. Uncached outputs, native FFmpeg builds, Moon
archives, and packaged binaries still take space in each checkout. Ten identical
builds can share eligible compiler artifacts; ten different revisions still need their
unique outputs. Windows keeps kache's default exclusion of executable caching.

Inspect actual reuse and retention from the client directory:

```sh
kache doctor
kache report --last-build
kache targets
```

Enabling kache does not retroactively deduplicate existing `target/` files. After
stopping builds in an existing worktree, `cargo clean` and the next Moon build
repopulate it through kache. Inspect `kache targets` before cleaning unused
worktrees; deleting a target can release blocks retained by its outputs.
`kache gc` reclaims eligible unreferenced entries. FFmpeg remains in this
checkout's `.cache/ffmpeg`.

Linux CI in both repositories and the client's macOS/Windows release jobs install
kache 0.27.0 with the official action. The store sits under `runner.temp` on the
build filesystem. `actions/cache` persists that store and Cargo downloads, never
`target/`. Each commit gets a new key, with compatible restore prefixes, so
source-only changes refresh the cache too. The action's separate GitHub cache
is disabled to avoid retaining a second copy. Cache keys separate platforms,
kache/Rust versions and native configurations; Windows also includes MSVC and LLVM.
GitHub caches remain separate for each repository.

Linux and macOS also retain `.moon/cache/outputs`. Moon can restore a matching
FFmpeg build into a fresh checkout; native preparation remains a prerequisite
when archives are absent or inputs change. Windows keeps its separate FFmpeg
cache and saves it before Cargo runs. The parent still builds the CLI and desktop
test executable for integrated tests; release packaging stays in this repository.

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

The same builds can run on native Windows machines of the matching architecture with Rust, Moon, Visual Studio's C++ tools for that architecture and Windows SDK, host-native [LLVM 22.1.8](https://github.com/llvm/llvm-project/releases/tag/llvmorg-22.1.8), cargo-nextest, and MSYS2 (`make`, `diffutils`, `tar`, `xz`, `openssl`). CI installs checksum-verified LLVM 22.1.8 for each architecture: LLVM 20/21 have a [libclang unload crash](https://github.com/llvm/llvm-project/issues/154361) exposed when generating FFmpeg bindings from a clean target directory. Set `MSYS2_LOCATION` to the MSYS2 installation directory and, if LLVM is not installed at `C:\Program Files\LLVM`, set `LIBCLANG_PATH` to the directory containing `libclang.dll`:

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

## YouTube sign-in checks

When YouTube asks you to sign in, open **Settings → YouTube** in the desktop app
(⌘, on macOS or Ctrl+, on Windows). Paste a **Netscape cookies.txt** export and
choose **Save cookies**, then return to the podcast and choose **Sync now**.
The CLI accepts the same format, without requiring a Listenbox login to save it:

```sh
listenbox youtube-cookies import /path/to/cookies.txt
listenbox shows sync youtube --show field-notes
listenbox youtube-cookies remove
```

Follow [the private-session export guide](https://listenbox.app/guides/import-youtube-as-a-podcast/#when-youtube-asks-you-to-sign-in).
Cookies stay in the shared profile's `youtube-cookies/jar.json`. The app never
shows the saved contents. A new import replaces the session; removal returns
future requests to anonymous access. Only YouTube receives these credentials;
media hosts and the Listenbox API do not.

The sync engine reads immutable snapshots without a reader lock. Writers take
an OS file lock, compare the request's generation and per-cookie revisions,
merge unrelated updates, then sync and atomically replace the snapshot. Late
responses cannot overwrite a newer rotation, resurrect a deleted cookie, or
undo a user replacement/removal. Interrupted writes leave the prior complete
snapshot readable. Response cookies are committed before consuming the body.
Unix directories are private (0700) and snapshots are 0600; Windows uses the
user profile's inherited access controls.

Tests use synthetic cookie exports and local YouTube fixtures. The optional
`verify_youtube` example is a manual live probe: it copies an explicitly supplied
export into a temporary profile, checks playlist extraction and media ranges,
and deletes that profile on exit. It never publishes and is not run by CI.

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

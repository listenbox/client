# Listenbox desktop

A native desktop application written in Go with [MyGo](https://mygo.run/).
The shared sync engine owns authorization, YouTube imports, resumable transfers
and media preparation. The desktop owns navigation, settings, progress and the
lifetime of admitted work.

[Import YouTube videos as a podcast](https://listenbox.app/guides/import-youtube-as-a-podcast/)
explains installation, sign-in, synchronization and YouTube cookies.

Run `mise install --locked` from the client root, then `moon run desktop:build`.
The user can start local development with `moon run desktop:dev` after starting
the parent workspace's local services (`moonx dev` from the repository root).
MyGo rebuilds and restarts the app as Go code changes. The task sets
`LISTENBOX_CLIENT_CONFIG` to `config/dev.yaml`, so the app talks to the local API
and Dashboard; `--config` takes precedence, and without either the app uses the
production origins. Development uses the shared debug profile;
`LISTENBOX_PROFILE_DIR` overrides it.

`moon run desktop:package` exercises MyGo's distribution tools. Native releases
use `desktop:dmg` on macOS ARM64 and `desktop:build-release-windows-x64` or
`desktop:build-release-windows-arm64` on Windows. MyGo generates native resources
and bundles; its macOS flow signs nested helpers and notarizes/staples the app
and DMG. Windows keeps the Inno Setup installer so existing WinSparkle clients
retain their installation identity and upgrade arguments.

The native release workflow follows
[MyGo Music's build matrix](https://github.com/egoist/mygo-music/blob/main/.github/workflows/release.yml):
Bash on every runner, Go-reported cache paths and MyGo's build command. Each
Windows architecture has its own native CGO/codec SDK. The shared
`tools/build-desktop.sh` builds and checks each payload; successful master CI
still reserves the next immutable version before dispatching native builds.

The window keeps its native titlebar, menus and a status item (Show Listenbox
and Quit); closing it hides the app. Cover artwork goes through a
workspace-owned cache that coalesces requests and decoding, keeps at most 128
covers, releases evicted GPU images, prunes covers that leave the catalog and
clears on logout or Reload. Its HTTP client persists bodyless GET responses
under the profile's `http-cache/` with shared-cache freshness and revalidation,
and startup trims that store past 128 MiB. Preview assets are synthetic
fixtures with provenance in `tests/fixtures/README.md`.

Sparkle and WinSparkle continue to own update checks, preferences and signature
verification. MyGo's own updater uses a different manifest format and is not
enabled. See [native updates](../../docs/native-updates.md).

`moon run client:test` runs the portable tests and headless MyGo interactions.
The parent API E2E suite supplies real API, storage and YouTube fixtures to the
live desktop drivers. It verifies import, sync, interruption, recovery and
logout through the production workspace. Native menu/chrome, signed installed
upgrades and OS performance still require macOS or Windows verification.

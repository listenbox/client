# Listenbox desktop

A native desktop application written in Go with [MyGo](https://mygo.run/).
The shared sync engine owns authorization, YouTube imports, resumable transfers
and media preparation. The desktop owns navigation, settings, progress and the
lifetime of admitted work.

[Import YouTube videos as a podcast](https://listenbox.app/guides/import-youtube-as-a-podcast/)
explains installation, sign-in, synchronization and YouTube cookies.

Run `mise install --locked` from the client root, then `moon run desktop:build`.
The user can start local development with `moon run desktop:dev` after starting
the parent workspace's local services. Development uses the shared debug profile;
`LISTENBOX_PROFILE_DIR` overrides it.

`moon run desktop:package` exercises MyGo's distribution tools. Native releases
use `desktop:dmg` on macOS ARM64 and `desktop:build-release-windows-x64` or
`desktop:build-release-windows-arm64` on Windows. MyGo generates native resources
and bundles; its macOS flow signs nested helpers and notarizes/staples the app
and DMG. Windows keeps the Inno Setup installer so existing WinSparkle clients
retain their installation identity and upgrade arguments.

Sparkle and WinSparkle continue to own update checks, preferences and signature
verification. MyGo's own updater uses a different manifest format and is not
enabled. See [native updates](../../docs/native-updates.md).

`moon run client:test` runs the portable tests and headless MyGo interactions.
The parent API E2E suite supplies real API, storage and YouTube fixtures to the
live desktop drivers. It verifies import, sync, interruption, recovery and
logout through the production workspace. Native menu/chrome, signed installed
upgrades and OS performance still require macOS or Windows verification.

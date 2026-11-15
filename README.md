# Listenbox

**Agent-friendly audio and video podcast hosting.**

[Listenbox](https://listenbox.app/) gives your recordings a home and your audience
a podcast feed. Publish audio and video, bring in your YouTube catalog, and keep
new episodes in sync.

Let your agent handle the publishing work. The open-source CLI gives any agent
with a shell the same commands you use to create podcasts, upload recordings,
and manage episodes. Keep uploads as drafts for review or publish when ready.

[CLI commands](crates/cli/README.md) ·
[Desktop app](crates/desktop/README.md) ·
[Shared sync engine](crates/sync-engine/README.md)

## Development

Install [Mise](https://mise.jdx.dev/), initialize Git submodules recursively,
then run `mise install --locked` and `moon run client:check`.
[mise.toml](mise.toml) pins Go, Node, Aube, Bun, Moon, ShellCheck and
the native build tools. Mise only provisions tools; every task belongs to Moon.
The parent workspace's Mise config is inherited when working in its submodule.
This checkout repeats shared pins so it also builds independently.

Go tools, including MyGo, OASmith and golangci-lint, are pinned in `go.mod` and
invoked with `go tool`. Code generation downloads the published OpenAPI document
from [the public API](https://v1.listenbox.app/); it needs no private specification
sources. Moon caches the document for a day. Run
`moon run client:public-contract --force` to refresh it immediately.
Generated SDK and configuration code stay
Git-ignored and are rebuilt before compilation.

The CLI and desktop share Go code, tailscale/sqlite, an Atlas declarative
schema, buke/quickjs-go with the real YouTube.js bundle, and astiav with static
FFmpeg/OpenH264. End users need no Go, Node or FFmpeg installation.

### Platform prerequisites

- **macOS:** Xcode with macOS components and Command Line Tools. Mise supplies
  LLVM, CMake, Ninja, Make, pkg-config and other native build CLIs.
- **Linux (Ubuntu 24.04):** `build-essential`, `libasound2-dev`, `libfontconfig1-dev`, `libwayland-dev`,
  `libxkbcommon-dev`, `libxkbcommon-x11-dev`, `libxcb1-dev`, `libx11-xcb-dev`,
  `libx11-dev`, `libegl1-mesa-dev`, and `libvulkan-dev`.
- **Windows:** MSYS2 CLANG64 (x64) or CLANGARM64 (ARM64), with its
  `mingw-w64-clang-x86_64-toolchain` or `mingw-w64-clang-aarch64-toolchain`
  group and matching `pkgconf` package, plus MSYS `make`, `diffutils`, `tar`,
  `xz` and `openssl`. x64 additionally needs `mingw-w64-clang-x86_64-nasm`.
  Set `MSYS2_LOCATION` before provisioning the scoped Mise environment.
  Use a host-native Go toolchain; release builds run on each architecture.

MyGo builds bundles and native executable resources. macOS release packaging
uses its Developer ID signing and notarization support. Windows retains the
existing Inno Setup installer identity for upgrades from previous releases.
See [native updates](docs/native-updates.md) and the
[migration verification](docs/go-migration.md).

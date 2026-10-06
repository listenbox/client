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

## Required development tools

- [rustup](https://rustup.rs/) — Rust, Cargo, Clippy, and rustfmt, pinned by
  [rust-toolchain.toml](rust-toolchain.toml).
- [Moon](https://moonrepo.dev/) 2.5.5 — workspace tasks.
- [kache](https://github.com/kunobi-ninja/kache/releases/tag/v0.27.0) 0.27.0 —
  Rust compiler caching.
- [Node.js](https://nodejs.org/) 26 and [Aube](https://aube.sh/) 2.5.1 —
  embedded YouTube.js build dependencies.
- [cargo-nextest](https://nexte.st/) 0.9.146 — Rust test runner.
- [ShellCheck](https://www.shellcheck.net/) and jq — shell checks and build tooling.
- Git, Bash, curl, make, tar, xz, and patch.
- A C/C++ compiler, CMake, libclang, and pkg-config — native dependencies.

### Platform prerequisites

- **macOS:** Xcode with macOS components and Command Line Tools, LLVM, and pkgconf.
- **Linux (Ubuntu 24.04):** `build-essential`, `cmake`, `clang`, `libclang-dev`,
  `pkg-config`, `libasound2-dev`, `libfontconfig1-dev`, `libwayland-dev`,
  `libxkbcommon-dev`, `libxkbcommon-x11-dev`, `libxcb1-dev`, `libx11-xcb-dev`,
  `libx11-dev`, `libegl1-mesa-dev`, `libvulkan-dev`, and `libssl-dev`.
- **Windows:** Visual Studio C++ Build Tools for the target architecture,
  Windows SDK, host-native LLVM 22.1.8, and MSYS2 with `make`, `diffutils`,
  `tar`, `xz`, and `openssl`.

#!/usr/bin/env bash
set -euo pipefail

client_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$client_root"

version=0.7.10
host="$(kache rustc -vV | sed -n 's/^host: //p')"
[[ -n "$host" ]] || { echo 'Rust did not report its host target.' >&2; exit 1; }
install_root="${XDG_CACHE_HOME:-$HOME/.cache}/listenbox/build-tools/dioxus-cli/$version/$host"

if [[ ! -x "$install_root/bin/dx" ]]; then
  mkdir -p .cache
  build_dir="$(mktemp -d "$client_root/.cache/dioxus-cli.XXXXXX")"
  trap 'rm -rf "$build_dir"' EXIT
  # Registry installs ignore project Cargo configuration. Set the wrapper and
  # isolate temporary artifacts explicitly; kache keeps the reusable outputs.
  RUSTC_WRAPPER=kache CARGO_INCREMENTAL=0 KACHE_CONFIG="$client_root/.kache.toml" \
    CARGO_TARGET_DIR="$build_dir" CARGO_BUILD_BUILD_DIR="$build_dir" \
    kache cargo -- install --locked --root "$install_root" dioxus-cli --version "$version"
fi

mkdir -p crates/desktop/dist/dev-tools/bin
ln -sfn "$install_root/bin/dx" crates/desktop/dist/dev-tools/bin/dx

#!/usr/bin/env bash
set -euo pipefail

client_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$client_root"

version=0.7.10
watch_patch="$client_root/tools/dioxus-watch.patch"
patch_hash="$(shasum -a 256 "$watch_patch" | cut -d ' ' -f 1)"
host="$(kache rustc -vV | sed -n 's/^host: //p')"
[[ -n "$host" ]] || { echo 'Rust did not report its host target.' >&2; exit 1; }
install_root="${XDG_CACHE_HOME:-$HOME/.cache}/listenbox/build-tools/dioxus-cli/$version/$patch_hash/$host"

if [[ ! -x "$install_root/bin/dx" ]]; then
  mkdir -p .cache
  build_dir="$(mktemp -d "$client_root/.cache/dioxus-cli.XXXXXX")"
  trap 'rm -rf "$build_dir"' EXIT
  curl --fail --location --silent --show-error \
    "https://static.crates.io/crates/dioxus-cli/dioxus-cli-$version.crate" \
    -o "$build_dir/source.crate"
  tar -xzf "$build_dir/source.crate" -C "$build_dir"
  source_dir="$build_dir/dioxus-cli-$version"
  patch --batch --fuzz=0 -p1 -d "$source_dir" < "$watch_patch"
  # The downloaded crate is independent of the enclosing client workspace.
  printf '\n[workspace]\n' >> "$source_dir/Cargo.toml"
  # Install the patched pinned crate with isolated temporary artifacts. Set the
  # wrapper explicitly; kache keeps reusable outputs across worktrees.
  RUSTC_WRAPPER=kache CARGO_INCREMENTAL=0 KACHE_CONFIG="$client_root/.kache.toml" \
    CARGO_TARGET_DIR="$build_dir/target" CARGO_BUILD_BUILD_DIR="$build_dir/target" \
    kache cargo -- install --locked --root "$install_root" --path "$source_dir"
fi

mkdir -p crates/desktop/dist/dev-tools/bin
ln -sfn "$install_root/bin/dx" crates/desktop/dist/dev-tools/bin/dx

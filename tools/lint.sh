#!/usr/bin/env bash
set -euo pipefail

cargo fmt --all -- tools/*.rs
shellcheck tools/*.sh vendor/*.sh
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings

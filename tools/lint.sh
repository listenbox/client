#!/usr/bin/env bash
set -euo pipefail

kache cargo -- fmt --all -- tools/*.rs
shellcheck tools/*.sh crates/desktop/tools/*.sh vendor/*.sh
kache cargo -- clippy --locked --workspace --all-targets --all-features -- -D warnings

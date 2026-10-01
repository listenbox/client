#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")"
mkdir -p bundle
temporary_bundle="$(mktemp bundle/cf-worker.js.XXXXXX)"
trap 'rm -f "$temporary_bundle"' EXIT

aube -C youtubejs exec --no-install esbuild \
  src/platform/cf-worker.ts \
  --bundle --target=es2020 --keep-names --minify --format=esm \
  --define:global=globalThis --conditions=module --platform=node \
  --outfile="../$temporary_bundle"

# Cargo watches this file's mtime through youtubei's build script.
if ! cmp -s "$temporary_bundle" bundle/cf-worker.js; then
  mv "$temporary_bundle" bundle/cf-worker.js
fi

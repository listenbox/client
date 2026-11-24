#!/usr/bin/env bash
# Records the embedded YouTube.js bundle's identity: the pinned fork commit, its
# committer time (the sequence signed releases are ordered by), size and
# digest. Moon cannot see submodule pins, so this runs every time and only
# rewrites the file when the identity changed.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
bundle=third_party/bundle/quickjs.js
read -r commit sequence < <(git -C third_party/youtubejs log -1 --format='%H %ct')
size="$(stat -c %s "$bundle" 2>/dev/null || stat -f %z "$bundle")"
digest="$(openssl dgst -sha256 -r "$bundle" | cut -d ' ' -f 1)"
identity="$(printf '{"commit":"%s","sequence":%s,"sha256":"%s","size":%s}' "$commit" "$sequence" "$digest" "$size")"
if [[ "$(cat third_party/bundle/quickjs.json 2>/dev/null)" != "$identity" ]]; then
  printf '%s' "$identity" > third_party/bundle/quickjs.json
fi

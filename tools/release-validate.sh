#!/usr/bin/env bash
set -euo pipefail

release_tag="${1:-}"
node tools/release.mjs validate "$release_tag"
current=true
source_commit="$(git show -s --format='%(trailers:key=Source-commit,valueonly)' HEAD)"
if [[ -n "$release_tag" && -n "$source_commit" ]]; then
  if [[ "$(git rev-parse HEAD^)" != "$source_commit" ]]; then
    echo 'Release source trailer does not match its parent commit.' >&2
    exit 1
  fi
  git fetch --no-recurse-submodules origin master
  if [[ "$source_commit" != "$(git rev-parse origin/master)" ]]; then
    echo 'Skipping native builds for a superseded release source.'
    current=false
  fi
fi
if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  printf 'current=%s\n' "$current" >> "$GITHUB_OUTPUT"
fi

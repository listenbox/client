#!/usr/bin/env bash
set -euo pipefail

source_commit="$(git rev-parse HEAD)"
if [[ $# != 1 || "$1" != "$source_commit" ]]; then
  echo 'Release preparation requires the checked-out CI source commit.' >&2
  exit 1
fi
if ! git diff --quiet || ! git diff --cached --quiet; then
  echo 'Release preparation requires a clean checkout.' >&2
  exit 1
fi

git fetch --no-recurse-submodules origin master --tags
if [[ "$source_commit" != "$(git rev-parse origin/master)" ]]; then
  echo 'Skipping CI for a superseded master update.'
  exit 0
fi

stable_key() {
  local major minor patch
  [[ "$1" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || return 1
  IFS=. read -r major minor patch <<< "$1"
  if [[ ${#major} -gt 5 || ${#minor} -gt 5 || ${#patch} -gt 5 ]] ||
    (( major > 65535 || minor > 65535 || patch > 65535 )); then
    echo 'Version component exceeds Windows limit.' >&2
    exit 1
  fi
  printf '%05d%05d%05d' "$major" "$minor" "$patch"
}

finish() {
  node tools/release.mjs validate "$release_tag"
  if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    printf 'tag=%s\nsource=%s\n' "$release_tag" "$source_commit" >> "$GITHUB_OUTPUT"
  fi
}

base_version="$(node tools/release.mjs validate | jq -er .version)"
base_key="$(stable_key "$base_version")"
latest_version=''
latest_key=''
tags="$(git tag --list 'v*')"
while IFS= read -r tag; do
  if ! key="$(stable_key "${tag#v}")"; then
    continue
  fi
  tagged_commit="$(git rev-parse "$tag^{commit}")"
  tagged_source="$(git show -s --format='%(trailers:key=Source-commit,valueonly)' "$tag")"
  if [[ "$tagged_commit" == "$source_commit" ]] ||
    { [[ "$tagged_source" == "$source_commit" ]] &&
      [[ "$(git rev-parse "$tagged_commit^")" == "$source_commit" ]]; }; then
    release_tag="$tag"
    git checkout --detach "$tagged_commit"
    finish
    exit 0
  fi
  if [[ "$key" > "$latest_key" ]]; then
    latest_version="${tag#v}"
    latest_key="$key"
  fi
done <<< "$tags"

next_version="$base_version"
if [[ -n "$latest_key" && ( "$base_key" < "$latest_key" || "$base_key" == "$latest_key" ) ]]; then
  IFS=. read -r major minor patch <<< "$latest_version"
  if (( patch == 65535 )); then
    echo 'Patch versions are exhausted; increase the workspace major/minor version.' >&2
    exit 1
  fi
  next_version="$major.$minor.$((patch + 1))"
fi
release_tag="v$next_version"
git checkout --detach "$source_commit"

manifest="$(mktemp)"
trap 'rm -f "$manifest"' EXIT
awk -v version="$next_version" '
  /^\[/ { workspace = ($0 == "[workspace.package]") }
  workspace && /^version = / { $0 = "version = \"" version "\"" }
  { print }
' Cargo.toml > "$manifest"
cat "$manifest" > Cargo.toml
kache cargo -- update --workspace

source_date="$(git show -s --format=%cI "$source_commit")"
GIT_AUTHOR_DATE="$source_date" GIT_COMMITTER_DATE="$source_date" \
  git -c user.name='github-actions[bot]' \
    -c user.email='41898282+github-actions[bot]@users.noreply.github.com' \
    -c commit.gpgsign=false commit --quiet --allow-empty --only Cargo.toml Cargo.lock \
    -m "Release $release_tag" -m "Source-commit: $source_commit"
git tag "$release_tag"
finish

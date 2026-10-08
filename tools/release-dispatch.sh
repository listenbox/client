#!/usr/bin/env bash
set -euo pipefail

if [[ $# != 2 || ! "$1" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
  echo 'Usage: release-dispatch.sh TAG SOURCE_COMMIT' >&2
  exit 1
fi
release_tag="$1"
source_commit="$2"
node tools/release.mjs validate "$release_tag"
tagged_commit="$(git rev-parse "$release_tag^{commit}")"
tagged_source="$(git show -s --format='%(trailers:key=Source-commit,valueonly)' "$release_tag")"
if [[ "$tagged_commit" != "$source_commit" ]] &&
  { [[ "$tagged_source" != "$source_commit" ]] ||
    [[ "$(git rev-parse "$tagged_commit^")" != "$source_commit" ]]; }; then
  echo 'Release tag does not belong to the requested CI source.' >&2
  exit 1
fi
git fetch --no-recurse-submodules origin master
if [[ "$source_commit" != "$(git rev-parse origin/master)" ]]; then
  echo 'Skipping a release superseded during preparation.'
  exit 0
fi

git push origin "refs/tags/$release_tag"
repository='listenbox/client'
if response="$(gh api --include "repos/$repository/releases/tags/$release_tag" --jq .draft)"; then
  if [[ "$(tail -n 1 <<< "$response")" == false ]]; then
    echo "$release_tag is already published."
    exit 0
  fi
elif [[ "$(awk 'NR == 1 { print $2 }' <<< "$response")" != 404 ]]; then
  echo 'Could not determine whether the release is already published.' >&2
  exit 1
fi
gh workflow run release.yaml --repo "$repository" --ref "$release_tag"

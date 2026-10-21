#!/usr/bin/env bash
set -euo pipefail

desktop_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
client_root="$(cd "$desktop_root/../.." && pwd)"
mode="${1:-capture}"
label="${2:-startup}"
kind="${3:-timing}"
fail() { echo "$*" >&2; exit 1; }
if [[ "$mode" == --help ]]; then
  echo 'moon run desktop:profile-startup -- [label=startup] [timing|cpu=timing]'
  echo 'moon run desktop:test-startup'
  exit 0
fi
[[ $# -le 3 && "$label" =~ ^[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}$ ]] || fail 'Invalid startup label or arguments'
[[ "$mode" == capture || "$mode" == check ]] || fail 'Expected capture or check'
[[ "$kind" == timing || "$kind" == cpu ]] || fail 'Expected timing or cpu'
[[ "$(uname -s)" == Darwin ]] || fail 'Startup profiling requires macOS and a visible, unlocked display'
cd "$client_root"
mkdir -p "$desktop_root/dist/profiles/startup"
capture="$(mktemp -d "$desktop_root/dist/profiles/startup/$(date +%Y%m%d-%H%M%S)-$label.XXXXXX")"
trap 'printf "Startup artifacts: %s\n" "$capture"' EXIT
kache rustc --crate-name package_macos --edition 2024 --out-dir "$capture" tools/package-macos.rs
"$capture/package_macos" --profiled "$capture"
app="$capture/image/Listenbox.app"
ditto "$desktop_root/dist/profiled/listenbox-desktop.dSYM" "$capture/listenbox-desktop.dSYM"
xcrun clang -fobjc-arc -fblocks -Wall -Wextra -Werror -framework AppKit -framework QuartzCore \
  "$desktop_root/tools/startup-launch.m" -o "$capture/launch"
xcrun clang -dynamiclib -fobjc-arc -fblocks -Wall -Wextra -Werror \
  -framework Foundation -framework QuartzCore -framework Metal \
  "$desktop_root/tools/startup-observer.m" -o "$capture/observer.dylib"
mkfifo "$capture/events"
arguments=("$app" "$capture/observer.dylib" "$capture/events")
if [[ "$mode" == check ]]; then
  # A real process, renderer, and native certificate reader. No sleeps, retries,
  # synthetic rendered-frame events, or production authentication/state.
  export LISTENBOX_PROFILE_DIR="$capture/profile"
  mkfifo "$capture/certificates.pem"
  arguments+=("$capture/certificates.pem")
fi
"$app/Contents/MacOS/listenbox-desktop" --version > "$capture/build.txt"
git status --porcelain > "$capture/worktree.txt"
git diff --binary HEAD > "$capture/worktree.patch"
echo 'Measuring Launch Services request -> first displayed Metal frame.'
if [[ "$mode" == capture ]]; then
  "$capture/launch" "${arguments[@]}" > "$capture/fresh-copy-result.json"
fi
"$capture/launch" "${arguments[@]}" > "$capture/result.json"
jq -e '.request_to_first_presented_ms > 0 and .request_to_first_presented_ms < 5000' "$capture/result.json" >/dev/null
jq '{request_to_first_presented_ms, certificate_gate, measurement}' "$capture/result.json"
if [[ "$mode" == capture ]]; then
  jq -s '{fresh_copy_ms: .[0].request_to_first_presented_ms, warm_ms: .[1].request_to_first_presented_ms}' \
    "$capture/fresh-copy-result.json" "$capture/result.json"
fi
printf '%s\n' "$capture" > "$desktop_root/dist/profiles/startup/latest.txt"
if [[ "$kind" == cpu ]]; then
  # This separate launch adds Instruments overhead; use result.json for timing.
  # Launch through the helper so the task exits through the app's graceful Quit.
  notification="app.listenbox.startup.$(uuidgen)"
  mkfifo "$capture/ready"
  LISTENBOX_STARTUP_NOTIFY="$notification" LISTENBOX_STARTUP_READY="$capture/ready" \
    "$capture/launch" "${arguments[@]}" > "$capture/instrumented-result.json" &
  launcher=$!
  read -r ready < "$capture/ready"
  [[ "$ready" == ready ]] || fail 'Startup launcher did not register its recording gate'
  xcrun xctrace record --template 'App Launch' --all-processes --time-limit 3s \
    --notify-tracing-started "$notification" --output "$capture/recording.trace" --no-prompt
  wait "$launcher"
  xcrun xctrace symbolicate --input "$capture/recording.trace" --dsym "$capture/listenbox-desktop.dSYM"
  xcrun xctrace export --input "$capture/recording.trace" --toc --output "$capture/toc.xml"
  xcrun xctrace export --input "$capture/recording.trace" \
    --xpath '/trace-toc/run[@number="1"]/data/table[@schema="time-profile"]' \
    --output "$capture/cpu-samples.xml"
fi

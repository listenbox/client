#!/usr/bin/env bash
set -euo pipefail

desktop_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
client_root="$(cd "$desktop_root/../.." && pwd)"
profiles="$desktop_root/dist/profiles"
binary="$desktop_root/dist/profiled/listenbox-desktop"

fail() { echo "$*" >&2; exit 1; }
usage() {
  echo 'moon run desktop:profiled'
  echo 'moon run desktop:profile -- [label=interaction] [seconds=10] [cpu|metal=cpu]'
}
if [[ "${1:-}" == --help || "${2:-}" == --help ]]; then usage; exit 0; fi
[[ "$(uname -s)" == Darwin ]] || fail 'Desktop profiling requires macOS and Xcode Instruments.'

mode="${1:-}"
shift || { usage; exit 64; }
case "$mode" in
  build)
    [[ $# == 0 ]] || fail 'build takes no arguments'
    cd "$client_root"
    # dSYM packing needs the original object paths, without compiler-cache remapping.
    CARGO_TARGET_DIR="$client_root/target/profiled" RUSTC_WRAPPER='' \
      cargo build --locked --profile profiling -p listenbox-desktop --features profiling
    mkdir -p "$desktop_root/dist/profiled"
    cp target/profiled/profiling/listenbox-desktop "$binary"
    rm -rf "$binary.dSYM"
    ditto target/profiled/profiling/listenbox-desktop.dSYM "$binary.dSYM"
    exit 0
    ;;
  launch)
    [[ $# == 0 ]] || fail 'profiled takes no arguments; configure data with LISTENBOX_PROFILE_DIR'
    command -v jq >/dev/null || fail 'jq is required for desktop profiling'
    mkdir -p "$profiles/sessions"
    if [[ -f "$profiles/active.json" ]]; then
      previous="$(jq -r '.pid' "$profiles/active.json")"
      if [[ "$(ps -p "$previous" -o comm= || true)" == "$binary" ]]; then
        fail "Profiled desktop $previous is already running. Quit it before launching another."
      fi
    fi
    session="$(mktemp -d "$profiles/sessions/$(date +%Y%m%d-%H%M%S).XXXXXX")"
    ditto "$binary.dSYM" "$session/listenbox-desktop.dSYM"
    jq -n --argjson pid "$$" --arg binary "$binary" --arg session "$session" \
      '{pid: $pid, binary: $binary, session: $session}' > "$profiles/active.json.tmp"
    mv "$profiles/active.json.tmp" "$profiles/active.json"
    export LISTENBOX_PROFILE_DIR="${LISTENBOX_PROFILE_DIR:-$HOME/.cache/listenbox/dev}"
    export LISTENBOX_RENDER_PROFILE="$session/gpui.jsonl"
    echo "Profiled desktop PID: $$"
    echo "Live GPUI events: $LISTENBOX_RENDER_PROFILE"
    echo "Application data: $LISTENBOX_PROFILE_DIR"
    echo 'In another terminal: moon run desktop:profile -- scroll'
    exec "$binary" --config "$client_root/config/dev.yaml"
    ;;
  capture) ;;
  *) usage; exit 64 ;;
esac

[[ $# -le 3 ]] || fail 'Expected at most label, seconds, and cpu or metal'
label="${1:-interaction}"
seconds="${2:-10}"
kind="${3:-cpu}"
[[ "$label" =~ ^[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}$ ]] || fail 'Label must be 1–64 letters, digits, underscores or hyphens'
[[ "$seconds" =~ ^[1-9][0-9]?$ ]] || fail 'Duration must be 1–99 seconds'
case "$kind" in
  cpu) template='Time Profiler' ;;
  metal) template='Metal System Trace' ;;
  *) fail 'Capture kind must be cpu or metal' ;;
esac
for tool in jq xmllint xcrun; do
  command -v "$tool" >/dev/null || fail "$tool is required for desktop profiling"
done
[[ -f "$profiles/active.json" ]] || fail 'Start moon run desktop:profiled in another terminal first'
pid="$(jq -er '.pid | select(type == "number" and . > 0)' "$profiles/active.json")"
session="$(jq -er '.session' "$profiles/active.json")"
[[ "$(ps -p "$pid" -o comm= || true)" == "$binary" ]] || fail 'The registered profiled desktop has stopped; run desktop:profiled again'
live="$session/gpui.jsonl"
[[ -f "$live" ]] || fail 'GPUI recorder is not ready yet'
head -n 1 "$live" | jq -e --argjson pid "$pid" '.type == "session" and .pid == $pid' >/dev/null \
  || fail 'GPUI recording does not belong to the registered process'

mkdir -p "$profiles/captures"
capture="$(mktemp -d "$profiles/captures/$(date +%Y%m%d-%H%M%S)-$label.XXXXXX")"
echo "Capture: $capture"
echo "Reproduce $label in the desktop when xctrace says Starting recording ($seconds seconds)."
xcrun xctrace record --template "$template" --attach "$pid" --time-limit "${seconds}s" \
  --output "$capture/recording.trace" --no-prompt
xcrun xctrace symbolicate --input "$capture/recording.trace" --dsym "$session/listenbox-desktop.dSYM"
xcrun xctrace export --input "$capture/recording.trace" --toc --output "$capture/toc.xml"

trace_date="$(xmllint --xpath 'string(/trace-toc/run[@number="1"]/info/summary/start-date)' "$capture/toc.xml")"
duration="$(xmllint --xpath 'string(/trace-toc/run[@number="1"]/info/summary/duration)' "$capture/toc.xml")"
# Instruments supplies an ISO timestamp with milliseconds and a numeric offset.
base="${trace_date%%.*}"
fraction="${trace_date#*.}"
fraction="${fraction%%[+-]*}"
zone="${trace_date: -6}"
epoch="$(date -j -f '%Y-%m-%dT%H:%M:%S%z' "$base${zone/:/}" +%s)"
start_ms="$(jq -n --argjson epoch "$epoch" --arg fraction "$fraction" '$epoch * 1000 + (("0." + $fraction) | tonumber) * 1000')"
end_ms="$(jq -n --argjson start "$start_ms" --argjson seconds "$duration" '$start + $seconds * 1000')"

# A checkpoint proves that background collection has drained through the trace's end.
ready=false
checkpoint_ms=0
for _ in {1..40}; do
  if checkpoint_ms="$(tail -n 1 "$live" | jq -er --argjson end "$end_ms" 'select(.type == "checkpoint" and .unix_ms >= $end) | .unix_ms' 2>/dev/null)"; then
    ready=true
    break
  fi
  sleep 0.05
done
[[ "$ready" == true ]] || fail "GPUI recorder did not reach the trace end. Partial capture: $capture"
# Discard the possibly unfinished last line while the live recorder is appending.
sed '$d' "$live" | jq -c --argjson start "$start_ms" --argjson end "$end_ms" --argjson checkpoint "$checkpoint_ms" \
  'select((.unix_ms >= $start and .unix_ms <= $end and .type != "checkpoint") or
    (.type == "lost" and .unix_ms >= $start and .unix_ms <= $checkpoint))' > "$capture/gpui.jsonl"
head -n 1 "$live" > "$capture/session.json"
jq -n --argjson pid "$pid" --arg label "$label" --arg template "$template" \
  --argjson seconds "$seconds" --argjson recorded "$duration" \
  --argjson start "$start_ms" --argjson end "$end_ms" \
  --arg session "$session" --arg binary "$binary" \
  '{pid: $pid, label: $label, template: $template, requested_seconds: $seconds,
    recorded_seconds: $recorded,
    start_unix_ms: $start, end_unix_ms: $end, session: $session, binary: $binary}' > "$capture/capture.json"
jq -s -f "$desktop_root/tools/profile-summary.jq" "$capture/gpui.jsonl" > "$capture/summary.json"

if [[ "$kind" == cpu ]]; then
  xcrun xctrace export --input "$capture/recording.trace" \
    --xpath '/trace-toc/run[@number="1"]/data/table[@schema="time-profile"]' \
    --output "$capture/cpu-samples.xml"
fi
printf '%s\n' "$capture" > "$profiles/latest.txt"
echo "Agent-readable summary: $capture/summary.json"
cat "$capture/summary.json"
echo "Open in Instruments: open '$capture/recording.trace'"
jq -e '.frames > 0 and .lost_entries == 0' "$capture/summary.json" >/dev/null \
  || fail 'Capture has no rendered frames or lost events. Inspect the retained artifacts before drawing conclusions.'

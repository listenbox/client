def stats:
  sort as $values |
  if length == 0 then null else {
    count: length,
    p50_ms: $values[(length * 0.50 | ceil) - 1],
    p90_ms: $values[(length * 0.90 | ceil) - 1],
    p99_ms: $values[(length * 0.99 | ceil) - 1],
    max_ms: $values[-1]
  } end;

[.[] | select(.type == "frame")] as $frames |
{
  frames: ($frames | length),
  lost_entries: ([.[] | select(.type == "lost") | .entries] | add // 0),
  draw: ([$frames[].draw_ms] | stats),
  platform_submission: ([$frames[].present_ms] | stats),
  dirty_to_submission: ([$frames[].dirty_to_present_ms | select(. != null)] | stats),
  input_dispatch: ([.[] | select(.type == "input") | .dispatch_ms] | stats),
  draw_over_8_33_ms: ([$frames[] | select(.draw_ms > (1000 / 120))] | length),
  draw_over_16_67_ms: ([$frames[] | select(.draw_ms > (1000 / 60))] | length),
  slowest_frames: ($frames | sort_by(.draw_ms) | reverse | .[:10]),
  longest_foreground_tasks: ([.[] | select(.type == "task")] | sort_by(.duration_ms) | reverse | .[:10]),
  interpretation: "CPU draw and platform submission timings; GPU completion and actual display latency are not measured. Threshold counts are CPU budget exceedances, not proven dropped frames."
}

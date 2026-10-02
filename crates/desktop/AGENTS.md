# Desktop profiling

- Use `moon run desktop:profiled` in one terminal and `moon run desktop:profile`
  in another. These macOS tasks require Xcode Instruments, `jq`, and `xmllint`.
  `profiled` uses Moon's local server preset; `profile` is persistent so Moon
  schedules it with other persistent tasks, and completes after one capture.
  Ctrl+C stops the native recording and retains/exports the shortened trace;
  Moon reports the run as interrupted. Cmd+C copies terminal text.
  Runtime tasks (`profiled`, `profile`, and `dev`) use `inputs: []`; historical
  recordings are never Moon task inputs or outputs. Build dependencies track
  source files and the executable/dSYM separately.
- Stop `desktop:dev` before launching the profiled app against the same data.
  `profiled` uses `config/dev.yaml` and `~/.cache/listenbox/dev/`; it preserves
  an explicit `LISTENBOX_PROFILE_DIR` override. It uses the optimized Cargo
  `profiling` profile, dSYM symbols, GPUI instrumentation, and a frame-time
  overlay. It does not hotpatch. Local services must already be running.
  The profiling build bypasses `kache` and uses `target/profiled/` so dSYM
  packing retains usable application and dependency source paths.
  The recorder has an application-lifetime task tracker, separate from sync
  work. Quit drains sync work, cancels the recorder, and flushes it before
  process exit; logout drains sync work while recording continues.
- Capture one interaction at a time:
  - `moon run desktop:profile -- scroll` — 10 seconds of Time Profiler.
  - `moon run desktop:profile -- resize 10 cpu` — resizing CPU samples.
  - `moon run desktop:profile -- resize 10 metal` — Metal System Trace.
  Start the interaction when xctrace prints `Starting recording`; keep the
  app foreground. Arguments are a short label, 1–99 seconds, and `cpu` or `metal`.
- All output is ignored by Git under `crates/desktop/dist/profiles/`:
  - `sessions/<timestamp>.<unique>/gpui.jsonl` holds the live recording; the
    same session retains its dSYM so later builds cannot change its symbols.
  - `captures/<timestamp>-<label>.<unique>/` holds each immutable capture.
  - `latest.txt` gives the absolute path to the most recently exported capture.
  - `active.json` identifies the app PID and session. Capture validates the
    executable and session PID; it never guesses from another desktop process.
- Each capture contains `recording.trace`, `toc.xml`, `capture.json`,
  `session.json`, `gpui.jsonl`, and `summary.json`. CPU captures also contain
  `cpu-samples.xml`. The trace is symbolicated with the session's dSYM before
  export. `session.json` identifies the build's source commit and
  version; `capture.json` records the PID, interaction, requested/recorded
  duration, and time bounds. The source commit is the build's Git HEAD;
  account for uncommitted edits when reproducing a development capture.

## Reading a capture as an agent

- Start with `summary.json` and `capture.json`, then inspect `gpui.jsonl` around
  the slowest frame's `unix_ms`. The JSONL has frame draw/submission durations,
  invalidation counts, input dispatch events, foreground task locations, and
  explicit lost-event records. All durations and wall-clock timestamps use
  milliseconds. GPUI events are sliced to the Instruments recording interval;
  a background checkpoint must prove collection has reached its end.
- `summary.json` reports p50/p90/p99/max and the ten slowest frames/tasks.
  A capture with no presented frames or lost journal entries fails and retains
  the artifacts. Do not call an idle, incomplete, or interrupted capture a
  successful reproduction. Verify scrolling/resizing input in the recording.
- CPU draw includes GPUI element construction, layout, prepaint, and paint.
  Submission covers the native renderer call and any drawable/scheduling wait;
  it does not measure GPU completion or the instant pixels reach the display.
  Dirty-to-submission is invalidation latency, not input-to-display latency.
  Counts above 8.33/16.67 ms are CPU budget exceedances, not proven dropped frames.
- `cpu-samples.xml` is the native Time Profiler export, including thread and
  backtrace references and symbolicated source paths/lines. Raw sample times
  and weights are nanoseconds; sample times are relative to capture start.
  Resolve XML `ref` attributes against their `id` nodes;
  repeated references are repeated samples, not missing stacks. Use bounded
  queries rather than loading an entire trace or XML file into context:

  ```sh
  capture="$(cat apps/client/crates/desktop/dist/profiles/latest.txt)"
  jq . "$capture/summary.json"
  xmllint --xpath '(//row)[position() <= 10]' "$capture/cpu-samples.xml"
  xmllint --xpath '//*[@id="BACKTRACE_ID"]' "$capture/cpu-samples.xml"
  ```

- The binary `.trace` bundle is for Instruments: `open "$capture/recording.trace"`.
  For Metal captures, inspect available schemas in `toc.xml`, then export a
  relevant table with `xcrun xctrace export --input "$capture/recording.trace"
  --xpath '/trace-toc/run[@number="1"]/data/table[@schema="SCHEMA"]'
  --output "$capture/metal-table.xml"`. Do not invent a schema name.
- Compare the same interaction, loaded rows, window size, and display refresh
  rate before and after a change. Attribute the bottleneck using samples and
  timings before changing rendering behavior. Profiling tools live in `tools/`
  as simple Bash scripts and are checked by `moon run client:lint`.

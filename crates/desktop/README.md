# Listenbox desktop

A cross-platform native desktop app built with Rust and [GPUI](https://gpui.rs/).

[Import YouTube videos as a podcast](https://listenbox.app/guides/import-youtube-as-a-podcast/) —
installation, sign-in, importing, synchronization, and YouTube cookies.

## macOS startup

`moon run desktop:macos-reinstall` installs an optimized release build against
production services. Its local bundle uses ad-hoc signing and omits the native
updater. It is not a debug build.

```sh
moon run desktop:profile-startup -- startup
moon run desktop:profile-startup -- startup cpu
moon run desktop:test-startup
```

Startup profiling packages the optimized `profiling` build with dSYM symbols.
Keep the display unlocked and the launched window visible. The task launches
through Launch Services, measures the first displayed Metal drawable, and asks
only its own captured process to quit gracefully. It leaves the installed app
and hot-reload process alone. Quit any app using the same profile first.
`LISTENBOX_PROFILE_DIR` selects a different profile; the default is production
application data. The test always uses its own empty profile.

Artifacts live in `dist/profiles/startup/`; `latest.txt` identifies the last
successful capture. `fresh-copy-result.json` records the first launch of the
newly packaged copy, and `result.json` records a second, warm launch of that
same bundle. Compare like conditions with other builds. Neither clears OS
caches or measures a cold boot. `request_to_observer_loaded_ms` separates work
before the observer's initializer from application startup. Local ad-hoc
signing and measurement injection differ from a signed, notarized distribution.

The clock starts immediately before `NSWorkspace` requests the launch and ends
at `MTLDrawable.presentedTime`, excluding discarded drawables. This covers the
Launch Services path used to open an app; it does not timestamp a physical Dock
click. The observer is an injected measurement library, never shipped in the
app. A missing displayed frame fails after five seconds instead of treating a
window-created or CPU-submitted event as success.

The optional `cpu` capture waits for Instruments' recording-start notification
before launching, retains the native App Launch trace, and exports its CPU
samples and table of contents. `instrumented-result.json` belongs to that
separate launch; Instruments adds overhead. Inspect the captured PID from the
JSON when reading the all-process trace. Each capture retains the executable,
symbols, build identity, and worktree status.

XML exports can still contain numeric Rust frames after symbolication. Resolve
their binary references to the executable's UUID and `load-addr`, verify the
retained dSYM with `xcrun dwarfdump --uuid`, then use `xcrun atos -arch arm64 -o
PATH_TO_DWARF -l LOAD_ADDRESS FRAME_ADDRESS` for source locations. Never guess
the ASLR load address or use symbols from a newer build.

`test-startup` holds the native certificate reader behind a FIFO until a frame
is displayed. It proves the sign-in window can render independently of TLS
initialization. On failure, the gate releases valid PEM data before requesting
shutdown, so the baseline can fail without a launch panic.

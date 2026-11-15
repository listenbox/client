# Desktop development

- The user owns the desktop app and its development watcher. Never launch,
  stop, restart, or signal either process. Leave application execution to the user.
- MyGo native UI, state and event handling live in this package. Shared HTTP,
  authentication, persistence and synchronization belong in `crates/sync-engine`.
- Use `moon run desktop:build` for compilation and `moon run client:test` for
  headless UI regressions. Integrated live drivers run through the parent's
  `api:test-e2e` target with isolated fixtures, never development servers.
- Build packages with MyGo through the existing Moon targets. Keep the previous
  Sparkle/WinSparkle feeds, keys, preferences, installer identity and profiles.
- UI callbacks run on MyGo's main thread. Cancel admission and drain admitted
  writes and native media work before logout, updater replacement or exit.
- Headless captures establish application layout and interaction, not native
  window chrome, installed upgrades, first-frame latency or OS accessibility.

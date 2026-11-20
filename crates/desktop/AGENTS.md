# Desktop development

- The user owns the desktop app and its development watcher. Never launch,
  stop, restart, or signal either process. Leave application execution to the user.
- Native UI only: every screen is MyGo's `ui` package. Never add a webview,
  HTML/JS frontend, `frontendDist`/`devUrl`, embedded browser view or `-web`
  build. Open web pages, such as Dashboard flows, in the user's browser.
- Style only through `design.go`: colors, type roles, radii, spacing,
  elevation and component metrics. Never write a hex, OKLCH, alpha, font size,
  weight, radius or spacing literal in view code; add or reuse a token mapped
  from the root DESIGN.md instead. Build custom looks on MyGo's `*Base` widgets.
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

# Production IDs

- Never hardcode IDs in production code. This applies to every Listenbox ID
  and every external-provider ID, including YouTube videos, channels, playlists,
  and accounts.
- IDs must come from validated inputs, configuration, persisted state, or
  authoritative provider responses. Never special-case a particular ID in
  control flow, fallback policy, logging, or diagnostics.
- This prohibition includes temporary debugging code and probes. Keep
  investigation targeting outside production source files; never insert an ID
  into implementation code and plan to remove it later.
- Synthetic fixture IDs belong only in tests and test-only fixtures.

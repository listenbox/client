# YouTube extraction traffic

## Layer ownership — keep the Rust API thin

- `youtubei` owns the QuickJS context, scheduling, object lifetimes, native
  platform primitives, and thin Rust bindings to the live YouTube.js API.
  Bindings call upstream methods and read upstream properties; they do not
  implement a second YouTube client.
- `vendor/youtubejs` owns YouTube HTTP operations: session and player bootstrap,
  endpoint selection, request construction, response parsing, continuations,
  and signature/`n` deciphering. Let its real code execute inside QuickJS.
- Never issue independent YouTube HTTP requests from this crate. Never add
  Rust endpoint calls, hand-built YouTube payloads, response parsers, player
  extraction algorithms, or fallback requests to these bindings.
- The HTTP boundary is **YouTube.js → its `fetch` → the host transport**.
  A Rust fetch adapter may perform the requested I/O and return response bytes;
  it must not invent additional YouTube requests or replace upstream protocol
  behavior. Supplying native fetch/streams/URL primitives is runtime glue,
  not permission to move extraction into Rust.
- `sync-engine` owns Listenbox policy: which upstream client to request,
  rendition selection, cancellation, resumable downloads, upload journals,
  and publication. Keep that policy outside `youtubei`.
- Fix YouTube.js parser, request, and player-runtime defects in the nested
  `vendor/youtubejs` repository, then update its pin. Do not conceal them with
  Rust API workarounds or hard-coded HTTP responses.
- Verify this boundary with the real Rust CLI and embedded YouTube.js against
  local HTTP fixtures. A mocked Rust extraction result cannot prove that
  YouTube.js made and handled the necessary requests.

## Request budget

- Minimize HTTP requests while still completing the caller's goal correctly.
  Request count is part of the extraction contract, not a later optimization.
- Keep legitimate extraction traffic low and consistent with the selected
  client's session to avoid unnecessary bot-detection triggers. Never claim
  that a client or request pattern guarantees staying undetected.
- Prefer one player response that supplies both metadata and usable streams.
  Reuse it for format selection, deciphering, and publication metadata; do not
  fetch the same video again merely because another layer needs those fields.
- Request another client only when the response lacks a required capability or
  field, or its media URLs refuse the actual download. A successful length
  probe does not prove permission to download all ranges. Inspect the failure
  before choosing a fallback, and stop once the goal can be completed. Keep
  this policy in `sync-engine`; do not probe every client speculatively.
- Preserve cookies, client identity, publication dates, default audio selection,
  cancellation, and genuine unavailability when reducing requests. Fewer
  requests do not justify silently publishing incomplete or incorrect media.
- Verify the successful request budget and necessary fallback paths through the
  real Rust client and embedded YouTube.js against local integrated fixtures.
  Keep live cookie-backed investigation read-only and credentials out of logs.

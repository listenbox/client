# YouTube extraction traffic

- Minimize HTTP requests while still completing the caller's goal correctly.
  Request count is part of the extraction contract, not a later optimization.
- Keep legitimate extraction traffic low and consistent with the selected
  client's session to avoid unnecessary bot-detection triggers. Never claim
  that a client or request pattern guarantees staying undetected.
- Prefer one player response that supplies both metadata and usable streams.
  Reuse it for format selection, deciphering, and publication metadata; do not
  fetch the same video again merely because another layer needs those fields.
- Request another client only when the response lacks a required capability or
  field. Inspect the structured response before choosing a fallback, and stop
  once the goal can be completed. Do not probe every client speculatively.
- Preserve cookies, client identity, publication dates, default audio selection,
  cancellation, and genuine unavailability when reducing requests. Fewer
  requests do not justify silently publishing incomplete or incorrect media.
- Verify the successful request budget and necessary fallback paths through the
  real Rust client and embedded YouTube.js against local integrated fixtures.
  Keep live cookie-backed investigation read-only and credentials out of logs.

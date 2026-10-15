when in doubt consult https://github.com/yt-dlp/yt-dlp/ sources on how to
properly do innertube

`youtubejs/` is much worse maintained than `yt-dlp`

## Upstream PR descriptions

- Write for upstream maintainers: describe the failing library operation, the
  response shape and exception, the fix, and relevant verification.
- Never include our application context merely because that is where we found
  the bug. Listenbox, Rust clients, embedding in QuickJS, CRM, and our internal
  architecture or infrastructure are irrelevant to a missing-title playlist
  parser fix. Describe the playlist failure itself.
- Mention runtime or environment details only when they are necessary to
  reproduce or explain the library defect.
- Keep unrelated internal build, toolchain, and test blockers in the handoff to
  the user; do not put them in the upstream PR description.
- Report only relevant verification actually completed. Never imply that
  blocked or unrun tests passed.

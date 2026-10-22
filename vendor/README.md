# Embedded YouTube.js fork

`youtubejs/` is the nested `listenbox/YouTube.js` repository. Build it with
`moon run youtubejs:bundle`; the Rust bindings embed that bundle. Dependency
resolution belongs to this directory's `aube-lock.yaml`.

On 2026-10-22 the fork was refreshed onto upstream main
`c4a7b3f876114fea656de3d26ca9dfade94aa034` (v18.2.0 runtime source). Session
bootstrap, optional lockup titles, toggleable menu parsing, and lockup duration
support now come from upstream. `ListView` uses upstream's typed menu parser.

Five source files intentionally differ directly from that upstream snapshot:

| Files below `youtubejs/src/` | Contract |
| --- | --- |
| `parser/helpers.ts`, `parser/parser.ts` | Parsed arrays expose whether every input node survived; dropped nodes and unrecognized alerts cannot silently establish a complete scan. |
| `parser/youtube/Playlist.ts` | Read member containers and their continuations in source order, retain unsupported nodes, and expose `is_complete`. Missing/ambiguous containers, dropped members, and unsupported token-only pagination fail closed. |
| `Innertube.ts`, `utils/HTTPClient.ts` | Load embedded-player configuration and forward its context, visitor identity, and encrypted host flags when the application requests `WEB_EMBEDDED`. |

Listing completeness is a local application safety extension. The Rust sync
engine consumes it before reconciling episodes; the thin `youtubei` bindings
only read the property. It is not evidence of an upstream membership bug.
Unknown or malformed menu content in integrated tests is deliberate synthetic
fault injection. It must not invalidate intact listing members, while malformed
members must prevent reconciliation.

PR #1312 was closed after its menu-video example proved synthetic. No captured
response established that proposed bug. Do not use that example as provider
evidence or reinstate its fixture merely to justify this fork.

Audit remaining divergence with a direct source comparison, not a three-dot
comparison from the merge base:

```sh
git -C vendor/youtubejs fetch git@github.com:LuanRT/YouTube.js.git main:refs/remotes/upstream/main
git -C vendor/youtubejs diff upstream/main HEAD -- src
```

The embedded configuration/host-flags behavior is independently supported by
yt-dlp's extraction implementation. The exact embedder used by our fixtures is
synthetic. The upstream `LockupView.duration` getter remains available; switching
Rust admission estimates from their existing validation to that getter would be
a separate policy change requiring integrated coverage.

# Embedded YouTube.js fork

`youtubejs/` is the nested `listenbox/YouTube.js` repository. Build it with
`moon run youtubejs:bundle`; the Go bindings embed that bundle. Dependency
resolution belongs to this directory's `aube-lock.yaml`.

## Releases

Clients embed the bundle they were built with and pick up newer ones from
signed GitHub releases of the fork, so YouTube.js fixes ship without a client
release. Like the client, the fork releases itself: every push to its `master`
runs [`bundle-release.yml`](youtubejs/.github/workflows/bundle-release.yml),
which builds the minified bundle, takes the next immutable version
`bundle-v<ABI>.<N>`, signs it and publishes it. Superseded and already released
commits are skipped. Push the fork only after the pinned commit passes the
client's integrated E2E, and bump the pin here.

- The major version is the bundle ABI in `youtubejs/.github/bundle/abi`. It must
  equal `youtubei.BundleABI`; bump both together when bindings or host primitives
  and bundles stop being interchangeable.
- Assets: the bundle (`youtubejs.js`) and an Ed25519-signed manifest
  (`youtubejs.json`) carrying ABI, release number, sequence (the fork commit's
  committer time), size, SHA-256 and commit.
- The signing seed lives only in the fork's `bundle-release` environment secret
  `BUNDLE_SIGNING_SEED`, restricted to `master`, with an offline copy at
  `~/.secrets/listenbox/youtubejs-signing.key`. Clients pin its public half in
  `crates/sync-engine/youtubejsrelease`.

The CLI's `--watch` mode and the desktop app check every ten minutes with
`GET /repos/listenbox/YouTube.js/git/matching-refs/tags/bundle-v<ABI>.` and
`If-None-Match`. The tag list changes only when a release is published, so
checks are 304s. Clients fetch the latest release's manifest. They evaluate it
only when its signature verifies against the pinned key, it names its own tag
and their ABI, its sequence is newer than the bundle in use (no rollback), and
the downloaded JS matches the signed size and digest. They store it in the
profile, re-verify it on every start, and compile it in-process; bytecode is
never downloaded. Even verified bundles can only fetch YouTube and Google hosts
through the host transport.

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

Listing completeness is a local application safety extension. The Go sync
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
git -C third_party/youtubejs fetch git@github.com:LuanRT/YouTube.js.git main:refs/remotes/upstream/main
git -C third_party/youtubejs diff upstream/main HEAD -- src
```

The embedded configuration/host-flags behavior is independently supported by
yt-dlp's extraction implementation. The exact embedder used by our fixtures is
synthetic. The upstream `LockupView.duration` getter remains available; switching
Go admission estimates from their existing validation to that getter would be
a separate policy change requiring integrated coverage.

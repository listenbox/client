# youtubei

Go bindings for the real [YouTube.js fork](https://github.com/listenbox/YouTube.js)
inside [buke/quickjs-go](https://github.com/buke/quickjs-go). The sync engine owns
HTTP transport, cookies, format selection, downloads and application policy.
This package owns the QuickJS runtime, its host primitives and live objects.

Initialize submodules recursively, provision `mise install --locked`, then run
`moon run client-youtubei:build` and `moon run client:test`. Moon bundles the pinned
`third_party/youtubejs` source using Aube's lockfile. `third_party/youtubejs.go`
embeds the resulting bytecode, so the first engine starts without a download.
`New` takes a `Bundle`: `Embedded()` or `Compile(js)`, which compiles a JS bundle
in-process. Bytecode is never loaded from anywhere else. Which bundle to run, and
how newer ones are fetched and verified, is `sync-engine` policy. Bump
`BundleABI` whenever bindings or host primitives and bundles stop being
interchangeable. The original Mazit extraction retains its MIT attribution
in `THIRD-PARTY-NOTICES.txt`.

`New(ctx, fetch)` starts a dedicated, locked OS thread. Every QuickJS value is
created, accessed and freed there. Calls can overlap while awaiting host I/O;
closing the engine drains accepted calls before freeing its runtime. Cancellation
interrupts CPU-bound JavaScript and cancels the owning host request. Fetch bodies
remain bytes, with headers, status and host errors preserved.

Typed playlist, channel and video operations call upstream methods. Playlist
continuations, formats and deciphering retain live handles; `Release` frees
those handles. Owned metadata snapshots remain usable afterward. The dynamic
`Engine.Call` escape hatch evaluates on the same thread and returns an explicit
JSON snapshot. It never exports QuickJS pointers across goroutines.

Session and player bootstrap, endpoint payloads, parsers, continuations and
signature/`n` deciphering belong to YouTube.js. Fix those defects in the nested
repository. The host supplies fetch, streams, URLs, timers, encoding, crypto,
platform cache and player evaluation. This is the subset required by the
embedded bundle, not a general browser or Node runtime.

The ported runtime tests cover live-object lifetimes, prototypes, callbacks,
errors, binary transport, client construction, actual flat continuations,
platform hooks, independent engines, overlapping calls and cancellation. The
parent's integrated API tests exercise the real Go CLI and this same bundle
against local service fixtures.

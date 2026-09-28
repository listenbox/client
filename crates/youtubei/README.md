# youtubei

The client workspace's Rust crate embeds QuickJS, our pinned
[YouTube.js fork](https://github.com/listenbox/YouTube.js). Live objects
retain a reusable engine; the sync engine owns cookies, transport, downloads, and
application policy.

Extracted from [Mazit](https://github.com/meoyawn/mazit/tree/fa0cd7c9edc3b0d25d91fd1bb93ef0f9bd58eb47/youtubei), preserving its MIT attribution.
Source builds require Rust, a C toolchain, CMake, Node.js 26, Aube and Moon.
Initialize submodules recursively, then run:

```sh
git submodule update --init --recursive
moon run client-youtubei:build client-youtubei:test
```

Moon bundles `vendor/youtubejs/src/platform/cf-worker.ts` with preserved names
from locked dependencies. Cargo embeds the generated bundle from its own `OUT_DIR` and performs no network download. Both standalone
and parent-workspace builds use these same prerequisites. End users need no
JavaScript runtime. This crate is workspace-only; its source dependency is pinned
by the nested Git submodule. License texts are in `THIRD-PARTY-NOTICES.txt`.

Sibling crates consume the checked-in source directly:

```toml
youtubei = { path = "../youtubei" }
```

```rust
use youtubei::{Innertube, SessionOptions, models::PlaylistItem};

# async fn example() -> youtubei::Result<()> {
let yt = Innertube::create(SessionOptions::local()).await?;
let mut page = yt.get_playlist("PLAYLIST_ID").await?;
loop {
    // One engine access for metadata, items, alerts and continuation state.
    let data = page.data().await?;
    for item in data.items {
        match item {
            PlaylistItem::PlaylistVideo(video) => println!("{}", video.title),
            PlaylistItem::LockupView(video) => println!("{}", video.content_id),
            PlaylistItem::Other { node_type } => eprintln!("Unsupported: {node_type}"),
        }
    }
    if !data.has_continuation { break; }
    page = page.get_continuation().await?;
}
# Ok(())
# }
```

`SessionOptions::default()` leaves upstream defaults intact (including network
session/player initialization). `SessionOptions::local()` explicitly disables
those startup requests. Multiple sessions can share an `Engine` with
`Innertube::create_in`; `Innertube::new(&session)` reuses an existing session.
Cloning a handle retains the same JS object. Dropping the final handle frees its
engine. Cross-engine arguments return an error.

The typed surface covers the upstream operations used by Mazit. These are
read-only projections of the fields it consumes, not complete copies of every
TypeScript declaration. Upstream methods still run on live JavaScript objects.

| Upstream | Rust |
| --- | --- |
| `Innertube.create`, `new Innertube(session)` | `Innertube::create`, `create_in`, `new` |
| `yt.getPlaylist`, `getChannel`, `getBasicInfo` | `get_playlist`, `get_channel`, `get_basic_info` |
| `yt.actions.execute('/browse', { browseId, params })` | `actions.browse(BrowseOptions { browse_id, params })` → `ApiResponse` |
| `new YT.Playlist(actions, response)` | `Playlist::new(&actions, &response, false)` |
| `page.getContinuation()` | `page.get_continuation().await?` → live `Playlist` |
| `page.info`, `page.items`, `page.page.alerts`, `page.has_continuation` | `page.data().await?` → owned `PlaylistData` in one context access |
| `PlaylistVideo \| LockupView \| …` | `PlaylistItem::{PlaylistVideo, LockupView, Other}` |
| `channel.metadata` | `channel.metadata().await?` → `ChannelMetadata` |
| `info.basic_info`, `playability_status`, `page[0].microformat`, `cpn` | `info.data().await?` → `VideoInfoData` in one context access |
| `info.streaming_data?.formats / adaptive_formats` | `info.formats()` / `info.adaptive_formats().await?` → `Vec<Format>` with cached `FormatInfo` |
| `{ client: 'VISIONOS' }` | `GetVideoInfoOptions { client: Some(Client::VisionOs), ..Default::default() }` |
| `yt.session.player`, `Player.create` | `session().await?.player`, `set_player`, `Player::create` |
| `format.content_length`, `format.has_audio`, … | `format.info().content_length`, `format.info().has_audio`, …; no JS access |
| `format.decipher(player)` | `format.decipher(player.as_ref()).await?` → `String` |
| `text.toString()` in listing metadata | `TextValue` with `Display`, `as_str()`, `into_string()` |

`models` contains ordinary Rust structs, enums, `Option`, and `Vec`. Missing
metadata on unavailable entries and continuation pages stays optional. Unknown
node/content types stay explicit so the app can reject incomplete listings.
JavaScript numbers are read natively, preserving NaN (important for rejecting
invalid lengths); dates preserve epoch milliseconds. Text conversion invokes the
upstream `toString`, including its fallback behavior.

Owned snapshots are `Send + Sync`; live engine/object handles are not. Snapshots
remain usable after dropping the engine. `Format::info()` is captured when the
handle is returned; modifying the JS object through `as_value()` does not update
that snapshot. Cloning a format shares its metadata without recopying strings.

Small declarative Rust macros generate model readers and handle boilerplate.
They expand to statically dispatched native property reads, with no runtime
schema interpreter or JSON round trip. QuickJS execution, property lookup and
copying strings into owned Rust values still have a cost. Batch extraction avoids
repeated context access and intermediate persistent handles. Audio selection is
entirely over cached Rust fields, with the live format retained for deciphering.

The dynamic escape hatch is explicit: `as_value()` on handles, or
`Engine::exports` / `export`, with
`JsValue::get`, `set`, `call`, `apply`, and `construct`. Method calls preserve
`this` and await promises; values retain prototypes, getters, callbacks, streams,
and identity. `value_with` / `with` expose rquickjs for native Rust callbacks,
symbols, typed arrays, and other non-JSON values. `Argument::Undefined` is distinct
from JSON null. `deserialize` is an explicit data snapshot, not the call protocol.
This is **not a generated, statically typed translation of every TypeScript
declaration**. APIs outside the typed surface use these dynamic Rust operations.

Application policy stays in Rust: for Mazit's unavailable-inclusive flat listing,
call `Actions::browse` with `browse_id: "VL" + id` and `params: "wgYCCAA="`, then construct
`Playlist` and follow every continuation. See `examples/playlist.rs`. The crate
does not expand listing videos or choose an audio format for the application.
Mazit keeps channel HTML resolution, listing validation, publication dates, and
audio selection in `src/youtube/`. The app does not call Innertube's download or
stream APIs: its Rust download manager retains range requests and concurrency
slots. `Engine::fetch_with` accepts a Rust async HTTP
callback, so the app retains its reqwest client and cookie jar. Request and
response bodies cross this boundary as bytes, without JSON or base64 wrapping.

Use `Worker<Request>` to share a session across threads. Its handle is cloneable,
`Send + Sync` when requests are `Send`. The crate owns the runtime thread and
runs initialization, operations, and state destruction there. Its initializer
can return local engines, sessions, caches, or application state; its handler
borrows that state and returns a boxed local future. Requests can contain typed
reply channels for owned results. Up to sixteen operations overlap while awaiting
I/O, with backpressure after 32 queued requests. Dropping all worker handles drains
accepted operations before destroying the state. See the runnable, network-free
`examples/workers.rs` and cross-thread lifecycle tests in `tests/worker.rs`.

Raw object handles remain `!Send` and `!Sync`, preventing accidental movement
between runtime threads at compile time. Advanced consumers may use them directly
on a current-thread executor or `LocalSet`, including `join!`, `spawn_local`, and
`FuturesUnordered`. The engine encapsulates the rquickjs 0.11 scheduler's single
waker: it forwards notifications to all pending Rust callers and unregisters each
caller on completion or cancellation. A finishing short call cannot leave a
longer call waiting on a retired task. No caller-side polling or wake-up workaround
is needed. Runtime regressions cover staggered responses and cancellation.

Mazit uses `Worker` with one reusable engine and Innertube session. Its YouTube
module supplies application operations and timeouts without implementing a runtime
thread or scheduler. Each playlist scan owns its continuation chain; media downloads
remain on the app's existing worker pool after receiving owned URL/metadata.

The crate embeds rquickjs 0.11 with LLRT 0.8.1-beta's Rust implementations of
fetch, streams, URL, events, timers, encoding, and crypto. Rust supplies
`Platform.load`, player evaluation, structured cloning, and caching. The complete
source-built `bundle/cf-worker.js` is loaded; Rust replaces its platform shim
after evaluation. `--keep-names` is required by the upstream parser.

The source bundle imports `module.createRequire` to probe optional worker
threads. The crate supplies a native-only require factory because LLRT 0.8.1
exports `require` directly under that name. Native module loads work; filesystem
CommonJS loads and worker threads are unavailable and raise JavaScript errors.
The bundle's own optional-worker handling catches that error.

Tests run the real bundle offline: object lifetime/identity, independent workers,
overlapping promises, Rust callbacks, errors, flat continuation pages, VISIONOS
request construction, parser nodes, formats, and the platform hooks.
These tests do not establish compatibility of every upstream feature.
LLRT implements a subset of browser APIs. Cancelling a Rust wait does not itself
abort a JavaScript operation; use the upstream AbortSignal/cancellation API when
needed, and drive background jobs with `Engine::idle` while using subscriptions.
`Engine::set_interrupt_handler` lets applications stop CPU-bound JavaScript.
The Rust HTTP callback must enforce its own cancellation and response limits.

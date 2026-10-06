# CLI documentation review

Disposition: **reviewed and documented; ship**.

The shipped CLI extends the existing command language and inherits terminal typography and background. The compact footer, permanent failure records, restrained semantic emphasis, and plain redirected output follow the [surface direction](../surfaces/apps-client-crates-cli.md). `PRODUCT.md` calls for predictable output and actionable failures; `DESIGN.md` reserves native monospace for commands and literal code. This work adds no shared visual tokens or new identity.

I checked `crates/cli/src/youtube.rs`, the `youtube` command model and help in `crates/cli/src/main.rs`, [the finish review](cli-finish-review.md), and its real-output [80-column](cli-sync-80.png) and [120-column](cli-sync-120.png) captures and text frames. The 80-column frame retains a failed video's title, show, full URL, and reason above four progress lines; the resized 120-column frame keeps counts, bar, percentage, and rates on single lines. The later frame has scrolled the earlier failure out of view, while the recorded transcript retains it. The production run reached 595/986 imported, 5 not imported, and 386 queued; SIGINT drained and exited 130.

The captures establish one live transfer and a resize. Hourly waiting and a fully completed production inventory were not exercised. The PNGs are review renderings of a macOS PTY transcript made with pyte, Pillow, and Menlo, not shipping raster assets or generated imagery.

The CLI README accurately covers commands, growing discovery totals, plain failures, watch, cancellation status 130, and Moon's `--` separator. The documentation pass found a wording mismatch about per-show summaries. The README now says “a count summary for each successful show”; failed shows retain their failure record. This correction preserves the reviewed terminal design.

Preserved: root `DESIGN.md`, root `.impeccable/design.json`, all global design tokens, implementation, and unrelated files.

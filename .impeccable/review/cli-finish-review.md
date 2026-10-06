# Disposition

ship

## Visual evidence and capture validity

The 80-column, 24-row and 120-column, 35-row captures render actual production CLI ANSI output recorded in a macOS `script` PTY, then emulated with pyte and drawn with Pillow/Menlo. The source transcript is `/tmp/listenbox-cli-production-final.typescript`; the paired text frames are `cli-sync-80.png.txt` and `cli-sync-120.png.txt`. These PNGs are review artifacts, not product imagery. The run discovered eight shows across two teams and 986 known episodes at the captured point.

At 80 columns, the failed video's complete title, show, YouTube URL, and reason remain legible above the four-line footer. At 120 columns after a real terminal resize, the count, bar, percentage, and active rates use the width cleanly without wrapping or clipping. The earlier failure has moved into scrollback in the 120-column frame; its absence from that single viewport is not evidence that it was cleared. The recorded transcript contains the failure once, with its full URL and reason.

## User-contract findings

`youtube cookies import/remove`, `youtube sync [--show SLUG] [--watch]`, and the expanded `youtube --help` copy match the requested command model. Bare sync discovers writable YouTube imports across teams; watch repeats discovery. The engine owns transfers and cancellation. The CLI keeps episode successes out of the transcript, writes a permanent failure record when an item fails or is skipped, and shows one summary per finished show.

The footer reports discovered inventory, so totals can grow while sources are read. Before any episode is known, its explicit reading message makes `0/0 imported` understandable. The 80-column capture shows 526/986 imported, one not imported, 447 queued, and active reading/download/upload stages; the remainder are in active stages. Source-level failures receive a separate `Not synced` record with the source link and reason. Redirected stderr uses plain text without cursor or color escapes. Ctrl+C requests cancellation, waits for owned work, prints saved-progress guidance, and exits 130.

## Material fixes

None from this finish review.

## Limitations

The two captures show one live transfer and one resize, not the hourly waiting, stopping, empty, or final state. Those state paths were reviewed in `crates/cli/src/youtube.rs` and `crates/sync-engine/src/client.rs`; the integrated CLI scenarios cover plain failure output, cross-team progress after a source failure, and cancellation with resumed uploads. This review did not rerun those tests or make a new production request. No recapture is needed for the stated 80/120-column visual evidence.

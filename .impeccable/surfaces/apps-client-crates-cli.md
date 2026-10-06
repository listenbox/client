---
version: 1
slug: "apps-client-crates-cli"
primary_target: "apps/client/crates/cli"
related_targets: ["apps/client/crates/sync-engine"]
---

# YouTube sync terminal output

Mode: Operate. A podcaster runs a foreground sync across tens of shows and thousands of episodes. The engine owns discovery, transfer limits, progress, checkpoints and cancellation; the CLI renders its state.

## Direction contract

THESIS: a stable, compact progress footer beneath permanent Not imported records. Success stays quiet; every failure retains the exact YouTube link and reason.

OWN-WORLD: inherit terminal fonts and background, use normal text for counts and semantic red/bold only for failure headings. No alternate screen, interactive controls, decorative boxes, or per-episode success log.

STORY: start one command for all eligible teams, scan aggregate progress, follow a failed video's link, and use the reported reason to recover. Ctrl+C drains owned writes and preserves the queue.

FIRST VIEWPORT: an account-wide show/team count above a four-line rewritten footer: show completion, imported/known/not-imported counts, an imported progress bar, and active stages with network rates. Reading sources and stopping have explicit text. Errors append above the footer; plain redirected output retains failures and per-show summaries.

FORM: code-led extension of the established CLI and the user's supplied terminal reference. Seed key: existing-listenbox-cli-footer. No new visual world or image assets.

FINISH: reviewed and documented; disposition ship. The CLI inherits the terminal and the existing Listenbox system. No global design tokens, DESIGN.md, or shipping raster assets changed.

Keep the global design system unchanged. Terminal width controls clipping of the footer; full failure links/reasons remain in the transcript. No color or cursor escapes ship to redirected stderr. Watch discovers new shows on each hourly scan; a cancelled command waits for workers and exits 130.

## Documented surface

- Keep ordinary terminal text as the default hierarchy. The host terminal owns font and background; a bold red heading identifies an episode that was not imported. Do not introduce a CLI palette, frame, alternate screen, or per-episode success feed.
- Print the show/team count and one summary for each successfully finished show as permanent lines. Render four rewritten progress lines for show completion, imported/known/not-imported/queued counts, an imported bar, and current stages with transfer rates. Totals can increase during discovery, so explain the initial `0/0` with a reading state.
- Print each failed or skipped video once above the footer with its full title, show, full YouTube URL, and reason. A source-level failure uses a distinct `Not synced` record with the source link and reason. Clip only transient footer lines to terminal width.
- Redirected stderr remains plain text, without color or cursor escapes. Ctrl+C switches to stopping guidance, drains owned writes, preserves resumable work, and exits 130. Watch starts immediately, then discovers sources again on hourly scans.

The finish review and 80/120-column production captures are in [the CLI review](../review/cli-finish-review.md). Those PNGs visualize real ANSI terminal output for review; they are not product images or design assets.

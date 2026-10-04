---
name: Listenbox Desktop Native
description: A native podcast library with team-first navigation, aligned controls, cover artwork, and automatic syncing.
colors:
  background-light: "oklch(0.9851 0 0)"
  background-dark: "oklch(0.2134 0 0)"
  sheet-light: "oklch(1 0 0)"
  sheet-dark: "oklch(0.252 0 0)"
  rail-light: "oklch(0.9612 0 0)"
  rail-dark: "oklch(0.2478 0 0)"
  ink-light: "oklch(0.2435 0 0)"
  ink-dark: "oklch(0.9702 0 0)"
  muted-light: "oklch(0.4532 0 0)"
  muted-dark: "oklch(0.8015 0 0)"
  border-light: "oklch(0.8761 0 0)"
  border-dark: "oklch(0.4349 0 0)"
  divider-light: "oklch(0.9219 0 0)"
  divider-dark: "oklch(0.3446 0 0)"
  selected-light: "oklch(0.9219 0 0)"
  selected-dark: "oklch(0.3368 0 0)"
  action-light: "oklch(0.3092 0 0)"
  action-dark: "oklch(0.54 0 0)"
  action-ink: "oklch(1 0 0)"
  danger-light: "#b44d42"
  danger-dark: "#ef9a8d"
typography:
  page-title:
    fontFamily: "ui-sans-serif, system-ui, sans-serif"
    fontSize: "30px"
    fontWeight: 700
  title:
    fontFamily: "ui-sans-serif, system-ui, sans-serif"
    fontSize: "18px"
    fontWeight: 600
  body:
    fontFamily: "ui-sans-serif, system-ui, sans-serif"
    fontSize: "14px"
    fontWeight: 400
  label:
    fontFamily: "ui-sans-serif, system-ui, sans-serif"
    fontSize: "14px"
    fontWeight: 600
  support:
    fontFamily: "ui-sans-serif, system-ui, sans-serif"
    fontSize: "12px"
    fontWeight: 400
rounded:
  control: "10px"
spacing:
  rail: "240px"
  rail-inset: "16px"
  navigation-inset: "8px"
  toolbar-height: "56px"
  control-height: "36px"
  content: "24px"
  control-gap: "12px"
  row-gap: "8px"
  row-padding-y: "12px"
components:
  button-primary-light:
    backgroundColor: "{colors.action-light}"
    textColor: "{colors.action-ink}"
    typography: "{typography.label}"
    rounded: "{rounded.control}"
  button-primary-dark:
    backgroundColor: "{colors.action-dark}"
    textColor: "{colors.action-ink}"
    typography: "{typography.label}"
    rounded: "{rounded.control}"
  button-ghost-light:
    textColor: "{colors.ink-light}"
    typography: "{typography.label}"
    rounded: "{rounded.control}"
  button-ghost-dark:
    textColor: "{colors.ink-dark}"
    typography: "{typography.label}"
    rounded: "{rounded.control}"
  input-playlist-light:
    backgroundColor: "{colors.sheet-light}"
    textColor: "{colors.ink-light}"
    rounded: "{rounded.control}"
  input-playlist-dark:
    backgroundColor: "{colors.sheet-dark}"
    textColor: "{colors.ink-dark}"
    rounded: "{rounded.control}"
  navigation-selected-light:
    backgroundColor: "{colors.selected-light}"
    textColor: "{colors.ink-light}"
    typography: "{typography.label}"
    rounded: "{rounded.control}"
  navigation-selected-dark:
    backgroundColor: "{colors.selected-dark}"
    textColor: "{colors.ink-dark}"
    typography: "{typography.label}"
    rounded: "{rounded.control}"
  transfer-row-light:
    textColor: "{colors.ink-light}"
    padding: "12px 0"
  transfer-row-dark:
    textColor: "{colors.ink-dark}"
    padding: "12px 0"
---

# Listenbox Desktop

This is the desktop interface’s design system. The shared Listenbox system owns
brand colors and contrast requirements; this file owns native hierarchy,
density, typography, alignment, control composition and interaction. Desktop
screens follow these rules rather than inheriting the web’s page layouts or
GPUI Kit’s default component composition.

## Purpose and hierarchy

The app keeps imported YouTube podcasts current automatically. The reading order
is **team → import action → podcast → source and sync status → Episodes**.
Teams own the library scope. A podcast never becomes the parent of a team.

The selected podcast's Episodes list is the merged status surface: published
episodes from the API appear alongside active and persisted source items scoped
to that podcast. Published rows retain API array order; pending source rows may
be placed around them by saved source position without reordering the API
sequence. The `Not imported` filter appears only when failed or skipped source
items exist, shows each item's reason or error and links its title to YouTube,
and is hidden when the issue count reaches zero.

The 240px sidebar and the working pane have aligned 56px toolbars. Listenbox is
a quiet 14px semibold application label; account and Reload actions occupy the
opposite toolbar. The selected team is the strongest text in the sidebar: 18px
bold, left aligned, with its chevron at the far right. No centered navigation
labels and no icon preceding the team name.

Import playlist is a full-width charcoal action beneath the team picker. Its
label is left aligned, with a plus at the far right. The button belongs to the
team section; it never sits above it. The form explicitly names the authorized
team in which the new podcast will be created.

Only `youtube.kind = import` podcasts belong in the library. Ordinary shows and
YouTube destinations remain in the web workspace. Import creates a new podcast;
a source is fixed at creation. Detail screens have a read-only source link.
There are no Save playlist, Disconnect or Keep syncing controls.

## Geometry and rhythm

Use the named constants in `src/tokens.rs`, with a 4px spacing base.

| Role | Size | Rule |
| --- | --- | --- |
| Sidebar | 240px | Fixed; the working pane takes remaining width. |
| Toolbar | 56px | Both panes share the same horizontal divider. |
| Sidebar content inset | 16px | Brand, team text, artwork, empty copy and footer share this left edge. |
| Navigation row inset | 8px | Row hit areas extend 8px beyond their content. |
| Sidebar action height | 36px | Stable through normal, disabled and loading states. |
| Control padding | 12px | Action text sits inside its visible button boundary. |
| Related control gap | 12px | Team picker and import button form one group. |
| Main pane inset / section gap | 24px | Separate source, status and episode-list regions. |
| List row gap | 4px | Compact, continuous library, without individual cards. |
| Artwork-to-text gap | 12px | Shared by all podcast rows. |
| Control radius | 10px | Rounded rectangles, not capsules. |

The team section has 16px vertical padding. Podcast rows use 8px internal
padding and a 44px cover. Their backgrounds extend to the row hit area, while
the covers align with the team heading. The sidebar footer uses the same 16px
inset. Do not apply one uniform gap to every level of the sidebar.

The team choices are left aligned, truncate long names and scroll within a
180px maximum height. Podcast titles truncate in the rail and can wrap in the
working pane. The sidebar list and main content own separate scroll areas.
At the 840×600 minimum window, important errors precede the form; taller content
remains reachable by scrolling.

The main pane has a visible vertical scrollbar whenever its content overflows,
including the virtualized episode list. Its thumb controls the same scroll
state as wheel input. The podcast list and team choices use GPUI Kit's native
scrollbar affordance and system visibility behavior.

## Type and color

Use the operating system font. Page titles are 30px bold, the team heading is
18px bold, region titles are 18px semibold, body and controls are 14px, and
supporting status is 12px. Titles lead through size and weight. Never add a
small uppercase label above them to create hierarchy.

Structural colors are achromatic: a silver sidebar, nearly white working
surface, charcoal text and actions. Dark appearance remaps every role; it is
not a translucent overlay on the light theme. Color belongs to actual status
and podcast artwork. No decorative gradients, ambient shadows or extra fonts.

Light primary actions use #303030 with white text (13.2:1). Dark primary actions
use OKLCH 0.54 with white text (5.06:1), with 3.14:1 against the rail; hover at
0.56 preserves 4.65:1 text contrast. Enabled text and placeholders meet 4.5:1;
essential control edges and focus indicators meet 3:1 against adjacent colors.
The input border role uses lightness 0.6167 in light and 0.62 in dark appearance.
Measure the composed result, including hover and focus.

## Native controls

Use GPUI Kit for focus, keyboard activation, disabled state and pointer state.
Use `.primary()` explicitly for Import playlist, creation, sign-in and Sync now.
Stop uses an outline; Reload, account, source links and browser handoffs use
ghost buttons. Not-imported video titles use underlined, keyboard-focusable links
to YouTube. White default buttons are not primary actions.

**Navigation content must own its alignment.** GPUI Kit centers its internal
label container even when the outer button has `.justify_start()`. Compose a
full-width child row for navigation, with a flexible left label and a trailing
chevron or plus. Give custom-content buttons an explicit accessibility label.
Do not repeat the ineffective outer-only alignment pattern.

Use the native focus treatment and do not move controls on hover. Input fields
have a visible border and a readable placeholder. Lock import controls while
creation is underway. Errors state the problem and the recovery above the form.

## Artwork and selection

Every podcast row has a 44px square cover and the selected podcast has a 120px
cover. Use native `img`, `ObjectFit::Cover` and 8px/14px corner radii. Missing,
loading and failed images use the same rounded headphone placeholder without
changing layout. Podcast rows share one text column for title and sync status;
the selected row uses a neutral fill rather than a decorative marker.

A workspace-owned cache coalesces requests and decoding across both cover
sizes. It retains at most 128 covers, releases evicted GPU images, prunes covers
that leave the catalog, and clears on logout or explicit Reload. Preview assets
are synthetic fixtures with provenance in `tests/fixtures/README.md`.

## Sync and episode status

Existing imports sync on startup and hourly while Listenbox is running. Sync
now requests another immediate pass. Stop cancels the current work and saves
progress; it does not permanently disable automatic sync. There is no opt-in
button for the app’s main purpose.

The engine owns checkpoints, transfers and the scan cadence for CLI and desktop.
Import failure after creation keeps the podcast available to resume by slug.
Never suggest importing a second podcast to recover the first.

The selected podcast's Episodes list uses flat rows separated by quiet dividers.
It merges published API episodes with active and persisted source items while
preserving the API array order and the podcast scope. Failed and skipped source
rows show their full YouTube explanation in the danger color; `Skipped` uses
muted gray. The video title is an underlined, keyboard-focusable link that opens
YouTube in the browser, without a separate handoff button. The
`Not imported` filter is conditional on those rows and disappears when none
remain. Its Retry imports action starts another sync through the engine's saved
work, including failures whose automatic attempts were exhausted. Disable it
while the selected podcast is syncing or stopping, or lacks an active plan.
Downloads show a horizontal progress bar, received/total bytes and
measured throughput. There is no queue or upload pause/resume control.
Checkpoints recover crashes and unexpected interruptions. Empty states describe
real next steps and never invent activity or podcast content.

## Window behavior

Honor system light/dark appearance and Reduce Motion. The app retains its
native titlebar, menu, status item and close-to-hide behavior. The first ⌘Q
press shows a centered, nonblocking shortcut HUD: 280px width, 24px padding,
20px radius and a 56px shortcut. It lasts two seconds, fading for 200ms unless
Reduce Motion is enabled. Shutdown reports draining work in the same position.
Do not introduce a full-window dimming layer or shift the workspace.

## Evidence

`src/tokens.rs` owns reusable dimensions and semantic colors. `src/workspace.rs`
owns production composition; `src/artwork.rs` owns covers and their cache.

Headless GPUI tests cover control activation and state, while parent API E2E
scenarios exercise actual import, sync and recovery. Native window chrome
requires the running application; headless captures do not establish OS-level
behavior or a monorepo-wide visual audit.

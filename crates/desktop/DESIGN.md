---
name: Listenbox Desktop Native
description: The monorepo's Listenbox design system, projected onto MyGo's native UI.
extends: ../../../../DESIGN.md
colors:
  background-light: "#ffffff"
  background-dark: "{colors.ink}"
  sheet-light: "#ffffff"
  sheet-dark: "{colors.ink-raised}"
  rail-light: "#ffffff"
  rail-dark: "{colors.ink-raised}"
  control-light: "{colors.paper}"
  control-dark: "{colors.ink-tile}"
  control-hover-light: "{colors.paper-deep}"
  control-hover-dark: "{colors.ink-line}"
  fg-light: "{colors.fg}"
  fg-dark: "{colors.snow}"
  fg-soft-light: "{colors.fg-soft}"
  fg-soft-dark: "{colors.snow-soft}"
  edge-light: "{colors.fg-faint}"
  edge-dark: "{colors.snow-faint}"
  line-light: "{colors.line}"
  line-dark: "{colors.ink-line}"
  selected-light: "{colors.mist}"
  action: "{colors.accent}"
  action-hover: "{colors.accent-hover}"
  focus-light: "{colors.accent}"
  focus-dark: "{colors.accent-light}"
  danger-light: "{colors.danger}"
  danger-dark: "{colors.danger-on-ink}"
  success-light: "{colors.ok}"
  success-dark: "{colors.ok-on-ink}"
typography:
  page-title: "{typography.headline}"
  title: "{typography.title-panel}"
  label: "{typography.label}"
  badge: "{typography.tag}"
rounded:
  control: "{rounded.ctl}"
  tile: "{rounded.tile}"
  sheet: "{rounded.sheet}"
spacing: "{spacing}"
---

# Listenbox Desktop

The desktop app has no design system of its own. The monorepo's
[DESIGN.md](../../../../DESIGN.md) owns identity, color, type, shape, depth,
copy and accessibility for every surface, the desktop included. This file only
says how those rules land in native UI. Where the two disagree, the root wins
and this file is wrong.

Screen composition and interaction concepts belong in the
[desktop surface brief](../../.impeccable/surfaces/apps-client-crates-desktop.md),
and the role-to-theme mapping in
[native-projection.json](../../.impeccable/native-projection.json).

## Native, never web

Every screen is MyGo's `ui` package. Web pages, such as Dashboard flows, open in
the user's browser. Keep the operating system's window chrome, menus, status
item, light/dark appearance and Reduce Motion setting; do not imitate a browser
page inside the window.

## Color

The root's Desktop surface: a white window, white panels (the sidebar, the
sync panel, notices and popovers), periwinkle primary actions with white text,
graphite and slate text, `line` hairlines. Map those roles onto the MyGo
theme, each with its light and ink value: background, sheet, rail, control,
fg, fg-soft, edge, line, selected, action, focus, danger and success.
Dark appearance swaps to the root's ink tone set (ink window, ink-raised
panels, snow text); it is never a translucent overlay on the light theme.
The desktop speaks the Dashboard's language. The sidebar is an inset white
panel (28-DIP corners, a `line` hairline, soft shadow, a 10-DIP margin) with
no visible divider; the sync status and the episode list are white 28-DIP
cards on the white window, each lifted by its hairline and the soft shadow.
Warm paper is never the window; it fills wells only: fields, quiet controls,
skeletons and empty tracks. Periwinkle marks primary actions, focus, progress and paid plan
badges; the selected podcast is mist with its title in deep periwinkle, as the
Dashboard's current nav item (on ink: #1f2235 with light periwinkle). Quiet
actions are white with a 12% hairline, fields are paper recesses, and the
Manage podcast handoff leads with the real Listenbox app icon (the white mark
on its black tile), never the bare mark.

Contrast follows the root: text and placeholders at least 4.5:1, control edges
and focus indicators at least 3:1 against what they sit on. `line` is too faint
to bound a control, so controls take `edge` (pewter on white, snow-faint on
ink). The root's danger is 4.0:1 on paper wells, so it edges failures while their
text darkens it 20% toward graphite. Measure the composed native result,
including hover, pressed, selected and focus states.

## Type

Use the operating system font, with the root ramp in points: page titles,
region titles, body and controls, labels, buttons and support text. Hierarchy
comes from size and weight; headings take the root's heavy-and-tight voice
(700, -0.03em; -0.02em for region titles), labels and actions 600. No eyebrow
labels and no extra fonts. Tabular
numerals for durations, counts and byte totals.

User content (podcast and episode titles, team names, YouTube reasons) arrives
in any script. The window keeps the app language's direction. Within it, user
content starts at the interface's start edge and each paragraph reads in the
direction of its first strong character, so mixed-script lists keep one edge.

## Shape, space and depth

Use the root's radius, spacing and elevation scales, as the Dashboard does: 14
DIPs for controls, 10 for menu items, 18 for podcast rows and popovers, 22 for
the quit notice, 28 for the sidebar panel and every content card, and covers
from 12 to 24 by size. The sidebar is inset 10 DIPs (the frame) with 20 DIPs
inside; cards take the root's card-pad floor, 24 DIPs, a 24-DIP gap between
them, and the content pane keeps a 40-DIP margin (the gutter). Every page
section (sync status, episodes, the import form, each settings group) is a
card; nothing sits loose on the window but the page title. White cards take a
`line` hairline and the root's soft shadow; popovers and the quit notice take its float shadow.
Episode rows are flat, split by `line` hairlines, as the Dashboard's panel
rows.

## Controls

Use MyGo controls for focus, keyboard activation, disabled, pressed and pointer
states. Every control (actions, fields and the team switcher) shares one
shape, the root's md button: 44 DIPs tall, the 14-DIP control radius and a
20-DIP inline inset (16 for fields), so stacked and side-by-side controls line up and match. No control is
a capsule; capsules are for badges. Primary actions are the root's accent
button; the others are its light button (white face, paper on hover) with a
12% `edge` hairline. Available primary actions carry the root's accent shadow (0 10 24 −12, periwinkle at 90%); a disabled one drops it. Large artwork takes the soft shadow, as on podcast websites. View code
never sizes or rounds a control itself. Links to the web
are keyboard-focusable and underline on hover. Controls never move on hover.
Custom-content buttons carry an explicit accessible label, and icon-only
buttons add a tooltip.

## Evidence

[`design.go`](design.go) is the only place values live: the root palette in
its OKLCH values for both appearances, the type roles in root rems, the radius
and spacing scales, elevation and component metrics. It applies them to a copy
of MyGo's theme, as MyGo's styling guide recommends, so built-in widgets follow
them and the theme still tracks the desktop's contrast and text size. Custom
looks are built on MyGo's `*Base` widgets, such as `MenuBase` for the team
switcher. Headless MyGo captures establish layout and interaction in both
appearances; native chrome and OS accessibility need the running app.

---
name: Listenbox Desktop Native
description: The monorepo's Listenbox design system, projected onto MyGo's native UI.
extends: ../../../../DESIGN.md
colors:
  background-light: "{bg}"
  background-dark: "{dark-bg}"
  sheet-light: "{sheet}"
  sheet-dark: "{dark-sheet}"
  rail-light: "{rail}"
  rail-dark: "{dark-rail}"
  rail-control-light: "{rail-control}"
  rail-control-dark: "{dark-rail-control}"
  ink-light: "{ink}"
  ink-dark: "{dark-ink}"
  muted-light: "{muted}"
  muted-dark: "{dark-muted}"
  border-light: "{border}"
  border-dark: "{dark-border}"
  divider-light: "{divider}"
  divider-dark: "{dark-divider}"
  selected-light: "{navigation-active}"
  selected-dark: "{dark-navigation-active}"
  action-light: "{primary}"
  action-dark: "{dark-primary}"
  danger: "{danger}"
typography:
  page-title: "{page-title}"
  title: "{title}"
  body: "{field}"
  label: "{label}"
  button: "{button}"
  support: "{support}"
rounded: "{rounded}"
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

Map the root's semantic roles onto the MyGo theme: background, sheet, rail,
rail control, ink, muted, border, divider, selected, action and danger, each
with its root light and dark value. Dark appearance remaps every role; it is
never a translucent overlay on the light theme. Structural neutrals stay
achromatic. Color belongs to status, platform marks and podcast artwork.

Contrast follows the root: text and placeholders at least 4.5:1, control edges
and focus indicators at least 3:1 against what they sit on. Measure the
composed native result, including hover, pressed, selected and focus states.

## Type

Use the operating system font, with the root ramp in points: page titles,
region titles, body and controls, labels, buttons and support text. Hierarchy
comes from size and weight; no eyebrow labels and no extra fonts. Tabular
numerals for durations, counts and byte totals.

User content (podcast and episode titles, team names, YouTube reasons) arrives
in any script. The window keeps the app language's direction. Within it, user
content starts at the interface's start edge and each paragraph reads in the
direction of its first strong character, so mixed-script lists keep one edge.

## Shape, space and depth

Use the root's radius, spacing and elevation scales. Borders and tone carry
structure; reserve the root's overlay shadow for popovers and other genuinely
layered surfaces. Lists are flat rows with quiet dividers, not stacks of cards.

## Controls

Use MyGo controls for focus, keyboard activation, disabled, pressed and pointer
states. Actions are the root's 36-DIP capsules: the charcoal primary action and
quiet secondary ones; fields take the root's field radius. Links to the web
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

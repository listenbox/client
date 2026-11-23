---
version: 1
slug: "apps-client-crates-desktop"
primary_target: "apps/client/crates/desktop"
related_targets: []
---

# Listenbox desktop

Mode: Operate. Podcasters with a paid audio or video plan keep their YouTube playlist and Listenbox show aligned. Teams and existing YouTube imports load live. Ordinary shows and destinations are hidden. Shared authorization opens the dashboard in the browser. Work is resumable and the engine owns every checkpoint.

## Direction contract

THESIS: a native podcast library on the monorepo's Listenbox system: team switcher, creation action and artwork rows share one left edge, and automatic sync makes progress visible without setup controls.

OWN-WORLD: none. The desktop is the root DESIGN.md projected onto MyGo's native UI (see the desktop DESIGN.md); where a pattern exists on the Dashboard, such as the team switcher, the desktop matches it. Native window chrome, menus and appearance replace web chrome. Mazit supplies transfer mechanics only.

STORY: sign in, import a fresh playlist or select an existing import, then let automatic sync keep it current. See what is downloading, preparing, or uploading; Pause saves a podcast’s progress and skips its automatic scans until Resume or the next app session; Reload preserves the pause and other podcasts keep syncing.

FIRST VIEWPORT: the native window title names the app; there is no in-app title bar. A resizable sidebar holds the Dashboard's team switcher, the charcoal import action, a podcast search, the podcast list and a footer toolbar with Reload and Settings. The main pane shows the selected podcast: cover, title, one line of facts, browser handoffs, one bordered sync panel, then the episode list.

FORM: code-led extension of the existing product workspace, selected from the established design authority. Seed key: existing-listenbox-native-workspace; no new visual-world tournament.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance

## Screen concepts

**Hierarchy.** Team, then import action, then podcast, then source and sync status, then episodes. Teams own the library scope; a podcast never becomes the parent of a team. Only `youtube.kind = import` podcasts belong here; ordinary shows and YouTube destinations stay in the web workspace.

**Team switcher.** The Dashboard's switcher: team name and plan badge on the trigger; teams with plan badges and a check, Create team, the signed-in account and Log out in the panel. The desktop library can span every team, so All teams leads the list when there is more than one team. Create team opens the Dashboard's form in the browser.

**Import.** A full-width charcoal action beneath the switcher, label at the start and a plus at the end. The form names the team that will own the new podcast and locks while creation runs. A source is fixed at creation; there are no Save playlist, Disconnect or Keep syncing controls.

**Podcast.** Cover beside the title, one muted line of episode count, format and latest release, then Manage podcast and Open in YouTube as content-width actions, Listenbox first, each led by its destination's mark: the Listenbox mark in ink, YouTube's red play mark. One bordered panel carries sync status, the last YouTube check, Sync now, Pause or Resume, and transfer progress while a sync runs. Loading or failed episode lists never claim a zero count.

**Episodes.** Flat rows: release date (and state, when not published) above the title, duration at the row's end. Every title with a YouTube video links to it; there is no separate handoff button. Published rows keep the API order and pending source rows sit around them by saved position. The Not imported filter appears only while failed or skipped items exist, shows each reason, and its Retry imports action reuses the engine's saved work.

**Sync.** Imports sync on startup and hourly; Sync now asks for another pass. Pause shows Pausing… until work drains, then Resume. Progress totals render only during an active sync; idle state keeps failure and paused notices when recovery is needed. Import failure after creation keeps the podcast resumable; never suggest importing a second podcast to recover the first.

**Navigation.** Option + Up/Down (Alt elsewhere) moves through the visible podcasts, even from a focused field, stopping at the ends. Reload is in the View menu (⌘R / Ctrl+R) and the sidebar footer; Settings and Log out are in the app menu.

**Quitting.** The first ⌘Q shows a centered, nonblocking shortcut hint for two seconds; hold or press again to quit. Shutdown reports draining work in the same place, without dimming the window.

**Empty and error states.** Describe real next steps and never invent activity or podcast content. Errors name the problem and the recovery, above the form they concern.

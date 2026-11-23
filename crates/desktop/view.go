//nolint:gosmopolitan // Local date rendering belongs at this display boundary.
package main

import (
	"fmt"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"
	s "github.com/listenbox/client/crates/sync-engine"
	p "github.com/listenbox/client/crates/sync-engine/publicapi"
)

//nolint:lll // Preserve embedded JavaScript, vector assets or independent wire fixtures verbatim.
var (
	podcastIcon = ui.MustParseSVG(
		[]byte(
			`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3 14h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-7a9 9 0 0 1 18 0v7a2 2 0 0 1-2 2h-1a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2h3"/></svg>`,
		),
	)
	refreshIcon = ui.MustParseSVG(
		[]byte(
			`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M20 12a8 8 0 1 1-2.34-5.66L20 8.5"/><path d="M20 3.5v5h-5"/></svg>`,
		),
	)
	plusIcon = ui.MustParseSVG(
		[]byte(
			`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>`,
		),
	)
	listenboxIcon = ui.MustParseSVG(listenboxIconSVG)
	youtubeMark   = ui.MustParseSVG(youtubeSVG)
)

//nolint:cyclop // Keep the immediate-mode frame hierarchy and its actor callbacks together.
func (w *Workspace) view(c *ui.Context) {
	w.start()
	width, _ := c.Size()
	w.sidebarWidth = max(minimumSidebar, min(w.sidebarWidth, min(maximumSidebar, width-minimumWorkspace)))
	for _, target := range w.openURLs {
		c.OpenURL(target)
	}
	w.openURLs = nil
	applyTheme(c)
	if c.Shortcut(ui.Alt, ui.KeyUp) && !w.navigationObserved {
		w.navigate(-1)
	}
	if c.Shortcut(ui.Alt, ui.KeyDown) && !w.navigationObserved {
		w.navigate(1)
	}
	if c.Shortcut(ui.Cmd, ui.KeyR) {
		w.reload(w.syncAll)
	}
	w.navigationObserved = false
	root := ui.Column(c).
		Fill().
		Background(colors(c).background).
		ObserveInput(func(event ui.InputEvent) {
			if event.Kind == ui.InputKeyDown && event.Mods == ui.Alt &&
				(event.Key == ui.KeyUp || event.Key == ui.KeyDown) {
				w.navigationObserved = true
				if event.Key == ui.KeyUp {
					w.navigate(-1)
				} else {
					w.navigate(1)
				}
			}
		})
	root.Children(func() {
		ui.Column(c).Grow(1).MinHeight(0).Disabled(w.stopping).Children(func() {
			if !w.authorized {
				w.signIn(c)
			} else {
				// The sidebar floats as an inset panel, so the divider between the
				// panes is drawn in the paper and shows only under the pointer.
				divided := *c.Theme()
				divided.Border = colors(c).background
				c.SetTheme(&divided)
				ui.Split(c, &w.sidebarWidth, func() { applyTheme(c); w.sidebar(c) }, func() { applyTheme(c); w.content(c) }).
					Grow(1).
					FillWidth().
					SplitReset(initialSidebarWidth)
			}
		})
	})
	w.quitHUD(c)
}

func (w *Workspace) signIn(c *ui.Context) {
	d := tokens(c)
	ui.Column(c).Fill().Center().Padding(d.xl).Gap(d.lg).Children(func() {
		// The Dashboard's auth screens lead with the app icon, never a glyph.
		ui.Image(c, listenboxIcon).Size(coverSmall, coverSmall).Label("Listenbox")
		typePageTitle.set(c, ui.Text(c, "Your podcasts, always in sync")).TextAlign(ui.Center)
		ui.Text(c, "Sign in to import from YouTube and keep your library up to date.").
			MaxWidth(readableWidth + readableWidth/2).
			TextAlign(ui.Center).
			TextColor(d.fgSoft)
		if w.errorText != "" {
			ui.Text(c, w.errorText).MaxWidth(readableWidth).TextColor(d.dangerText)
		}
		if w.signingIn {
			ui.Text(c, "Finish signing in your browser.")
			if w.loginURL != "" && button(c, "Reopen sign in").Clicked() {
				c.OpenURL(w.loginURL)
			}
			if button(c, "Cancel sign in").Clicked() {
				w.cancelLogin()
			}
		} else if primaryButton(c, "Sign in to Listenbox").Clicked() {
			w.login()
		}
	})
}

func (w *Workspace) sidebar(c *ui.Context) {
	ui.Column(c).Fill().MinWidth(minimumSidebar).Padding(frame, 0, frame, frame).Children(func() {
		w.sidebarPanel(c)
	})
}

// sidebarPanel is the Dashboard's rail: a white panel inset in the paper.
func (w *Workspace) sidebarPanel(c *ui.Context) {
	d := tokens(c)
	panel(c, ui.Column(c).
		Fill().
		Padding(railPad).
		Gap(d.md)).
		Background(d.rail).
		Children(func() {
			w.teamSwitcher(c)
			importButton := primaryAction(c, "", w.creating).
				FillWidth().
				Gap(d.sm).
				Label("Import from YouTube")
			importButton.Children(func() {
				ui.Text(c, "Import from YouTube").Grow(1).SingleLine()
				ui.Icon(c, plusIcon).Size(iconMedium, iconMedium)
			})
			if importButton.Clicked() {
				w.importOpen, w.settingsOpen, w.errorText = true, false, ""
				w.importTeam = w.team
				if w.importTeam == "" {
					w.importTeam = w.catalog.ImportTeam
				}
			}
			textField(c, &w.filter).
				Placeholder("Find a podcast").
				Label("Find a podcast").
				FillWidth()
			w.library(c)
			ui.Toolbar(c, func() {
				if w.loading {
					ui.Spinner(c).Label("Refreshing library…").Margin(0, d.sm)
				} else if button(c, "").
					Label("Refresh library").
					Tooltip("Refresh library").
					Children(func() { ui.Icon(c, refreshIcon).Size(iconMedium, iconMedium) }).
					Clicked() {
					w.reload(w.syncAll)
				}
				ui.Spacer(c)
				if button(c, "Settings").Clicked() {
					w.openSettings()
				}
			}).FillWidth().Label("Account")
		})
}

// library lists the visible podcasts with their artwork and sync state.
func (w *Workspace) library(c *ui.Context) {
	d := tokens(c)
	shows := w.visibleShows()
	w.libraryList.Key = func(index int) any { return shows[index].Id }
	ui.List(c, &w.libraryList, len(shows), func(index int) {
		show := shows[index]
		row := ui.Row(c.Key(show.Id)).
			FillWidth().
			Padding(d.sm).
			Gap(d.md).
			Radius(radiusTile).
			AlignItems(ui.Center).
			Label(show.Title)
		chosen := show.Slug == w.selected
		switch {
		case chosen:
			row.Background(d.selected)
		case row.Hovered():
			row.Background(d.control)
		}
		row.Children(func() {
			w.coverView(c, value(show.ImageUrl), coverSmall)
			ui.Column(c).Grow(1).MinWidth(0).Gap(d.xs).Children(func() {
				title := typeLabel.set(c, ui.Text(c, show.Title)).SingleLine()
				if chosen {
					title.TextColor(d.selectedText)
				}
				startAligned(c, title)
				label := episodeCount(show.EpisodeCount)
				if w.paused[show.Slug] {
					label = "Paused"
				} else if w.jobs[show.Slug] != nil {
					label = "Syncing…"
				}
				typeField.set(c, ui.Text(c, label)).TextColor(d.fgSoft).SingleLine()
			})
		})
		if row.Clicked() {
			w.selectShow(show.Slug)
		}
	}).Grow(1).MinHeight(0).Label("Podcast library")
}

// startAligned keeps user content on the interface's start edge. MyGo aligns
// a right-to-left paragraph by its language inside a left-to-right window, so
// a list mixing Hebrew and Latin titles would zigzag between both edges. A
// right-to-left element's End is its left edge; the paragraph still reads in
// the direction of its first strong character.
func startAligned(c *ui.Context, text ui.Element) {
	if c.Direction() == ui.LeftToRight {
		text.Direction(ui.RightToLeft).TextAlign(ui.End)
	}
}

func episodeCount(count int64) string {
	if count == 1 {
		return "1 episode"
	}
	return fmt.Sprintf("%d episodes", count)
}

func (w *Workspace) content(c *ui.Context) {
	// Settings scroll as a whole page, so no scroll edge clips a card's shadow.
	if w.settingsOpen {
		ui.Scroll(c).Fill().MinWidth(0).Children(func() {
			ui.Column(c).FillWidth().Padding(pageInset, pageInset, gridGap, pageInset).Gap(gridGap).Children(func() {
				w.contentBody(c)
			})
		})
		return
	}
	ui.Column(c).Fill().MinWidth(0).Padding(pageInset, pageInset, gridGap, pageInset).Gap(gridGap).Children(func() {
		w.contentBody(c)
	})
}

// contentBody is the content pane's page: a notice, then settings, the import
// form, the selected podcast or the empty library.
//
//nolint:cyclop // Render the mutually exclusive workspace and error states in one immediate-mode tree.
func (w *Workspace) contentBody(c *ui.Context) {
	d := tokens(c)
	{
		if w.errorText != "" && !w.settingsOpen {
			panel(c, ui.Column(c).FillWidth().Padding(cardPad).Gap(d.md)).
				Border(1, d.danger).
				Children(func() {
					ui.Text(c, w.errorText).TextColor(d.dangerText)
					ui.Row(c).Gap(d.sm).Children(func() {
						if w.upgradeURL != "" && button(c, "Upgrade plan").Clicked() {
							c.OpenURL(w.upgradeURL)
						}
						if w.signInRequired && button(c, "Open YouTube settings").Clicked() {
							w.openSettings()
						}
						if button(c, "Dismiss").Clicked() {
							w.errorText = ""
						}
					})
				})
		}
		switch {
		case w.settingsOpen:
			w.settings(c)
		case w.importOpen || w.creating:
			w.importForm(c)
		case w.show() != nil:
			w.podcast(c, *w.show())
		default:
			ui.Column(c).Fill().Center().Gap(d.lg).Children(func() {
				typePageTitle.set(c, ui.Text(c, "Your podcast library"))
				ui.Text(c, "Import a YouTube playlist or channel to get started.").TextColor(d.fgSoft)
				if primaryButton(c, "Import a podcast").Clicked() {
					w.importOpen = true
					w.importTeam = w.team
				}
			})
		}
	}
}

func (w *Workspace) coverView(c *ui.Context, target string, size float32) {
	d := tokens(c)
	radius := float32(radiusSmall)
	if size >= coverLarge {
		radius = radiusCover
	}
	var art ui.Element
	if bitmap := w.cover(target); bitmap != nil {
		art = ui.Image(c, bitmap).Size(size, size).Shrink(0).Radius(radius)
	} else {
		art = ui.Box(c).
			Size(size, size).
			Shrink(0).
			Radius(radius).
			Background(d.placeholder).
			Center().
			Children(func() { ui.Icon(c, podcastIcon).Size(size*placeholderIcon, size*placeholderIcon).TextColor(d.fgSoft) })
	}
	// Large artwork sits on the page with the soft shadow, as on podcast sites.
	if size >= coverLarge {
		soft(art)
	}
}

//nolint:cyclop,funlen,gocognit,gocyclo // Keep podcast controls and states in one render tree.
func (w *Workspace) podcast(c *ui.Context, show p.Show) {
	d := tokens(c)
	ui.Row(c).Gap(d.xl).AlignItems(ui.Start).Children(func() {
		w.coverView(c, value(show.ImageUrl), coverLarge)
		ui.Column(c).Grow(1).MinWidth(0).Gap(d.sm).Children(func() {
			startAligned(c, typePageTitle.set(c, ui.Text(c, show.Title)).Balance().MaxLines(titleLines))
			meta := episodeCount(show.EpisodeCount) + " · " + sourceKindLabel(show.SourceKind)
			if show.LastEpisodeAt != nil {
				meta += " · Latest " + time.UnixMilli(*show.LastEpisodeAt).Local().Format("Jan 2, 2006")
			}
			ui.Text(c, meta).TextColor(d.fgSoft)
			ui.Row(c).Wrap().Gap(d.sm).Margin(d.sm, 0, 0, 0).Children(func() {
				// Both open the browser; the leading mark names the destination.
				if handoffButton(c, "Manage podcast", listenboxIcon).Clicked() {
					c.OpenURL(w.managementURL())
				}
				if source := value(show.Youtube.Url); source != "" {
					if handoffButton(c, "Open in YouTube", youtubeMark).Label("YouTube source").Clicked() {
						c.OpenURL(source)
					}
				}
			})
		})
	})
	var total, complete, unfinished int
	var sent, bytes int64
	for _, item := range w.items {
		if item.SourceID == show.Slug {
			total++
			if item.Phase == s.Complete() {
				complete++
			}
			if item.Phase.Active() || item.Phase == s.Queued() {
				unfinished++
			}
			sent += item.Uploaded
			bytes += item.UploadTotal
		}
	}
	syncing := w.jobs[show.Slug] != nil
	panel(c, ui.Column(c).FillWidth().Padding(railPad, railPad, railPad, cardPad).Gap(d.md)).
		Children(func() {
			ui.Row(c).Gap(d.md).AlignItems(ui.Center).Children(func() {
				switch {
				case !show.HasActiveSubscription:
					ui.Text(c, "Choose a plan to keep this podcast in sync.").Grow(1)
					if primaryButton(c, "Choose a plan").Clicked() {
						if target, err := w.client.Config.UpgradeURL(show.TeamId); err == nil {
							c.OpenURL(target)
						}
					}
				case syncing:
					ui.Spinner(c).Label("Syncing")
					ui.Text(c, "Syncing with YouTube…").Grow(1)
					if button(c, "Pause").Clicked() {
						w.pause(show.Slug)
					}
				default:
					ui.Column(c).Grow(1).MinWidth(0).Gap(d.xs).Children(func() {
						label := "Not synced yet"
						if w.paused[show.Slug] {
							label = "Paused for this session"
						} else if synced := w.lastSynced(show); synced != nil {
							label = "Last synced " + synced.Local().Format("Jan 2, 15:04")
						}
						ui.Text(c, label)
						if w.checking || w.loading {
							ui.Text(c, "Checking Listenbox…").TextColor(d.fgSoft)
						}
					})
					action := syncNowLabel
					if w.paused[show.Slug] {
						action = "Resume"
					}
					if primaryButton(c, action).Clicked() {
						w.sync(show.Slug)
					}
				}
			})
			// A finished sync leaves no queued or active media, so the card drops
			// back to its status line instead of a full bar and empty totals.
			if total > 0 && (syncing || unfinished > 0) {
				// Byte totals grow as each episode is prepared, so only the item
				// count is a stable base for overall progress.
				ratio := float64(complete) / float64(total)
				ui.Progress(c, ratio).FillWidth().Background(d.track).Label("Imported episode progress")
				progress := fmt.Sprintf("%d of %d imported", complete, total)
				if bytes > 0 {
					progress += " · " + s.TransferTotals(sent, bytes) + " sent"
				}
				ui.Text(c, progress).FontFeatures("tnum").TextColor(d.fgSoft)
			}
		})
	hasIssues := false
	for _, item := range w.state.Items {
		hasIssues = hasIssues || item.Reason != "" || item.Error != ""
	}
	for _, item := range w.items {
		hasIssues = hasIssues ||
			(item.SourceID == show.Slug && (item.Reason != "" || item.Error != ""))
	}
	if !hasIssues {
		w.notImported = false
	}
	rows := w.rows()
	panel(c, ui.Column(c).FillWidth().Grow(1).MinHeight(0).Padding(cardPad, cardPad, d.sm, cardPad).Gap(d.sm)).
		Children(func() { w.episodeCard(c, show, rows, hasIssues, syncing) })
}

// episodeCard is the episode card: its heading, the not-imported filter and the
// list.
func (w *Workspace) episodeCard(c *ui.Context, show p.Show, rows []episodeRow, hasIssues, syncing bool) {
	d := tokens(c)
	ui.Row(c).Gap(d.md).AlignItems(ui.Center).Children(func() {
		typeTitle.set(c, ui.Text(c, "Episodes"))
		if len(rows) > 0 {
			ui.Textf(c, "%d", len(rows)).FontFeatures("tnum").TextColor(d.fgSoft)
		}
		ui.Spacer(c)
		if hasIssues {
			ui.Checkbox(c, &w.notImported, "Not imported")
			if button(c, "Retry imports").
				Disabled(!show.HasActiveSubscription || syncing).
				Clicked() {
				w.sync(show.Slug)
			}
		}
	})
	w.episodeTable(c, rows)
}

// episodeTable lists the merged episode rows: date above a title that links to
// its YouTube video, the duration at the end.
func (w *Workspace) episodeTable(c *ui.Context, rows []episodeRow) {
	d := tokens(c)
	if len(rows) == 0 {
		empty := "No episodes to show yet."
		if w.checking {
			empty = "Loading episodes…"
		}
		ui.Text(c, empty).TextColor(d.fgSoft)
		return
	}
	w.episodeList.Key = func(index int) any { return rows[index].key }
	ui.List(c, &w.episodeList, len(rows), func(index int) {
		row := rows[index]
		// Rows are split by hairlines, as the Dashboard's panel rows.
		tile := ui.Row(c.Key(row.key)).
			FillWidth().
			Padding(d.lg, 0).
			Gap(d.lg).
			AlignItems(ui.Center)
		if index > 0 {
			tile.Border(1, d.line).BorderWidth(1, 0, 0, 0)
		}
		tile.Children(func() {
			ui.Column(c).Grow(1).MinWidth(0).Gap(d.xs).Children(func() {
				if meta := row.meta(); meta != "" {
					ui.Text(c, meta).TextColor(d.fgSoft).SingleLine()
				}
				var title ui.Element
				if row.source != "" {
					title = ui.Link(c, row.title, row.source).
						Label("Open " + row.title + " on YouTube").
						TextColor(d.fg)
				} else {
					title = ui.Text(c, row.title)
				}
				startAligned(c, typeFeatureTitle.set(c, title).MaxLines(titleLines-1).AlignSelf(ui.Start))
				if row.reason != "" {
					startAligned(c, ui.Text(c, row.reason).TextColor(d.dangerText))
				}
			})
			ui.Text(c, duration(row.duration)).
				Shrink(0).
				FontFeatures("tnum").
				TextColor(d.fgSoft)
		})
	}).Grow(1).MinHeight(0).Label("Episode list")
}

// meta is the line above an episode title: its release date and any state
// other than published.
func (row episodeRow) meta() string {
	var parts []string
	if row.published != nil {
		parts = append(parts, time.UnixMilli(*row.published).Local().Format("Jan 2, 2006"))
	}
	if row.status != "" && row.status != string(p.EpisodeStatusPublished) {
		parts = append(parts, strings.ToUpper(row.status[:1])+row.status[1:])
	}
	return strings.Join(parts, " · ")
}

func sourceKindLabel(kind p.ShowSourceKind) string {
	if kind == p.ShowSourceKindVideo {
		return "Video"
	}
	return "Audio"
}

func (w *Workspace) importForm(c *ui.Context) {
	d := tokens(c)
	typePageTitle.set(c, ui.Text(c, "Import from YouTube"))
	ui.Text(c, "Create a podcast from a playlist or channel.").TextColor(d.fgSoft)
	card(c, func() { w.importFields(c) })
}

// importFields is the import card: the source, its format, the owning team,
// preparation progress and the actions.
//
//nolint:cyclop // Render the import form and its mutually exclusive preparation states together.
func (w *Workspace) importFields(c *ui.Context) {
	d := tokens(c)
	textField(c, &w.importSource).
		Label("YouTube URL").
		Placeholder("Paste a YouTube URL").
		FillWidth().
		Disabled(w.creating)
	ui.Row(c).Gap(d.lg).Children(func() {
		ui.Radio(c, &w.importKind, p.ShowSourceKindAudio, "Audio").Disabled(w.creating)
		ui.Radio(c, &w.importKind, p.ShowSourceKindVideo, "Video").Disabled(w.creating)
	})
	typeLabel.set(c, ui.Text(c, "Import into team"))
	ui.Scroll(c).MaxHeight(menuListHeight).Gap(d.xs).Children(func() {
		for _, team := range w.catalog.Teams {
			ui.Radio(c.Key(team.Id), &w.importTeam, team.Id, team.Name).Disabled(w.creating)
		}
	})
	if w.creating {
		heading := "Checking your plan…"
		switch w.preparation.Stage {
		case "reading_playlist", "scanning_playlist":
			heading = "Reading your YouTube playlist…"
		case "checking_storage":
			heading = "Checking video storage…"
		case "creating_podcast":
			heading = "Creating your podcast…"
		}
		ui.Progress(c, -1).FillWidth().Background(d.track)
		typeLabel.set(c, ui.Text(c, heading))
		if scan := w.preparation.Scan; scan != nil {
			ui.Textf(c, "%d videos · %s estimated", scan.Videos, duration(&scan.EstimatedSeconds))
			if scan.UnknownDurations > 0 {
				ui.Textf(c, "%d videos have an unknown duration", scan.UnknownDurations).TextColor(d.fgSoft)
			}
		}
		if remaining := w.preparation.VideoRemainingSeconds; remaining != nil {
			ui.Textf(c, "%s of video storage available", duration(remaining)).TextColor(d.fgSoft)
		}
	}
	ui.Row(c).Gap(d.md).Children(func() {
		if primaryAction(c, "Import",
			w.creating || strings.TrimSpace(w.importSource) == "" || w.importTeam == "").
			Clicked() {
			w.importShow()
		}
		if button(c, "Cancel import").Clicked() {
			if w.importCancel != nil {
				w.importCancel()
			}
			w.importOpen, w.creating = false, false
		}
	})
}

func (w *Workspace) settings(c *ui.Context) {
	ui.Row(c).AlignItems(ui.Center).Children(func() {
		typePageTitle.set(c, ui.Text(c, "Settings")).Grow(1)
		if button(c, "Done").Disabled(w.cookieBusy).Clicked() {
			w.settingsOpen = false
		}
	})
	card(c, func() { w.cookieSettings(c) })
	if updatesSupported() {
		card(c, func() { w.updateSettings(c) })
	}
}

// cookieSettings is the YouTube card: why cookies, the guide, the editor and
// its actions.
func (w *Workspace) cookieSettings(c *ui.Context) {
	d := tokens(c)
	typeTitle.set(c, ui.Text(c, "YouTube"))
	ui.Column(c).Gap(d.xs).Children(func() {
		ui.Text(c, "If YouTube asks you to sign in, add cookies from a private browser session to co"+
			"ntinue importing.").
			TextColor(d.fgSoft)
		ui.Text(c, "Cookies stay on this computer and are shared with the Listenbox CLI. "+
			"Keep them private, like a password.").TextColor(d.fgSoft)
	})
	if button(c, "How to export YouTube cookies").AlignSelf(ui.Start).Clicked() {
		c.OpenURL(s.CookieGuideURL)
	}
	typeLabel.set(c, ui.Text(c, "YouTube cookies"))
	label := "Paste a Netscape cookies.txt export, including the header line."
	if w.cookieSaved {
		label = "A session is saved. Pasting a new export replaces it."
	}
	ui.Text(c, label).TextColor(d.fgSoft)
	ui.TextArea(c.Key(w.cookieEditor), &w.cookieInput).
		Placeholder("Paste the complete Netscape cookies.txt export here").
		Label("YouTube cookies").
		FillWidth().
		Height(cookieEditorHeight).
		Radius(controlRadius).
		Background(d.control).
		Padding(d.md, fieldInset).
		Disabled(w.cookieBusy)
	if w.cookieError != "" {
		ui.Text(c, w.cookieError).TextColor(d.dangerText)
	}
	if w.cookieMessage != "" {
		ui.Text(c, w.cookieMessage).TextColor(d.fgSoft)
	}
	ui.Row(c).Gap(d.md).Children(func() {
		if primaryAction(c, "Save cookies", w.cookieBusy).Clicked() {
			w.saveCookies(false)
		}
		if button(c, "Remove cookies").
			Disabled(w.cookieBusy || (!w.cookieSaved && w.cookieError == "")).
			Clicked() {
			w.saveCookies(true)
		}
	})
}

// quitHUD covers the window while a confirmed quit drains admitted work.
func (w *Workspace) quitHUD(c *ui.Context) {
	if !w.stopping {
		return
	}
	d := tokens(c)
	ui.Overlay(c, func() {
		ui.Box(c).Fill().Center().Children(func() {
			overlay(ui.Column(c).
				Width(hudWidth).
				Padding(d.xl).
				Gap(d.md).
				Radius(radiusSheet).
				Background(d.sheet).
				Border(1, d.line)).
				Children(func() {
					typeTitle.set(c, ui.Text(c, "Finishing up…"))
					ui.Text(c, "Saving progress and stopping active work.")
				})
		})
	})
}

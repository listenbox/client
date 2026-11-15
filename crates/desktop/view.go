//nolint:mnd,gosmopolitan // Layout values and local date rendering belong at this display boundary.
package main

import (
	"fmt"
	"runtime"
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
			`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"><rect x="4" y="3" width="16" height="18" rx="3"/><path d="M8 8h8M8 12h8M8 16h4"/></svg>`,
		),
	)
	chevronIcon = ui.MustParseSVG(
		[]byte(
			`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>`,
		),
	)
	plusIcon = ui.MustParseSVG(
		[]byte(
			`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>`,
		),
	)
)

//nolint:cyclop,funlen,gocognit // Keep the immediate-mode frame hierarchy and its actor callbacks together.
func (w *Workspace) view(c *ui.Context) {
	w.start()
	width, _ := c.Size()
	w.sidebarWidth = max(240, min(w.sidebarWidth, min(480, width-360)))
	for _, target := range w.openURLs {
		c.OpenURL(target)
	}
	w.openURLs = nil
	theme := *c.Theme()
	theme.FontSize, theme.Radius = 14, 10
	if theme.Dark {
		theme.Background, theme.Surface, theme.Text, theme.TextMuted = ui.Hex(
			"#191919",
		), ui.Hex(
			"#222222",
		), ui.Hex(
			"#f5f5f5",
		), ui.Hex(
			"#bebebe",
		)
		theme.Border, theme.Accent, theme.AccentHover, theme.AccentPressed = ui.Hex(
			"#515151",
		), ui.Hex(
			"#6f6f6f",
		), ui.Hex(
			"#7e7e7e",
		), ui.Hex(
			"#616161",
		)
	} else {
		theme.Background, theme.Surface, theme.Text, theme.TextMuted = ui.Hex(
			"#fafafa",
		), ui.Hex(
			"#ffffff",
		), ui.Hex(
			"#202020",
		), ui.Hex(
			"#555555",
		)
		theme.Border, theme.Accent, theme.AccentHover, theme.AccentPressed = ui.Hex(
			"#d6d6d6",
		), ui.Hex(
			"#303030",
		), ui.Hex(
			"#414141",
		), ui.Hex(
			"#202020",
		)
	}
	theme.AccentText = ui.Hex("#ffffff")
	c.SetTheme(&theme)
	if c.Shortcut(ui.Alt, ui.KeyUp) && !w.navigationObserved {
		w.navigate(-1)
	}
	if c.Shortcut(ui.Alt, ui.KeyDown) && !w.navigationObserved {
		w.navigate(1)
	}
	if c.Shortcut(ui.Cmd, ui.KeyR) {
		w.reload()
	}
	w.navigationObserved = false
	if c.Shortcut(ui.Cmd, ui.KeyQ) {
		repeat := c.ShortcutRepeated()
		w.guard.press(c.Now(), repeat)
		if !repeat {
			w.quitShown = c.Now()
		}
	}
	if !w.guard.heldSince.IsZero() {
		if !c.KeyDown(ui.KeyQ) {
			if w.guard.release() {
				w.stop(false)
			}
		} else {
			w.quitConfirmed = w.guard.holding(c.Now())
			c.After(50 * time.Millisecond)
		}
	}
	root := ui.Column(c).
		Fill().
		Background(theme.Background).
		ObserveInput(func(event ui.InputEvent) {
			if event.Kind == ui.InputKeyDown && event.Mods == ui.Alt &&
				(event.Key == ui.KeyUp || event.Key == ui.KeyDown) {
				w.navigationObserved = true
				if event.Key == ui.KeyUp {
					w.navigate(-1)
				} else {
					w.navigate(1)
				}
				return
			}
			if event.Key != ui.KeyQ {
				return
			}
			if event.Kind == ui.InputKeyUp {
				if w.guard.release() {
					w.stop(false)
				}
			}
		})
	root.Children(func() {
		ui.Row(c).
			Height(56).
			Shrink(0).
			Padding(10, 16).
			Gap(12).
			AlignItems(ui.Center).
			Border(1, theme.Border).
			BorderWidth(0, 0, 1, 0).
			Children(func() {
				ui.Text(c, applicationName).Bold().FontSize(18).Grow(1)
				if ui.Button(c, "Settings").Disabled(w.stopping).Clicked() {
					w.openSettings()
				}
				if w.authorized && ui.Button(c, "Sign out").Disabled(w.stopping).Clicked() {
					w.stop(true)
				}
			})
		ui.Column(c).Grow(1).MinHeight(0).Disabled(w.stopping).Children(func() {
			if !w.authorized {
				w.signIn(c)
			} else {
				ui.Split(c, &w.sidebarWidth, func() { w.sidebar(c) }, func() { w.content(c) }).
					Grow(1).
					FillWidth().
					SplitReset(240)
			}
		})
	})
	w.quitHUD(c)
}

func (w *Workspace) signIn(c *ui.Context) {
	ui.Column(c).Fill().Center().Padding(24).Gap(20).Children(func() {
		ui.Icon(c, podcastIcon).Size(56, 56).TextColor(c.Theme().TextMuted)
		ui.Text(c, "Your podcasts, always in sync").FontSize(26).Bold()
		ui.Text(c, "Sign in to import from YouTube and keep your library up to date.").
			MaxWidth(420).
			TextAlign(ui.Center).
			TextColor(c.Theme().TextMuted)
		if w.errorText != "" {
			ui.Text(c, w.errorText).MaxWidth(420).TextColor(c.Theme().Danger)
		}
		if w.signingIn {
			ui.Text(c, "Finish signing in your browser.")
			if w.loginURL != "" && ui.Button(c, "Reopen sign in").Clicked() {
				c.OpenURL(w.loginURL)
			}
			if ui.Button(c, "Cancel sign in").MinWidth(140).Clicked() {
				w.cancelLogin()
			}
		} else if ui.PrimaryButton(c, "Sign in to Listenbox").MinWidth(180).Clicked() {
			w.login()
		}
	})
}

//nolint:cyclop,funlen,gocognit // Keep the immediate-mode sidebar hierarchy and selection callbacks together.
func (w *Workspace) sidebar(c *ui.Context) {
	fill := ui.Hex("#f2f2f2")
	if c.Theme().Dark {
		fill = ui.Hex("#222222")
	}
	ui.Column(c).Fill().MinWidth(240).Padding(16, 8).Gap(12).Background(fill).Children(func() {
		teamName := "All teams"
		for _, team := range w.catalog.Teams {
			if team.Id == w.team {
				teamName = team.Name
			}
		}
		picker := ui.Button(c, "").FillWidth().MinHeight(36).Padding(8).Gap(8).Label("Choose team")
		picker.Children(func() {
			ui.Text(c, teamName).Grow(1).MinWidth(0).SingleLine().FontSize(18).Bold()
			ui.Icon(c, chevronIcon).Size(16, 16)
		})
		if picker.Clicked() {
			w.teamPicker = !w.teamPicker
		}
		ui.Popover(c, picker, &w.teamPicker, func() {
			ui.List(c, &w.teamList, len(w.catalog.Teams)+1, func(index int) {
				team := p.ClientTeam{Name: "All teams"}
				if index > 0 {
					team = w.catalog.Teams[index-1]
				}
				if ui.Button(c.Key(team.Id), team.Name).
					FillWidth().
					Justify(ui.Start).
					Selected(w.team == team.Id).
					Clicked() {
					w.team, w.teamPicker, w.selected = team.Id, false, ""
					if shows := w.visibleShows(); len(shows) > 0 {
						w.selectShow(shows[0].Slug)
					}
				}
			}).Width(280).Height(min(280, float32((len(w.catalog.Teams)+1)*44))).Label("Teams")
		})
		importButton := ui.PrimaryButton(c, "").
			FillWidth().
			MinHeight(36).
			Padding(8).
			Gap(8).
			Label("Import from YouTube").
			Disabled(w.creating)
		importButton.Children(func() {
			ui.Text(c, "Import from YouTube").Grow(1).SingleLine()
			ui.Icon(c, plusIcon).Size(16, 16)
		})
		if importButton.Clicked() {
			w.importOpen, w.settingsOpen, w.errorText = true, false, ""
			w.importTeam = w.team
			if w.importTeam == "" {
				w.importTeam = w.catalog.ImportTeam
			}
		}
		ui.TextInput(c, &w.filter).Placeholder("Find a podcast").Label("Find a podcast").FillWidth()
		shows := w.visibleShows()
		w.libraryList.Key = func(index int) any { return shows[index].Id }
		ui.List(c, &w.libraryList, len(shows), func(index int) {
			show := shows[index]
			row := ui.Row(c.Key(show.Id)).
				FillWidth().
				Padding(8).
				Gap(10).
				Radius(8).
				AlignItems(ui.Center).
				Label(show.Title)
			if show.Slug == w.selected {
				row.Background(c.Theme().Border.Alpha(.4))
			}
			row.Children(func() {
				w.coverView(c, value(show.ImageUrl), 44)
				ui.Column(c).Grow(1).MinWidth(0).Gap(4).Children(func() {
					ui.Text(c, show.Title).Bold().SingleLine()
					label := fmt.Sprintf("%d episodes", show.EpisodeCount)
					if show.EpisodeCount == 1 {
						label = "1 episode"
					}
					if show.LastEpisodeAt != nil {
						label += " · Latest release " + time.UnixMilli(*show.LastEpisodeAt).
							Local().
							Format("Jan 2, 2006")
					}
					if w.paused[show.Slug] {
						label = "Paused"
					} else if w.jobs[show.Slug] != nil {
						label = "Syncing…"
					}
					ui.Text(c, label).FontSize(12).TextColor(c.Theme().TextMuted).SingleLine()
				})
			})
			if row.Clicked() {
				w.selectShow(show.Slug)
			}
		}).Grow(1).MinHeight(0).Label("Podcast library")
		if w.loading {
			ui.Text(c, "Refreshing library…").FontSize(12).TextColor(c.Theme().TextMuted)
		}
		if ui.Button(c, "Refresh library").FillWidth().Disabled(w.loading).Clicked() {
			w.reload()
		}
	})
}

//nolint:cyclop // Render the mutually exclusive workspace and error states in one immediate-mode tree.
func (w *Workspace) content(c *ui.Context) {
	ui.Column(c).Fill().MinWidth(0).Padding(24).Gap(20).Children(func() {
		if w.errorText != "" && !w.settingsOpen {
			ui.Column(c).
				FillWidth().
				Padding(12).
				Gap(8).
				Background(c.Theme().Danger.Alpha(.08)).
				Radius(8).
				Children(func() {
					ui.Text(c, w.errorText).TextColor(c.Theme().Danger)
					ui.Row(c).Gap(8).Children(func() {
						if w.upgradeURL != "" && ui.Button(c, "Upgrade plan").Clicked() {
							c.OpenURL(w.upgradeURL)
						}
						if w.signInRequired && ui.Button(c, "Open YouTube settings").Clicked() {
							w.openSettings()
						}
						if ui.Button(c, "Dismiss").Clicked() {
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
			ui.Column(c).Fill().Center().Gap(16).Children(func() {
				ui.Text(c, "Your podcast library").FontSize(30).Bold()
				ui.Text(c, "Import a YouTube playlist or channel to get started.").
					TextColor(c.Theme().TextMuted)
				if ui.PrimaryButton(c, "Import a podcast").Clicked() {
					w.importOpen = true
					w.importTeam = w.team
				}
			})
		}
	})
}

func (w *Workspace) coverView(c *ui.Context, target string, size float32) {
	if bitmap := w.cover(target); bitmap != nil {
		ui.Image(c, bitmap).Size(size, size).Radius(6)
	} else {
		ui.Box(c).
			Size(size, size).
			Shrink(0).
			Radius(6).
			Background(c.Theme().Border.Alpha(.4)).
			Center().
			Children(func() { ui.Icon(c, podcastIcon).Size(size*.5, size*.5).TextColor(c.Theme().TextMuted) })
	}
}

//nolint:cyclop,funlen,gocognit,gocyclo,maintidx,nestif // Keep podcast controls and states in one render tree.
func (w *Workspace) podcast(c *ui.Context, show p.Show) {
	ui.Row(c).Gap(24).AlignItems(ui.Start).Children(func() {
		w.coverView(c, value(show.ImageUrl), 120)
		ui.Column(c).Grow(1).MinWidth(0).Gap(12).Children(func() {
			ui.Text(c, show.Title).FontSize(30).Bold()
			ui.Textf(c, "%d episodes · %s", show.EpisodeCount, strings.ToUpper(string(show.SourceKind))).
				TextColor(c.Theme().TextMuted)
			if show.LastEpisodeAt != nil {
				ui.Text(c, "Latest release "+time.UnixMilli(*show.LastEpisodeAt).Local().Format("Jan 2, 2006")).
					FontSize(12).
					TextColor(c.Theme().TextMuted)
			}
			if show.Youtube.LastCheckedAt != nil {
				ui.Text(c, "YouTube checked "+time.UnixMilli(*show.Youtube.LastCheckedAt).Local().Format("Jan 2, 15:04")).
					FontSize(12).
					TextColor(c.Theme().TextMuted)
			}
			if source := value(show.Youtube.Url); source != "" {
				if ui.Button(c, "Open in YouTube").Label("YouTube source").Clicked() {
					c.OpenURL(source)
				}
			}
			if ui.Button(c, "Manage podcast").Clicked() {
				c.OpenURL(w.managementURL())
			}
		})
	})
	ui.Row(c).Gap(12).AlignItems(ui.Center).Children(func() {
		if !show.HasActiveSubscription {
			ui.Text(c, "Choose a plan to keep this podcast in sync.").
				Grow(1).
				TextColor(c.Theme().TextMuted)
			if ui.PrimaryButton(c, "Choose a plan").Clicked() {
				if target, err := w.client.Config.UpgradeURL(show.TeamId); err == nil {
					c.OpenURL(target)
				}
			}
		} else if cancel := w.jobs[show.Slug]; cancel != nil {
			ui.Text(c, "Syncing with YouTube…").Grow(1)
			if ui.Button(c, "Pause").Clicked() {
				w.pause(show.Slug)
			}
		} else {
			label := "Not synced on this device yet"
			if w.paused[show.Slug] {
				label = "Paused for this session"
			} else if w.state.LastSynced != nil {
				label = "Last synced " + w.state.LastSynced.Local().Format("Jan 2, 15:04")
			}
			ui.Text(c, label).Grow(1).TextColor(c.Theme().TextMuted)
			action := syncNowLabel
			if w.paused[show.Slug] {
				action = "Resume"
			}
			if ui.PrimaryButton(c, action).Clicked() {
				w.sync(show.Slug)
			}
			if !w.paused[show.Slug] && ui.Button(c, "Pause").Clicked() {
				w.pause(show.Slug)
			}
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
	ui.Row(c).Gap(16).AlignItems(ui.Center).Children(func() {
		ui.Text(c, "Episodes").FontSize(18).Bold().Grow(1)
		if hasIssues {
			ui.Checkbox(c, &w.notImported, "Not imported")
			if ui.Button(c, "Retry imports").
				Disabled(!show.HasActiveSubscription || w.jobs[show.Slug] != nil).
				Clicked() {
				w.sync(show.Slug)
			}
		}
	})
	rows := w.rows()
	w.episodeList.Key = func(index int) any { return rows[index].key }
	ui.List(c, &w.episodeList, len(rows), func(index int) {
		row := rows[index]
		ui.Row(c.Key(row.key)).
			FillWidth().
			Padding(10, 0).
			Gap(12).
			AlignItems(ui.Center).
			Border(1, c.Theme().Border.Alpha(.5)).
			BorderWidth(0, 0, 1, 0).
			Children(func() {
				w.coverView(c, row.artwork, 44)
				ui.Column(c).Grow(1).MinWidth(0).Gap(4).Children(func() {
					if row.reason != "" && row.source != "" {
						ui.Link(c, row.title, row.source).
							Label("Open " + row.title).
							Bold().
							SingleLine().
							TextColor(c.Theme().Text)
					} else {
						ui.Text(c, row.title).Bold().SingleLine()
					}
					if row.reason != "" {
						ui.Text(c, row.reason).TextColor(c.Theme().Danger)
					} else {
						ui.Text(c, row.status).FontSize(12).TextColor(c.Theme().TextMuted)
					}
				})
				ui.Text(c, duration(row.duration)).Width(65).TextColor(c.Theme().TextMuted)
				if row.published != nil {
					ui.Text(c, time.UnixMilli(*row.published).Local().Format("Jan 2, 2006")).
						Width(100).
						FontSize(12).
						TextColor(c.Theme().TextMuted)
				}
				if row.source != "" && row.reason == "" &&
					ui.Button(c, "Open").Label("Open "+row.title).Clicked() {
					c.OpenURL(row.source)
				}
			})
	}).Grow(1).MinHeight(0).Label("Episode list")
	if len(rows) == 0 {
		ui.Text(c, "No episodes to show yet.").TextColor(c.Theme().TextMuted)
	}
	var total, complete int
	var sent, bytes int64
	for _, item := range w.items {
		if item.SourceID == show.Slug {
			total++
			if item.Phase == s.Complete() {
				complete++
			}
			sent += item.Uploaded
			bytes += item.UploadTotal
		}
	}
	if total > 0 {
		ratio := float64(complete) / float64(total)
		if bytes > 0 {
			ratio = float64(sent) / float64(bytes)
		}
		ui.Progress(c, ratio).FillWidth().Label("Prepared media upload progress")
		ui.Textf(c, "%d of %d imported · %.1f / %.1f MB sent", complete, total, float64(sent)/1e6, float64(bytes)/1e6).
			FontSize(12).
			TextColor(c.Theme().TextMuted)
	}
}

//nolint:cyclop // Render the import form and its mutually exclusive preparation states together.
func (w *Workspace) importForm(c *ui.Context) {
	ui.Text(c, "Import from YouTube").FontSize(30).Bold()
	ui.Text(c, "Create a podcast from a playlist or channel.").TextColor(c.Theme().TextMuted)
	ui.TextInput(c, &w.importSource).
		Label("YouTube URL").
		Placeholder("Paste a YouTube URL").
		FillWidth().
		Disabled(w.creating)
	ui.Row(c).Gap(20).Children(func() {
		ui.Radio(c, &w.importKind, p.ShowSourceKindAudio, "Audio").Disabled(w.creating)
		ui.Radio(c, &w.importKind, p.ShowSourceKindVideo, "Video").Disabled(w.creating)
	})
	ui.Text(c, "Import into team").Bold()
	ui.Scroll(c).MaxHeight(200).Gap(4).Children(func() {
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
		ui.Progress(c, -1).FillWidth()
		ui.Text(c, heading).Bold()
		if scan := w.preparation.Scan; scan != nil {
			ui.Textf(c, "%d videos · %s estimated", scan.Videos, duration(&scan.EstimatedSeconds))
			if scan.UnknownDurations > 0 {
				ui.Textf(c, "%d videos have an unknown duration", scan.UnknownDurations).
					TextColor(c.Theme().TextMuted)
			}
		}
		if remaining := w.preparation.VideoRemainingSeconds; remaining != nil {
			ui.Textf(c, "%s of video storage available", duration(remaining)).
				TextColor(c.Theme().TextMuted)
		}
	}
	ui.Row(c).Gap(12).Children(func() {
		if ui.PrimaryButton(c, "Import").
			Disabled(w.creating || strings.TrimSpace(w.importSource) == "" || w.importTeam == "").
			Clicked() {
			w.importShow()
		}
		if ui.Button(c, "Cancel import").Clicked() {
			if w.importCancel != nil {
				w.importCancel()
			}
			w.importOpen, w.creating = false, false
		}
	})
}

func (w *Workspace) settings(c *ui.Context) {
	ui.Row(c).AlignItems(ui.Center).Children(func() {
		ui.Text(c, "Settings").FontSize(30).Bold().Grow(1)
		if ui.Button(c, "Done").Disabled(w.cookieBusy).Clicked() {
			w.settingsOpen = false
		}
	})
	ui.Text(c, "YouTube").Bold()
	ui.Text(c, "If YouTube asks you to sign in, add cookies from a private browser session to co"+
		"ntinue importing.").
		TextColor(c.Theme().TextMuted)
	ui.Text(c, "Cookies stay on this computer and are shared with the Listenbox CLI. "+
		"Keep them private, like a password.").TextColor(c.Theme().TextMuted)
	if ui.Button(c, "How to export YouTube cookies").Clicked() {
		c.OpenURL(s.CookieGuideURL)
	}
	ui.Text(c, "YouTube cookies").Bold()
	label := "Paste a Netscape cookies.txt export, including the header line."
	if w.cookieSaved {
		label = "A session is saved. Pasting a new export replaces it."
	}
	ui.Text(c, label).TextColor(c.Theme().TextMuted)
	ui.TextArea(c.Key(w.cookieEditor), &w.cookieInput).
		Placeholder("Paste the complete Netscape cookies.txt export here").
		Label("YouTube cookies").
		FillWidth().
		Height(160).
		Disabled(w.cookieBusy)
	if w.cookieError != "" {
		ui.Text(c, w.cookieError).TextColor(c.Theme().Danger)
	}
	if w.cookieMessage != "" {
		ui.Text(c, w.cookieMessage).TextColor(c.Theme().TextMuted)
	}
	ui.Row(c).Gap(12).Children(func() {
		if ui.PrimaryButton(c, "Save cookies").Disabled(w.cookieBusy).Clicked() {
			w.saveCookies(false)
		}
		if ui.Button(c, "Remove cookies").
			Disabled(w.cookieBusy || (!w.cookieSaved && w.cookieError == "")).
			Clicked() {
			w.saveCookies(true)
		}
	})
	w.updateSettings(c)
}

func (w *Workspace) quitHUD(c *ui.Context) {
	visible := w.stopping ||
		(!w.quitShown.IsZero() && c.Now().Sub(w.quitShown) < quitNotice+quitFade)
	if !visible {
		return
	}
	opacity := float32(1)
	if !w.stopping {
		elapsed := c.Now().Sub(w.quitShown)
		if elapsed > quitNotice {
			opacity = 1 - float32(elapsed-quitNotice)/float32(quitFade)
		}
		c.After(min(50*time.Millisecond, max(time.Millisecond, quitNotice+quitFade-elapsed)))
	}
	ui.Overlay(c, func() {
		ui.Box(c).Fill().Center().Children(func() {
			ui.Column(c).
				Width(280).
				Padding(24).
				Gap(12).
				Radius(20).
				Background(c.Theme().Surface).
				Border(1, c.Theme().Border).
				Opacity(opacity).
				Children(func() {
					shortcut := "⌘Q"
					if runtime.GOOS != "darwin" {
						shortcut = "Ctrl+Q"
					}
					if w.stopping {
						ui.Text(c, "Finishing up…").FontSize(22).Bold()
						ui.Text(c, "Saving progress and stopping active work.")
					} else {
						ui.Text(c, shortcut).FontSize(56).Bold()
						instruction := fmt.Sprintf("Hold %s or press it again to quit", shortcut)
						if w.quitConfirmed {
							instruction = "Release to quit"
						}
						ui.Text(c, instruction)
					}
				})
		})
	})
}

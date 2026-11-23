package main

import (
	"net/url"
	"strings"

	"github.com/egoist/mygo/ui"
	p "github.com/listenbox/client/crates/sync-engine/publicapi"
)

//nolint:lll // Preserve vector assets verbatim.
var (
	chevronsUpDownIcon = ui.MustParseSVG(
		[]byte(
			`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m7 15 5 5 5-5"/><path d="m7 9 5-5 5 5"/></svg>`,
		),
	)
	checkIcon = ui.MustParseSVG(
		[]byte(
			`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>`,
		),
	)
	logOutIcon = ui.MustParseSVG(
		[]byte(
			`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="m16 17 5-5-5-5"/><path d="M21 12H9"/><path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/></svg>`,
		),
	)
)

const allTeamsLabel = "All teams"

// shownTeam is the team the switcher names: the filter's team, or the only
// team when there is nothing to filter between.
func (w *Workspace) shownTeam() *p.ClientTeam {
	for i := range w.catalog.Teams {
		if w.catalog.Teams[i].Id == w.team {
			return &w.catalog.Teams[i]
		}
	}
	if w.team == "" && len(w.catalog.Teams) == 1 {
		return &w.catalog.Teams[0]
	}
	return nil
}

// teamSwitcher is the Dashboard's team picker on MyGo's MenuBase, which owns
// the menu's keys, focus and accessibility: the team and its plan on the
// trigger; teams, team creation, the signed-in account and Log out in the
// menu. The desktop library also spans every team, so "All teams" leads the
// list whenever there is more than one.
func (w *Workspace) teamSwitcher(c *ui.Context) {
	d := tokens(c)
	shown := w.shownTeam()
	name := allTeamsLabel
	if shown != nil {
		name = shown.Name
	}
	m := ui.MenuBase(c.Key("team-switcher"), &w.teamPicker)
	face := d.control
	if m.Trigger.Hovered() || m.Open() {
		face = d.controlHover
	}
	control(c, m.Trigger).
		FillWidth().
		Gap(d.sm).
		Background(face).
		Border(1, d.edge).
		Label("Switch team").
		Children(func() {
			ui.Row(c).Grow(1).MinWidth(0).Gap(d.sm).AlignItems(ui.Center).Children(func() {
				typeButton.set(c, ui.Text(c, name)).MinWidth(0).Shrink(1).SingleLine()
				if shown != nil && shown.PlanFamily != nil {
					planBadge(c, *shown.PlanFamily)
				}
			})
			ui.Icon(c, chevronsUpDownIcon).Size(iconSmall, iconSmall).Shrink(0).TextColor(d.fgSoft)
		})
	m.Popup(func(panel ui.Element) { w.teamMenu(c, m, panel, shown) })
}

// teamMenu builds the switcher's open menu around the shown team.
func (w *Workspace) teamMenu(c *ui.Context, m ui.MenuParts, panel ui.Element, shown *p.ClientTeam) {
	d := tokens(c)
	overlay(panel.Margin(d.sm, 0, 0, 0).
		Width(menuWidth).
		Padding(d.sm).
		Gap(d.xs).
		Radius(radiusTile).
		Background(d.sheet).
		Border(1, d.line))
	menuHeading(c, "Teams")
	w.teamItems(c, m, shown)
	createTeam := w.catalog.ImportTeam
	if shown != nil {
		createTeam = shown.Id
	}
	if createTeam != "" {
		menuSeparator(c)
		if menuItem(c, m.Item(), ui.Color{}, func() {
			ui.Icon(c, plusIcon).Size(iconMedium, iconMedium).TextColor(d.fgSoft)
			ui.Text(c, "Create team")
		}).Clicked() {
			c.OpenURL(strings.TrimRight(w.client.Config.DashboardOrigin, "/") +
				"/" + url.PathEscape(createTeam) + "/teams/new")
		}
	}
	menuSeparator(c)
	if w.catalog.Email != "" {
		ui.Column(c).Padding(d.sm).Gap(d.xs).Children(func() {
			ui.Text(c, "Signed in").TextColor(d.fgSoft)
			typeLabel.set(c, ui.Text(c, w.catalog.Email)).SingleLine()
		})
	}
	if menuItem(c, m.Item(), d.dangerText, func() {
		ui.Icon(c, logOutIcon).Size(iconMedium, iconMedium)
		ui.Text(c, "Log out")
	}).Clicked() {
		w.stop(true)
	}
}

// teamItems lists the teams as radio items, led by "All teams" when there
// are several, each with its plan and a check on the shown one.
func (w *Workspace) teamItems(c *ui.Context, m ui.MenuParts, shown *p.ClientTeam) {
	teams := w.catalog.Teams
	lead := 0
	if len(teams) > 1 {
		lead = 1
	}
	count := len(teams) + lead
	w.teamList.Key = func(index int) any {
		if index < lead {
			return allTeamsLabel
		}
		return teams[index-lead].Id
	}
	ui.List(c, &w.teamList, count, func(index int) {
		team := p.ClientTeam{Name: allTeamsLabel}
		if index >= lead {
			team = teams[index-lead]
		}
		selected := (shown == nil && team.Id == "") || (shown != nil && shown.Id == team.Id)
		if menuItem(c, m.RadioItem(selected), ui.Color{}, func() {
			typeLabel.set(c, ui.Text(c, team.Name)).Grow(1).MinWidth(0).SingleLine()
			if team.PlanFamily != nil {
				planBadge(c, *team.PlanFamily)
			}
			ui.Box(c).Size(iconMedium, iconMedium).Shrink(0).Children(func() {
				if selected {
					ui.Icon(c, checkIcon).Size(iconMedium, iconMedium)
				}
			})
		}).Clicked() {
			w.team, w.selected = team.Id, ""
			if shows := w.visibleShows(); len(shows) > 0 {
				w.selectShow(shows[0].Slug)
			}
		}
	}).FillWidth().Height(min(menuListHeight, float32(count*menuItemHeight))).Label("Team list")
}

// menuItem styles one of MenuBase's items: highlighted under the pointer or
// the keys, in a tint where it has one, as Log out's danger.
func menuItem(c *ui.Context, item ui.Element, tint ui.Color, content func()) ui.Element {
	d := tokens(c)
	fill := d.controlHover
	if tint != (ui.Color{}) {
		item.TextColor(tint)
		fill = tint.Alpha(hoverTint)
	}
	item.FillWidth().
		Height(menuItemHeight).
		Padding(0, d.sm).
		Gap(d.sm).
		Radius(radiusItem).
		AlignItems(ui.Center)
	if item.Highlighted() {
		item.Background(fill)
	}
	return item.Children(content)
}

func menuHeading(c *ui.Context, label string) {
	d := tokens(c)
	ui.Text(c, label).TextColor(d.fgSoft).Padding(d.xs, d.sm)
}

func menuSeparator(c *ui.Context) {
	d := tokens(c)
	ui.Box(c).FillWidth().Height(1).Background(d.line).Margin(d.xs, 0)
}

// planBadge is the plan pill, a badge in the root's tag type: Free quiet as
// the root's New badge, paid plans solid in the text colour, Video 4K's resolution
// set apart.
func planBadge(c *ui.Context, family p.TeamPlanFamily) {
	d := tokens(c)
	background, text := d.badge, d.badgeText
	if family == p.TeamPlanFamilyFree {
		background, text = d.badgeSoft, d.badgeSoftText
	}
	ui.Row(c).
		Height(badgeHeight).
		Shrink(0).
		Padding(0, d.sm).
		Gap(d.xs).
		Radius(radiusPill).
		AlignItems(ui.Center).
		Background(background).
		Children(func() {
			label := planFamilyLabel(family)
			if family == p.TeamPlanFamilyVideo4k {
				label = "Video"
			}
			typeBadge.set(c, ui.Text(c, label)).TextColor(text).SingleLine()
			if family == p.TeamPlanFamilyVideo4k {
				typeBadgeDetail.set(c, ui.Text(c, "4K")).
					TextColor(text).
					Padding(0, d.xs).
					Radius(radiusPill).
					Background(text.Alpha(badgeDetailFill))
			}
		})
}

func planFamilyLabel(family p.TeamPlanFamily) string {
	switch family {
	case p.TeamPlanFamilyAudio:
		return "Audio"
	case p.TeamPlanFamilyVideoHd:
		return "Video HD"
	case p.TeamPlanFamilyVideo4k:
		return "Video 4K"
	case p.TeamPlanFamilyFree:
		return "Free"
	}
	return "Free"
}

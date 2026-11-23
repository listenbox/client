package main

import (
	"runtime"

	"github.com/egoist/mygo/ui"
	s "github.com/listenbox/client/crates/sync-engine"
)

// updatesSupported reports whether this platform ships the in-app updater.
func updatesSupported() bool {
	return runtime.GOOS == "darwin" || runtime.GOOS == "windows"
}

func (w *Workspace) updateSettings(c *ui.Context) {
	if !updatesSupported() {
		return
	}
	typeTitle.set(c, ui.Text(c, "Updates"))
	ui.Textf(c, "%s %s", applicationName, s.BuildVersion()).TextColor(c.Theme().TextMuted)
	available := w.updater != nil
	if !available {
		ui.Text(c, "Updates are unavailable in this development build.").TextColor(c.Theme().TextMuted)
	}
	automatic := available && w.updater.automatic()
	if ui.Checkbox(c, &automatic, "Automatically check for updates").
		Disabled(!available || w.stopping).Changed() {
		w.updater.setAutomatic(automatic)
	}
	w.updateDownloadSettings(c, available)
	if button(c, "Check for Updates…").
		Disabled(!available || w.stopping || !w.updater.canCheck()).Clicked() {
		w.updater.check()
	}
}

func (w *Workspace) updateDownloadSettings(c *ui.Context, available bool) {
	if runtime.GOOS == "windows" {
		ui.Text(c, "Automatic-check changes take effect the next time you open Listenbox.").
			TextColor(c.Theme().TextMuted)
	} else {
		downloads := available && w.updater.automaticDownloads()
		if ui.Checkbox(c, &downloads, "Automatically download and install updates").
			Disabled(!available || w.stopping).Changed() {
			w.updater.setAutomaticDownloads(downloads)
		}
	}
}

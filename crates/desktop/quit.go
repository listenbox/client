package main

import (
	"log/slog"
	"runtime"

	"github.com/egoist/mygo"
)

// confirmQuit warns that syncing stops with the app; closing the window keeps
// it running from the tray instead.
func confirmQuit(parent *mygo.Window) bool {
	tray := "system tray"
	if runtime.GOOS == "darwin" {
		tray = "menu bar"
	}
	result, err := mygo.Dialog.Message(mygo.MessageOptions{
		Parent:  parent,
		Type:    mygo.MessageWarning,
		Message: "Quit Listenbox?",
		Detail: "YouTube sync only runs while Listenbox is open, so quitting pauses it. " +
			"To keep syncing, close the window instead: Listenbox stays in the " + tray + ".",
		Buttons:       []string{"Quit", "Cancel"},
		DefaultButton: 0,
		CancelButton:  1,
	})
	if err != nil {
		slog.Error("quit confirmation failed", "error", err)
		return false
	}
	return result.Button == 0
}

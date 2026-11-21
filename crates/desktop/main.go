package main

import (
	"bytes"
	_ "embed"
	"errors"
	"flag"
	"fmt"
	"image/color"
	"image/png"
	"log/slog"
	"os"
	"os/signal"
	"runtime/debug"
	"syscall"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
	s "github.com/listenbox/client/crates/sync-engine"
)

//go:embed assets/tray.svg
var traySVG []byte

const (
	initialWindowWidth  = 1080
	initialWindowHeight = 760
	minimumWindowWidth  = 840
	minimumWindowHeight = 600
	trayIconSize        = 32
)

const (
	applicationName = "Listenbox"
	syncNowLabel    = "Sync now"
)

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func run() error {
	flags := flag.NewFlagSet("listenbox-desktop", flag.ContinueOnError)
	configPath := flags.String("config", "", "Client YAML configuration")
	version := flags.Bool("version", false, "Show the version")
	diagnostics := flags.Bool("diagnostics", false, "Print recent desktop diagnostics")
	if err := flags.Parse(os.Args[1:]); err != nil {
		if errors.Is(err, flag.ErrHelp) {
			return nil
		}
		return err
	}
	if flags.NArg() != 0 {
		return errors.New("unexpected desktop arguments")
	}
	if *version {
		fmt.Printf("Listenbox %s (%s)\n", s.BuildVersion(), s.SourceCommit)
		return nil
	}
	// mygo dev cannot pass arguments to the app it relaunches, so the
	// development task selects the local services' config through the
	// environment it does pass through.
	if *configPath == "" {
		*configPath = os.Getenv("LISTENBOX_CLIENT_CONFIG")
	}
	config, err := s.LoadConfig(*configPath)
	if err != nil {
		return err
	}
	if *diagnostics {
		return reportDiagnostics(config.Directory)
	}
	return runDesktop(config)
}

func runDesktop(config s.Config) error {
	closeLog, err := openDiagnostics(config.Directory)
	if err != nil {
		return err
	}
	defer closeLog()
	defer reportDesktopPanic()
	unlock, err := installationLock()
	if err != nil {
		return err
	}
	defer unlock()
	client := s.NewClient(config)
	defer client.Close()
	mygo.App.SetName(applicationName)
	if !mygo.App.RequestSingleInstanceLock() {
		return nil
	}
	session := desktopSession{client: client}
	mygo.App.OnBeforeQuit(func(event *mygo.QuitEvent) {
		if !session.permitQuit {
			event.PreventDefault()
			session.requestQuit()
		}
	})
	mygo.App.OnWindowAllClosed(func() {})
	mygo.App.OnActivate(func(bool) {
		session.show()
	})
	mygo.App.OnSecondInstance(func([]string, string) {
		session.show()
	})
	mygo.App.WhenReady(session.ready)
	signals := make(chan os.Signal, 1)
	signal.Notify(signals, os.Interrupt, syscall.SIGTERM)
	defer signal.Stop(signals)
	finished := make(chan struct{})
	defer close(finished)
	go func() {
		select {
		case <-signals:
			mygo.RunOnMain(session.requestQuit)
		case <-finished:
		}
	}()
	err = mygo.App.Run()
	if session.workspace != nil {
		session.workspace.cancel()
		_ = session.workspace.tasks.Wait()
	}
	if session.updater != nil {
		session.updater.close()
	}
	return err
}

func installMenus(window *mygo.Window, w *Workspace, updater nativeUpdater, quit func()) {
	appItems := []*mygo.MenuItem{
		{Label: "About Listenbox", Role: mygo.RoleAbout},
		{
			Label: "Settings…", Accelerator: "CmdOrCtrl+,",
			Click: func(*mygo.MenuItem, *mygo.Window) { window.Update(w.openSettings) },
		},
		{
			Label: "Check for Updates…", Disabled: updater == nil,
			Click: func(*mygo.MenuItem, *mygo.Window) {
				if updater != nil && updater.canCheck() {
					updater.check()
				}
			},
		},
		{Type: mygo.MenuItemSeparator},
		{Label: "Log out", Click: func(*mygo.MenuItem, *mygo.Window) { window.Update(func() { w.stop(true) }) }},
		{Type: mygo.MenuItemSeparator},
		{Label: "Quit Listenbox", Click: func(*mygo.MenuItem, *mygo.Window) { quit() }},
	}
	mygo.App.SetMenu(mygo.NewMenu([]*mygo.MenuItem{
		{Label: applicationName, Submenu: appItems},
		{Role: mygo.RoleEditMenu},
		{Label: "View", Submenu: []*mygo.MenuItem{
			{
				Label: "Reload", Accelerator: "CmdOrCtrl+R",
				Click: func(*mygo.MenuItem, *mygo.Window) { window.Update(func() { w.reload(w.syncAll) }) },
			},
		}},
		{Label: "Podcast", Submenu: []*mygo.MenuItem{
			{
				Label:       "Import from YouTube…",
				Accelerator: "CmdOrCtrl+N",
				Click: func(*mygo.MenuItem, *mygo.Window) {
					window.Update(
						func() { w.importOpen, w.settingsOpen, w.importTeam = true, false, w.team },
					)
				},
			},
			{
				Label: syncNowLabel,
				Click: func(*mygo.MenuItem, *mygo.Window) {
					window.Update(func() {
						if w.selected != "" {
							w.sync(w.selected)
						}
					})
				},
			},
		}},
		{Role: mygo.RoleWindowMenu},
	}))
}

type desktopSession struct {
	client     *s.Client
	window     *mygo.Window
	workspace  *Workspace
	updater    nativeUpdater
	permitQuit bool
}

func (d *desktopSession) requestQuit() {
	if d.workspace != nil {
		d.workspace.stop(false)
	}
}

func (d *desktopSession) ready() {
	var err error
	d.workspace = newWorkspace(d.client, func(apply func()) { d.window.Update(apply) }, func() {
		d.permitQuit = true
		if d.updater == nil || !d.updater.drained() {
			mygo.App.Quit()
		}
	})
	d.window = mygo.NewWindow(
		mygo.WindowOptions{
			Title:     applicationName,
			Width:     initialWindowWidth,
			Height:    initialWindowHeight,
			MinWidth:  minimumWindowWidth,
			MinHeight: minimumWindowHeight,
			Content:   ui.View(d.workspace.view),
		},
	)
	d.window.OnClose(func(event *mygo.CloseEvent) {
		if !d.permitQuit {
			event.PreventDefault()
			d.window.Hide()
		}
	})
	d.updater, err = startUpdater(
		func() { mygo.RunOnMain(func() { d.window.Show(); d.requestQuit() }) },
		func() { d.window.Update(func() {}) },
	)
	d.workspace.updater = d.updater
	if err != nil {
		d.workspace.showError(err, "")
		slog.Error("updater initialization failed", "error", err)
	}
	installMenus(d.window, d.workspace, d.updater, d.requestQuit)
	d.installTray()
}

func (d *desktopSession) installTray() {
	var icon bytes.Buffer
	_ = png.Encode(
		&icon,
		ui.MustParseSVG(traySVG).Image(trayIconSize, trayIconSize, color.Black),
	)
	_, trayErr := mygo.NewTray(
		mygo.TrayOptions{
			ID:             "app.listenbox.client",
			Icon:           icon.Bytes(),
			IconIsTemplate: true,
			ToolTip:        applicationName,
			Menu: mygo.NewMenu([]*mygo.MenuItem{
				{
					Label: "Show Listenbox",
					Click: func(*mygo.MenuItem, *mygo.Window) { d.window.Show(); d.window.Focus() },
				},
				{
					Label: syncNowLabel,
					Click: func(*mygo.MenuItem, *mygo.Window) {
						d.window.Update(func() { d.workspace.syncAll() })
					},
				},
				{
					Label: "Quit Listenbox",
					Click: func(*mygo.MenuItem, *mygo.Window) { d.requestQuit() },
				},
			}),
		},
	)
	if trayErr != nil {
		slog.Warn("desktop tray unavailable", "error", trayErr)
	}
}

func reportDesktopPanic() {
	if panicValue := recover(); panicValue != nil {
		slog.Error(
			"desktop panic",
			"error",
			fmt.Sprint(panicValue),
			"stack",
			string(debug.Stack()),
		)
		panic(panicValue)
	}
}

func (d *desktopSession) show() {
	if d.window != nil {
		d.window.Show()
		d.window.Focus()
	}
}

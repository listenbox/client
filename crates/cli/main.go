package main

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"os/signal"
	"strings"
	"syscall"

	engine "github.com/listenbox/client/crates/sync-engine"
	p "github.com/listenbox/client/crates/sync-engine/publicapi"
	"github.com/spf13/cobra"
)

type configPath struct {
	value string
	set   bool
}

const (
	interruptedExitCode = 130
)

const (
	listCommand   = "list"
	removeCommand = "remove"
	inviteCommand = "invite"
	showFlag      = "show"
	slugFlag      = "slug"
)

const (
	titleFlag = "title"
)

func (p *configPath) String() string { return p.value }
func (p *configPath) Type() string   { return "path" }
func (p *configPath) Set(value string) error {
	if p.set {
		return errors.New("--config cannot be used multiple times")
	}
	p.set = true
	if value == "" {
		return errors.New("--config path is empty")
	}
	p.value = value
	return nil
}

type application struct {
	configPath configPath
	config     engine.Config
	api        *engine.API
	client     *engine.Client
}

func main() { os.Exit(run()) }
func run() int {
	level := slog.LevelWarn
	if strings.Contains(os.Getenv("LISTENBOX_LOG"), "info") {
		level = slog.LevelInfo
	}
	slog.SetDefault(slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: level})))
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	app := &application{}
	root := app.commands()
	root.SetContext(ctx)
	err := root.Execute()
	if app.client != nil {
		if closeErr := app.client.Close(); err == nil {
			err = closeErr
		}
	}
	if ctx.Err() != nil {
		fmt.Fprintln(os.Stderr, "Stopped. Progress saved; run `listenbox youtube sync` to resume.")
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
		}
		return interruptedExitCode
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	return 0
}

func (app *application) commands() *cobra.Command {
	root := &cobra.Command{
		Use:           "listenbox",
		Short:         "Publish and manage podcasts on Listenbox",
		Version:       engine.BuildVersion(),
		SilenceErrors: true,
		SilenceUsage:  true,
	}
	root.SetVersionTemplate("listenbox {{.Version}}\n")
	root.PersistentFlags().Var(&app.configPath, "config", "Path to CLI config file")
	root.PersistentPreRunE = app.prepareCommand
	login := &cobra.Command{
		Use:   "login",
		Short: "Authorize this CLI with Listenbox",
		Args:  cobra.NoArgs,
		RunE:  app.login,
	}
	auth := &cobra.Command{Use: "auth", Short: "Manage shared authorization"}
	auth.AddCommand(
		&cobra.Command{
			Use:  "status",
			Args: cobra.NoArgs,
			RunE: app.authStatus,
		},
		&cobra.Command{
			Use:  "logout",
			Args: cobra.NoArgs,
			RunE: app.logout,
		},
	)
	importCommand := &cobra.Command{
		Use:   "import URL",
		Short: "Import an RSS feed or public YouTube video, playlist or channel",
		Args:  cobra.ExactArgs(1),
		RunE:  app.importSource,
	}
	importCommand.Flags().String(slugFlag, "", "Podcast slug")
	root.AddCommand(
		login,
		auth,
		importCommand,
		app.youtubeCommands(),
		app.showCommands(),
		app.episodeCommands(),
		app.memberCommands(),
	)
	return root
}

func (app *application) youtubeCommands() *cobra.Command {
	youtube := &cobra.Command{
		Use:   "youtube",
		Short: "Sync YouTube podcasts and manage the session shared with desktop",
	}
	cookies := &cobra.Command{Use: "cookies", Short: "Import or remove the shared YouTube session"}
	cookies.AddCommand(
		&cobra.Command{
			Use:  "import NETSCAPE.txt",
			Args: cobra.ExactArgs(1),
			RunE: app.importCookies,
		},
		&cobra.Command{
			Use:  removeCommand,
			Args: cobra.NoArgs,
			RunE: app.removeCookies,
		},
	)
	syncCommand := &cobra.Command{
		Use:   "sync",
		Short: "Sync all eligible YouTube shows across all teams, or a single show",
		Args:  cobra.NoArgs,
		RunE:  app.syncYouTube,
	}
	syncCommand.Flags().String(showFlag, "", "Restrict the run to one podcast slug")
	syncCommand.Flags().
		Bool("watch", false, "Sync immediately, then check hourly until Ctrl+C or SIGTERM")
	youtube.AddCommand(cookies, syncCommand)
	return youtube
}
func clean(value string) string { return strings.Join(strings.Fields(value), " ") }
func flagString(c *cobra.Command, name string) string {
	value, _ := c.Flags().GetString(name)
	return value
}

func optionalFlag(c *cobra.Command, name string) *string {
	if !c.Flags().Changed(name) {
		return nil
	}
	value := strings.TrimSpace(flagString(c, name))
	return &value
}
func showSelector(c *cobra.Command) (string, error) { return engine.Slug(flagString(c, showFlag)) }
func (app *application) shows(ctx context.Context) ([]p.Show, error) {
	var shows []p.Show
	err := app.api.JSON(ctx, "GET", "/s/shows", nil, &shows)
	return shows, err
}

func requireID(value, prefix string) error {
	if !engine.ValidID(value, prefix) {
		return fmt.Errorf("invalid identifier %q", value)
	}
	return nil
}

func mustConfirm(c *cobra.Command) error {
	yes, _ := c.Flags().GetBool("yes")
	if !yes {
		return fmt.Errorf("%s: --yes is required", c.CommandPath())
	}
	return nil
}

func (app *application) login(c *cobra.Command, _ []string) error {
	return app.api.Login(
		c.Context(),
		p.AuthorizationClientCli,
		func(u string) { fmt.Printf("Verification URL: %s\n", u) },
	)
}

func (app *application) authStatus(c *cobra.Command, _ []string) error {
	status, err := app.api.AuthStatus(c.Context())
	if err == nil {
		fmt.Println(status)
	}
	return err
}

func (app *application) logout(*cobra.Command, []string) error { return engine.Logout(app.config) }

func (app *application) importSource(c *cobra.Command, args []string) error {
	slug, _ := c.Flags().GetString(slugFlag)
	if slug != "" {
		if _, err := engine.Slug(slug); err != nil {
			return err
		}
	}
	if !engine.IsYouTubeSource(args[0]) {
		return app.importRSS(c.Context(), args[0], slug)
	}
	team, err := app.api.TeamID(c.Context())
	if err != nil {
		return err
	}
	show, report, err := app.client.Engine.Import(
		c.Context(),
		app.api,
		args[0],
		team,
		slug,
		p.ShowSourceKindVideo,
		func(event engine.ImportEvent) {
			if event.Show != nil && !event.Existing {
				fmt.Fprintf(
					os.Stderr,
					"Created podcast %q. Resume with youtube sync --show %s\n",
					event.Show.Slug,
					event.Show.Slug,
				)
			}
		},
	)
	if err != nil {
		return err
	}
	fmt.Println(show.Slug)
	fmt.Fprintf(os.Stderr, "%d added, %d skipped\n100%%\n", report.Added, report.Skipped)
	location, err := app.config.ShowURL(show.TeamId, show.Id)
	if err == nil {
		fmt.Fprintln(os.Stderr, "Open in Listenbox: "+location)
	}
	return err
}

func (app *application) importCookies(c *cobra.Command, args []string) error {
	if err := engine.NewCookieJar(app.config.Directory).
		ImportFile(c.Context(), args[0]); err != nil {
		return err
	}
	fmt.Println("YouTube cookies updated. Run `listenbox youtube sync` to continue.")
	return nil
}

func (app *application) removeCookies(c *cobra.Command, _ []string) error {
	if err := engine.NewCookieJar(app.config.Directory).
		Remove(c.Context()); err != nil {
		return err
	}
	fmt.Println("YouTube cookies updated. Run `listenbox youtube sync` to continue.")
	return nil
}

func (app *application) syncYouTube(c *cobra.Command, _ []string) error {
	show, _ := c.Flags().GetString(showFlag)
	if show != "" {
		if _, err := engine.Slug(show); err != nil {
			return err
		}
	}
	watch, _ := c.Flags().GetBool("watch")
	done := make(chan struct{})
	defer close(done)
	go func() {
		select {
		case <-c.Context().Done():
			fmt.Fprintln(
				os.Stderr,
				"Stopping… finishing current writes and saving progress.",
			)
		case <-done:
		}
	}()
	return app.client.SyncYouTube(c.Context(), show, watch, func(event engine.SyncEvent) {
		switch {
		case event.Slug != "":
			printShowSync(c.Context(), event)
		case event.Finished:
			app.printSyncSummary(c.Context(), watch, event.Failed)
		default:
			printSyncCatalog(event.Shows)
		}
	})
}

func printShowSync(ctx context.Context, event engine.SyncEvent) {
	for _, item := range event.Items {
		reason := item.Reason
		if reason == "" {
			reason = item.Error
		}
		fmt.Fprintf(
			os.Stderr,
			"\nNot imported · %s\n  %s (--show %s)\n  %s\n  %s\n",
			clean(item.Title),
			clean(item.SourceTitle),
			item.SourceID,
			item.SourceURL,
			clean(reason),
		)
	}
	switch {
	case event.Err != nil:
		if ctx.Err() == nil {
			fmt.Fprintf(
				os.Stderr,
				"Not synced · %s\n  %s\n  %s\n",
				event.Slug,
				event.SourceURL,
				engine.Redact(event.Err.Error()),
			)
		}
	case event.Report.Stopped:
		fmt.Fprintf(
			os.Stderr,
			"Not synced · %s\n  Listenbox stopped YouTube importing for this show. Progress "+
				"is saved.\n",
			event.Slug,
		)
	default:
		suffix := ""
		if event.Report.Reordered {
			suffix = "; feed order updated"
		}
		fmt.Printf(
			"%s: %d added, %d removed, %d unchanged, %d skipped%s\n",
			event.Slug,
			event.Report.Added,
			event.Report.Removed,
			event.Report.Unchanged,
			event.Report.Skipped,
			suffix,
		)
	}
}

func (app *application) printSyncSummary(ctx context.Context, watch bool, failed int) {
	items := app.client.Engine.Downloads.Snapshot()
	imported, notImported := 0, 0
	for _, item := range items {
		if item.Phase == engine.Complete() {
			imported++
		}
		if item.Phase == engine.Skipped() || item.Phase == engine.Failed() {
			notImported++
		}
	}
	fmt.Fprintf(
		os.Stderr,
		"Sync finished · %d/%d episodes imported · %d not imported · %d shows failed\n",
		imported,
		len(items),
		notImported,
		failed,
	)
	if watch && ctx.Err() == nil {
		fmt.Fprintln(
			os.Stderr,
			"Watching for new episodes; the next scan is hourly. Ctrl+C to stop.",
		)
	}
}

func printSyncCatalog(shows []p.Show) {
	teams := make(map[string]bool)
	for _, show := range shows {
		teams[show.TeamId] = true
	}
	fmt.Fprintf(
		os.Stderr,
		"YouTube sync · %d shows across %d teams\n",
		len(shows),
		len(teams),
	)
	if len(shows) == 0 {
		fmt.Fprintln(
			os.Stderr,
			"No eligible YouTube shows. Import a source with `listenbox import <url>`; an act"+
				"ive paid plan is required.",
		)
	}
}

func (app *application) prepareCommand(command *cobra.Command, _ []string) error {
	config, err := engine.LoadConfig(app.configPath.value)
	if err != nil {
		return err
	}
	app.config = config
	if command.CommandPath() == "listenbox auth logout" ||
		strings.HasPrefix(command.CommandPath(), "listenbox youtube cookies") {
		return nil
	}
	app.client = engine.NewClient(config)
	app.api, err = app.client.API(command.Context())
	if err != nil {
		return err
	}
	if command.CommandPath() != "listenbox login" && app.api.Credential == "" {
		return errors.New("not logged in; run listenbox login")
	}
	return nil
}

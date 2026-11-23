//nolint:contextcheck // UI callbacks schedule work in the workspace generation, independently of completed requests.
package main

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"slices"
	"strings"
	"sync/atomic"
	"time"

	"github.com/egoist/mygo/ui"
	s "github.com/listenbox/client/crates/sync-engine"
	p "github.com/listenbox/client/crates/sync-engine/publicapi"
	"golang.org/x/sync/errgroup"
)

// Workspace state belongs to the UI actor. All admitted background operations
// belong to tasks; logout and quit close admission, cancel, then join them.
//
//nolint:containedctx // The UI actor owns and cancels this entire workspace lifetime.
type Workspace struct {
	client                                                    *s.Client
	post                                                      func(func())
	quit                                                      func()
	updater                                                   nativeUpdater
	ctx                                                       context.Context
	cancel                                                    context.CancelFunc
	tasks                                                     *errgroup.Group
	generation                                                uint64
	started, loaded, authorized, loading, stopping, signingIn bool
	refreshPending, catalogLive, episodesLive, checking       bool
	loginCancel                                               context.CancelFunc
	loginURL                                                  string
	catalog                                                   s.Catalog
	team, selected                                            string
	sidebarWidth                                              float32
	libraryList, episodeList, teamList                        ui.ListState
	teamPicker                                                bool
	navigationObserved                                        bool
	filter                                                    string
	notImported                                               bool
	episodes                                                  []p.EpisodeListItem
	state                                                     s.SyncState
	episodeCancel                                             context.CancelFunc
	episodeRequest                                            uint64
	jobs                                                      map[string]context.CancelFunc
	paused                                                    map[string]bool
	reports                                                   map[string]s.Report
	items                                                     []s.Download
	artwork                                                   map[string]*ui.Bitmap
	artworkPending                                            map[string]bool
	artworkOrder                                              []string
	artworkGeneration                                         uint64
	errorText, upgradeURL                                     string
	signInRequired                                            bool
	importOpen, creating                                      bool
	importSource, importTeam                                  string
	importKind                                                p.ShowSourceKind
	preparation                                               s.ImportEvent
	importCancel                                              context.CancelFunc
	settingsOpen, cookieBusy, cookieSaved                     bool
	cookieInput, cookieError, cookieMessage                   string
	cookieEditor                                              uint64
	openURLs                                                  []string
	terminated                                                bool
}

const (
	initialSidebarWidth = 300
	artworkCacheEntries = 128
	secondsPerHour      = 3600
	secondsPerMinute    = 60
)

func newWorkspace(client *s.Client, post func(func()), quit func()) *Workspace {
	w := &Workspace{
		client: client, post: post, quit: quit, sidebarWidth: initialSidebarWidth,
		jobs: make(map[string]context.CancelFunc), paused: make(map[string]bool),
		reports: make(map[string]s.Report),
		artwork: make(map[string]*ui.Bitmap), artworkPending: make(map[string]bool),
		importKind: p.ShowSourceKindAudio,
	}
	w.newLifetime()
	return w
}

func (w *Workspace) newLifetime() {
	w.ctx, w.cancel = context.WithCancel(context.Background())
	w.tasks = new(errgroup.Group)
	w.generation++
}

func (w *Workspace) deliver(generation uint64, apply func()) {
	w.post(func() {
		if generation == w.generation && !w.stopping {
			apply()
		}
	})
}

func (w *Workspace) launch(work func(context.Context) func()) {
	if w.stopping {
		return
	}
	ctx, generation := w.ctx, w.generation
	w.tasks.Go(func() error {
		apply := work(ctx)
		if apply != nil {
			w.deliver(generation, apply)
		}
		return nil
	})
}

func (w *Workspace) start() {
	if w.started {
		return
	}
	w.started = true
	if !w.loaded {
		w.launch(func(_ context.Context) func() {
			credentials := w.client.HasCredentials()
			return func() {
				w.authorized, w.loaded = credentials, true
				if credentials {
					w.reload(w.syncDue)
				}
			}
		})
	}
	// Coalesce frames without dropping the newest snapshot. A snapshot is read
	// after the queued flag clears, so the following change always gets a frame.
	ctx, generation := w.ctx, w.generation
	var pending atomic.Bool
	w.tasks.Go(func() error {
		for {
			changed := w.client.Engine.Downloads.Changes()
			select {
			case <-ctx.Done():
				return nil
			case <-changed:
				if pending.CompareAndSwap(false, true) {
					w.post(func() {
						pending.Store(false)
						if generation == w.generation && !w.stopping {
							w.items = w.client.Engine.Downloads.Snapshot()
						}
					})
				}
			}
		}
	})
	w.tasks.Go(func() error {
		for w.client.Engine.NextScan(ctx) {
			w.deliver(generation, w.syncAll)
		}
		return nil
	})
}

func (w *Workspace) showError(err error, team string) {
	if err == nil || errors.Is(err, context.Canceled) {
		return
	}
	if errors.Is(err, s.ErrAuthentication) {
		w.authorized, w.signingIn = false, false
		w.errorText = "Your session has expired. Sign in to continue."
		return
	}
	w.errorText, w.upgradeURL = err.Error(), ""
	w.signInRequired = errors.Is(err, s.ErrSignInRequired)
	var response *s.HTTPError
	if errors.As(err, &response) && response.Status == http.StatusPaymentRequired && team != "" {
		w.upgradeURL, _ = w.client.Config.UpgradeURL(team)
		if w.importKind == p.ShowSourceKindVideo &&
			(w.importOpen || strings.Contains(strings.ToLower(response.Message), "video")) {
			w.upgradeURL += "?family=video_hd"
		}
	}
}

// reload renders the catalog cached by the last session at once, then
// replaces it with Listenbox's and runs sync, when given, against that.
func (w *Workspace) reload(sync func()) {
	w.clearArtwork()
	if !w.catalogLive {
		w.launch(func(ctx context.Context) func() {
			catalog, found, err := w.client.CachedCatalog(ctx)
			return func() {
				if w.catalogLive {
					return
				}
				if err != nil {
					w.showError(err, "")
					return
				}
				if found {
					w.applyCatalog(catalog)
				}
			}
		})
	}
	w.refreshCatalog(sync)
}

func (w *Workspace) clearArtwork() {
	w.artworkGeneration++
	w.artwork, w.artworkPending = make(map[string]*ui.Bitmap), make(map[string]bool)
	w.artworkOrder = nil
}

func (w *Workspace) refreshCatalog(sync func()) {
	if !w.authorized || w.stopping {
		return
	}
	if w.loading {
		w.refreshPending = true
		return
	}
	w.loading = true
	w.launch(func(ctx context.Context) func() {
		catalog, err := w.client.Catalog(ctx)
		return func() {
			w.loading = false
			if err != nil {
				w.showError(err, "")
				return
			}
			w.catalogLive = true
			w.applyCatalog(catalog)
			if sync != nil {
				sync()
			}
			if w.refreshPending {
				w.refreshPending = false
				w.refreshCatalog(nil)
			}
		}
	})
}

func (w *Workspace) applyCatalog(catalog s.Catalog) {
	w.catalog = catalog
	w.retainArtwork()
	if w.team != "" &&
		!slices.ContainsFunc(
			catalog.Teams,
			func(t p.ClientTeam) bool { return t.Id == w.team },
		) {
		w.team = ""
	}
	if w.importTeam == "" {
		w.importTeam = catalog.ImportTeam
	}
	if !slices.ContainsFunc(
		w.visibleShows(),
		func(show p.Show) bool { return show.Slug == w.selected },
	) {
		w.selected = ""
		if shows := w.visibleShows(); len(shows) > 0 {
			w.selectShow(shows[0].Slug)
		}
	} else {
		w.loadEpisodes()
	}
}

func (w *Workspace) login() {
	if w.signingIn || w.stopping {
		return
	}
	w.signingIn, w.errorText, w.loginURL = true, "", ""
	ctx, cancel := context.WithCancel(w.ctx)
	w.loginCancel = cancel
	generation := w.generation
	w.launch(func(context.Context) func() {
		err := w.client.Login(ctx, func(target string) {
			w.deliver(generation, func() {
				if ctx.Err() == nil {
					w.loginURL = target
					w.openURLs = append(w.openURLs, target)
				}
			})
		})
		return func() {
			if ctx.Err() != nil {
				return
			}
			w.signingIn = false
			if err != nil {
				w.showError(err, "")
				return
			}
			w.authorized, w.errorText = true, ""
			w.reload(w.syncDue)
		}
	})
}

func (w *Workspace) cancelLogin() {
	if w.loginCancel != nil {
		w.loginCancel()
	}
	w.signingIn, w.loginURL = false, ""
}

func (w *Workspace) stop(logout bool) {
	if w.stopping {
		return
	}
	w.stopping = true
	w.cancel()
	tasks := w.tasks
	// The joiner is deliberately outside the group it joins. Writes already
	// admitted by the engine remain owned until their durable checkpoints end.
	go func() {
		_ = tasks.Wait()
		var err error
		if logout {
			err = w.client.Logout()
		} else {
			err = w.client.Close()
		}
		w.post(func() {
			if !logout {
				w.terminated = true
				if w.quit != nil {
					w.quit()
				}
				return
			}
			w.newLifetime()
			w.started, w.stopping, w.authorized, w.signingIn, w.loading = false, false, false, false, false
			w.catalogLive, w.episodesLive, w.checking = false, false, false
			w.loaded = true
			w.catalog, w.state = s.Catalog{}, s.SyncState{}
			w.team, w.selected, w.errorText, w.upgradeURL, w.cookieInput = "", "", "", "", ""
			w.episodes, w.items = nil, nil
			w.jobs, w.paused = make(map[string]context.CancelFunc), make(map[string]bool)
			w.reports = make(map[string]s.Report)
			w.settingsOpen, w.importOpen, w.creating, w.cookieBusy = false, false, false, false
			w.clearArtwork()
			w.showError(err, "")
		})
	}()
}

func (w *Workspace) visibleShows() []p.Show {
	var shows []p.Show
	for _, show := range w.catalog.Shows {
		if (w.team == "" || show.TeamId == w.team) &&
			strings.Contains(strings.ToLower(show.Title), strings.ToLower(w.filter)) {
			shows = append(shows, show)
		}
	}
	return shows
}

func (w *Workspace) show() *p.Show {
	for i := range w.catalog.Shows {
		if w.catalog.Shows[i].Slug == w.selected {
			return &w.catalog.Shows[i]
		}
	}
	return nil
}

func (w *Workspace) selectShow(slug string) {
	if w.selected == slug {
		w.importOpen, w.settingsOpen = false, false
		return
	}
	w.selected, w.settingsOpen, w.importOpen, w.notImported = slug, false, false, false
	w.episodes, w.state, w.episodesLive = nil, s.SyncState{}, false
	w.episodeList = ui.ListState{}
	w.loadEpisodes()
}

func (w *Workspace) navigate(delta int) {
	shows := w.visibleShows()
	index := slices.IndexFunc(shows, func(show p.Show) bool { return show.Slug == w.selected })
	if len(shows) == 0 {
		return
	}
	if index < 0 {
		if delta < 0 {
			index = len(shows) - 1
		} else {
			index = 0
		}
	} else {
		index = max(0, min(len(shows)-1, index+delta))
	}
	w.selectShow(shows[index].Slug)
	w.libraryList.ScrollIntoView(index)
}

func (w *Workspace) loadEpisodes() {
	if w.selected == "" || w.stopping {
		return
	}
	if w.episodeCancel != nil {
		w.episodeCancel()
	}
	ctx, cancel := context.WithCancel(w.ctx)
	w.episodeCancel = cancel
	w.episodeRequest++
	request, slug := w.episodeRequest, w.selected
	current := func() bool { return request == w.episodeRequest && ctx.Err() == nil }
	w.checking = true
	// This computer's journal renders the podcast at once; Listenbox's list
	// replaces the cached one when it arrives.
	w.launch(func(context.Context) func() {
		var episodes []p.EpisodeListItem
		var found bool
		state, err := w.client.SyncState(ctx, slug)
		if err == nil {
			episodes, found, err = w.client.CachedEpisodes(ctx, slug)
		}
		return func() {
			if !current() {
				return
			}
			if err != nil {
				w.showError(err, "")
				return
			}
			w.state = state
			if found && !w.episodesLive {
				w.episodes = episodes
				w.retainArtwork()
			}
		}
	})
	w.launch(func(context.Context) func() {
		episodes, err := w.client.Episodes(ctx, slug)
		return func() {
			if !current() {
				return
			}
			w.checking = false
			if err != nil {
				w.showError(err, "")
				return
			}
			w.episodes, w.episodesLive = episodes, true
			w.retainArtwork()
		}
	})
}

// syncDue syncs, at sign-in or launch, only the podcasts that are due; the
// hourly scan and Sync now still cover every podcast.
func (w *Workspace) syncDue() {
	if !w.authorized || w.stopping {
		return
	}
	var shows []p.Show
	for _, show := range w.catalog.Shows {
		if show.HasActiveSubscription && !w.paused[show.Slug] {
			shows = append(shows, show)
		}
	}
	w.launch(func(ctx context.Context) func() {
		due, err := w.client.SyncDue(ctx, shows)
		return func() {
			if err != nil {
				w.showError(err, "")
				return
			}
			for _, slug := range due {
				if !w.paused[slug] {
					w.sync(slug)
				}
			}
		}
	})
}

// lastSynced is when any client last finished syncing the podcast between
// YouTube and Listenbox, or this computer did, whichever is later.
func (w *Workspace) lastSynced(show p.Show) *time.Time {
	latest := w.state.LastSynced
	if show.Youtube.LastCheckedAt != nil {
		checked := time.UnixMilli(*show.Youtube.LastCheckedAt)
		if latest == nil || checked.After(*latest) {
			latest = &checked
		}
	}
	return latest
}

func (w *Workspace) syncAll() {
	if !w.authorized || w.stopping {
		return
	}
	for _, show := range w.catalog.Shows {
		if show.HasActiveSubscription && !w.paused[show.Slug] {
			w.sync(show.Slug)
		}
	}
}

func (w *Workspace) sync(slug string) {
	if _, active := w.jobs[slug]; active || w.stopping {
		return
	}
	ctx, cancel := context.WithCancel(w.ctx)
	w.jobs[slug] = cancel
	w.paused[slug] = false
	team := ""
	for _, show := range w.catalog.Shows {
		if show.Slug == slug {
			team = show.TeamId
		}
	}
	w.launch(func(context.Context) func() {
		report, err := w.client.Sync(ctx, slug)
		return func() {
			delete(w.jobs, slug)
			w.items = w.client.Engine.Downloads.Snapshot()
			w.showError(err, team)
			if err == nil {
				w.reports[slug] = report
			}
			w.refreshCatalog(nil)
		}
	})
}

func (w *Workspace) pause(slug string) {
	w.paused[slug] = true
	if cancel := w.jobs[slug]; cancel != nil {
		cancel()
	}
}

func (w *Workspace) importShow() {
	if w.creating || w.stopping {
		return
	}
	w.creating, w.errorText, w.upgradeURL = true, "", ""
	w.preparation = s.ImportEvent{}
	ctx, cancel := context.WithCancel(w.ctx)
	w.importCancel = cancel
	source, team, kind, generation := w.importSource, w.importTeam, w.importKind, w.generation
	var slug string
	w.launch(func(context.Context) func() {
		_, report, err := w.client.Import(ctx, source, team, kind, func(event s.ImportEvent) {
			if event.Show != nil {
				slug = event.Show.Slug
			}
			w.deliver(generation, func() {
				w.preparation = event
				if event.Show != nil {
					show := *event.Show
					index := slices.IndexFunc(
						w.catalog.Shows,
						func(item p.Show) bool { return item.Slug == show.Slug },
					)
					if index < 0 {
						w.catalog.Shows = append(w.catalog.Shows, show)
					} else {
						w.catalog.Shows[index] = show
					}
					w.team = show.TeamId
					w.selectShow(show.Slug)
					w.creating, w.importOpen, w.importSource = false, false, ""
					w.jobs[show.Slug] = cancel
				}
			})
		})
		return func() {
			w.creating = false
			if slug != "" {
				delete(w.jobs, slug)
			}
			w.items = w.client.Engine.Downloads.Snapshot()
			w.showError(err, team)
			if err == nil && slug != "" {
				w.reports[slug] = report
			}
			w.refreshCatalog(nil)
		}
	})
}

func (w *Workspace) openSettings() {
	if w.stopping {
		return
	}
	w.settingsOpen, w.importOpen = true, false
	w.cookieError, w.cookieMessage = "", ""
	if w.cookieBusy {
		return
	}
	w.cookieBusy = true
	w.launch(func(context.Context) func() {
		enabled, err := w.client.CookieJar().Enabled()
		return func() {
			w.cookieBusy = false
			w.cookieSaved = enabled
			if err != nil {
				w.cookieError = err.Error()
			}
		}
	})
}

func (w *Workspace) saveCookies(remove bool) {
	if w.cookieBusy || w.stopping {
		return
	}
	w.cookieBusy, w.cookieError, w.cookieMessage = true, "", ""
	text := w.cookieInput
	w.launch(func(ctx context.Context) func() {
		var err error
		if remove {
			err = w.client.CookieJar().Remove(ctx)
		} else {
			err = w.client.CookieJar().Import(ctx, text)
		}
		return func() {
			w.cookieBusy = false
			if err != nil {
				w.cookieError = err.Error()
				return
			}
			w.cookieSaved, w.cookieInput = !remove, ""
			w.cookieEditor++ // Discard the text editor's undo history as well.
			w.cookieMessage = "Cookies saved. Return to your podcast and choose Sync now."
			if remove {
				w.cookieMessage = "Cookies removed. Future requests will use anonymous access."
			}
		}
	})
}

func (w *Workspace) cover(target string) *ui.Bitmap {
	if target == "" {
		return nil
	}
	if bitmap, exists := w.artwork[target]; exists {
		if index := slices.Index(w.artworkOrder, target); index >= 0 {
			w.artworkOrder = slices.Delete(w.artworkOrder, index, index+1)
			w.artworkOrder = append(w.artworkOrder, target)
		}
		return bitmap
	}
	if w.artworkPending[target] || w.stopping {
		return nil
	}
	w.artworkPending[target] = true
	generation := w.artworkGeneration
	w.launch(func(ctx context.Context) func() {
		var bitmap *ui.Bitmap
		raw, err := w.client.Artwork(ctx, target)
		if err == nil {
			bitmap, _ = ui.DecodeBitmap(raw)
		}
		return func() {
			if generation != w.artworkGeneration {
				return
			}
			if !w.artworkPending[target] {
				return
			}
			delete(w.artworkPending, target)
			w.artwork[target] = bitmap
			w.artworkOrder = append(w.artworkOrder, target)
			if len(w.artworkOrder) > artworkCacheEntries {
				delete(w.artwork, w.artworkOrder[0])
				w.artworkOrder = w.artworkOrder[1:]
			}
		}
	})
	return nil
}

func (w *Workspace) retainArtwork() {
	wanted := make(map[string]bool, len(w.catalog.Shows)+len(w.episodes))
	for _, show := range w.catalog.Shows {
		wanted[value(show.ImageUrl)] = true
	}
	for _, episode := range w.episodes {
		wanted[value(episode.ThumbnailUrl)] = true
	}
	w.artworkOrder = slices.DeleteFunc(w.artworkOrder, func(target string) bool {
		if wanted[target] {
			return false
		}
		delete(w.artwork, target)
		return true
	})
	for target := range w.artworkPending {
		if !wanted[target] {
			delete(w.artworkPending, target)
		}
	}
}

func value(v *string) string {
	if v == nil {
		return ""
	}
	return *v
}

func (w *Workspace) managementURL() string {
	show := w.show()
	if show == nil {
		return w.client.Config.DashboardOrigin
	}
	return strings.TrimRight(
		w.client.Config.DashboardOrigin,
		"/",
	) + "/" + url.PathEscape(
		show.TeamId,
	) + "/shows/" + url.PathEscape(
		show.Id,
	)
}

type episodeRow struct {
	key, title, source, status, reason string
	duration                           *int64
	published                          *int64
}

func (w *Workspace) rows() []episodeRow {
	rows := make([]episodeRow, 0, len(w.episodes)+len(w.state.Items))
	bySource := make(map[string]s.Download)
	for _, item := range w.state.Items {
		bySource[item.SourceURL] = item
	}
	for _, item := range w.items {
		if item.SourceID == w.selected {
			bySource[item.SourceURL] = item
		}
	}
	seen := make(map[string]bool)
	for _, episode := range w.episodes {
		source := value(episode.YoutubeUrl)
		if source == "" {
			source = value(episode.SourceUrl)
		}
		seen[source] = true
		if w.notImported {
			continue
		}
		rows = append(
			rows,
			episodeRow{
				key:       episode.Id,
				title:     episode.Title,
				source:    source,
				status:    string(episode.Status),
				duration:  episode.DurationSeconds,
				published: episode.RepublishedAt,
			},
		)
	}
	return append(rows, pendingEpisodeRows(bySource, seen)...)
}

func pendingEpisodeRows(bySource map[string]s.Download, seen map[string]bool) []episodeRow {
	var pending []s.Download
	for source, item := range bySource {
		if !seen[source] && item.Phase != s.Complete() {
			pending = append(pending, item)
		}
	}
	slices.SortFunc(pending, func(a, b s.Download) int {
		if a.Position != nil && b.Position != nil {
			return int(*a.Position - *b.Position)
		}
		return strings.Compare(a.SourceURL, b.SourceURL)
	})
	rows := make([]episodeRow, 0, len(pending))
	for _, item := range pending {
		reason := item.Reason
		if reason == "" {
			reason = item.Error
		}
		rows = append(
			rows,
			episodeRow{
				key:      item.ID,
				title:    item.Title,
				source:   item.SourceURL,
				status:   item.Phase.String(),
				reason:   reason,
				duration: item.DurationSeconds,
			},
		)
	}
	return rows
}

func duration(seconds *int64) string {
	if seconds == nil {
		return "—"
	}
	if *seconds >= secondsPerHour {
		return fmt.Sprintf(
			"%d:%02d:%02d",
			*seconds/secondsPerHour,
			*seconds/secondsPerMinute%secondsPerMinute,
			*seconds%secondsPerMinute,
		)
	}
	return fmt.Sprintf("%d:%02d", *seconds/secondsPerMinute, *seconds%secondsPerMinute)
}

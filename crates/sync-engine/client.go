package syncengine

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"sync"
	"time"

	"github.com/jonboulle/clockwork"
	p "github.com/listenbox/client/crates/sync-engine/publicapi"
	"golang.org/x/sync/errgroup"
)

type Catalog struct {
	Teams      []p.ClientTeam
	ImportTeam string
	Email      string
	Shows      []p.Show
}
type SyncState struct {
	Items      []Download
	LastSynced *time.Time
}
type SyncEvent struct {
	Shows     []p.Show
	Slug      string
	SourceURL string
	Report    Report
	Err       error
	Items     []Download
	Finished  bool
	Failed    int
}
type Client struct {
	Config       Config
	Engine       *Engine
	mu           sync.Mutex
	http, upload *http.Client
	artwork      *artworkHTTP
}

const (
	maximumEpisodePageSize = 500
)

func NewClient(config Config) *Client { return &Client{Config: config, Engine: NewEngine(nil)} }
func NewClientWithClock(config Config, clock clockwork.Clock) *Client {
	return &Client{Config: config, Engine: NewEngine(clock)}
}

func (c *Client) Close() error {
	err := c.Engine.Close()
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.http != nil {
		c.http.CloseIdleConnections()
		c.upload.CloseIdleConnections()
	}
	return err
}

func (c *Client) API(ctx context.Context) (*API, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.http == nil {
		httpClient, upload, err := NewHTTPClients()
		if err != nil {
			return nil, err
		}
		c.http, c.upload = httpClient, upload
	}
	api, err := NewAPIWithClients(c.Config, c.http, c.upload)
	if err == nil {
		api.Clock = c.Engine.clock
	}
	return api, err
}
func (c *Client) HasCredentials() bool { return LoadAuth(c.Config) != "" }
func (c *Client) Logout() error {
	if err := Logout(c.Config); err != nil {
		return err
	}
	c.Engine.Downloads.Clear()
	return c.clearCache()
}
func (c *Client) CookieJar() *CookieJar { return NewCookieJar(c.Config.Directory) }
func (c *Client) Login(ctx context.Context, open func(string)) error {
	api, err := c.API(ctx)
	if err != nil {
		return err
	}
	return api.Login(ctx, p.AuthorizationClientDesktop, open)
}

func (c *Client) Catalog(ctx context.Context) (Catalog, error) {
	var catalog Catalog
	api, err := c.API(ctx)
	if err != nil {
		return catalog, err
	}
	if api.Credential == "" {
		return catalog, ErrAuthentication
	}
	var account p.APIKeyWhoami
	group, groupCtx := errgroup.WithContext(ctx)
	group.Go(func() error {
		return api.JSON(
			groupCtx,
			"GET",
			"/s/teams?writable_only=true&include_plan_family=true",
			nil,
			&catalog.Teams,
		)
	})
	group.Go(func() error {
		return api.JSON(groupCtx, "GET", "/s/shows?youtube_imports_only=true", nil, &catalog.Shows)
	})
	group.Go(func() error { return api.JSON(groupCtx, "GET", "/s/whoami", nil, &account) })
	if err := group.Wait(); err != nil {
		return catalog, err
	}
	catalog.Email = account.Email
	for _, team := range catalog.Teams {
		if catalog.ImportTeam == "" || team.Id == account.TeamId {
			catalog.ImportTeam = team.Id
		}
	}
	c.saveCatalog(ctx, catalog)
	return catalog, nil
}

func (c *Client) Episodes(ctx context.Context, slug string) ([]p.EpisodeListItem, error) {
	api, err := c.API(ctx)
	if err != nil {
		return nil, err
	}
	episodes, err := api.Episodes(ctx, slug, maximumEpisodePageSize)
	if err == nil {
		c.saveEpisodes(ctx, slug, episodes)
	}
	return episodes, err
}

func (api *API) Episodes(ctx context.Context, slug string, limit int) ([]p.EpisodeListItem, error) {
	var episodes []p.EpisodeListItem
	ids, cursors := make(map[string]bool), make(map[string]bool)
	cursor := ""
	for {
		path := fmt.Sprintf("/s/shows/%s/episodes?limit=%d", url.PathEscape(slug), limit)
		if cursor != "" {
			path += "&cursor=" + url.QueryEscape(cursor)
		}
		var page p.EpisodePage
		if err := api.JSON(ctx, "GET", path, nil, &page); err != nil {
			return nil, err
		}
		for _, episode := range page.Episodes {
			if !ids[episode.Id] {
				ids[episode.Id] = true
				episodes = append(episodes, episode)
			}
		}
		if page.NextCursor == nil {
			return episodes, nil
		}
		cursor = *page.NextCursor
		if cursor == "" || cursors[cursor] {
			return nil, errors.New("empty or repeated episode list cursor")
		}
		cursors[cursor] = true
	}
}

func (c *Client) SyncState(ctx context.Context, slug string) (SyncState, error) {
	var state SyncState
	if !c.HasCredentials() {
		return state, ErrAuthentication
	}
	db, err := c.Engine.Database(ctx, c.Config)
	if err != nil {
		return state, err
	}
	state.Items, err = db.Items(ctx, c.Config.APIOrigin, slug)
	if err != nil {
		return state, err
	}
	state.LastSynced, err = db.LastSynced(ctx, c.Config.APIOrigin, slug)
	return state, err
}

func (c *Client) Import(
	ctx context.Context,
	source, team string,
	kind p.ShowSourceKind,
	event func(ImportEvent),
) (p.Show, Report, error) {
	api, err := c.API(ctx)
	if err != nil {
		return p.Show{}, Report{}, err
	}
	if _, err := CollectionSource(source); err != nil {
		return p.Show{}, Report{}, err
	}
	return c.Engine.Import(ctx, api, source, team, "", kind, event)
}

func (c *Client) Sync(ctx context.Context, slug string) (Report, error) {
	api, err := c.API(ctx)
	if err != nil {
		return Report{}, err
	}
	return c.Engine.Once(ctx, api, slug)
}

//nolint:nilerr // User cancellation ends the CLI watch successfully after its workers drain.
func (c *Client) SyncYouTube(
	ctx context.Context,
	show string,
	watch bool,
	event func(SyncEvent),
) error {
	for {
		if ctx.Err() != nil {
			return nil
		}
		failed, err := c.syncYouTubeShows(ctx, show, event)
		if err != nil {
			return err
		}
		if ctx.Err() != nil {
			return nil
		}
		if !watch {
			if failed > 0 {
				return fmt.Errorf(
					"%d show(s) could not finish syncing; see the failures above",
					failed,
				)
			}
			return nil
		}
		if !c.Engine.NextScan(ctx) {
			return nil
		}
	}
}

func (c *Client) syncYouTubeShows(
	ctx context.Context,
	show string,
	event func(SyncEvent),
) (int, error) {
	api, err := c.API(ctx)
	if err != nil {
		return 0, err
	}
	shows, err := youtubeSyncShows(ctx, api, show)
	if err != nil {
		return 0, err
	}
	c.Engine.Downloads.Clear()
	if event != nil {
		event(SyncEvent{Shows: shows})
	}
	var group errgroup.Group
	var resultMu sync.Mutex
	failed := 0
	for _, candidate := range shows {
		group.Go(func() error {
			report, items, err := c.syncYouTubeShow(ctx, api, candidate)
			resultMu.Lock()
			defer resultMu.Unlock()
			if err != nil && ctx.Err() == nil {
				failed++
			}
			if event != nil {
				event(
					SyncEvent{
						Slug:      candidate.Slug,
						SourceURL: stringValue(candidate.Youtube.Url),
						Report:    report,
						Err:       err,
						Items:     items,
					},
				)
			}
			return nil
		})
	}
	_ = group.Wait()
	if event != nil {
		event(SyncEvent{Finished: true, Failed: failed})
	}
	return failed, nil
}

func (c *Client) syncYouTubeShow(
	ctx context.Context,
	api *API,
	candidate p.Show,
) (Report, []Download, error) {
	report, err := c.Engine.Once(ctx, api, candidate.Slug)
	var items []Download
	for _, item := range c.Engine.Downloads.Snapshot() {
		if item.SourceID == candidate.Slug &&
			(item.Phase == Failed() || item.Phase == Skipped()) {
			items = append(items, item)
		}
	}
	return report, items, err
}

func youtubeSyncShows(ctx context.Context, api *API, show string) ([]p.Show, error) {
	var listed []p.Show
	if err := api.JSON(
		ctx,
		"GET",
		"/s/shows?youtube_imports_only=true",
		nil,
		&listed,
	); err != nil {
		return nil, err
	}
	var shows []p.Show
	for _, candidate := range listed {
		if (show != "" && candidate.Slug == show) ||
			(show == "" && candidate.HasActiveSubscription) {
			shows = append(shows, candidate)
		}
	}
	if show != "" && len(shows) == 0 {
		//nolint:staticcheck // Preserve existing end-user guidance.
		return nil, fmt.Errorf(
			"No accessible YouTube import with slug %q",
			show,
		)
	}
	return shows, nil
}

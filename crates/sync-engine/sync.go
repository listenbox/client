package syncengine

import (
	"context"
	"crypto/sha256"
	"errors"
	"fmt"
	"net/url"
	"path/filepath"
	"sync"
	"time"

	"github.com/gofrs/flock"
	"github.com/jonboulle/clockwork"
	p "github.com/listenbox/client/crates/sync-engine/publicapi"
	"golang.org/x/sync/errgroup"
)

type Report struct {
	Added, Removed, Unchanged, Skipped int
	// Deferred counts videos left queued for the next sync because YouTube
	// rate-limited this session.
	Deferred           int
	Reordered, Stopped bool
}
type Engine struct {
	Downloads *DownloadManager
	clock     clockwork.Clock
	anchor    time.Time
	mu        sync.Mutex
	journal   *Database
	opening   chan struct{}
	cancel    context.CancelFunc
	sampler   errgroup.Group
}

const (
	// scanInterval is how often every subscribed podcast is checked.
	scanInterval       = time.Hour
	maximumSyncWorkers = 26
	deferredReason     = "YouTube is rate-limiting this computer; retrying on the next sync"
)

func NewEngine(clock clockwork.Clock) *Engine {
	if clock == nil {
		clock = clockwork.NewRealClock()
	}
	ctx, cancel := context.WithCancel(context.Background())
	engine := &Engine{
		Downloads: newDownloadManager(clock),
		clock:     clock,
		anchor:    clock.Now(),
		cancel:    cancel,
	}
	engine.sampler.Go(func() error { engine.Downloads.sampleLoop(ctx); return nil })
	return engine
}

func (e *Engine) Close() error {
	e.cancel()
	_ = e.sampler.Wait()
	e.mu.Lock()
	defer e.mu.Unlock()
	if e.journal != nil {
		return e.journal.Close()
	}
	return nil
}

func (e *Engine) Database(ctx context.Context, config Config) (*Database, error) {
	for {
		e.mu.Lock()
		if e.journal != nil {
			db := e.journal
			e.mu.Unlock()
			return db, nil
		}
		if e.opening != nil {
			ready := e.opening
			e.mu.Unlock()
			select {
			case <-ctx.Done():
				return nil, ctx.Err()
			case <-ready:
			}
			continue
		}
		e.opening = make(chan struct{})
		ready := e.opening
		e.mu.Unlock()
		db, err := OpenDatabase(ctx, config.Directory)
		e.mu.Lock()
		if err == nil {
			e.journal = db
		}
		e.opening = nil
		close(ready)
		e.mu.Unlock()
		return db, err
	}
}

func (e *Engine) NextScan(ctx context.Context) bool {
	elapsed := e.clock.Now().Sub(e.anchor)
	next := e.anchor.Add(elapsed.Truncate(scanInterval) + scanInterval)
	timer := e.clock.NewTimer(next.Sub(e.clock.Now()))
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return false
	case <-timer.Chan():
		return true
	}
}

func (api *API) Inventory(ctx context.Context, slug string) (p.SyncInventory, error) {
	var complete p.SyncInventory
	cursor := ""
	seen := make(map[string]bool)
	for {
		path := "/s/shows/" + url.PathEscape(slug) + "/sync?limit=500"
		if cursor != "" {
			path += "&cursor=" + url.QueryEscape(cursor)
		}
		var page p.SyncInventory
		if err := api.JSON(ctx, "GET", path, nil, &page); err != nil {
			return complete, err
		}
		if page.Show.Slug != slug {
			return complete, errors.New("sync inventory returned a different show")
		}
		if complete.Show.Id == "" {
			complete = page
		} else {
			if complete.Show.Id != page.Show.Id ||
				stringValue(complete.Show.Youtube.Url) != stringValue(page.Show.Youtube.Url) {
				return complete, errors.New("show source changed while reading inventory")
			}
			complete.Episodes = append(complete.Episodes, page.Episodes...)
		}
		if page.NextCursor == nil {
			complete.NextCursor = nil
			return complete, nil
		}
		cursor = *page.NextCursor
		if cursor == "" || seen[cursor] {
			return complete, errors.New("sync inventory returned an invalid or repeated cursor")
		}
		seen[cursor] = true
	}
}

func stringValue(value *string) string {
	if value == nil {
		return ""
	}
	return *value
}

func (e *Engine) Once(ctx context.Context, api *API, slug string) (Report, error) {
	report, err := e.sync(ctx, api, slug, nil, nil, "", 0)
	if errors.Is(err, ErrSyncStopped) {
		report.Stopped = true
		return report, nil
	}
	return report, err
}

func (e *Engine) sync(
	ctx context.Context,
	api *API,
	slug string,
	scanned *PlaylistSnapshot,
	youtube *YouTube,
	scannedSource string,
	scannedHeight int,
) (Report, error) {
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	var report Report
	lock, err := lockSync(api.Config, slug)
	if err != nil {
		return report, err
	}
	defer lock.Close()
	journal, err := e.Database(ctx, api.Config)
	if err != nil {
		return report, err
	}
	before, err := api.Inventory(ctx, slug)
	if errors.Is(err, ErrSyncStopped) {
		report.Stopped = true
		return report, nil
	}
	if err != nil {
		return report, err
	}
	return e.syncInventory(
		ctx,
		cancel,
		api,
		slug,
		journal,
		before,
		scanned,
		youtube,
		scannedSource,
		scannedHeight,
	)
}

func (e *Engine) saveOutcome(
	ctx context.Context,
	journal *Database,
	origin, slug, id string,
) error {
	for _, item := range e.Downloads.Snapshot() {
		if item.ID == id {
			return journal.Outcome(ctx, origin, slug, item)
		}
	}
	return fmt.Errorf("missing transfer %q", id)
}

func (api *API) updateSyncMetadata(
	ctx context.Context,
	slug, collection string,
	image *string,
	titles []p.SyncEpisodeTitle,
	checked bool,
) error {
	if titles == nil {
		titles = []p.SyncEpisodeTitle{}
	}
	body := p.UpdateSyncMetadata{YoutubeUrl: collection, ImageAssetId: image, EpisodeTitles: titles}
	if checked {
		body.Checked = &checked
	}
	return api.JSON(ctx, "PUT", "/s/shows/"+url.PathEscape(slug)+"/sync/metadata", body, nil)
}

func (api *API) SetOrder(
	ctx context.Context,
	slug string,
	collection *string,
	ids []string,
) (p.EpisodeOrder, error) {
	if ids == nil {
		ids = []string{}
	}
	var order p.EpisodeOrder
	err := api.JSON(
		ctx,
		"PUT",
		"/s/shows/"+url.PathEscape(slug)+"/episode-order",
		p.SetEpisodeOrder{EpisodeIds: ids, YoutubeUrl: collection},
		&order,
	)
	return order, err
}

func (api *API) deleteSyncEpisode(ctx context.Context, slug, episode, collection string) error {
	var created struct {
		Run string `json:"episode_deletion_run_id"`
	}
	path := "/s/shows/" + url.PathEscape(
		slug,
	) + "/sync/episodes/" + url.PathEscape(
		episode,
	) + "/deletion?youtube_url=" + url.QueryEscape(
		collection,
	)
	if err := api.JSON(ctx, "POST", path, nil, &created); err != nil {
		return err
	}
	events, err := api.Events(ctx, "/s/episode-deletions/"+created.Run+"/events")
	if err != nil {
		return err
	}
	defer events.Close()
	for {
		event, err := events.Next()
		if err != nil {
			return err
		}
		if event["type"] != "terminal" {
			continue
		}
		if event["episode_id"] != episode {
			return errors.New("deletion completed for a different episode")
		}
		if event["status"] != completedStatus {
			return errors.New("episode deletion failed")
		}
		return nil
	}
}

func lockSync(config Config, slug string) (*flock.Flock, error) {
	if err := osCreateProfile(config.Directory); err != nil {
		return nil, err
	}
	lockKey := sha256.Sum256([]byte(config.APIOrigin + "\x00" + slug))
	lock := flock.New(filepath.Join(config.Directory, fmt.Sprintf("sync-%x.lock", lockKey)))
	ok, err := lock.TryLock()
	if err != nil || !ok {
		lock.Close()
		//nolint:staticcheck // Preserve existing end-user guidance.
		return nil, errors.New(
			"Another client is already syncing this show on this computer",
		)
	}
	return lock, nil
}

func syncSourceSnapshot(
	ctx context.Context,
	youtube *YouTube,
	source *url.URL,
	scanned *PlaylistSnapshot,
	collection string,
) (PlaylistSnapshot, error) {
	var snapshot PlaylistSnapshot
	var err error
	if scanned != nil {
		snapshot = *scanned
	} else if playlist := collectionPlaylistID(source); playlist != "" {
		snapshot, err = youtube.Snapshot(ctx, playlist)
	} else {
		id := source.Query().Get("v")
		if !validVideoID(id) {
			return snapshot, errors.New("show source has no video ID")
		}
		snapshot, err = youtube.VideoSnapshot(ctx, id)
	}
	if err != nil {
		return snapshot, fmt.Errorf("YouTube source %s: %w", collection, err)
	}
	return snapshot, nil
}

type syncRun struct {
	engine                   *Engine
	api                      *API
	journal                  *Database
	youtube                  *YouTube
	origin, slug, collection string
	before                   p.SyncInventory
	snapshot                 PlaylistSnapshot
	height                   int
	report                   *Report
}

func (r *syncRun) admit(ctx context.Context) ([]Video, []string, map[string]bool, error) {
	if err := r.updateArtwork(ctx); err != nil {
		return nil, nil, nil, err
	}
	if err := r.journal.Snapshot(ctx,
		r.origin,
		r.slug,
		r.collection,
		r.snapshot.Present,
		r.snapshot.CanRemove,
	); err != nil {
		return nil, nil, nil, err
	}
	remote := make(map[string]bool)
	for _, video := range r.snapshot.Present {
		remote[video.URL()] = true
	}
	existingSet, err := r.checkpointPublished(ctx, remote)
	if err != nil {
		return nil, nil, nil, err
	}
	var additions []Video
	for _, video := range r.snapshot.Present {
		if !existingSet[video.URL()] {
			additions = append(additions, video)
			if _, err := r.journal.Operation(
				ctx,
				r.origin,
				r.slug,
				video.URL(),
				r.collection,
			); err != nil {
				return nil, nil, nil, err
			}
		}
	}
	ids := r.engine.Downloads.enqueue(r.slug, r.before.Show.Title, r.snapshot.Present, existingSet)
	for _, item := range r.engine.Downloads.Snapshot() {
		if item.SourceID == r.slug {
			if err := r.journal.Outcome(ctx, r.origin, r.slug, item); err != nil {
				return nil, nil, nil, err
			}
		}
	}
	return additions, ids, remote, nil
}

func (r *syncRun) checkpointPublished(
	ctx context.Context,
	remote map[string]bool,
) (map[string]bool, error) {
	existingSet := make(map[string]bool)
	published := make([]string, 0, len(r.before.Episodes))
	for _, episode := range r.before.Episodes {
		existingSet[episode.SourceUrl] = true
		published = append(published, episode.SourceUrl)
		if !r.snapshot.CanRemove || remote[episode.SourceUrl] {
			r.report.Unchanged++
		}
	}
	if err := r.journal.Published(ctx, r.origin, r.slug, published); err != nil {
		return nil, err
	}
	for _, episode := range r.before.Episodes {
		if err := r.journal.Forget(ctx, r.origin, r.slug, episode.SourceUrl); err != nil {
			return nil, err
		}
	}
	return existingSet, nil
}

func (r *syncRun) transferAdditions(
	ctx context.Context,
	cancel context.CancelFunc,
	additions []Video,
	ids []string,
) error {
	var group errgroup.Group
	group.SetLimit(maximumSyncWorkers)
	var resultMu sync.Mutex
	var failures []error
	var limited error
	stopped := false
	for index, video := range additions {
		id := ids[index]

		group.Go(func() error {
			reason, transferErr := r.transferVideo(ctx, cancel, &resultMu, &stopped, video, id)
			resultMu.Lock()
			defer resultMu.Unlock()
			switch {
			case errors.Is(transferErr, ErrRateLimited):
				r.report.Deferred++
				if limited == nil {
					limited = transferErr
				}
			case transferErr != nil:
				failures = append(failures, transferErr)
			case reason != "":
				r.report.Skipped++
			default:
				r.report.Added++
			}
			return nil
		})
	}
	_ = group.Wait()
	if stopped {
		r.report.Stopped = true
		return nil
	}
	if ctx.Err() != nil {
		return ctx.Err()
	}
	var errs []error
	if len(failures) > 0 {
		errs = append(errs, fmt.Errorf(
			"%d transfer(s) failed: %w",
			len(failures),
			errors.Join(failures...),
		))
	}
	if limited != nil {
		errs = append(errs, fmt.Errorf(
			"%d episode(s) will be retried on the next sync: %w",
			r.report.Deferred,
			limited,
		))
	}
	return errors.Join(errs...)
}

func (r *syncRun) transferVideo(
	ctx context.Context,
	cancel context.CancelFunc,
	mu *sync.Mutex,
	stopped *bool,
	video Video,
	id string,
) (string, error) {
	reason, transferErr := r.retryVideo(ctx, cancel, mu, stopped, video, id)
	r.engine.Downloads.update(id, func(item *Download) {
		switch {
		case errors.Is(transferErr, ErrRateLimited):
			item.Phase = Queued()
			item.Reason = deferredReason
			item.Error = ""
		case transferErr != nil:
			item.Error = Redact(transferErr.Error())
			if ctx.Err() != nil {
				item.Phase = Queued()
			} else {
				item.Phase = Failed()
			}
		case reason != "":
			item.Phase = Skipped()
			item.Reason = reason
		default:
			item.Phase = Complete()
			item.Error = ""
			item.Uploaded = item.UploadTotal
		}
	})
	if err := r.engine.saveOutcome(ctx, r.journal, r.origin, r.slug, id); err != nil {
		transferErr = err
	}
	return reason, transferErr
}

func (r *syncRun) retryVideo(
	ctx context.Context,
	cancel context.CancelFunc,
	mu *sync.Mutex,
	stopped *bool,
	video Video,
	id string,
) (string, error) {
	var reason string
	var transferErr error
	for attempt := 1; attempt <= 5; attempt++ {
		r.engine.Downloads.beginAttempt(id, attempt)
		reason, transferErr = r.engine.importVideo(
			ctx,
			r.api,
			r.youtube,
			r.journal,
			r.slug,
			r.collection,
			video,
			r.before.Show.SourceKind == p.ShowSourceKindAudio,
			r.height,
			r.before.Show.Language,
			id,
		)
		if transferErr == nil {
			break
		}
		if errors.Is(transferErr, ErrSyncStopped) {
			mu.Lock()
			*stopped = true
			mu.Unlock()
			cancel()
			break
		}
		if ctx.Err() != nil || !retryable(transferErr) || attempt == 5 {
			break
		}
		r.engine.Downloads.update(
			id,
			func(item *Download) { item.Error = Redact(transferErr.Error()); item.Phase = Retrying() },
		)
		if err := r.engine.saveOutcome(ctx, r.journal, r.origin, r.slug, id); err != nil {
			transferErr = err
			break
		}
		select {
		case <-ctx.Done():
			transferErr = ctx.Err()
		case <-r.engine.clock.After(time.Second << uint(attempt-1)):
			continue
		}
		break
	}
	return reason, transferErr
}

func (r *syncRun) reconcile(ctx context.Context, additions []Video, remote map[string]bool) error {
	var err error
	after := r.before
	needInventory := needsSyncInventory(r.before, len(additions))
	if needInventory {
		after, err = r.api.Inventory(ctx, r.slug)
		if err != nil {
			return err
		}
	}
	if stringValue(after.Show.Youtube.Url) != r.collection {
		return errors.New("show linkage changed during sync")
	}
	if err := r.removeEpisodes(ctx, remote); err != nil {
		return err
	}
	if r.report.Removed > 0 {
		after, err = r.api.Inventory(ctx, r.slug)
		if err != nil {
			return err
		}
	}
	if stringValue(after.Show.Youtube.Url) != r.collection {
		return errors.New("show source changed during sync")
	}
	if err := r.reconcileMetadata(ctx, after); err != nil {
		return err
	}
	if err := r.api.updateSyncMetadata(ctx, r.slug, r.collection, nil, nil, true); err != nil {
		return err
	}
	if err := r.journal.CompleteSync(ctx, r.origin, r.slug); err != nil {
		return err
	}
	return nil
}

func (r *syncRun) reconcileMetadata(ctx context.Context, after p.SyncInventory) error {
	titles, order, reordered := syncEpisodeMetadata(r.snapshot, after)
	if len(titles) > 0 {
		if err := r.api.updateSyncMetadata(
			ctx,
			r.slug,
			r.collection,
			nil,
			titles,
			false,
		); err != nil {
			return err
		}
	}
	if reordered {
		if _, err := r.api.SetOrder(ctx, r.slug, &r.collection, order); err != nil {
			return err
		}
		r.report.Reordered = true
	}
	return nil
}

func (e *Engine) syncInventory(
	ctx context.Context,
	cancel context.CancelFunc,
	api *API,
	slug string,
	journal *Database,
	before p.SyncInventory,
	scanned *PlaylistSnapshot,
	youtube *YouTube,
	scannedSource string,
	scannedHeight int,
) (Report, error) {
	var report Report
	collection := stringValue(before.Show.Youtube.Url)
	if collection == "" {
		//nolint:revive,staticcheck // Preserve existing end-user guidance.
		return report, errors.New(
			"This podcast has no linked YouTube playlist, channel or video.",
		)
	}
	source, err := url.Parse(collection)
	if err != nil {
		return report, err
	}
	if youtube == nil {
		youtube, err = NewYouTube(ctx, api)
		if err != nil {
			return report, err
		}
		defer youtube.Close()
	}
	if scanned != nil && scannedSource != collection {
		return report, errors.New("show source differs from the admitted playlist")
	}
	snapshot, err := syncSourceSnapshot(ctx, youtube, source, scanned, collection)
	if err != nil {
		return report, err
	}
	height := int(before.VideoMaxHeight)
	if scanned != nil {
		height = scannedHeight
	}
	run := syncRun{
		engine: e, api: api, journal: journal, youtube: youtube,
		origin: api.Config.APIOrigin, slug: slug, collection: collection,
		before: before, snapshot: snapshot, height: height, report: &report,
	}
	additions, ids, remote, err := run.admit(ctx)
	if err != nil {
		return report, err
	}
	return report, run.finish(ctx, cancel, additions, ids, remote)
}

// finish transfers the additions and reconciles the pass. Per-video failures
// and deferrals leave those videos for the next sync; the rest of the pass
// still reconciles what was published.
func (r *syncRun) finish(
	ctx context.Context,
	cancel context.CancelFunc,
	additions []Video,
	ids []string,
	remote map[string]bool,
) error {
	transferErr := r.transferAdditions(ctx, cancel, additions, ids)
	if r.report.Stopped || ctx.Err() != nil {
		return transferErr
	}
	if err := r.reconcile(ctx, additions, remote); err != nil {
		return errors.Join(transferErr, err)
	}
	return transferErr
}

func (r *syncRun) updateArtwork(ctx context.Context) error {
	if stringValue(r.before.Show.ImageUrl) == "" && r.snapshot.ArtworkURL != "" {
		image, err := r.api.UploadSourceArtwork(ctx, r.before.Show.Id, r.snapshot.ArtworkURL)
		if err != nil {
			return err
		}
		if err := r.api.updateSyncMetadata(
			ctx,
			r.slug,
			r.collection,
			&image,
			nil,
			false,
		); err != nil {
			return err
		}
	}
	return nil
}

func (m *DownloadManager) beginAttempt(id string, attempt int) {
	m.update(id, func(item *Download) {
		item.Attempt = attempt
		item.Phase = Resolving()
		item.Error = ""
		item.Total = 0
		item.Received = 0
		item.UploadTotal = 0
		item.Uploaded = 0
		item.Ranges = nil
	})
}

func (r *syncRun) removeEpisodes(ctx context.Context, remote map[string]bool) error {
	for _, episode := range r.before.Episodes {
		if episode.Imported && r.snapshot.CanRemove && !remote[episode.SourceUrl] {
			if err := r.api.deleteSyncEpisode(ctx, r.slug, episode.Id, r.collection); err != nil {
				return err
			}
			if err := r.journal.Forget(ctx, r.origin, r.slug, episode.SourceUrl); err != nil {
				return err
			}
			r.report.Removed++
		}
	}
	return nil
}

func syncEpisodeMetadata(
	snapshot PlaylistSnapshot,
	after p.SyncInventory,
) ([]p.SyncEpisodeTitle, []string, bool) {
	epBySource := make(map[string]p.SyncEpisode)
	for _, episode := range after.Episodes {
		epBySource[episode.SourceUrl] = episode
	}
	var titles []p.SyncEpisodeTitle
	var order []string
	reordered := false
	for _, video := range snapshot.Present {
		episode, ok := epBySource[video.URL()]
		if !ok {
			continue
		}
		if video.SourceTitle != nil && episode.Imported && episode.Title != *video.SourceTitle {
			titles = append(
				titles,
				p.SyncEpisodeTitle{
					EpisodeId: episode.Id,
					SourceUrl: video.URL(),
					Title:     *video.SourceTitle,
				},
			)
		}
		position := int64(len(order))
		if episode.Position == nil || *episode.Position != position {
			reordered = true
		}
		order = append(order, episode.Id)
	}
	return titles, order, reordered
}

func needsSyncInventory(before p.SyncInventory, additions int) bool {
	needInventory := additions > 0
	for _, episode := range before.Episodes {
		needInventory = needInventory || !episode.Imported
	}
	return needInventory
}

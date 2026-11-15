package main

import (
	"context"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"log/slog"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/gofrs/flock"
	s "github.com/listenbox/client/crates/sync-engine"
	p "github.com/listenbox/client/crates/sync-engine/publicapi"
)

var desktopFixtureInvocation bool

func TestMain(m *testing.M) {
	aliases := map[string]string{
		"live_player_runtime":                "TestLivePlayerRuntime",
		"live_upload_progress":               "TestLiveUploadProgress",
		"live_authorization_recovery":        "TestLiveAuthorizationRecovery",
		"live_authorization_cancel_starting": "TestLiveAuthorizationCancelStarting",
		"live_logout_contended_lock":         "TestLiveLogoutContendedLock",
		"live_logout_initializing":           "TestLiveLogoutInitializing",
		"live_artwork":                       "TestLiveArtwork",
		"live_server_direction":              "TestLiveServerDirection",
		"live_episode_scrolling":             "TestLiveEpisodeScrolling",
		"live_not_imported":                  "TestLiveNotImported",
		"live_backend":                       "TestLiveBackend",
		"live_concurrent_imports":            "TestLiveConcurrentImports",
		"live_pause_imports":                 "TestLivePauseImports",
		"live_video_creation_progress":       "TestLiveVideoCreationProgress",
		"live_team_import":                   "TestLiveTeamImport",
		"live_import":                        "TestLiveImport",
		"live_import_payment_required":       "TestLiveImportPaymentRequired",
	}
	if len(os.Args) > 1 {
		if name, ok := aliases[os.Args[1]]; ok {
			desktopFixtureInvocation = true
			os.Args = []string{
				os.Args[0],
				"-test.run=^" + name + "$",
				"-test.v",
				"-test.timeout=30s",
			}
		}
	}
	os.Exit(m.Run())
}

type desktopScenario struct {
	Slug         string   `json:"Slug"`
	Source       string   `json:"Source"`
	Team         string   `json:"Team"`
	Control      string   `json:"Control"`
	Mode         string   `json:"Mode"`
	Sources      []string `json:"Sources"`
	EpisodeCount int      `json:"EpisodeCount"`
}

func desktopScenarioConfig(t *testing.T) (s.Config, desktopScenario) {
	t.Helper()
	if !desktopFixtureInvocation {
		t.Skip("invoked by the parent API's isolated E2E services")
	}
	config, err := s.LoadConfig("")
	desktopNoError(t, err)
	raw, err := os.ReadFile(filepath.Join(config.Directory, "test-desktop.json"))
	desktopNoError(t, err)
	var scenario desktopScenario
	desktopNoError(t, json.Unmarshal(raw, &scenario))
	return config, scenario
}

func liveDesktop(t *testing.T, automatic bool) (*desktopHarness, desktopScenario) {
	t.Helper()
	config, scenario := desktopScenarioConfig(t)
	h := desktopTest(t, config, automatic)
	if !automatic {
		h.w.authorized = h.w.client.HasCredentials()
		h.w.started = false
		h.w.start()
		h.w.refreshCatalog(false)
	}
	h.wait(func() bool { return h.w.loaded && !h.w.loading })
	if h.w.errorText != "" {
		t.Fatal(h.w.errorText)
	}
	return h, scenario
}

func (h *desktopHarness) control(scenario desktopScenario, action string) {
	h.t.Helper()
	ctx, cancel := context.WithTimeout(h.t.Context(), 2*time.Second)
	defer cancel()
	api, err := h.w.client.API(ctx)
	desktopNoError(h.t, err)
	request, err := http.NewRequestWithContext(
		ctx,
		http.MethodGet,
		scenario.Control+"/"+action,
		nil,
	)
	desktopNoError(h.t, err)
	response, err := api.HTTP.Do(request)
	desktopNoError(h.t, err)
	defer response.Body.Close()
	if response.StatusCode != http.StatusNoContent {
		h.t.Fatalf("desktop gate %s: HTTP %d", action, response.StatusCode)
	}
}

func (h *desktopHarness) beginImport(source, team string, kind p.ShowSourceKind) {
	h.t.Helper()
	h.click("Import from YouTube")
	h.click("YouTube URL")
	h.screen.Key(ui.Cmd, ui.KeyA)
	h.screen.Type(source)
	if team != "" {
		h.w.importTeam = team
	}
	if kind == p.ShowSourceKindVideo {
		h.click("Video")
	}
	h.click("Import")
	if !h.w.creating {
		h.t.Fatal("import did not start", h.w.errorText)
	}
}

func (h *desktopHarness) importFinished() {
	h.t.Helper()
	h.wait(func() bool { return !h.w.creating && !h.w.loading && len(h.w.jobs) == 0 })
	if h.w.errorText != "" {
		h.t.Fatal(h.w.errorText)
	}
}

func TestLivePlayerRuntime(t *testing.T) {
	t.Parallel()
	config, scenario := desktopScenarioConfig(t)
	client := s.NewClient(config)
	defer client.Close()
	report, err := client.Sync(t.Context(), scenario.Slug)
	desktopNoError(t, err)
	items := client.Engine.Downloads.Snapshot()
	if len(items) != 1 {
		t.Fatalf("player result: %+v", items)
	}
	if scenario.Mode == "recursive" {
		if report.Skipped != 1 || items[0].Phase != s.Skipped() ||
			!strings.Contains(items[0].Reason, "Maximum call stack size exceeded") {
			t.Fatalf("recursive player escaped its owner: %+v %+v", report, items)
		}
	} else if report.Added != 1 || items[0].Phase != s.Complete() {
		t.Fatalf("player import: %+v %+v", report, items)
	}
	_, err = client.Catalog(t.Context())
	desktopNoError(t, err)
}

func TestLiveUploadProgress(t *testing.T) {
	t.Parallel()
	h, scenario := liveDesktop(t, true)
	h.control(scenario, "upload-body")
	h.wait(func() bool {
		return slices.ContainsFunc(
			h.w.items,
			func(item s.Download) bool { return item.Phase == s.Uploading() && item.Uploaded > 0 },
		)
	})
	if _, exists := h.screen.Find("Prepared media upload progress"); !exists {
		t.Fatal("transmitted body has no rendered progress")
	}
	if len(h.w.episodes) != 0 {
		t.Fatal("unacknowledged upload appeared as an episode")
	}
	h.click("Pause")
	h.wait(func() bool { return len(h.w.jobs) == 0 })
	if !h.screen.HasText("Resume") {
		t.Fatal("paused upload has no resume control")
	}
	for _, item := range h.w.client.Engine.Downloads.Snapshot() {
		if item.Phase != s.Queued() {
			t.Fatalf("pause retained active work: %+v", item)
		}
	}
	h.control(scenario, "resume-paused")
	h.click("Resume")
	h.wait(func() bool {
		return len(h.w.jobs) == 0 && h.w.reports[scenario.Slug].Added == 1 && len(h.w.episodes) == 1
	})
	if !h.screen.HasText(h.w.episodes[0].Title) {
		t.Fatal("resumed upload did not appear in the episode list")
	}
}

func TestLiveAuthorizationRecovery(t *testing.T) {
	t.Parallel()
	h, scenario := liveDesktop(t, false)
	h.click("Sign in to Listenbox")
	h.control(scenario, "started/1")
	h.wait(func() bool { return h.w.loginURL != "" })
	first := h.w.loginURL
	h.click("Reopen sign in")
	if opened := h.screen.OpenedURLs(); len(opened) < 2 || opened[len(opened)-1] != first {
		t.Fatal("reopen did not recover the pending browser")
	}
	h.click("Cancel sign in")
	h.control(scenario, "disconnected")
	u, err := url.Parse(first)
	desktopNoError(t, err)
	h.control(scenario, "approve?code="+url.QueryEscape(u.Query().Get("code")))
	if h.w.client.HasCredentials() || h.w.errorText != "" {
		t.Fatal("cancelled authorization restored credentials or displayed an error")
	}
	h.click("Sign in to Listenbox")
	h.control(scenario, "started/2")
	h.wait(func() bool { return h.w.loginURL != "" })
	if h.w.loginURL == first {
		t.Fatal("fresh authorization reused the cancelled attempt")
	}
	u, err = url.Parse(h.w.loginURL)
	desktopNoError(t, err)
	h.control(scenario, "approve?code="+url.QueryEscape(u.Query().Get("code")))
	h.wait(func() bool { return h.w.authorized && !h.w.loading && len(h.w.catalog.Teams) > 0 })
	if !h.w.client.HasCredentials() || h.screen.HasText("Cancel sign in") {
		t.Fatal("approval did not finish sign in")
	}
}

func TestLiveAuthorizationCancelStarting(t *testing.T) {
	t.Parallel()
	h, scenario := liveDesktop(t, false)
	h.screen.SetSize(480, 600)
	h.click("Sign in to Listenbox")
	h.control(scenario, "committed")
	if len(h.screen.OpenedURLs()) != 0 {
		t.Fatal("browser opened before authorization reply")
	}
	h.click("Cancel sign in")
	h.control(scenario, "disconnected")
	h.control(scenario, "release")
	h.wait(func() bool { return !h.w.signingIn })
	if h.w.client.HasCredentials() || h.w.errorText != "" || len(h.screen.OpenedURLs()) != 0 {
		t.Fatal("cancelled starting attempt escaped cancellation")
	}
}

type lockEventWriter struct{ events chan struct{} }

func (w lockEventWriter) Write(raw []byte) (int, error) {
	if strings.Contains(string(raw), "acquiring profile lock") {
		select {
		case w.events <- struct{}{}:
		default:
		}
	}
	return len(raw), nil
}

func TestLiveLogoutContendedLock(t *testing.T) {
	t.Parallel()
	config, scenario := desktopScenarioConfig(t)
	path := filepath.Join(config.Directory, "sync-schema.lock")
	if scenario.Mode == "cookies" {
		path = filepath.Join(config.Directory, "youtube-cookies", "write.lock")
	}
	desktopNoError(t, os.MkdirAll(filepath.Dir(path), 0o700))
	held := flock.New(path)
	desktopNoError(t, held.Lock())
	defer held.Close()
	events := make(chan struct{}, 16)
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(lockEventWriter{events}, nil)))
	defer slog.SetDefault(previous)
	h := desktopTest(t, config, true)
	if scenario.Mode == "cookies" {
		h.wait(func() bool { return h.w.loaded && !h.w.loading })
		h.w.openSettings()
		h.wait(func() bool { return !h.w.cookieBusy })
		h.w.cookieInput = "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tfixture-session\n"
		h.w.saveCookies(false)
	}
	reached := false
	go func() {
		select {
		case <-events:
			h.messages <- func() { reached = true }
		case <-t.Context().Done():
		}
	}()
	h.wait(func() bool { return reached })
	h.click("Sign out")
	h.wait(func() bool { return !h.w.stopping && !h.w.authorized })
	if h.w.client.HasCredentials() {
		t.Fatal("logout retained credentials while a lock owner remained alive")
	}
	if _, err := os.Stat(
		filepath.Join(config.Directory, "youtube-cookies", "jar.json"),
	); err == nil {
		t.Fatal("cancelled cookie lock waiter committed state")
	}
}

func TestLiveLogoutInitializing(t *testing.T) {
	t.Parallel()
	h, scenario := liveDesktop(t, true)
	h.control(scenario, "initializing")
	h.click("Sign out")
	h.wait(func() bool { return !h.w.stopping && !h.w.authorized })
	h.control(scenario, "disconnected")
	if h.w.client.HasCredentials() || len(h.w.catalog.Shows) != 0 {
		t.Fatal("logout did not clear the account after draining initialization")
	}
	key := sha256.Sum256([]byte(h.w.client.Config.APIOrigin + "\x00" + scenario.Slug))
	lock := flock.New(filepath.Join(h.w.client.Config.Directory, fmt.Sprintf("sync-%x.lock", key)))
	defer lock.Close()
	ok, err := lock.TryLock()
	desktopNoError(t, err)
	if !ok {
		t.Fatal("logout left the sync owner alive")
	}
}

func TestLiveArtwork(t *testing.T) {
	t.Parallel()
	h, _ := liveDesktop(t, false)
	h.wait(func() bool {
		return len(h.w.catalog.Shows) > 0 &&
			h.w.artwork[value(h.w.catalog.Shows[0].ImageUrl)] != nil
	})
	if len(h.w.artworkPending) != 0 {
		t.Fatal("decoded artwork retained an outstanding request")
	}
}

func TestLiveServerDirection(t *testing.T) {
	t.Parallel()
	h, scenario := liveDesktop(t, true)
	if len(h.w.catalog.Shows) != 0 || h.w.selected != "" || len(h.w.jobs) != 0 {
		t.Fatal("server publishing direction leaked into the import library")
	}
	h.control(scenario, "pause-destination")
	h.click("Refresh library")
	h.wait(func() bool {
		return !h.w.loading && len(h.w.catalog.Shows) == 1 && len(h.w.jobs) == 0 &&
			len(h.w.reports) == 1
	})
	if h.w.selected != scenario.Slug || h.w.errorText != "" {
		t.Fatal("paused destination did not restore its import", h.w.errorText)
	}
}

func TestLiveEpisodeScrolling(t *testing.T) {
	t.Parallel()
	h, scenario := liveDesktop(t, false)
	h.wait(func() bool { return len(h.w.episodes) == scenario.EpisodeCount })
	if len(h.w.episodes) != 520 {
		t.Fatal("episode pagination stopped at a page boundary")
	}
	first, last := h.w.episodes[0], h.w.episodes[len(h.w.episodes)-1]
	if !h.screen.HasText(first.Title) {
		t.Fatal("first episode is not rendered")
	}
	list, ok := h.screen.Find("Episode list")
	if !ok {
		t.Fatal("episode list is absent")
	}
	h.screen.Scroll(list.X+list.W/2, list.Y+list.H/2, 0, 100000)
	h.screen.Frame()
	if !h.screen.HasText(last.Title) {
		t.Fatal("last episode cannot be scrolled into view")
	}
	if len(h.w.jobs) != 0 {
		t.Fatal("reading the library started an import")
	}
}

func TestLiveNotImported(t *testing.T) {
	t.Parallel()
	h, scenario := liveDesktop(t, false)
	h.wait(func() bool { return len(h.w.episodes) == 1 && len(h.w.state.Items) > 1 })
	if h.w.state.LastSynced == nil {
		t.Fatal("CLI sync completion did not survive desktop restart")
	}
	h.click("Not imported")
	rows := h.w.rows()
	if len(rows) != 1 || rows[0].status != "skipped" || rows[0].reason == "" {
		t.Fatalf("saved source issue is not visible: %+v", rows)
	}
	if rows[0].source == scenario.Source {
		t.Fatal("published episode appeared in Not imported")
	}
}

//nolint:cyclop // Verify the complete isolated desktop backend lifecycle and resulting source state.
func TestLiveBackend(t *testing.T) {
	t.Parallel()
	h, scenario := liveDesktop(t, true)
	h.wait(func() bool {
		return len(h.w.reports) > 0 && len(h.w.jobs) == 0 && !h.w.loading &&
			len(h.w.episodes) == 1 &&
			h.w.show().EpisodeCount == 1
	})
	show := h.w.show()
	if show.LastEpisodeAt == nil || show.Youtube.LastCheckedAt == nil ||
		h.w.state.LastSynced == nil {
		t.Fatal("completed sync did not refresh library metadata")
	}
	if h.w.reports[scenario.Slug].Added != 1 ||
		h.w.episodes[0].Title != "Listenbox deterministic fixture" {
		t.Fatal("native workspace did not import the real media fixture")
	}
	h.click("Manage podcast")
	opened := h.screen.OpenedURLs()
	if len(opened) == 0 || opened[len(opened)-1] != h.w.managementURL() {
		t.Fatal("management link targets another podcast")
	}
	h.click("Sync now")
	h.wait(
		func() bool { return len(h.w.jobs) == 0 && !h.w.loading && h.w.reports[scenario.Slug].Unchanged == 1 },
	)
	h.click("Settings")
	h.wait(func() bool { return !h.w.cookieBusy })
	h.click("Done")
	request := h.w.episodeRequest
	h.screen.Key(ui.Cmd, ui.KeyR)
	h.wait(func() bool { return !h.w.loading && h.w.episodeRequest > request })
	h.click("Sign out")
	h.wait(func() bool { return !h.w.stopping && !h.w.authorized })
	if h.w.client.HasCredentials() {
		t.Fatal("workspace retained credentials")
	}
}

func TestLiveConcurrentImports(t *testing.T) {
	t.Parallel()
	h, scenario := liveDesktop(t, false)
	h.beginImport(scenario.Sources[0], scenario.Team, p.ShowSourceKindAudio)
	h.control(scenario, "first-admitted")
	h.wait(func() bool { return len(h.w.jobs) == 1 && !h.w.creating })
	first := h.w.selected
	h.beginImport(scenario.Sources[1], scenario.Team, p.ShowSourceKindAudio)
	h.control(scenario, "second-admitted")
	h.wait(func() bool { return len(h.w.jobs) == 2 && !h.w.creating })
	second := h.w.selected
	h.click("Import from YouTube")
	h.click("YouTube URL")
	h.screen.Type("https://www.youtube.com/playlist?list=PLnextdraft")
	h.control(scenario, "release-first")
	h.wait(func() bool { return h.w.jobs[first] == nil })
	if h.w.jobs[second] == nil || !h.w.importOpen ||
		!strings.Contains(h.w.importSource, "PLnextdraft") ||
		h.w.reports[first].Added != 1 {
		t.Fatal("a sibling completion discarded an active import or next draft")
	}
	h.w.selectShow(second)
	h.screen.Frame()
	h.click("Pause")
	h.wait(func() bool { return len(h.w.jobs) == 0 })
	h.control(scenario, "release-second")
	h.click("Resume")
	h.wait(func() bool { return len(h.w.jobs) == 0 && h.w.reports[second].Added == 1 })
}

//nolint:cyclop // Verify cancellation, durable pause and explicit resume together in the isolated desktop driver.
func TestLivePauseImports(t *testing.T) {
	t.Parallel()
	h, scenario := liveDesktop(t, false)
	h.beginImport(scenario.Sources[0], scenario.Team, p.ShowSourceKindAudio)
	h.control(scenario, "first-admitted")
	h.wait(func() bool {
		return !h.w.creating && len(h.w.jobs) == 1 && len(h.w.items) == 32 &&
			slices.ContainsFunc(
				h.w.items,
				func(item s.Download) bool { return item.Phase == s.Resolving() },
			)
	})
	first := h.w.selected
	h.beginImport(scenario.Sources[1], scenario.Team, p.ShowSourceKindAudio)
	h.wait(func() bool { return !h.w.creating && h.w.selected != first })
	second := h.w.selected
	h.w.selectShow(first)
	h.screen.Frame()
	h.click("Pause")
	h.wait(func() bool { return len(h.w.jobs) == 0 && !h.w.loading })
	if h.w.reports[second].Added != 1 {
		t.Fatal("pausing one source stopped its sibling")
	}
	state, err := h.w.client.SyncState(t.Context(), first)
	desktopNoError(t, err)
	if len(state.Items) != 32 {
		t.Fatalf("pause lost queued work: %d", len(state.Items))
	}
	for _, item := range state.Items {
		if item.Phase != s.Queued() {
			t.Fatalf("active item survived pause: %+v", item)
		}
	}
	h.click("Refresh library")
	h.wait(func() bool { return !h.w.loading && len(h.w.jobs) == 0 })
	if !h.screen.HasText("Resume") || h.w.jobs[first] != nil {
		t.Fatal("automatic scan cleared the session pause")
	}
	h.control(scenario, "resume-paused")
	h.click("Resume")
	h.wait(
		func() bool { return len(h.w.jobs) == 0 && !h.w.loading && h.w.reports[first].Skipped == 32 },
	)
	h.click("Pause")
	if !h.w.paused[first] {
		t.Fatal("idle podcast cannot be paused")
	}
}

func TestLiveVideoCreationProgress(t *testing.T) {
	t.Parallel()
	h, scenario := liveDesktop(t, false)
	h.beginImport(scenario.Source, scenario.Team, p.ShowSourceKindVideo)
	h.wait(func() bool { return h.w.preparation.Scan != nil && h.w.preparation.Scan.Videos == 2 })
	if len(h.w.catalog.Shows) != 0 || h.w.preparation.Scan.EstimatedSeconds != 900 ||
		h.w.preparation.Scan.UnknownDurations != 1 {
		t.Fatal("video creation did not wait for the complete deduplicated scan")
	}
	for _, dark := range []bool{false, true} {
		h.screen.SetDark(dark)
		for _, width := range []int{840, 1080} {
			h.screen.SetSize(width, 760)
			if !h.screen.HasText("2 videos · 15:00 estimated") {
				t.Fatal("preparation progress is missing")
			}
		}
	}
	h.control(scenario, "release-scan")
	h.control(scenario, "creation-requested")
	h.wait(func() bool { return h.w.preparation.Stage == "creating_podcast" })
	if h.w.preparation.Scan == nil || h.w.preparation.Scan.Videos != 3 ||
		len(h.w.catalog.Shows) != 0 {
		t.Fatal("creation dropped the final scan or skipped its acknowledgement")
	}
	h.control(scenario, "release-creation")
	h.importFinished()
	if len(h.w.catalog.Shows) != 1 {
		t.Fatal("video import did not select its new podcast")
	}
}

func TestLiveTeamImport(t *testing.T) {
	t.Parallel()
	h, scenario := liveDesktop(t, false)
	if len(h.w.catalog.Teams) < 2 {
		t.Fatal("team fixture is not ambiguous")
	}
	h.beginImport(scenario.Source, scenario.Team, p.ShowSourceKindAudio)
	h.importFinished()
	if h.w.show() == nil || h.w.show().TeamId != scenario.Team {
		t.Fatal("import ignored the chosen writable destination")
	}
}

func TestLiveImport(t *testing.T) {
	t.Parallel()
	h, scenario := liveDesktop(t, false)
	h.beginImport(scenario.Source, scenario.Team, p.ShowSourceKindAudio)
	h.importFinished()
	if len(h.w.catalog.Shows) != 1 || value(h.w.show().Youtube.Url) != scenario.Source ||
		h.w.reports[h.w.selected].Added != 1 {
		t.Fatal("playlist import did not finish in its podcast")
	}
	original := *h.w.show()
	h.beginImport(scenario.Source, scenario.Team, p.ShowSourceKindVideo)
	h.importFinished()
	if len(h.w.catalog.Shows) != 1 || h.w.show().Id != original.Id ||
		h.w.show().SourceKind != original.SourceKind {
		t.Fatal("repeat import created a second podcast or changed its media kind")
	}
	h.click("Sync now")
	h.wait(func() bool { return len(h.w.jobs) == 0 && !h.w.loading })
	if h.w.reports[h.w.selected].Added != 0 {
		t.Fatal("manual sync duplicated imported media")
	}
}

func TestLiveImportPaymentRequired(t *testing.T) {
	t.Parallel()
	h, scenario := liveDesktop(t, false)
	h.beginImport(scenario.Source, scenario.Team, p.ShowSourceKindVideo)
	h.wait(func() bool { return !h.w.creating && !h.w.loading })
	if h.w.errorText == "" || len(h.w.catalog.Shows) != 0 || h.w.importSource != scenario.Source {
		t.Fatal("payment refusal discarded the draft or created a podcast")
	}
	h.click("Upgrade plan")
	opened := h.screen.OpenedURLs()
	want, _ := h.w.client.Config.UpgradeURL(scenario.Team)
	if len(opened) == 0 || opened[len(opened)-1] != want+"?family=video_hd" {
		t.Fatalf("upgrade action: %v", opened)
	}
	h.click("YouTube URL")
	h.screen.Key(ui.Cmd, ui.KeyA)
	h.screen.Type("invalid")
	h.click("Import")
	h.wait(func() bool { return !h.w.creating })
	if h.w.upgradeURL != "" {
		t.Fatal("validation retained a previous payment action")
	}
}

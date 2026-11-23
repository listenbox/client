package main

import (
	"context"
	"fmt"
	"image/png"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/egoist/mygo/ui"
	s "github.com/listenbox/client/crates/sync-engine"
	p "github.com/listenbox/client/crates/sync-engine/publicapi"
)

type desktopHarness struct {
	t        *testing.T
	w        *Workspace
	screen   *ui.Tester
	messages chan func()
}

const firstPodcastSlug = "podcast-0"

func desktopNoError(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatal(err)
	}
}

func desktopTest(t *testing.T, config s.Config, start bool) *desktopHarness {
	t.Helper()
	h := &desktopHarness{t: t, messages: make(chan func(), 4096)}
	h.w = newWorkspace(s.NewClient(config), func(apply func()) { h.messages <- apply }, nil)
	if !start {
		h.w.started = true
		h.w.loaded = true
	}
	t.Cleanup(func() { h.w.cancel(); _ = h.w.tasks.Wait(); _ = h.w.client.Close() })
	h.screen = ui.NewTester(h.w.view, 1080, 760)
	return h
}

func isolatedDesktop(t *testing.T) *desktopHarness {
	t.Helper()
	config, err := s.LoadConfigIn("", t.TempDir())
	desktopNoError(t, err)
	return desktopTest(t, config, false)
}

func (h *desktopHarness) click(label string) {
	h.t.Helper()
	desktopNoError(h.t, h.screen.Click(label))
}

// logout signs out from the team switcher, as the Dashboard does.
func (h *desktopHarness) logout() {
	h.t.Helper()
	h.click("Switch team")
	h.click("Log out")
}

// stageGuard bounds one observable stage of a desktop workflow.
const stageGuard = 3 * time.Second

func (h *desktopHarness) wait(condition func() bool) {
	h.t.Helper()
	h.waitStages(func() string { return "" }, condition)
}

// waitStages applies UI messages until condition holds. Each change in the
// reported stage restarts the guard, so a loaded host may slow every stage
// while any single stalled stage still fails within stageGuard. Progress ticks
// inside one stage never restart it.
func (h *desktopHarness) waitStages(stage func() string, condition func() bool) {
	h.t.Helper()
	guard := time.NewTimer(stageGuard)
	defer guard.Stop()
	current := stage()
	for !condition() {
		select {
		case apply := <-h.messages:
			apply()
			h.screen.Frame()
			if next := stage(); next != current {
				current = next
				guard.Reset(stageGuard)
			}
		case <-guard.C:
			h.t.Fatalf(
				"desktop state gate was not reached: stage=%q error=%s texts=%q",
				current,
				h.w.errorText,
				h.screen.Texts(),
			)
		case <-h.t.Context().Done():
			h.t.Fatal(h.t.Context().Err())
		}
	}
	h.screen.Frame()
}

func libraryDesktop(t *testing.T) *desktopHarness {
	t.Helper()
	h := isolatedDesktop(t)
	h.w.authorized = true
	for index := 1; index <= 3; index++ {
		h.w.catalog.Teams = append(
			h.w.catalog.Teams,
			p.ClientTeam{
				Id:   fmt.Sprintf("team_%016x", index),
				Name: fmt.Sprintf("Library %d", index),
			},
		)
	}
	h.w.catalog.ImportTeam = h.w.catalog.Teams[0].Id
	for index := range 8 {
		team := 0
		if index == 3 {
			team = 1
		}
		source := "https://www.youtube.com/playlist?list=PLlibrary"
		h.w.catalog.Shows = append(
			h.w.catalog.Shows,
			p.Show{
				Id:         fmt.Sprintf("shw_%016x", index),
				TeamId:     h.w.catalog.Teams[team].Id,
				Title:      fmt.Sprintf("Podcast %d", index),
				Slug:       fmt.Sprintf("podcast-%d", index),
				Language:   "en",
				SourceKind: p.ShowSourceKindAudio,
				Youtube:    p.YouTubeLinkage{Url: &source},
			},
		)
	}
	h.w.selected = firstPodcastSlug
	// Layout cases use the same actor with cancelled I/O, as the incumbent
	// headless fixtures did. Service behavior is covered by the live cases.
	h.w.cancel()
	h.screen.Frame()
	return h
}

func TestStartupRendersWhileCredentialsAreBlocked(t *testing.T) {
	t.Parallel()
	if runtime.GOOS == "windows" {
		t.Skip("the incumbent startup regression uses a Unix FIFO")
	}
	directory := t.TempDir()
	auth := filepath.Join(directory, "auth.json")
	desktopNoError(t, credentialFIFO(auth))
	painted := make(chan struct{})
	rendered := make(chan bool, 1)
	go func() {
		observed := false
		select {
		case <-painted:
			observed = true
		case <-time.After(time.Second):
		}
		err := os.WriteFile(auth, []byte("{}"), 0o600)
		rendered <- observed && err == nil
	}()
	config, err := s.LoadConfigIn("", directory)
	desktopNoError(t, err)
	h := desktopTest(t, config, true)
	h.screen.Frame()
	close(painted)
	if !<-rendered {
		t.Fatal("credential I/O blocked the first desktop frame")
	}
	h.wait(func() bool { return h.w.loaded })
}

func TestSidebarResizesWithoutResettingOnNavigation(t *testing.T) {
	t.Parallel()
	h := libraryDesktop(t)
	x := h.w.sidebarWidth
	h.screen.Press(x+2, 400)
	h.screen.Move(x+122, 400)
	h.screen.Release(x+122, 400)
	h.screen.Frame()
	widened := h.w.sidebarWidth
	if widened < x+80 {
		t.Fatalf("divider did not widen: %f", widened)
	}
	h.click("Import from YouTube")
	if h.w.sidebarWidth != widened || !h.w.importOpen {
		t.Fatal("navigation reset the sidebar")
	}
	h.screen.SetSize(840, 600)
	h.screen.Press(widened+2, 400)
	h.screen.Move(1200, 400)
	h.screen.Release(1200, 400)
	h.screen.Frame()
	if h.w.sidebarWidth > maximumSidebar || h.w.sidebarWidth < minimumSidebar {
		t.Fatal("sidebar constraints", h.w.sidebarWidth)
	}
	if directory := os.Getenv("LISTENBOX_UI_REVIEW_DIR"); directory != "" {
		// #nosec G703 -- The test owns the configured temporary profile and artifact paths.
		desktopNoError(t, os.MkdirAll(directory, 0o700))
		h.w.importOpen = false
		h.screen.SetSize(1080, 760)
		for _, dark := range []bool{false, true} {
			h.screen.SetDark(dark)
			// #nosec G703 -- The test owns the configured temporary profile and artifact paths.
			file, err := os.Create(
				filepath.Join(directory, fmt.Sprintf("workspace-dark-%t.png", dark)),
			)
			desktopNoError(t, err)
			desktopNoError(t, png.Encode(file, h.screen.Image()))
			desktopNoError(t, file.Close())
		}
	}
}

func TestTeamPickerScrollsToTheLastTeamWithoutMovingTheLibrary(t *testing.T) {
	t.Parallel()
	h := libraryDesktop(t)
	for index := 4; index <= 32; index++ {
		h.w.catalog.Teams = append(
			h.w.catalog.Teams,
			p.ClientTeam{
				Id:   fmt.Sprintf("team_%016x", index),
				Name: fmt.Sprintf("Library %d with a long name", index),
			},
		)
	}
	h.w.catalog.Shows[3].TeamId = h.w.catalog.Teams[31].Id
	h.screen.Frame()
	h.click("Switch team")
	before, ok := h.screen.Find("Import from YouTube")
	if !ok {
		t.Fatal("missing import control")
	}
	list, ok := h.screen.Find("Team list")
	if !ok {
		t.Fatal("missing team list")
	}
	h.screen.Scroll(list.X+list.W/2, list.Y+list.H/2, 0, 2000)
	h.screen.Frame()
	h.click("Library 32 with a long name")
	if h.w.team != h.w.catalog.Teams[31].Id || h.w.selected != "podcast-3" {
		t.Fatal("last team was not selectable", h.w.team, h.w.selected)
	}
	after, ok := h.screen.Find("Import from YouTube")
	if !ok || before != after {
		t.Fatal("team picker moved the library")
	}
	h.click("Switch team")
	h.w.teamList.ScrollTo(0, ui.Start)
	h.screen.Frame()
	h.click("All teams")
	if h.w.team != "" {
		t.Fatal("All teams did not clear the filter")
	}
}

func TestTeamSwitcherShowsEachTeamsPlan(t *testing.T) {
	t.Parallel()
	h := libraryDesktop(t)
	for index, family := range []p.TeamPlanFamily{
		p.TeamPlanFamilyFree, p.TeamPlanFamilyAudio, p.TeamPlanFamilyVideo4k,
	} {
		h.w.catalog.Teams[index].PlanFamily = &family
	}
	shows := func(labels ...string) bool {
		texts := h.screen.Texts()
		for _, label := range labels {
			if !slices.Contains(texts, label) {
				return false
			}
		}
		return true
	}
	h.screen.Frame()
	if shows("Free") || shows("Audio") || shows("Video") {
		t.Fatalf("All teams shows a plan: %q", h.screen.Texts())
	}
	h.click("Switch team")
	if !shows("Free", "Audio", "Video", "4K") {
		t.Fatalf("team menu misses a plan: %q", h.screen.Texts())
	}
	h.click("Library 2")
	h.screen.Frame()
	if !shows("Audio") || shows("Free") || shows("Video") {
		t.Fatalf("switcher misses the selected team's plan: %q", h.screen.Texts())
	}
}

func TestTeamSwitcherCreatesATeamInTheDashboard(t *testing.T) {
	t.Parallel()
	h := libraryDesktop(t)
	team := h.w.catalog.Teams[1]
	h.w.team = team.Id
	h.screen.Frame()
	h.click("Switch team")
	h.click("Create team")
	urls := h.screen.OpenedURLs()
	if len(urls) != 1 || !strings.HasSuffix(urls[0], "/"+team.Id+"/teams/new") {
		t.Fatal("Create team did not open the selected team's form", urls)
	}
	if h.w.teamPicker {
		t.Fatal("Create team left the switcher open")
	}
}

func TestOptionArrowsNavigateVisiblePodcastsFromFocusedControls(t *testing.T) {
	t.Parallel()
	h := libraryDesktop(t)
	request := h.w.episodeRequest
	h.screen.Key(ui.Alt, ui.KeyUp)
	if h.w.selected != firstPodcastSlug || h.w.episodeRequest != request {
		t.Fatal("Up at the first podcast reloaded it")
	}
	h.screen.Key(ui.Alt|ui.Shift, ui.KeyDown)
	if h.w.selected != firstPodcastSlug {
		t.Fatal("another shortcut navigated")
	}
	h.click("Import from YouTube")
	h.click("YouTube URL")
	h.screen.Key(ui.Alt, ui.KeyDown)
	if h.w.selected != "podcast-1" || h.w.importOpen {
		t.Fatal("focused input swallowed navigation", h.w.selected, h.w.importOpen)
	}
	for index := 2; index < 8; index++ {
		h.screen.Key(ui.Alt, ui.KeyDown)
		if h.w.selected != fmt.Sprintf("podcast-%d", index) {
			t.Fatal(h.w.selected)
		}
	}
	request = h.w.episodeRequest
	h.screen.Key(ui.Alt, ui.KeyDown)
	if h.w.episodeRequest != request {
		t.Fatal("Down at the end reloaded it")
	}
	h.w.team = h.w.catalog.Teams[1].Id
	h.w.selectShow("podcast-3")
	h.screen.Frame()
	h.screen.Key(ui.Alt, ui.KeyDown)
	if h.w.selected != "podcast-3" {
		t.Fatal("navigation crossed the team filter")
	}
	h.click("Manage podcast")
	urls := h.screen.OpenedURLs()
	if len(urls) == 0 ||
		!strings.HasSuffix(
			urls[len(urls)-1],
			"/"+h.w.catalog.Teams[1].Id+"/shows/"+h.w.catalog.Shows[3].Id,
		) {
		t.Fatal("management link did not follow selection", urls)
	}
}

func TestPublishedEpisodeTitleOpensItsYouTubeVideo(t *testing.T) {
	t.Parallel()
	h := libraryDesktop(t)
	title := "דף יומי: ערכין ד' - " + t.Name()
	video := "https://www.youtube.com/watch?v=" + strings.ToLower(t.Name())
	h.w.episodes = []p.EpisodeListItem{{
		Id:         "ep_" + t.Name(),
		Title:      title,
		Status:     p.EpisodeStatusPublished,
		YoutubeUrl: &video,
	}}
	h.screen.Frame()
	h.click("Open " + title + " on YouTube")
	if urls := h.screen.OpenedURLs(); len(urls) != 1 || urls[0] != video {
		t.Fatal("episode title did not open its video", urls)
	}
}

func TestSignInCancelRemainsClickableInANarrowWindow(t *testing.T) {
	t.Parallel()
	h := isolatedDesktop(t)
	ctx, cancel := context.WithCancel(h.w.ctx)
	defer cancel()
	h.w.signingIn, h.w.loginCancel = true, cancel
	h.screen.SetSize(480, 600)
	rect, ok := h.screen.Find("Cancel sign in")
	if !ok || rect.X < 0 || rect.X+rect.W > 480 {
		t.Fatalf("Cancel is outside the narrow window: %+v", rect)
	}
	h.click("Cancel sign in")
	if ctx.Err() == nil || h.w.signingIn {
		t.Fatal("Cancel did not cancel the authorization owner")
	}
}

func TestUnpaidPodcastOpensItsTeamsUpgradePage(t *testing.T) {
	t.Parallel()
	h := libraryDesktop(t)
	h.w.catalog.Shows[0].TeamId = "team_fedcba9876543210"
	h.screen.Frame()
	h.click("Choose a plan")
	urls := h.screen.OpenedURLs()
	if len(urls) != 1 || urls[0] != "https://web.listenbox.app/team_fedcba9876543210/upgrade" {
		t.Fatal(urls)
	}
}

func admittedDesktopWork(t *testing.T, h *desktopHarness) (chan struct{}, chan struct{}) {
	t.Helper()
	started, cancelled, commit := make(chan struct{}), make(chan struct{}), make(chan struct{})
	h.w.launch(
		func(ctx context.Context) func() { close(started); <-ctx.Done(); close(cancelled); <-commit; return nil },
	)
	select {
	case <-started:
	case <-time.After(time.Second):
		t.Fatal("work was not admitted")
	}
	return cancelled, commit
}

func TestConfirmedQuitWaitsForAdmittedWorkToDrain(t *testing.T) {
	t.Parallel()
	h := isolatedDesktop(t)
	cancelled, commit := admittedDesktopWork(t, h)
	defer func() {
		select {
		case <-commit:
		default:
			close(commit)
		}
	}()
	h.w.stop(false)
	h.screen.Frame()
	select {
	case <-cancelled:
	case <-time.After(time.Second):
		t.Fatal("Quit did not cancel admitted work")
	}
	if !h.w.stopping || h.w.terminated || !h.screen.HasText("Finishing up…") {
		t.Fatal("Quit did not wait for the commit")
	}
	close(commit)
	h.wait(func() bool { return h.w.terminated })
}

func TestLogoutWaitsForAdmittedWorkToDrain(t *testing.T) {
	t.Parallel()
	h := isolatedDesktop(t)
	cancelled, commit := admittedDesktopWork(t, h)
	defer func() {
		select {
		case <-commit:
		default:
			close(commit)
		}
	}()
	h.w.stop(true)
	select {
	case <-cancelled:
	case <-time.After(time.Second):
		t.Fatal("logout did not cancel admitted work")
	}
	if !h.w.stopping {
		t.Fatal("logout did not wait for the owned commit")
	}
	close(commit)
	h.wait(func() bool { return !h.w.stopping })
	if h.w.client.HasCredentials() || len(h.w.catalog.Shows) > 0 {
		t.Fatal("logout retained account state")
	}
	if _, err := os.Stat(
		filepath.Join(h.w.client.Config.Directory, "sync.sqlite"),
	); !os.IsNotExist(
		err,
	) {
		t.Fatal("authentication opened the journal")
	}
}

func TestLibrarySharesArtworkRequests(t *testing.T) {
	t.Parallel()
	h := isolatedDesktop(t)
	var requests atomic.Int64
	entered, release := make(chan struct{}, 1), make(chan struct{})
	raw, err := os.ReadFile("tests/fixtures/podcast.png")
	desktopNoError(t, err)
	server := httptest.NewServer(
		http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
			requests.Add(1)
			select {
			case entered <- struct{}{}:
			default:
			}
			select {
			case <-release:
				writer.Header().Set("Content-Type", "image/png")
				_, _ = writer.Write(raw)
			case <-request.Context().Done():
			}
		}),
	)
	defer func() {
		h.w.cancel()
		_ = h.w.tasks.Wait()
		server.Close()
	}()
	for range 12 {
		h.w.cover(server.URL)
	}
	select {
	case <-entered:
	case <-time.After(time.Second):
		t.Fatal("artwork request did not start")
	}
	if requests.Load() != 1 {
		t.Fatal("library duplicated artwork requests", requests.Load())
	}
	close(release)
	h.wait(func() bool { return h.w.artwork[server.URL] != nil })
	h.w.clearArtwork()
	for range 12 {
		h.w.cover(server.URL)
	}
	h.wait(func() bool { return h.w.artwork[server.URL] != nil })
	if requests.Load() != 2 {
		t.Fatal("reload did not refresh stale artwork", requests.Load())
	}
}

func TestYoutubeSettingsValidateSaveForgetAndLinkToGuide(t *testing.T) {
	t.Parallel()
	h := isolatedDesktop(t)
	h.w.authorized = true
	h.w.openSettings()
	h.wait(func() bool { return !h.w.cookieBusy })
	h.click("How to export YouTube cookies")
	if urls := h.screen.OpenedURLs(); len(urls) != 1 || urls[0] != s.CookieGuideURL {
		t.Fatal(urls)
	}
	h.click("YouTube cookies")
	h.screen.Type("private-value")
	h.click("Save cookies")
	h.wait(func() bool { return !h.w.cookieBusy })
	if h.w.cookieError == "" || strings.Contains(h.w.cookieError, "private-value") {
		t.Fatal("invalid export was accepted or disclosed a value")
	}
	h.w.cookieInput = "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\tprivate-value\n"
	h.screen.Frame()
	h.click("Save cookies")
	h.wait(func() bool { return !h.w.cookieBusy })
	if !h.w.cookieSaved || h.w.cookieInput != "" || h.w.cookieError != "" {
		t.Fatal("valid export was not saved")
	}
	h.click("YouTube cookies")
	h.screen.Key(ui.Cmd, ui.KeyZ)
	if strings.Contains(h.w.cookieInput, "private-value") {
		t.Fatal("Undo recovered saved secrets")
	}
	h.click("Remove cookies")
	h.wait(func() bool { return !h.w.cookieBusy })
	enabled, err := h.w.client.CookieJar().Enabled()
	desktopNoError(t, err)
	if enabled || h.w.cookieSaved {
		t.Fatal("removal retained YouTube credentials")
	}
}

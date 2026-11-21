package main

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/egoist/mygo/ui"
	s "github.com/listenbox/client/crates/sync-engine"
	p "github.com/listenbox/client/crates/sync-engine/publicapi"
)

// fakeListenbox serves the library endpoints for shows, each with one episode
// titled after the show. While blocked, or for a sync inventory, it holds the
// request open until the client gives up.
type fakeListenbox struct {
	config  s.Config
	blocked atomic.Bool
}

func newFakeListenbox(t *testing.T, shows []p.Show) *fakeListenbox {
	t.Helper()
	team := p.ClientTeam{Id: "team_" + strings.Repeat("a", 16), Name: "Library"}
	fake := &fakeListenbox{}
	reply := func(writer http.ResponseWriter, value any) {
		writer.Header().Set("Content-Type", "application/json")
		if err := json.NewEncoder(writer).Encode(value); err != nil {
			t.Error(err)
		}
	}
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		path := request.URL.Path
		if fake.blocked.Load() || strings.HasSuffix(path, "/sync") {
			<-request.Context().Done()
			return
		}
		switch {
		case path == "/s/teams":
			reply(writer, []p.ClientTeam{team})
		case path == "/s/shows":
			reply(writer, shows)
		case path == "/s/whoami":
			reply(writer, p.APIKeyWhoami{Email: "host@example.com", TeamId: team.Id})
		case strings.HasSuffix(path, "/episodes"):
			slug := strings.TrimSuffix(strings.TrimPrefix(path, "/s/shows/"), "/episodes")
			reply(writer, p.EpisodePage{Episodes: []p.EpisodeListItem{{
				Id: "ep_" + slug, Title: slug + " episode", Status: p.EpisodeStatusPublished,
			}}})
		default:
			http.NotFound(writer, request)
		}
	}))
	t.Cleanup(server.Close)
	directory := t.TempDir()
	desktopNoError(t, os.WriteFile(
		filepath.Join(directory, "auth.json"),
		[]byte(`{"api_key":"lb_test_`+strings.ToLower(t.Name())+`"}`),
		0o600,
	))
	config, err := s.LoadConfigIn("", directory)
	desktopNoError(t, err)
	config.APIOrigin, config.DashboardOrigin = server.URL, server.URL
	fake.config = config
	return fake
}

func syncedShow(slug string, checked *time.Time) p.Show {
	source := "https://www.youtube.com/playlist?list=PL" + slug
	show := p.Show{
		Id: "shw_" + slug, TeamId: "team_" + strings.Repeat("a", 16), Title: slug, Slug: slug,
		Language: "en", SourceKind: p.ShowSourceKindAudio, HasActiveSubscription: true,
		Youtube: p.YouTubeLinkage{Url: &source},
	}
	if checked != nil {
		millis := checked.UnixMilli()
		show.Youtube.LastCheckedAt = &millis
	}
	return show
}

func (h *desktopHarness) hasTextPrefix(prefix string) bool {
	return slices.ContainsFunc(h.screen.Texts(), func(text string) bool { return strings.HasPrefix(text, prefix) })
}

func TestRestartRendersCachedPodcastsWhileListenboxIsChecked(t *testing.T) {
	t.Parallel()
	checked := time.Now().Add(-5 * time.Minute)
	listenbox := newFakeListenbox(t, []p.Show{
		syncedShow("podcast-a", &checked),
		syncedShow("podcast-b", &checked),
	})
	first := desktopTest(t, listenbox.config, true)
	shows := func(h *desktopHarness, title string) func() bool {
		return func() bool {
			return !h.w.loading && len(h.w.episodes) == 1 && h.w.episodes[0].Title == title
		}
	}
	first.wait(shows(first, "podcast-a episode"))
	first.screen.Key(ui.Alt, ui.KeyDown)
	first.wait(shows(first, "podcast-b episode"))
	first.w.cancel()
	_ = first.w.tasks.Wait()
	desktopNoError(t, first.w.client.Close())

	listenbox.blocked.Store(true)
	second := desktopTest(t, listenbox.config, true)
	second.wait(func() bool { return len(second.w.episodes) == 1 })
	second.screen.Key(ui.Alt, ui.KeyDown)
	second.wait(func() bool { return second.w.selected == "podcast-b" && len(second.w.episodes) == 1 })
	if !second.screen.HasText("podcast-b episode") || !second.hasTextPrefix("Last synced ") ||
		!second.screen.HasText("Checking Listenbox…") || second.hasTextPrefix("Not synced") {
		t.Fatalf("switching podcasts did not render the cache while checking: %q", second.screen.Texts())
	}
}

func TestLaunchSyncsOnlyPodcastsDueForASync(t *testing.T) {
	t.Parallel()
	recent, stale := time.Now().Add(-5*time.Minute), time.Now().Add(-2*time.Hour)
	listenbox := newFakeListenbox(t, []p.Show{
		syncedShow("podcast-recent", &recent),
		syncedShow("podcast-stale", &stale),
		syncedShow("podcast-new", nil),
	})
	h := desktopTest(t, listenbox.config, true)
	h.wait(func() bool { return len(h.w.jobs) >= 2 })
	h.screen.Frame()
	if len(h.w.jobs) != 2 || h.w.jobs["podcast-stale"] == nil || h.w.jobs["podcast-new"] == nil {
		t.Fatalf("launch synced the wrong podcasts: %v", h.w.jobs)
	}
}

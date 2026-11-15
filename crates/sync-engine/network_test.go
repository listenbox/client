package syncengine

import (
	"context"
	"encoding/json"
	"errors"
	"net"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/jonboulle/clockwork"
	p "github.com/listenbox/client/crates/sync-engine/publicapi"
	"golang.org/x/sync/errgroup"
)

var fixtureInvocation bool

const dnsFailureScenario = "dns"

func TestMain(m *testing.M) {
	aliases := map[string]string{
		"live_catalog_transport":   "TestLiveCatalogTransport",
		"live_download_adaptation": "TestLiveDownloadAdaptation",
		"live_pipeline_stall":      "TestLivePipelineStall",
		"live_upload_idle_timeout": "TestLiveUploadIdleTimeout",
		"live_network_recovery":    "TestLiveNetworkRecovery",
	}
	if len(os.Args) > 1 {
		if name, ok := aliases[os.Args[1]]; ok {
			fixtureInvocation = true
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

func liveConfig(t *testing.T) Config {
	t.Helper()
	if !fixtureInvocation {
		t.Skip("invoked by the parent API's isolated E2E fixture")
	}
	config, err := LoadConfig("")
	requireNoError(t, err)
	return config
}

type pipelineSettings struct{ Slug, Control, Prelude, Mock string }

func liveSettings(t *testing.T, config Config, file string, out any) {
	t.Helper()
	raw, err := os.ReadFile(filepath.Join(config.Directory, file))
	requireNoError(t, err)
	requireNoError(t, json.Unmarshal(raw, out))
}

func TestLiveCatalogTransport(t *testing.T) {
	t.Parallel()
	client := NewClient(liveConfig(t))
	defer client.Close()
	first, err := client.Catalog(t.Context())
	requireNoError(t, err)
	if len(first.Teams) == 0 {
		t.Fatal("empty team catalog")
	}
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	if _, err := client.Catalog(ctx); !errors.Is(err, context.Canceled) {
		t.Fatal("catalog ignored cancellation", err)
	}
	for range 2 {
		next, err := client.Catalog(t.Context())
		requireNoError(t, err)
		if next.Teams[0].Id != first.Teams[0].Id {
			t.Fatal("catalog changed identity")
		}
	}
	requireNoError(t, client.Logout())
	if _, err := client.Catalog(t.Context()); !errors.Is(err, ErrAuthentication) {
		t.Fatal("logout retained catalog credentials", err)
	}
}

func testAPI(t *testing.T, config Config) *API {
	t.Helper()
	httpClient, upload, err := NewHTTPClients()
	requireNoError(t, err)
	t.Cleanup(func() { httpClient.CloseIdleConnections(); upload.CloseIdleConnections() })
	api, err := NewAPIWithClients(config, httpClient, upload)
	requireNoError(t, err)
	return api
}

func waitDownloads(t *testing.T, manager *DownloadManager, condition func([]Download) bool) {
	t.Helper()
	guard := time.NewTimer(3 * time.Second)
	defer guard.Stop()
	for {
		changed := manager.Changes()
		snapshot := manager.Snapshot()
		if condition(snapshot) {
			return
		}
		select {
		case <-changed:
		case <-guard.C:
			t.Fatalf("transfer gate was not reached: %+v", snapshot)
		case <-t.Context().Done():
			t.Fatal(t.Context().Err())
		}
	}
}

func phaseCount(items []Download, phase Phase) int {
	count := 0
	for _, item := range items {
		if item.Phase == phase {
			count++
		}
	}
	return count
}

func controlRequest(ctx context.Context, api *API, method, target string, body any) error {
	var request *http.Request
	var err error
	if body == nil {
		request, err = http.NewRequestWithContext(ctx, method, target, nil)
	} else {
		raw, e := json.Marshal(body)
		if e != nil {
			return e
		}
		request, err = http.NewRequestWithContext(
			ctx,
			method,
			target,
			strings.NewReader(string(raw)),
		)
		if err == nil {
			request.Header.Set("Content-Type", "application/json")
		}
	}
	if err != nil {
		return err
	}
	response, err := api.Do(request)
	if err != nil {
		return err
	}
	response.Body.Close()
	return nil
}

func advanceNetworkSample(t *testing.T, clock *clockwork.FakeClock, manager *DownloadManager) {
	t.Helper()
	changed := manager.Changes()
	clock.Advance(time.Second)
	for {
		manager.mu.Lock()
		sampled := manager.sampledAt
		manager.mu.Unlock()
		if !sampled.Before(clock.Now()) {
			return
		}
		select {
		case <-changed:
			changed = manager.Changes()
		case <-time.After(2 * time.Second):
			t.Fatal("production network sampler did not consume its clock tick")
		}
	}
}

//nolint:cyclop,funlen // Gate all admitted pipeline stages and verify cancellation and checkpoint cleanup together.
func TestLivePipelineStall(t *testing.T) {
	t.Parallel()
	config := liveConfig(t)
	var settings pipelineSettings
	liveSettings(t, config, "test-pipeline.json", &settings)
	api := testAPI(t, config)
	clock := clockwork.NewFakeClock()
	engine := NewEngine(clock)
	defer engine.Close()
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	done := make(chan error, 1)
	var report Report
	go func() { var err error; report, err = engine.Once(ctx, api, settings.Slug); done <- err }()
	waitDownloads(t, engine.Downloads, func(items []Download) bool {
		return phaseCount(items, Uploading()) == 2 && phaseCount(items, WaitingToUpload()) == 1
	})
	requireNoError(t, controlRequest(ctx, api, "GET", settings.Control+"/ready", nil))
	initial, _ := func() (int, int) { d, u := engine.Downloads.Limits(); return u, d }()
	for range 12 {
		advanceNetworkSample(t, clock, engine.Downloads)
	}
	downloads, uploads := engine.Downloads.Limits()
	if uploads >= initial || downloads != 2 {
		t.Fatalf("independent budgets: download=%d upload=%d", downloads, uploads)
	}
	items := engine.Downloads.Snapshot()
	if phaseCount(items, Uploading()) != 2 || phaseCount(items, WaitingToUpload()) != 1 {
		t.Fatal("backoff displaced admitted work", items)
	}
	requireNoError(t, controlRequest(ctx, api, "GET", settings.Control+"/release/0", nil))
	waitDownloads(
		t,
		engine.Downloads,
		func(items []Download) bool { return phaseCount(items, Complete()) >= 1 },
	)
	items = engine.Downloads.Snapshot()
	if phaseCount(items, Uploading()) != 1 || phaseCount(items, WaitingToUpload()) != 1 {
		t.Fatal("new work bypassed the reduced budget", items)
	}
	requireNoError(t, controlRequest(ctx, api, "GET", settings.Control+"/release/1", nil))
	select {
	case err := <-done:
		requireNoError(t, err)
	case <-time.After(3 * time.Second):
		t.Fatal("released pipeline did not finish")
	}
	if report.Added != 3 || report.Skipped != 0 {
		t.Fatal(report)
	}
	for _, item := range engine.Downloads.Snapshot() {
		if item.Phase != Complete() || item.Attempt != 1 {
			t.Fatal(item)
		}
	}
}

func downloadingGate(t *testing.T, manager *DownloadManager, count int) {
	t.Helper()
	waitDownloads(t, manager, func(items []Download) bool {
		if phaseCount(items, Downloading()) != count {
			return false
		}
		for _, item := range items {
			if item.Phase != Downloading() {
				continue
			}
			if len(item.Ranges) == 0 {
				return false
			}
			for _, r := range item.Ranges {
				if r.Start > 0 && !r.Complete {
					return false
				}
			}
		}
		return true
	})
}

func receiveWindow(
	t *testing.T,
	api *API,
	mock string,
	clock *clockwork.FakeClock,
	manager *DownloadManager,
	credit int64,
) {
	t.Helper()
	before := make(map[string]int64)
	for _, item := range manager.Snapshot() {
		if item.Phase == Downloading() {
			before[item.SourceURL] = item.Received
		}
	}
	if len(before) == 0 {
		t.Fatal("throughput window has no admitted receiver")
	}
	var group errgroup.Group
	for source := range before {
		group.Go(func() error {
			return controlRequest(
				t.Context(),
				api,
				"POST",
				mock+"/__mock__/media-window",
				map[string]any{
					"context": strings.TrimPrefix(source, "https://www.youtube.com/watch?v="),
					"bytes":   credit,
				},
			)
		})
	}
	requireNoError(t, group.Wait())
	waitDownloads(t, manager, func(items []Download) bool {
		received := make(map[string]int64)
		for _, item := range items {
			received[item.SourceURL] = item.Received
		}
		for source, value := range before {
			if received[source] < value+credit {
				return false
			}
		}
		return true
	})
	advanceNetworkSample(t, clock, manager)
}

//nolint:cyclop,funlen // Verify the successive controlled network observations and resulting stage limits.
func TestLiveDownloadAdaptation(t *testing.T) {
	t.Parallel()
	config := liveConfig(t)
	var settings pipelineSettings
	liveSettings(t, config, "test-pipeline.json", &settings)
	api := testAPI(t, config)
	clock := clockwork.NewFakeClock()
	engine := NewEngine(clock)
	defer engine.Close()
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	var group errgroup.Group
	group.Go(func() error { _, err := engine.Once(ctx, api, settings.Prelude); return err })
	waitDownloads(
		t,
		engine.Downloads,
		func(items []Download) bool { return phaseCount(items, Uploading()) == 2 },
	)
	group.Go(func() error { _, err := engine.Once(ctx, api, settings.Slug); return err })
	waitDownloads(t, engine.Downloads, func(items []Download) bool {
		if phaseCount(items, Uploading()) != 2 {
			return false
		}
		for _, item := range items {
			if item.Phase == Uploading() &&
				(item.UploadTotal == 0 || item.Uploaded != item.UploadTotal) {
				return false
			}
		}
		return true
	})
	downloadingGate(t, engine.Downloads, 2)
	advanceNetworkSample(t, clock, engine.Downloads)
	receiveWindow(t, api, settings.Mock, clock, engine.Downloads, 1024)
	downloadingGate(t, engine.Downloads, 3)
	advanceNetworkSample(t, clock, engine.Downloads)
	receiveWindow(t, api, settings.Mock, clock, engine.Downloads, 1024)
	downloads, _ := engine.Downloads.Limits()
	if downloads != 3 {
		t.Fatalf("useful throughput growth was rolled back: %d", downloads)
	}
	receiveWindow(t, api, settings.Mock, clock, engine.Downloads, 1024)
	downloadingGate(t, engine.Downloads, 4)
	advanceNetworkSample(t, clock, engine.Downloads)
	receiveWindow(t, api, settings.Mock, clock, engine.Downloads, 768)
	downloads, _ = engine.Downloads.Limits()
	if downloads != 3 {
		t.Fatalf("throughput plateau retained an extra connection: %d", downloads)
	}
	for range 10 {
		advanceNetworkSample(t, clock, engine.Downloads)
	}
	downloads, uploads := engine.Downloads.Limits()
	items := engine.Downloads.Snapshot()
	if downloads != 1 || uploads != 2 || phaseCount(items, Downloading()) != 4 ||
		phaseCount(items, Queued()) != 1 ||
		phaseCount(items, Uploading()) != 2 {
		t.Fatalf(
			"backoff cancelled or displaced work: download=%d upload=%d items=%+v",
			downloads,
			uploads,
			items,
		)
	}
	cancel()
	if err := group.Wait(); err == nil {
		t.Fatal("interrupted sources unexpectedly completed")
	}
	for _, item := range engine.Downloads.Snapshot() {
		if item.Phase != Queued() || item.Attempt > 1 {
			t.Fatal("Stop did not drain active transfers", item)
		}
	}
}

func TestLiveUploadIdleTimeout(t *testing.T) {
	t.Parallel()
	config := liveConfig(t)
	var settings pipelineSettings
	liveSettings(t, config, "test-pipeline.json", &settings)
	api := testAPI(t, config)
	var body p.CreateEpisodePackage
	requireNoError(t, json.Unmarshal([]byte(settings.Mock), &body))
	var admitted p.EpisodePackage
	requireNoError(t, api.JSON(t.Context(), "POST", "/s/episode-packages", body, &admitted))
	if len(admitted.Uploads) == 0 || len(admitted.Uploads[0].Parts) == 0 {
		t.Fatal("multipart admission has no part")
	}
	target := admitted.Uploads[0].Parts[0].UploadUrl
	stat, err := os.Stat(settings.Prelude)
	requireNoError(t, err)
	clock := clockwork.NewFakeClock()
	api.Clock = clock
	done := make(chan error, 1)
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	go func() {
		_, err := api.UploadPart(ctx, settings.Prelude, 0, stat.Size(), "PUT", target, nil)
		done <- err
	}()
	requireNoError(t, controlRequest(ctx, api, "GET", settings.Control+"/ready", nil))
	clock.Advance(61 * time.Second)
	select {
	case err := <-done:
		if !errors.Is(err, ErrUploadStalled) {
			t.Fatal("upload deadline lost its cause", err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("upload did not consume the idle deadline")
	}
	requireNoError(t, controlRequest(ctx, api, "GET", settings.Control+"/release", nil))
	api.Clock = clockwork.NewRealClock()
	etag, err := api.UploadPart(ctx, settings.Prelude, 0, stat.Size(), "PUT", target, nil)
	requireNoError(t, err)
	if etag == "" {
		t.Fatal("explicit same-part resume has no acknowledgement")
	}
}

//nolint:cyclop,funlen,gocognit,gocyclo // Verify interruption, saved outcome and explicit recovery together.
func TestLiveNetworkRecovery(t *testing.T) {
	t.Parallel()
	config := liveConfig(t)
	var settings struct{ Slug, Mode string }
	liveSettings(t, config, "test-network.json", &settings)
	api := testAPI(t, config)
	origin, err := url.Parse(config.APIOrigin)
	requireNoError(t, err)
	address := origin.Host
	var calls atomic.Int32
	baseTransport, ok := api.HTTP.Transport.(*http.Transport)
	if !ok {
		t.Fatalf("unexpected API transport %T", api.HTTP.Transport)
	}
	transport := baseTransport.Clone()
	transport.DisableKeepAlives = true
	originalProxy := transport.Proxy
	transport.Proxy = func(request *http.Request) (*url.URL, error) {
		if request.URL.Scheme == httpsScheme {
			return originalProxy(request)
		}
		return nil, nil
	}
	dialer := &net.Dialer{Timeout: 3 * time.Second}
	transport.DialContext = func(ctx context.Context, network, target string) (net.Conn, error) {
		host, _, _ := net.SplitHostPort(target)
		if host == "sync.test.invalid" {
			call := calls.Add(1)
			if settings.Mode == dnsFailureScenario && call == 2 {
				return nil, &net.DNSError{
					Err:        "test-owned resolver refusal",
					Name:       host,
					IsNotFound: true,
				}
			}
			target = address
		}
		return dialer.DialContext(ctx, network, target)
	}
	if settings.Mode != "api" {
		origin.Host = net.JoinHostPort("sync.test.invalid", origin.Port())
		config.APIOrigin = origin.String()
	}
	httpClient := &http.Client{Transport: transport}
	defer transport.CloseIdleConnections()
	api, err = NewAPIWithClients(config, httpClient, api.UploadHTTP)
	requireNoError(t, err)
	engine := NewEngine(nil)
	defer engine.Close()
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	done := make(chan error, 1)
	var report Report
	go func() { var err error; report, err = engine.Once(ctx, api, settings.Slug); done <- err }()
	stopped := false
	var result error
	guard := time.NewTimer(4 * time.Second)
	defer guard.Stop()
work:
	for {
		changed := engine.Downloads.Changes()
		for _, item := range engine.Downloads.Snapshot() {
			if item.Phase == Retrying() && !stopped {
				t.Logf("network recovery %s observed transport failure: %s", settings.Mode, item.Error)
				if item.Attempt != 1 || item.Error == "" {
					t.Fatal("retry lost its first transport error", item)
				}
				if settings.Mode == dnsFailureScenario && !strings.Contains(item.Error, "lookup") {
					t.Fatal(item.Error)
				}
				if settings.Mode == "server" && !strings.Contains(item.Error, "HTTP 5") {
					t.Fatal(item.Error)
				}
				stopped = true
				cancel()
			}
		}
		select {
		case result = <-done:
			break work
		case <-changed:
		case <-guard.C:
			t.Fatalf("network scenario %s blocked before its expected event: %+v", settings.Mode, engine.Downloads.Snapshot())
		}
	}
	items := engine.Downloads.Snapshot()
	db, err := engine.Database(t.Context(), config)
	requireNoError(t, err)
	lastSynced, err := db.LastSynced(t.Context(), config.APIOrigin, settings.Slug)
	requireNoError(t, err)
	saved, err := db.Items(t.Context(), config.APIOrigin, settings.Slug)
	requireNoError(t, err)
	if (lastSynced != nil) != (result == nil) {
		t.Fatal("interrupted work changed successful sync completion")
	}
	if len(items) != 1 || len(saved) != 1 {
		t.Fatalf("source state: live=%+v saved=%+v", items, saved)
	}
	switch settings.Mode {
	case "resume":
		requireNoError(t, result)
		if stopped || report.Added != 1 || report.Skipped != 0 || items[0].Phase != Complete() ||
			items[0].Attempt != 1 ||
			saved[0].Phase != Complete() {
			t.Fatalf("resume: report=%+v live=%+v saved=%+v", report, items, saved)
		}
	case "published":
		requireNoError(t, result)
		if report.Unchanged != 1 || report.Added != 0 || items[0].Phase != Complete() ||
			items[0].Attempt != 0 ||
			saved[0].Phase != Complete() {
			t.Fatal("published media started another transfer", report, items, saved)
		}
	case dnsFailureScenario, "player", "completion", "server", "api":
		if result == nil || !stopped || items[0].Attempt != 1 || items[0].Phase != Queued() ||
			saved[0].Phase != Queued() {
			t.Fatal("Stop did not preserve the retry checkpoint", result, items, saved)
		}
		if settings.Mode == dnsFailureScenario && calls.Load() != 2 {
			t.Fatal("resolver calls", calls.Load())
		}
	default:
		t.Fatal("unknown scenario", settings.Mode)
	}
}

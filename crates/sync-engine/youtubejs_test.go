package syncengine

import (
	"crypto/ed25519"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"github.com/listenbox/client/crates/sync-engine/youtubejsrelease"
	"github.com/listenbox/client/crates/youtubei"
)

const (
	refsChanged   = "refs 200"
	refsUnchanged = "refs 304"
)

// releaseFixture serves the two GitHub surfaces clients use: the tag listing
// API with its ETag validators and release asset downloads.
type releaseFixture struct {
	mu       sync.Mutex
	tags     []string
	assets   map[string][]byte
	failures map[string]int
	requests []string
	server   *httptest.Server
}

func newReleaseFixture(t *testing.T) *releaseFixture {
	t.Helper()
	fixture := &releaseFixture{assets: map[string][]byte{}, failures: map[string]int{}}
	prefix := "/repos/" + youtubejsrelease.Repository + "/git/matching-refs/tags/" +
		youtubejsrelease.TagPrefix(youtubei.BundleABI)
	fixture.server = httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		fixture.mu.Lock()
		defer fixture.mu.Unlock()
		if request.URL.Path == prefix {
			refs := make([]map[string]string, 0, len(fixture.tags))
			for _, tag := range fixture.tags {
				refs = append(refs, map[string]string{"ref": "refs/tags/" + tag})
			}
			raw, _ := json.Marshal(refs)
			digest := sha256.Sum256(raw)
			etag := `W/"` + hex.EncodeToString(digest[:]) + `"`
			writer.Header().Set("ETag", etag)
			if request.Header.Get("If-None-Match") == etag {
				fixture.requests = append(fixture.requests, refsUnchanged)
				writer.WriteHeader(http.StatusNotModified)
				return
			}
			fixture.requests = append(fixture.requests, refsChanged)
			_, _ = writer.Write(raw)
			return
		}
		asset := strings.TrimPrefix(request.URL.Path, "/"+youtubejsrelease.Repository+"/releases/download/")
		fixture.requests = append(fixture.requests, asset)
		if fixture.failures[asset] > 0 {
			fixture.failures[asset]--
			writer.WriteHeader(http.StatusBadGateway)
			return
		}
		raw, ok := fixture.assets[asset]
		if !ok {
			writer.WriteHeader(http.StatusNotFound)
			return
		}
		_, _ = writer.Write(raw)
	}))
	t.Cleanup(fixture.server.Close)
	return fixture
}

// publish adds a release whose tag, manifest and source may disagree, as a
// tampered or replayed release would.
func (f *releaseFixture) publish(tag string, signed, source []byte) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.tags = append(f.tags, tag)
	f.assets[tag+"/"+youtubejsrelease.ManifestAsset] = signed
	f.assets[tag+"/"+youtubejsrelease.SourceAsset] = source
}

func (f *releaseFixture) drain() []string {
	f.mu.Lock()
	defer f.mu.Unlock()
	requests := f.requests
	f.requests = nil
	return requests
}

type releaseKeys struct {
	public  ed25519.PublicKey
	private ed25519.PrivateKey
}

func newReleaseKeys(t *testing.T) releaseKeys {
	t.Helper()
	public, private, err := ed25519.GenerateKey(rand.Reader)
	requireNoError(t, err)
	return releaseKeys{public, private}
}

// release builds a real bundle that marks the engines evaluating it with its
// release number, and its
// manifest signed the way the fork's release workflow signs it.
func (k releaseKeys) release(
	t *testing.T, abi int, release, sequence int64,
) (youtubejsrelease.Manifest, []byte, []byte) {
	t.Helper()
	source, err := os.ReadFile("../../third_party/bundle/quickjs.js")
	requireNoError(t, err)
	source = fmt.Appendf(source, "\n;globalThis.__releaseMarker=%d;\n", release)
	digest := sha256.Sum256(source)
	manifest := youtubejsrelease.Manifest{
		ABI: abi, Release: release, Sequence: sequence, SHA256: hex.EncodeToString(digest[:]),
		Size: int64(len(source)), Commit: strings.Repeat("a", 40),
	}
	payload, err := json.Marshal(manifest)
	requireNoError(t, err)
	signature := ed25519.Sign(k.private, append([]byte("listenbox youtubejs release v1\n"), payload...))
	signed, err := json.Marshal(map[string][]byte{"payload": payload, "signature": signature})
	requireNoError(t, err)
	return manifest, signed, source
}

func releaseClient(t *testing.T, directory string, fixture *releaseFixture, key ed25519.PublicKey) *Client {
	t.Helper()
	client := NewClient(Config{APIOrigin: "https://api.example.test", Directory: directory})
	t.Cleanup(func() { _ = client.Close() })
	_, err := client.API(t.Context())
	requireNoError(t, err)
	client.youtubeJS.apiOrigin = fixture.server.URL
	client.youtubeJS.downloadOrigin = fixture.server.URL
	client.youtubeJS.key = key
	return client
}

// runningMarker starts an engine the way a sync does and reports which
// bundle it evaluated: 0 is the embedded bundle.
func runningMarker(t *testing.T, client *Client) int {
	t.Helper()
	api, err := client.API(t.Context())
	requireNoError(t, err)
	engine, err := youtubei.New(t.Context(), nil, api.youtubeJS.bundle().bundle)
	requireNoError(t, err)
	defer engine.Close()
	var marker int
	requireNoError(t, engine.Call(t.Context(), "()=>globalThis.__releaseMarker??0", nil, &marker))
	return marker
}

func requireRequests(t *testing.T, fixture *releaseFixture, want ...string) {
	t.Helper()
	if got := fixture.drain(); strings.Join(got, ", ") != strings.Join(want, ", ") {
		t.Fatalf("requests = %q, want %q", got, want)
	}
}

func TestYouTubeJSAdoptsSignedReleasesWithConditionalChecks(t *testing.T) {
	t.Parallel()
	fixture, keys, directory := newReleaseFixture(t), newReleaseKeys(t), t.TempDir()
	client := releaseClient(t, directory, fixture, keys.public)
	embedded := embeddedYouTubeJS().manifest.Sequence
	manifest, signed, source := keys.release(t, youtubei.BundleABI, 1, embedded+1)
	tag := manifest.Tag()
	fixture.publish(tag, signed, source)
	fixture.failures[tag+"/"+youtubejsrelease.SourceAsset] = 1

	// A failed download keeps no validator, so the next check retries it.
	if err := client.CheckYouTubeJS(t.Context()); err == nil {
		t.Fatal("check succeeded although the bundle download failed")
	}
	requireRequests(t, fixture, refsChanged, tag+"/youtubejs.json", tag+"/youtubejs.js")
	if marker := runningMarker(t, client); marker != 0 {
		t.Fatalf("engine runs release %d after a failed download", marker)
	}
	requireNoError(t, client.CheckYouTubeJS(t.Context()))
	requireRequests(t, fixture, refsChanged, tag+"/youtubejs.json", tag+"/youtubejs.js")
	if marker := runningMarker(t, client); marker != 1 {
		t.Fatalf("engine runs bundle %d, want the release", marker)
	}
	requireNoError(t, client.CheckYouTubeJS(t.Context()))
	requireRequests(t, fixture, refsUnchanged)

	// A restarted client runs the stored release at once and revalidates.
	restarted := releaseClient(t, directory, fixture, keys.public)
	if marker := runningMarker(t, restarted); marker != 1 {
		t.Fatalf("restarted engine runs bundle %d, want the stored release", marker)
	}
	requireNoError(t, restarted.CheckYouTubeJS(t.Context()))
	requireRequests(t, fixture, refsUnchanged)

	// A source swapped on disk fails its signed digest; the embedded bundle runs.
	requireNoError(t, os.WriteFile(filepath.Join(directory, "youtubejs", manifest.SHA256+".js"),
		append(source, ";globalThis.__releaseMarker=666;"...), privateFileMode))
	tampered := releaseClient(t, directory, fixture, keys.public)
	if marker := runningMarker(t, tampered); marker != 0 {
		t.Fatalf("engine runs tampered stored bundle %d", marker)
	}
}

func TestYouTubeJSRejectsUntrustedReleases(t *testing.T) {
	t.Parallel()
	embedded := embeddedYouTubeJS().manifest.Sequence
	for _, scenario := range []struct {
		name string
		// publish returns the requests a client may make for this release.
		publish func(*releaseFixture, releaseKeys) []string
	}{
		{"source swapped after signing", func(f *releaseFixture, k releaseKeys) []string {
			manifest, signed, source := k.release(t, youtubei.BundleABI, 1, embedded+1)
			f.publish(manifest.Tag(), signed, append(source, ";globalThis.__releaseMarker=666;"...))
			return []string{refsChanged, manifest.Tag() + "/youtubejs.json", manifest.Tag() + "/youtubejs.js"}
		}},
		{"signed by another key", func(f *releaseFixture, _ releaseKeys) []string {
			manifest, signed, source := newReleaseKeys(t).release(t, youtubei.BundleABI, 1, embedded+1)
			f.publish(manifest.Tag(), signed, source)
			return []string{refsChanged, manifest.Tag() + "/youtubejs.json"}
		}},
		{"older than the embedded bundle", func(f *releaseFixture, k releaseKeys) []string {
			manifest, signed, source := k.release(t, youtubei.BundleABI, 7, embedded-1)
			f.publish(manifest.Tag(), signed, source)
			return []string{refsChanged, manifest.Tag() + "/youtubejs.json"}
		}},
		{"release relabeled with a newer version", func(f *releaseFixture, k releaseKeys) []string {
			_, signed, source := k.release(t, youtubei.BundleABI, 1, embedded+1)
			tag := youtubejsrelease.TagPrefix(youtubei.BundleABI) + "2"
			f.publish(tag, signed, source)
			return []string{refsChanged, tag + "/youtubejs.json"}
		}},
		{"built for another ABI", func(f *releaseFixture, k releaseKeys) []string {
			_, signed, source := k.release(t, youtubei.BundleABI+1, 1, embedded+1)
			tag := youtubejsrelease.TagPrefix(youtubei.BundleABI) + "1"
			f.publish(tag, signed, source)
			return []string{refsChanged, tag + "/youtubejs.json"}
		}},
	} {
		t.Run(scenario.name, func(t *testing.T) {
			t.Parallel()
			fixture, keys := newReleaseFixture(t), newReleaseKeys(t)
			client := releaseClient(t, t.TempDir(), fixture, keys.public)
			requests := scenario.publish(fixture, keys)
			requireNoError(t, client.CheckYouTubeJS(t.Context()))
			requireRequests(t, fixture, requests...)
			if marker := runningMarker(t, client); marker != 0 {
				t.Fatalf("engine runs untrusted release %d", marker)
			}
			// The verdict is remembered until the release list changes.
			requireNoError(t, client.CheckYouTubeJS(t.Context()))
			requireRequests(t, fixture, refsUnchanged)
		})
	}
}

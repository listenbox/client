package syncengine

import (
	"context"
	"crypto/ed25519"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/listenbox/client/crates/sync-engine/youtubejsrelease"
	"github.com/listenbox/client/crates/youtubei"
	thirdparty "github.com/listenbox/client/third_party"
)

// YouTube.js ships in two ways: the build embeds a bundle so the first engine
// starts at once, and signed GitHub releases of the fork replace it without a
// client release. Only source signed by the pinned release key, built for this
// client's bundle ABI and newer than the bundle in use is ever evaluated.
const (
	youtubeJSCheckInterval = 10 * time.Minute
	youtubeJSRefsLimit     = 1 << 20
	youtubeJSManifestFile  = "manifest.json"
	youtubeJSStateFile     = "state.json"
)

type youtubeJSBundle struct {
	manifest   youtubejsrelease.Manifest
	bundle     youtubei.Bundle
	downloaded bool
}

// youtubeJSUpdates owns the profile's bundle directory. GitHub, the network
// and the directory are all untrusted: everything read from them is verified
// against the signed manifest before it is compiled.
type youtubeJSUpdates struct {
	directory      string
	apiOrigin      string
	downloadOrigin string
	key            ed25519.PublicKey
	checking       sync.Mutex
	mu             sync.Mutex
	current        *youtubeJSBundle
	rejected       map[int64]bool
}

type youtubeJSState struct {
	ETag string `json:"etag"`
}

func newYouTubeJSUpdates(profile string) *youtubeJSUpdates {
	return &youtubeJSUpdates{
		directory:      filepath.Join(profile, "youtubejs"),
		apiOrigin:      "https://api.github.com",
		downloadOrigin: "https://github.com",
		key:            youtubejsrelease.PublicKey,
		rejected:       make(map[int64]bool),
	}
}

func embeddedYouTubeJS() *youtubeJSBundle {
	var manifest youtubejsrelease.Manifest
	if err := json.Unmarshal(thirdparty.YouTubeJSIdentity, &manifest); err != nil {
		panic("embedded YouTube.js identity: " + err.Error())
	}
	manifest.ABI = youtubei.BundleABI
	return &youtubeJSBundle{manifest: manifest, bundle: youtubei.Embedded()}
}

// bundle returns the newest verified bundle. The first call adopts a bundle a
// previous process downloaded, re-verifying it against its signed manifest.
func (u *youtubeJSUpdates) bundle() *youtubeJSBundle {
	if u == nil {
		return embeddedYouTubeJS()
	}
	u.mu.Lock()
	defer u.mu.Unlock()
	if u.current == nil {
		u.current = embeddedYouTubeJS()
		if stored, err := u.stored(); err != nil {
			slog.Warn("ignore stored YouTube.js bundle", "error", err)
		} else if stored != nil && u.adoptable(stored.manifest) {
			u.current = stored
		}
	}
	return u.current
}

func (u *youtubeJSUpdates) stored() (*youtubeJSBundle, error) {
	raw, err := readBoundedFile(filepath.Join(u.directory, youtubeJSManifestFile), youtubejsrelease.ManifestLimit)
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	manifest, err := youtubejsrelease.Open(raw, u.key)
	if err != nil {
		return nil, err
	}
	source, err := readBoundedFile(u.sourcePath(manifest), youtubejsrelease.SourceLimit)
	if err != nil {
		return nil, err
	}
	return compileYouTubeJS(manifest, source)
}

func compileYouTubeJS(manifest youtubejsrelease.Manifest, source []byte) (*youtubeJSBundle, error) {
	if err := manifest.Matches(source); err != nil {
		return nil, err
	}
	bundle, err := youtubei.Compile(source)
	if err != nil {
		return nil, err
	}
	return &youtubeJSBundle{manifest: manifest, bundle: bundle, downloaded: true}, nil
}

// adoptable requires u.mu. Moving only forward stops a replayed older release
// from downgrading the bundle in use.
func (u *youtubeJSUpdates) adoptable(manifest youtubejsrelease.Manifest) bool {
	return manifest.ABI == youtubei.BundleABI && !u.rejected[manifest.Sequence] &&
		manifest.Sequence > u.current.manifest.Sequence
}

func (u *youtubeJSUpdates) sourcePath(manifest youtubejsrelease.Manifest) string {
	return filepath.Join(u.directory, manifest.SHA256+".js")
}

// reject drops a downloaded bundle the runtime could not evaluate, for the
// rest of this process, and returns to the embedded one.
func (u *youtubeJSUpdates) reject(failed *youtubeJSBundle, err error) {
	slog.Warn("YouTube.js release failed to start; using the embedded bundle",
		"sequence", failed.manifest.Sequence, "error", err)
	u.mu.Lock()
	defer u.mu.Unlock()
	u.rejected[failed.manifest.Sequence] = true
	if u.current == failed {
		u.current = embeddedYouTubeJS()
		// Later processes start from the embedded bundle too; the saved
		// validator keeps this release from downloading again.
		_ = os.Remove(filepath.Join(u.directory, youtubeJSManifestFile))
	}
}

// WatchYouTubeJS checks for YouTube.js releases now and then every ten
// minutes until ctx ends or the returned stop function is called. Checks are
// conditional, so unchanged releases cost one 304 response.
func (c *Client) WatchYouTubeJS(ctx context.Context) func() {
	ctx, cancel := context.WithCancel(ctx)
	done := make(chan struct{})
	go func() {
		defer close(done)
		for {
			if err := c.CheckYouTubeJS(ctx); err != nil && ctx.Err() == nil {
				slog.Debug("check YouTube.js releases", "error", err)
			}
			select {
			case <-ctx.Done():
				return
			case <-c.Engine.clock.After(youtubeJSCheckInterval):
			}
		}
	}()
	return func() { cancel(); <-done }
}

// CheckYouTubeJS adopts a newer signed YouTube.js release, if there is one.
func (c *Client) CheckYouTubeJS(ctx context.Context) error {
	api, err := c.API(ctx)
	if err != nil {
		return err
	}
	return api.youtubeJS.check(ctx, api)
}

func (u *youtubeJSUpdates) check(ctx context.Context, api *API) error {
	u.checking.Lock()
	defer u.checking.Unlock()
	u.bundle() // Load the stored release before comparing against it.
	var state youtubeJSState
	if raw, err := readBoundedFile(filepath.Join(u.directory, youtubeJSStateFile), errorResponseLimit); err == nil {
		_ = json.Unmarshal(raw, &state)
	}
	request, err := http.NewRequestWithContext(
		ctx, http.MethodGet,
		u.apiOrigin+"/repos/"+youtubejsrelease.Repository+"/git/matching-refs/tags/"+
			youtubejsrelease.TagPrefix(youtubei.BundleABI),
		nil,
	)
	if err != nil {
		return err
	}
	request.Header.Set("Accept", "application/vnd.github+json")
	request.Header.Set("X-Github-Api-Version", "2022-11-28")
	if state.ETag != "" {
		request.Header.Set("If-None-Match", state.ETag)
	}
	response, err := api.send(api.HTTP, request)
	if err != nil {
		return err
	}
	defer response.Body.Close()
	if response.StatusCode == http.StatusNotModified {
		return nil
	}
	if response.StatusCode != http.StatusOK {
		return fmt.Errorf("list YouTube.js releases: HTTP %d", response.StatusCode)
	}
	latest, err := latestYouTubeJSRelease(response)
	if err != nil {
		return err
	}
	if latest > 0 {
		if err := u.download(ctx, api, latest); err != nil {
			if _, rejected := errors.AsType[*releaseRejectedError](err); !rejected {
				return err // Transient: keep no validator so the next check retries.
			}
			slog.Warn("reject YouTube.js release", "release", latest, "error", err)
		}
	}
	if etag := response.Header.Get("ETag"); etag != "" {
		return WritePrivateJSON(filepath.Join(u.directory, youtubeJSStateFile), youtubeJSState{etag})
	}
	return nil
}

// latestYouTubeJSRelease returns the highest release number for this
// client's bundle ABI, or 0 when there is none.
func latestYouTubeJSRelease(response *http.Response) (int64, error) {
	raw, err := ReadBounded(response.Body, youtubeJSRefsLimit)
	if err != nil {
		return 0, err
	}
	var refs []struct {
		Ref string `json:"ref"`
	}
	if err := json.Unmarshal(raw, &refs); err != nil {
		return 0, fmt.Errorf("decode YouTube.js releases: %w", err)
	}
	var latest int64
	for _, ref := range refs {
		if release, ok := youtubejsrelease.ReleaseOf(
			strings.TrimPrefix(ref.Ref, "refs/tags/"), youtubei.BundleABI,
		); ok {
			latest = max(latest, release)
		}
	}
	return latest, nil
}

// releaseRejectedError marks a release that will never verify, so it is not
// downloaded again until the release list changes.
type releaseRejectedError struct{ err error }

func (e *releaseRejectedError) Error() string { return e.err.Error() }
func (e *releaseRejectedError) Unwrap() error { return e.err }

// download adopts the latest release when its signed manifest verifies and
// its bundle is newer than the one in use; only then is the bundle fetched.
func (u *youtubeJSUpdates) download(ctx context.Context, api *API, release int64) error {
	tag := youtubejsrelease.TagPrefix(youtubei.BundleABI) + strconv.FormatInt(release, 10)
	signed, err := u.asset(ctx, api, tag, youtubejsrelease.ManifestAsset, youtubejsrelease.ManifestLimit)
	if err != nil {
		return err
	}
	manifest, err := youtubejsrelease.Open(signed, u.key)
	if err != nil {
		return &releaseRejectedError{err}
	}
	if manifest.Tag() != tag {
		return &releaseRejectedError{fmt.Errorf("release %s carries the manifest of %s", tag, manifest.Tag())}
	}
	u.mu.Lock()
	adoptable := u.adoptable(manifest)
	u.mu.Unlock()
	if !adoptable {
		return nil // The bundle in use is this release or a later commit.
	}
	source, err := u.asset(ctx, api, tag, youtubejsrelease.SourceAsset, manifest.Size)
	if err != nil {
		return err
	}
	next, err := compileYouTubeJS(manifest, source)
	if err != nil {
		return &releaseRejectedError{err}
	}
	if err := u.store(manifest, signed, source); err != nil {
		return err
	}
	u.mu.Lock()
	defer u.mu.Unlock()
	if u.adoptable(manifest) {
		u.current = next
		slog.Info("adopted YouTube.js release", "sequence", manifest.Sequence, "commit", manifest.Commit)
	}
	return nil
}

func (u *youtubeJSUpdates) asset(ctx context.Context, api *API, tag, name string, limit int64) ([]byte, error) {
	request, err := http.NewRequestWithContext(
		ctx, http.MethodGet,
		u.downloadOrigin+"/"+youtubejsrelease.Repository+"/releases/download/"+tag+"/"+name,
		nil,
	)
	if err != nil {
		return nil, err
	}
	response, err := api.send(api.HTTP, request)
	if err != nil {
		return nil, err
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("download %s/%s: HTTP %d", tag, name, response.StatusCode)
	}
	// An oversized body is returned truncated so verification rejects it
	// rather than retrying it as a transport failure.
	return io.ReadAll(io.LimitReader(response.Body, limit+1))
}

// store writes the source under its digest before the manifest that names it,
// so a crash leaves either the old pair or the new one.
func (u *youtubeJSUpdates) store(manifest youtubejsrelease.Manifest, signed, source []byte) error {
	if err := writePrivateFile(u.sourcePath(manifest), source); err != nil {
		return err
	}
	if err := writePrivateFile(filepath.Join(u.directory, youtubeJSManifestFile), signed); err != nil {
		return err
	}
	entries, err := os.ReadDir(u.directory)
	if err != nil {
		return err
	}
	for _, entry := range entries {
		if strings.HasSuffix(entry.Name(), ".js") && entry.Name() != manifest.SHA256+".js" {
			_ = os.Remove(filepath.Join(u.directory, entry.Name()))
		}
	}
	return nil
}

func readBoundedFile(path string, limit int64) ([]byte, error) {
	// #nosec G304 -- Paths are fixed names or verified digests inside the profile.
	file, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer file.Close()
	return ReadBounded(file, limit)
}

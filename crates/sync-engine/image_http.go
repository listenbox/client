package syncengine

import (
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"log/slog"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/jonboulle/clockwork"
	"github.com/pquerna/cachecontrol/cacheobject"
)

const (
	artworkBodyLimit      = 8 << 20
	artworkRecordLimit    = 2 * artworkBodyLimit
	artworkDiskLimit      = 128 << 20
	artworkRequestTimeout = 30 * time.Second
	maximumHTTPAge        = 1<<31 - 1
)

type artworkHTTP struct {
	directory string
	client    *http.Client
	clock     clockwork.Clock
}

type artworkResponse struct {
	URL       string      `json:"url"`
	Header    http.Header `json:"header"`
	Vary      http.Header `json:"vary"`
	Body      []byte      `json:"body"`
	ExpiresAt int64       `json:"expires_at"`
}

// Artwork retrieves a public cover through the profile's persistent HTTP cache.
// Revalidation stays synchronous so cancellation and logout own every request.
func (c *Client) Artwork(ctx context.Context, target string) ([]byte, error) {
	api, err := c.API(ctx)
	if err != nil {
		return nil, err
	}
	c.mu.Lock()
	if c.artwork == nil {
		directory := filepath.Join(c.Config.Directory, "http-cache")
		if err = trimArtworkCache(directory); err == nil {
			client := *api.HTTP
			client.Timeout = artworkRequestTimeout
			c.artwork = &artworkHTTP{
				directory: filepath.Join(directory, "go-v1", "follow-10"), client: &client, clock: api.Clock,
			}
		}
	}
	cache := c.artwork
	c.mu.Unlock()
	if err != nil {
		return nil, err
	}
	return cache.load(ctx, target, api)
}

func (cache *artworkHTTP) load(ctx context.Context, target string, api *API) ([]byte, error) {
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, target, nil)
	if err != nil {
		return nil, err
	}
	cached := cache.read(request)
	if cached != nil {
		if cache.clock.Now().UnixMilli() < cached.ExpiresAt {
			return cached.Body, nil
		}
		if etag := cached.Header.Get("ETag"); etag != "" {
			request.Header.Set("If-None-Match", etag)
		} else if modified := cached.Header.Get("Last-Modified"); modified != "" {
			request.Header.Set("If-Modified-Since", modified)
		}
	}
	started := cache.clock.Now()
	response, err := api.send(cache.client, request)
	if err != nil {
		return nil, err
	}
	defer response.Body.Close()
	if response.StatusCode == http.StatusNotModified && cached != nil {
		return cache.revalidated(request, response, started, cached)
	}
	if response.StatusCode != http.StatusOK {
		cache.remove(target)
		return nil, fmt.Errorf("artwork: HTTP %d", response.StatusCode)
	}
	raw, err := ReadBounded(response.Body, artworkBodyLimit)
	if err != nil {
		return nil, err
	}
	entry := &artworkResponse{URL: target, Header: response.Header.Clone(), Body: raw}
	cache.store(request, started, entry)
	return raw, nil
}

func (cache *artworkHTTP) revalidated(
	request *http.Request, response *http.Response, started time.Time, cached *artworkResponse,
) ([]byte, error) {
	if response.Header.Get("ETag") != "" && cached.Header.Get("ETag") != response.Header.Get("ETag") {
		cache.remove(cached.URL)
		return nil, errors.New("artwork: HTTP 304 changed the stored validator")
	}
	// A successful validation starts a new age calculation. Representation
	// framing and hop-by-hop headers from the bodyless 304 cannot replace the body.
	cached.Header.Del("Age")
	for key, values := range response.Header {
		switch key {
		case "Content-Length", "Content-Range", "Transfer-Encoding", "Connection", "Keep-Alive", "Trailer", "Upgrade":
			continue
		default:
			cached.Header[key] = values
		}
	}
	cache.store(request, started, cached)
	return cached.Body, nil
}

func (cache *artworkHTTP) read(request *http.Request) *artworkResponse {
	// #nosec G703 -- The filename is a SHA-256 digest inside the active profile.
	file, err := os.Open(cache.path(request.URL.String()))
	if err != nil {
		return nil
	}
	defer file.Close()
	raw, err := ReadBounded(file, artworkRecordLimit)
	if err != nil {
		return nil
	}
	var entry artworkResponse
	if json.Unmarshal(raw, &entry) != nil || entry.URL != request.URL.String() ||
		entry.Header == nil || len(entry.Body) > artworkBodyLimit {
		return nil
	}
	for key, values := range entry.Vary {
		if strings.Join(values, ",") != strings.Join(request.Header.Values(key), ",") {
			return nil
		}
	}
	return &entry
}

func (cache *artworkHTTP) store(request *http.Request, started time.Time, entry *artworkResponse) {
	now := cache.clock.Now().UTC()
	entry.Vary = make(http.Header)
	if !entry.policy(request, started, now) {
		cache.remove(entry.URL)
		return
	}
	if err := WritePrivateJSON(cache.path(entry.URL), entry); err != nil {
		slog.Warn("artwork HTTP cache write failed", "error", err)
	}
}

func (entry *artworkResponse) policy(request *http.Request, started, now time.Time) bool {
	directives, err := cacheobject.ParseResponseCacheControl(strings.Join(entry.Header.Values("Cache-Control"), ","))
	if err != nil || len(entry.Header.Values("Set-Cookie")) != 0 {
		return false
	}
	date, _ := http.ParseTime(entry.Header.Get("Date"))
	expires, _ := http.ParseTime(entry.Header.Get("Expires"))
	modified, _ := http.ParseTime(entry.Header.Get("Last-Modified"))
	object := cacheobject.Object{
		RespDirectives: directives, RespHeaders: entry.Header, RespStatusCode: http.StatusOK,
		RespDateHeader: date, RespExpiresHeader: expires, RespLastModifiedHeader: modified,
		ReqHeaders: request.Header, ReqMethod: request.Method, NowUTC: now,
	}
	var result cacheobject.ObjectResults
	cacheobject.CachableObject(&object, &result)
	if len(result.OutReasons) != 0 {
		return false
	}
	cacheobject.ExpirationObject(&object, &result)
	if result.OutErr != nil {
		return false
	}
	entry.ExpiresAt = result.OutExpirationTime.Add(-artworkResponseAge(entry.Header, started, now)).UnixMilli()
	if directives.NoCachePresent {
		entry.ExpiresAt = now.UnixMilli()
	}
	for _, line := range entry.Header.Values("Vary") {
		for key := range strings.SplitSeq(line, ",") {
			key = strings.TrimSpace(key)
			if key == "*" {
				return false
			}
			if key != "" {
				entry.Vary[http.CanonicalHeaderKey(key)] = request.Header.Values(key)
			}
		}
	}
	return true
}

func artworkResponseAge(header http.Header, started, now time.Time) time.Duration {
	var apparent time.Duration
	if date, err := http.ParseTime(header.Get("Date")); err == nil {
		apparent = max(0, now.Sub(date))
	}
	var age time.Duration
	if raw := header.Get("Age"); raw != "" {
		seconds, err := strconv.ParseInt(raw, 10, 32)
		if err != nil || seconds < 0 {
			return maximumHTTPAge * time.Second
		}
		age = time.Duration(seconds) * time.Second
	}
	return max(apparent, age+max(0, now.Sub(started)))
}

func (cache *artworkHTTP) path(target string) string {
	hash := sha256.Sum256([]byte(target))
	return filepath.Join(cache.directory, fmt.Sprintf("%x.json", hash))
}

func (cache *artworkHTTP) remove(target string) {
	if err := os.Remove(cache.path(target)); err != nil && !os.IsNotExist(err) {
		slog.Warn("artwork HTTP cache removal failed", "error", err)
	}
}

func trimArtworkCache(directory string) error {
	var total int64
	err := filepath.WalkDir(directory, func(_ string, entry fs.DirEntry, err error) error {
		if os.IsNotExist(err) {
			return nil
		}
		if err != nil || entry.IsDir() {
			return err
		}
		info, err := entry.Info()
		if err != nil {
			return err
		}
		total += info.Size()
		if total > artworkDiskLimit {
			return fs.SkipAll
		}
		return nil
	})
	if err == nil && total > artworkDiskLimit {
		return os.RemoveAll(directory)
	}
	return err
}

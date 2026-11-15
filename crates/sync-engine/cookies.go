package syncengine

import (
	"context"
	"crypto/sha1" // #nosec G505 -- Required by the upstream YouTube protocol.
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/google/uuid"
)

const (
	CookieGuideURL = "https://listenbox.app/guides/import-youtube-as-a-podcast/#when-youtube-asks-you-" +
		"to-sign-in"
	maxCookieBytes = 1 << 20
)

type storedCookie struct {
	Raw     string            `json:"raw_cookie"`
	Path    [2]any            `json:"path"`
	Domain  map[string]string `json:"domain"`
	Expires json.RawMessage   `json:"expires"`
}
type cookieEntry struct {
	Cookie   storedCookie `json:"cookie"`
	Revision string       `json:"revision"`
}
type cookieSnapshot struct {
	Generation string        `json:"generation"`
	Enabled    bool          `json:"enabled"`
	Entries    []cookieEntry `json:"entries"`
}
type CookieJar struct{ directory string }

const (
	firstCookieDataRow   = 2
	netscapeCookieFields = 7
	maximumCookieYear    = 9999
	maximumCookies       = 1024
)

func NewCookieJar(profile string) *CookieJar {
	return &CookieJar{filepath.Join(profile, "youtube-cookies")}
}

func (jar *CookieJar) Enabled() (bool, error) {
	snapshot, err := jar.snapshot()
	return snapshot.Enabled, err
}

func (jar *CookieJar) ImportFile(ctx context.Context, path string) error {
	file, err := os.Open(path)
	if err != nil {
		return fmt.Errorf("open cookie export: %w", err)
	}
	defer file.Close()
	raw, err := ReadBounded(file, maxCookieBytes)
	if err != nil {
		return err
	}
	if !utf8.Valid(raw) {
		//nolint:staticcheck // Preserve existing end-user guidance.
		return errors.New(
			"Cookie export must be UTF-8 text",
		)
	}
	return jar.Import(ctx, string(raw))
}

func (jar *CookieJar) Import(ctx context.Context, text string) error {
	if len(text) > maxCookieBytes {
		//nolint:staticcheck // Preserve existing end-user guidance.
		return errors.New(
			"Cookie export is too large (maximum 1 MiB)",
		)
	}
	lines := strings.Split(strings.TrimPrefix(text, "\ufeff"), "\n")
	header := strings.TrimSpace(lines[0])
	if header != "# Netscape HTTP Cookie File" && header != "# HTTP Cookie File" {
		//nolint:staticcheck // Preserve existing end-user guidance.
		return errors.New(
			"Paste the entire Netscape cookies.txt export, including its header. JSON is not " +
				"supported.",
		)
	}
	saved := cookieSnapshot{Generation: uuid.NewString(), Enabled: true}
	keys := make(map[string]bool)
	for index, line := range lines[1:] {
		entry, err := parseNetscapeCookie(line, index)
		if err != nil {
			return err
		}
		if entry == nil {
			continue
		}
		key := entry.key()
		if keys[key] {
			//nolint:staticcheck // Preserve existing end-user guidance.
			return fmt.Errorf(
				"Duplicate cookie in row %d; export cookies again",
				index+firstCookieDataRow,
			)
		}
		keys[key] = true
		saved.Entries = append(saved.Entries, cookieEntry{*entry, uuid.NewString()})
		if len(saved.Entries) > maximumCookies {
			//nolint:staticcheck // Preserve existing end-user guidance.
			return errors.New(
				"Too many YouTube cookies in this export",
			)
		}
	}
	if len(saved.Entries) == 0 {
		//nolint:revive,staticcheck // Preserve existing end-user guidance.
		return errors.New(
			"No unexpired YouTube cookies found. Export a fresh YouTube session.",
		)
	}
	lock, err := profileLock(ctx, filepath.Join(jar.directory, "write.lock"), "YouTube cookies")
	if err != nil {
		return err
	}
	defer lock.Close()
	return jar.save(saved)
}

func (jar *CookieJar) Remove(ctx context.Context) error {
	lock, err := profileLock(ctx, filepath.Join(jar.directory, "write.lock"), "YouTube cookies")
	if err != nil {
		return err
	}
	defer lock.Close()
	return jar.save(cookieSnapshot{Generation: uuid.NewString()})
}

func (jar *CookieJar) snapshot() (cookieSnapshot, error) {
	raw, err := os.ReadFile(filepath.Join(jar.directory, "jar.json"))
	if os.IsNotExist(err) {
		return cookieSnapshot{}, nil
	}
	if err != nil {
		return cookieSnapshot{}, err
	}
	var saved cookieSnapshot
	if len(raw) > maxCookieBytes || json.Unmarshal(raw, &saved) != nil ||
		len(saved.Entries) > maximumCookies {
		//nolint:staticcheck // Preserve existing end-user guidance.
		return saved, errors.New(
			"Saved YouTube cookies are invalid; import a fresh export",
		)
	}
	for _, entry := range saved.Entries {
		if !entry.Cookie.valid() {
			//nolint:staticcheck // Preserve existing end-user guidance.
			return saved, errors.New(
				"Saved YouTube cookies are invalid; import a fresh export",
			)
		}
	}
	return saved, nil
}

func (jar *CookieJar) save(saved cookieSnapshot) error {
	if saved.Entries == nil {
		saved.Entries = []cookieEntry{}
	}
	raw, err := json.Marshal(saved)
	if err != nil {
		return err
	}
	if len(raw) >= maxCookieBytes {
		return errors.New("YouTube cookie jar is too large; import a fresh export")
	}
	return WritePrivateJSON(filepath.Join(jar.directory, "jar.json"), saved)
}

func youtubeDomain(host string) bool {
	return host == youtubeDomainName || strings.HasSuffix(host, ".youtube.com")
}

func validCookiePair(name, value string) bool {
	if name == "" {
		return false
	}
	for _, c := range []byte(name) {
		if !cookieNameByte(c) {
			return false
		}
	}
	for _, c := range []byte(value) {
		if c < 0x21 || c > 0x7e || strings.ContainsRune("\";,\\", rune(c)) {
			return false
		}
	}
	return true
}

func (c storedCookie) parsed() *http.Cookie {
	cookie, err := http.ParseSetCookie(c.Raw)
	if err != nil {
		return nil
	}
	return cookie
}

func (c storedCookie) host() string {
	if host := c.Domain["HostOnly"]; host != "" {
		return host
	}
	return c.Domain["Suffix"]
}
func (c storedCookie) path() string { path, _ := c.Path[0].(string); return path }
func (c storedCookie) key() string {
	cookie := c.parsed()
	if cookie == nil {
		return ""
	}
	return c.host() + "\x00" + c.path() + "\x00" + cookie.Name
}

func (c storedCookie) valid() bool {
	cookie := c.parsed()
	return cookie != nil && youtubeDomain(c.host()) && validCookiePair(cookie.Name, cookie.Value) &&
		strings.HasPrefix(c.path(), "/") &&
		(!strings.HasPrefix(cookie.Name, "__Secure-") || cookie.Secure) &&
		(!strings.HasPrefix(cookie.Name, "__Host-") || (cookie.Secure && c.path() == "/" && c.Domain["HostOnly"] != ""))
}

func (c storedCookie) expired() bool {
	var expires map[string]string
	if json.Unmarshal(c.Expires, &expires) == nil {
		instant, err := time.Parse(time.RFC3339Nano, expires["AtUtc"])
		return err != nil || !instant.After(time.Now())
	}
	return string(c.Expires) != `"SessionEnd"`
}

// #nosec G124 -- Preserve imported cookie attributes; this jar only sends cookies to HTTPS YouTube origins.
func makeStoredCookie(cookie *http.Cookie, host, path string) storedCookie {
	domain := map[string]string{"HostOnly": host}
	if cookie.Domain != "" {
		domain = map[string]string{"Suffix": strings.TrimLeft(strings.ToLower(cookie.Domain), ".")}
	}
	expires := json.RawMessage(`"SessionEnd"`)
	if cookie.MaxAge < 0 {
		cookie.Expires = time.Unix(1, 0).UTC()
	}
	if cookie.MaxAge > 0 {
		cookie.Expires = time.Now().UTC().Add(time.Duration(cookie.MaxAge) * time.Second)
	}
	if !cookie.Expires.IsZero() {
		expires, _ = json.Marshal( //nolint:errchkjson // Only string keys and values are encoded.
			map[string]string{"AtUtc": cookie.Expires.UTC().Format(time.RFC3339Nano)},
		)
	}
	return storedCookie{
		Raw:     cookie.String(),
		Path:    [2]any{path, true},
		Domain:  domain,
		Expires: expires,
	}
}

func (snapshot cookieSnapshot) header(u *url.URL) string {
	if !snapshot.Enabled || u.Scheme != httpsScheme || !youtubeDomain(u.Hostname()) {
		return ""
	}
	var cookies []storedCookie
	for _, entry := range snapshot.Entries {
		c := entry.Cookie
		if c.expired() {
			continue
		}
		if c.matches(u) {
			cookies = append(cookies, c)
		}
	}
	sort.Slice(cookies, func(i, j int) bool {
		if len(cookies[i].path()) != len(cookies[j].path()) {
			return len(cookies[i].path()) > len(cookies[j].path())
		}
		return cookies[i].parsed().Name < cookies[j].parsed().Name
	})
	pairs := make([]string, 0, len(cookies))
	for _, cookie := range cookies {
		parsed := cookie.parsed()
		pairs = append(pairs, parsed.Name+"="+parsed.Value)
	}
	return strings.Join(pairs, "; ")
}

func (snapshot cookieSnapshot) revision(key string) string {
	for _, entry := range snapshot.Entries {
		if entry.Cookie.key() == key {
			return entry.Revision
		}
	}
	return ""
}

func (jar *CookieJar) update(
	ctx context.Context,
	base cookieSnapshot,
	u *url.URL,
	headers []string,
) error {
	if !acceptResponseCookies(base, u, headers) {
		return nil
	}
	updates := responseCookies(u, headers)
	if len(updates) == 0 {
		return nil
	}
	lock, err := profileLock(ctx, filepath.Join(jar.directory, "write.lock"), "YouTube cookies")
	if err != nil {
		return err
	}
	defer lock.Close()
	current, err := jar.snapshot()
	if err != nil {
		return err
	}
	if current.Generation != base.Generation || !current.Enabled {
		return nil
	}
	changed := false
	for key, cookie := range updates {
		if current.revision(key) != base.revision(key) {
			continue
		}
		if err := mergeCookie(&current, cookie, key); err != nil {
			return err
		}
		changed = true
	}
	if changed {
		return jar.save(current)
	}
	return nil
}

func cookieAuthorization(header string, timestamp int64) string {
	values := make(map[string]string)
	for pair := range strings.SplitSeq(header, "; ") {
		key, value, ok := strings.Cut(pair, "=")
		if ok {
			values[key] = value
		}
	}
	var output []string
	sid := values["SAPISID"]
	if sid == "" {
		sid = values["__Secure-3PAPISID"]
	}
	for _, pair := range [][2]string{
		{"SAPISIDHASH", sid},
		{"SAPISID1PHASH", values["__Secure-1PAPISID"]},
		{"SAPISID3PHASH", values["__Secure-3PAPISID"]},
	} {
		if pair[1] != "" {
			// #nosec G401 -- SHA-1 is required by YouTube's existing protocol.
			hash := sha1.Sum(
				[]byte(fmt.Sprintf("%d %s https://www.youtube.com", timestamp, pair[1])),
			)
			output = append(
				output,
				fmt.Sprintf("%s %d_%s", pair[0], timestamp, hex.EncodeToString(hash[:])),
			)
		}
	}
	return strings.Join(output, " ")
}

func cookiePathMatches(path, cookiePath string) bool {
	if path == cookiePath {
		return true
	}
	if !strings.HasPrefix(path, cookiePath) {
		return false
	}
	return strings.HasSuffix(cookiePath, "/") ||
		strings.HasPrefix(strings.TrimPrefix(path, cookiePath), "/")
}

func cookieNameByte(c byte) bool {
	return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') ||
		strings.ContainsRune("!#$%&'*+-.^_`|~", rune(c))
}

func parseNetscapeCookie(line string, index int) (*storedCookie, error) {
	line = strings.TrimSuffix(line, "\r")
	httpOnly := false
	if rest, ok := strings.CutPrefix(line, "#HttpOnly_"); ok {
		line = rest
		httpOnly = true
	} else if cookieExportComment(line) {
		return nil, nil
	}
	fields := strings.Split(line, "\t")
	invalid := func() error {
		//nolint:revive,staticcheck // Preserve existing end-user guidance.
		return fmt.Errorf(
			"Invalid cookies.txt row %d. Export cookies in Netscape format again.",
			index+firstCookieDataRow,
		)
	}
	if len(fields) != netscapeCookieFields {
		return nil, invalid()
	}
	host := strings.ToLower(strings.TrimLeft(fields[0], "."))
	if !youtubeDomain(host) {
		return nil, nil
	}
	sub, path, secure := fields[1], fields[2], fields[3]
	expires, err := netscapeExpiry(fields)
	if err != nil {
		return nil, invalid()
	}
	if !validNetscapeLocation(host, path) {
		return nil, invalid()
	}
	// #nosec G124 -- Preserve imported cookie attributes; this jar only sends cookies to HTTPS YouTube origins.
	cookie := &http.Cookie{
		Name:     fields[5],
		Value:    fields[6],
		Path:     path,
		Secure:   secure == netscapeTrue,
		HttpOnly: httpOnly,
	}
	if sub == netscapeTrue {
		cookie.Domain = host
	}
	if expires != 0 {
		cookie.Expires = time.Unix(expires, 0).UTC()
		if cookie.Expires.Year() > maximumCookieYear {
			return nil, invalid()
		}
		if !cookie.Expires.After(time.Now()) {
			return nil, nil
		}
	}
	entry := makeStoredCookie(cookie, host, path)
	if !entry.valid() {
		return nil, invalid()
	}
	return &entry, nil
}

func validNetscapeLocation(host, path string) bool {
	for _, c := range []byte(path) {
		if c <= 0x20 || c >= 0x7f || c == ';' {
			return false
		}
	}
	for _, c := range []byte(host) {
		if (c < 'a' || c > 'z') && (c < '0' || c > '9') && c != '.' && c != '-' {
			return false
		}
	}
	return true
}

func (c storedCookie) matches(u *url.URL) bool {
	hostMatches := u.Hostname() == c.host() ||
		(c.Domain["Suffix"] != "" && strings.HasSuffix(u.Hostname(), "."+c.host()))
	path := u.EscapedPath()
	if path == "" {
		path = "/"
	}
	pathMatches := cookiePathMatches(path, c.path())
	return hostMatches && pathMatches
}

func responseCookies(u *url.URL, headers []string) map[string]storedCookie {
	updates := make(map[string]storedCookie)
	for _, header := range headers {
		cookie, err := http.ParseSetCookie(header)
		if err != nil {
			continue
		}
		domain := strings.TrimLeft(strings.ToLower(cookie.Domain), ".")
		if domain != "" &&
			(domain != u.Hostname() && !strings.HasSuffix(u.Hostname(), "."+domain)) {
			continue
		}
		path := cookie.Path
		if !strings.HasPrefix(path, "/") {
			path = u.Path
			if i := strings.LastIndex(path, "/"); i > 0 {
				path = path[:i]
			} else {
				path = "/"
			}
		}
		stored := makeStoredCookie(cookie, u.Hostname(), path)
		if stored.valid() {
			updates[stored.key()] = stored
		}
	}
	return updates
}

func mergeCookie(current *cookieSnapshot, cookie storedCookie, key string) error {
	entry := cookieEntry{cookie, uuid.NewString()}
	found := false
	for i, existing := range current.Entries {
		if existing.Cookie.key() == key {
			current.Entries[i] = entry
			found = true
			break
		}
	}
	if !found {
		if len(current.Entries) >= maximumCookies {
			return errors.New("YouTube cookie jar is full; import a fresh export")
		}
		current.Entries = append(current.Entries, entry)
	}
	return nil
}

func netscapeExpiry(fields []string) (int64, error) {
	sub, path, secure := fields[1], fields[2], fields[3]
	expires, err := strconv.ParseInt(fields[4], 10, 64)
	if err != nil || expires < 0 || (sub != netscapeTrue && sub != "FALSE") ||
		(secure != netscapeTrue && secure != "FALSE") ||
		!strings.HasPrefix(path, "/") ||
		!validCookiePair(fields[5], fields[6]) {
		return 0, errors.New("invalid Netscape cookie fields")
	}
	return expires, nil
}

func acceptResponseCookies(base cookieSnapshot, u *url.URL, headers []string) bool {
	return base.Enabled && u.Scheme == httpsScheme && youtubeDomain(u.Hostname()) &&
		len(headers) > 0
}

func cookieExportComment(line string) bool {
	return strings.HasPrefix(line, "#") || strings.TrimSpace(line) == ""
}

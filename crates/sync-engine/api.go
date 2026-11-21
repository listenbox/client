package syncengine

import (
	"bytes"
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net"
	"net/http"
	"net/http/httptrace"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/jonboulle/clockwork"
	p "github.com/listenbox/client/crates/sync-engine/publicapi"
)

var (
	//nolint:revive,staticcheck // Preserve existing end-user guidance.
	ErrAuthentication = errors.New(
		"Sign in to Listenbox to continue.",
	)
	//nolint:staticcheck // Preserve existing end-user guidance.
	ErrSyncStopped = errors.New(
		"The server stopped YouTube importing for this show. Progress is saved; refresh t" +
			"he podcast list.",
	)
	//nolint:revive,staticcheck // Preserve existing end-user guidance.
	ErrMediaUnavailable = errors.New(
		"YouTube refused the media download (HTTP 403). Try syncing again.",
	)
	//nolint:staticcheck // Preserve existing end-user guidance.
	ErrSignInRequired = errors.New(
		"YouTube needs a signed-in session. Open Settings → YouTube to paste fresh cook" +
			"ies, or run listenbox youtube cookies import <cookies.txt>, then sync again.",
	)
	// ErrRateLimited reports that YouTube throttled this session's player
	// requests; YouTube lifts it after up to an hour.
	ErrRateLimited = errors.New("YouTube is rate-limiting this computer")
)

type HTTPError struct {
	Status  int
	Message string
	Body    json.RawMessage
}

const (
	maximumIdleConnections        = 128
	maximumIdleConnectionsPerHost = 32
	maximumRedirects              = 10
	idleConnectionTimeout         = 90 * time.Second
	httpConnectTimeout            = 30 * time.Second
	httpRequestTimeout            = 30 * time.Minute
	errorResponseLimit            = 64 << 10
	jsonResponseLimit             = 4 << 20
)

func (e *HTTPError) Error() string { return fmt.Sprintf("HTTP %d: %s", e.Status, e.Message) }
func (e *HTTPError) Is(target error) bool {
	return (e.Status == 401 && target == ErrAuthentication) ||
		(e.Status == 423 && target == ErrSyncStopped)
}

type API struct {
	Config     Config
	Credential string
	HTTP       *http.Client
	UploadHTTP *http.Client
	Public     *p.Client
	Clock      clockwork.Clock
}

type StoredAuth struct {
	APIKey string `json:"api_key"`
}

func LoadAuth(config Config) string {
	raw, err := os.ReadFile(filepath.Join(config.Directory, "auth.json"))
	if err != nil || len(raw) > errorResponseLimit {
		return ""
	}
	var auth StoredAuth
	if json.Unmarshal(raw, &auth) != nil || !CredentialValid(auth.APIKey) {
		return ""
	}
	return auth.APIKey
}

func Logout(config Config) error {
	err := os.Remove(filepath.Join(config.Directory, "auth.json"))
	if os.IsNotExist(err) {
		return nil
	}
	return err
}

func NewHTTPClients() (*http.Client, *http.Client, error) {
	roots, err := x509.SystemCertPool()
	if err != nil {
		return nil, nil, fmt.Errorf("load system certificates: %w", err)
	}
	if path := os.Getenv("SSL_CERT_FILE"); path != "" {
		// #nosec G703 -- The user explicitly selects this local config, profile or certificate file.
		raw, err := os.ReadFile(path)
		if err != nil {
			return nil, nil, err
		}
		if !roots.AppendCertsFromPEM(raw) {
			return nil, nil, fmt.Errorf("SSL_CERT_FILE %q contains no certificates", path)
		}
	}
	transport := &http.Transport{
		Proxy:                 http.ProxyFromEnvironment,
		DialContext:           dialOwnedConnection,
		TLSClientConfig:       &tls.Config{RootCAs: roots, MinVersion: tls.VersionTLS12},
		ForceAttemptHTTP2:     true,
		MaxIdleConns:          maximumIdleConnections,
		MaxIdleConnsPerHost:   maximumIdleConnectionsPerHost,
		IdleConnTimeout:       idleConnectionTimeout,
		TLSHandshakeTimeout:   httpConnectTimeout,
		ResponseHeaderTimeout: httpConnectTimeout,
	}
	redirect := func(req *http.Request, via []*http.Request) error {
		if len(via) >= maximumRedirects {
			return errors.New("too many HTTP redirects")
		}
		if len(via) > 0 && req.URL.Host != via[0].URL.Host {
			req.Header.Del("Authorization")
			req.Header.Del("Cookie")
		}
		return nil
	}
	return &http.Client{
		Transport:     transport,
		Timeout:       httpRequestTimeout,
		CheckRedirect: redirect,
	}, &http.Client{
		Transport:     transport.Clone(),
		CheckRedirect: redirect,
	}, nil
}

func NewAPI(config Config) (*API, error) {
	httpClient, upload, err := NewHTTPClients()
	if err != nil {
		return nil, err
	}
	return NewAPIWithClients(config, httpClient, upload)
}

func NewAPIWithClients(config Config, httpClient, upload *http.Client) (*API, error) {
	api := &API{
		Config:     config,
		Credential: LoadAuth(config),
		HTTP:       httpClient,
		UploadHTTP: upload,
		Clock:      clockwork.NewRealClock(),
	}
	client, err := p.NewClient(
		p.ClientOptions{BaseURL: config.APIOrigin},
		p.WithHTTPClient(p.HTTPClientFunc(api.Do)),
		p.WithResponseTimeout(httpRequestTimeout),
		p.WithSSEMaxRetries(0),
		p.WithSSEReconnectOnStreamEnd(false),
		p.WithRequestEditorFn(func(_ context.Context, req *http.Request) error {
			if api.Credential != "" {
				req.Header.Set("Authorization", "Bearer "+api.Credential)
			}
			return nil
		}),
	)
	api.Public = client
	return api, err
}

func (api *API) Do(request *http.Request) (*http.Response, error) {
	response, err := api.send(api.HTTP, request)
	if err != nil {
		return nil, err
	}
	if response.StatusCode < 200 || response.StatusCode >= 300 {
		return nil, responseError(response)
	}
	return response, nil
}

func (api *API) JSON(ctx context.Context, method, path string, body, out any) error {
	var reader io.Reader
	if body != nil {
		raw, err := json.Marshal(body)
		if err != nil {
			return err
		}
		reader = bytes.NewReader(raw)
	}
	request, err := http.NewRequestWithContext(ctx, method, api.Config.APIOrigin+path, reader)
	if err != nil {
		return err
	}
	request.Header.Set("Accept", "application/json")
	if body != nil {
		request.Header.Set("Content-Type", "application/json")
	}
	if api.Credential != "" {
		request.Header.Set("Authorization", "Bearer "+api.Credential)
	}
	response, err := api.Do(request)
	if err != nil {
		return err
	}
	defer response.Body.Close()
	raw, err := ReadBounded(response.Body, jsonResponseLimit)
	if err != nil {
		return err
	}
	if out != nil {
		if err := json.Unmarshal(raw, out); err != nil {
			return fmt.Errorf("decode %s %s: %w", method, path, err)
		}
	}
	return nil
}

// Transport detaches in-flight connection setup from a request so another
// request can reuse it. Keep CONNECT/TLS setup owned until GotConn transfers
// the connection to Transport; cancelling a runtime must close that socket.
type (
	dialOwnerKey    struct{}
	ownedConnection struct {
		net.Conn

		release func() bool
	}
)

func (conn *ownedConnection) Close() error { conn.release(); return conn.Conn.Close() }

//nolint:contextcheck // Restore the request owner carried through Transport's detached CONNECT/TLS context.
func dialOwnedConnection(ctx context.Context, network, address string) (net.Conn, error) {
	owner, ok := ctx.Value(dialOwnerKey{}).(context.Context)
	if !ok {
		owner = ctx
	}
	dialCtx, cancel := context.WithCancel(ctx)
	stop := context.AfterFunc(owner, cancel)
	conn, err := (&net.Dialer{Timeout: httpConnectTimeout, KeepAlive: httpConnectTimeout}).DialContext(
		dialCtx,
		network,
		address,
	)
	stop()
	cancel()
	if err != nil {
		return nil, err
	}
	return &ownedConnection{
		Conn:    conn,
		release: context.AfterFunc(owner, func() { _ = conn.Close() }),
	}, nil
}

func responseError(response *http.Response) error {
	defer response.Body.Close()
	raw, err := ReadBounded(response.Body, errorResponseLimit)
	if err != nil {
		return err
	}
	message := strings.TrimSpace(string(raw))
	var body struct {
		Message string `json:"message"`
	}
	if json.Unmarshal(raw, &body) == nil && body.Message != "" {
		message = body.Message
	}
	if response.StatusCode == http.StatusForbidden {
		message = "permission denied"
	}
	return &HTTPError{response.StatusCode, Redact(message), raw}
}

func ReadBounded(reader io.Reader, limit int64) ([]byte, error) {
	raw, err := io.ReadAll(io.LimitReader(reader, limit+1))
	if err != nil {
		return nil, err
	}
	if int64(len(raw)) > limit {
		return nil, fmt.Errorf("HTTP response exceeds %d bytes", limit)
	}
	return raw, nil
}

func (api *API) TeamID(ctx context.Context) (string, error) {
	var identity p.APIKeyWhoami
	err := api.JSON(ctx, "GET", "/s/whoami", nil, &identity)
	return identity.TeamId, err
}

func (api *API) send(client *http.Client, request *http.Request) (*http.Response, error) {
	started := time.Now()
	owner := request.Context()
	ctx := context.WithValue(owner, dialOwnerKey{}, owner)
	ctx = httptrace.WithClientTrace(
		ctx,
		&httptrace.ClientTrace{GotConn: func(info httptrace.GotConnInfo) {
			conn := info.Conn
			if tlsConn, ok := conn.(*tls.Conn); ok {
				conn = tlsConn.NetConn()
			}
			if owned, ok := conn.(*ownedConnection); ok {
				owned.release()
			}
		}},
	)
	request = request.WithContext(ctx)
	// #nosec G704 -- Client requests use explicitly configured API and provider endpoints.
	response, err := client.Do(request)
	api.logRequest(request, response, started)
	if err != nil {
		if urlError, ok := errors.AsType[*url.Error](err); ok {
			return nil, fmt.Errorf("send HTTP request: %w", urlError.Err)
		}
		return nil, err
	}
	return response, nil
}

func isStatus(err error, status int) bool {
	var failure *HTTPError
	return errors.As(err, &failure) && failure.Status == status
}

func (api *API) logRequest(request *http.Request, response *http.Response, started time.Time) {
	if strings.HasSuffix(request.URL.Hostname(), "googlevideo.com") {
		return
	}

	route := ""
	if request.URL.Scheme+"://"+request.URL.Host == api.Config.APIOrigin {
		route = request.URL.Path
		if strings.HasPrefix(route, "/cli/authorizations/") {
			route = "/cli/authorizations/{code}/events"
		}
	}
	status := 0
	trace := "-"
	if response != nil {
		status = response.StatusCode
		trace = response.Header.Get("X-Trace-Id")
	}
	if Production != "1" {
		fmt.Fprintf(
			os.Stderr,
			"HTTP %s %s://%s%s -> %d (%dms) trace_id=%s\n",
			request.Method,
			request.URL.Scheme,
			request.URL.Host,
			route,
			status,
			time.Since(started).Milliseconds(),
			trace,
		)
	}
	slog.Info(
		"HTTP completed",
		"method",
		request.Method,
		"host",
		request.URL.Host,
		"route",
		route,
		"status",
		status,
		"elapsed_ms",
		time.Since(started).Milliseconds(),
		"trace_id",
		trace,
	)
}

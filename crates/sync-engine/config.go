package syncengine

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"runtime"
	"strconv"
	"strings"

	buildmeta "github.com/listenbox/client"
	"go.yaml.in/yaml/v3"
)

var (
	Production   = "0"
	Version      string
	SourceCommit string
)

// BuildVersion uses the embedded release identity unless a build overrides it.
func BuildVersion() string {
	if Version != "" {
		return Version
	}
	return buildmeta.Version()
}

type Config struct {
	APIOrigin       string `json:"api_origin"       yaml:"api_origin"`
	DashboardOrigin string `json:"dashboard_origin" yaml:"dashboard_origin"`
	Directory       string `json:"-"                yaml:"-"`
}

const (
	profileDirectoryMode = 0o700
	privateFileMode      = 0o600
)

const (
	httpScheme        = "http"
	httpsScheme       = "https"
	fileScheme        = "file"
	windowsOS         = "windows"
	netscapeTrue      = "TRUE"
	youtubeDomainName = "youtube.com"
	youtubeWebHost    = "www.youtube.com"
	mp4ContentType    = "video/mp4"
	completedStatus   = "completed"
	preparedAudioName = "audio.m4a"
	faststartFlag     = "+faststart"
)

func LoadConfig(explicit string) (Config, error) {
	directory, present := os.LookupEnv("LISTENBOX_PROFILE_DIR")
	if present && directory == "" {
		return Config{}, errors.New("LISTENBOX_PROFILE_DIR is empty")
	}
	if !present {
		home, err := os.UserHomeDir()
		if err != nil {
			return Config{}, err
		}
		directory = defaultProfileDirectory(home)
	}
	return LoadConfigIn(explicit, directory)
}

func LoadConfigIn(explicit, directory string) (Config, error) {
	config := Config{
		APIOrigin:       "https://v1.listenbox.app",
		DashboardOrigin: "https://web.listenbox.app",
		Directory:       directory,
	}
	path := explicit
	if path == "" {
		path = filepath.Join(directory, "config.yaml")
	}
	// #nosec G703 -- The user explicitly selects this local config, profile or certificate file.
	raw, err := os.ReadFile(path)
	if err != nil && (explicit != "" || !os.IsNotExist(err)) {
		return Config{}, fmt.Errorf("load client config %q: %w", path, err)
	}
	if err == nil {
		config.APIOrigin, config.DashboardOrigin = "", ""
		decoder := yaml.NewDecoder(bytes.NewReader(raw))
		decoder.KnownFields(true)
		if err := decoder.Decode(&config); err != nil {
			if strings.Contains(err.Error(), "not found in type") {
				return Config{}, fmt.Errorf("decode client config %q: unknown field: %w", path, err)
			}
			return Config{}, fmt.Errorf("decode client config %q: %w", path, err)
		}
		var extra any
		if err := decoder.Decode(&extra); !errors.Is(err, io.EOF) {
			return Config{}, fmt.Errorf("decode client config %q: expected one YAML document", path)
		}
	}
	config.Directory = directory
	if config.APIOrigin, err = validateOrigin(config.APIOrigin); err != nil {
		return Config{}, fmt.Errorf("validate api_origin: %w", err)
	}
	if config.DashboardOrigin, err = validateOrigin(config.DashboardOrigin); err != nil {
		return Config{}, fmt.Errorf("validate dashboard_origin: %w", err)
	}
	return config, nil
}

func validateOrigin(value string) (string, error) {
	value = strings.TrimSpace(value)
	u, err := url.Parse(value)
	if err != nil || u == nil || !originAddress(u) || originHasSuffix(u) ||
		strings.ContainsAny(value, "\\?#@") {
		return "", fmt.Errorf(
			"origin %q must be an absolute HTTP(S) origin with only a host and optional port",
			value,
		)
	}
	port := u.Port()
	if port != "" {
		if _, err := strconv.ParseUint(port, 10, 16); err != nil {
			return "", fmt.Errorf("origin %q has an invalid port", value)
		}
	}
	if originUsesDefaultPort(u) {
		u.Host = u.Hostname()
		if strings.Contains(u.Host, ":") {
			u.Host = "[" + u.Host + "]"
		}
	}
	u.Host = strings.ToLower(u.Host)
	u.Path = ""
	return u.String(), nil
}

func (c Config) ShowURL(team, show string) (string, error) {
	if !ValidID(team, "team_") || !ValidID(show, "shw_") {
		return "", fmt.Errorf("invalid show management identifiers %q/%q", team, show)
	}
	return c.DashboardOrigin + "/" + team + "/shows/" + show, nil
}

func (c Config) UpgradeURL(team string) (string, error) {
	if !ValidID(team, "team_") {
		return "", fmt.Errorf("invalid upgrade team identifier %q", team)
	}
	return c.DashboardOrigin + "/" + team + "/upgrade", nil
}

var (
	slugPattern     = regexp.MustCompile(`^[a-z0-9]+(-[a-z0-9]+)*$`)
	languagePattern = regexp.MustCompile(`^[a-z]{2}(-[A-Z]{2})?$`)
	idSuffix        = regexp.MustCompile(`^[0-9a-f]{16}$`)
)

func Nonblank(value string) (string, error) {
	value = strings.TrimSpace(value)
	if value == "" {
		return "", fmt.Errorf("value %q must not be blank", value)
	}
	return value, nil
}

func Slug(value string) (string, error) {
	value = strings.TrimSpace(value)
	if !slugPattern.MatchString(value) {
		return "", fmt.Errorf(
			"slug %q must use lowercase letters, numbers, and single hyphens",
			value,
		)
	}
	return value, nil
}

func Language(value string) (string, error) {
	value = strings.TrimSpace(value)
	if !languagePattern.MatchString(value) {
		return "", fmt.Errorf("language %q must use a code such as en or en-US", value)
	}
	return value, nil
}

func ValidID(value, prefix string) bool {
	return strings.HasPrefix(value, prefix) &&
		idSuffix.MatchString(strings.TrimPrefix(value, prefix))
}

func CredentialValid(value string) bool {
	if value == "" {
		return false
	}
	for _, c := range []byte(value) {
		if c < 0x21 || c > 0x7e {
			return false
		}
	}
	return true
}

func Redact(message string) string {
	words := strings.Fields(message)
	for i, w := range words {
		if strings.Contains(w, "https://") && strings.Contains(w, "?") {
			words[i] = "[URL]"
		}
	}
	return strings.Join(words, " ")
}

func WritePrivateJSON(path string, value any) error {
	raw, err := json.MarshalIndent(value, "", "  ")
	if err != nil {
		return err
	}
	return writePrivateFile(path, append(raw, '\n'))
}

// writePrivateFile durably replaces path with raw, readable only by the user.
func writePrivateFile(path string, raw []byte) error {
	directory := filepath.Dir(path)
	if err := os.MkdirAll(directory, profileDirectoryMode); err != nil {
		return err
	}
	if runtime.GOOS != windowsOS {
		if err := os.Chmod(directory, profileDirectoryMode); err != nil {
			return err
		}
	}
	f, err := os.CreateTemp(directory, ".listenbox-*")
	if err != nil {
		return err
	}
	name := f.Name()
	defer os.Remove(name)
	if _, err = f.Write(raw); err == nil {
		err = f.Sync()
	}
	closeErr := f.Close()
	if err != nil {
		return err
	}
	if closeErr != nil {
		return closeErr
	}
	if err := os.Rename(name, path); err != nil {
		return fmt.Errorf("replace %q: %w", path, err)
	}
	if runtime.GOOS != windowsOS {
		d, err := os.Open(directory)
		if err != nil {
			return err
		}
		defer d.Close()
		return d.Sync()
	}
	return nil
}

func originAddress(u *url.URL) bool {
	return (u.Scheme == httpScheme || u.Scheme == httpsScheme) && u.Hostname() != "" &&
		u.User == nil
}

func originHasSuffix(u *url.URL) bool {
	return (u.Path != "" && u.Path != "/") || u.RawQuery != "" || u.ForceQuery || u.Fragment != ""
}

func originUsesDefaultPort(u *url.URL) bool {
	return (u.Scheme == httpsScheme && u.Port() == "443") ||
		(u.Scheme == httpScheme && u.Port() == "80")
}

func defaultProfileDirectory(home string) string {
	var directory string
	if Production != "1" {
		directory = filepath.Join(home, ".cache", "listenbox", "dev")
	} else {
		switch runtime.GOOS {
		case "darwin":
			directory = filepath.Join(home, "Library", "Application Support", "Listenbox")
		case windowsOS:
			directory = filepath.Join(os.Getenv("LOCALAPPDATA"), "Listenbox")
		default:
			base := os.Getenv("XDG_DATA_HOME")
			if base == "" {
				base = filepath.Join(home, ".local", "share")
			}
			directory = filepath.Join(base, "listenbox")
		}
	}
	return directory
}

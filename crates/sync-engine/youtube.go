package syncengine

import (
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"net/http"
	"net/url"
	"os"
	"path"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"time"
	"unicode"

	"github.com/google/uuid"
	"github.com/gosimple/slug"
	p "github.com/listenbox/client/crates/sync-engine/publicapi"
	"github.com/listenbox/client/crates/youtubei"
	"golang.org/x/sync/errgroup"
)

const (
	playerResponseLimit   = 8 << 20
	maximumShowSlugLength = 63
	maximumPartWorkers    = 4
	secondsPerHour        = 3600
)

func osCreateProfile(directory string) error { return os.MkdirAll(directory, profileDirectoryMode) }
func IsYouTubeSource(source string) bool {
	u, err := url.Parse(source)
	return err == nil &&
		(u.Hostname() == youtubeDomainName || u.Hostname() == youtubeWebHost ||
			u.Hostname() == "m.youtube.com" || u.Hostname() == "youtu.be")
}

func CollectionSource(source string) (string, error) {
	u, err := url.Parse(strings.TrimSpace(source))
	if err != nil || !validCollectionURL(u, source) {
		//nolint:staticcheck // Preserve existing end-user guidance.
		return "", errors.New(
			"Enter an HTTPS YouTube playlist or channel URL",
		)
	}
	if path, err := channelPath(u); err != nil {
		return "", err
	} else if path != "" {
		if u.Query().Has("list") || u.Query().Has("v") {
			return "", errors.New("channel URL cannot contain a playlist or video ID")
		}
		return "https://www.youtube.com" + path, nil
	}
	ids := u.Query()["list"]
	if u.Path != "/playlist" || len(ids) != 1 || len(ids[0]) < 2 || len(ids[0]) > 200 ||
		!validYouTubeID(ids[0]) {
		//nolint:staticcheck // Preserve existing end-user guidance.
		return "", errors.New(
			"Enter a public YouTube playlist or channel URL",
		)
	}
	return "https://www.youtube.com/playlist?list=" + ids[0], nil
}

func channelPath(u *url.URL) (string, error) {
	path := strings.TrimSuffix(strings.TrimRight(u.Path, "/"), "/videos")
	if id, ok := strings.CutPrefix(path, "/channel/"); ok {
		if !validChannelID(id) {
			return "", errors.New("invalid YouTube channel ID")
		}
		return "/channel/" + id, nil
	}
	if handle, ok := strings.CutPrefix(path, "/@"); ok {
		if handle == "" || len([]rune(handle)) > 100 {
			return "", errors.New("invalid YouTube channel handle")
		}
		for _, c := range handle {
			if !unicode.IsLetter(c) && !unicode.IsNumber(c) && c != '.' && c != '_' && c != '-' {
				return "", errors.New("invalid YouTube channel handle")
			}
		}
		return "/@" + url.PathEscape(handle), nil
	}
	return "", nil
}

func validYouTubeID(id string) bool {
	if id == "" {
		return false
	}
	for _, c := range id {
		if (c < 'a' || c > 'z') && (c < 'A' || c > 'Z') && (c < '0' || c > '9') && c != '_' &&
			c != '-' {
			return false
		}
	}
	return true
}

func validChannelID(id string) bool {
	return len(id) == 24 && strings.HasPrefix(id, "UC") && validYouTubeID(id)
}

func collectionPlaylistID(u *url.URL) string {
	if id, ok := strings.CutPrefix(u.Path, "/channel/"); ok && validChannelID(id) {
		return "UU" + id[2:]
	}
	return u.Query().Get("list")
}

var initialDataPattern = regexp.MustCompile(`<script\b[^>]*>\s*(?:var\s+)?ytInitialData\s*=\s*`)

func (api *API) resolveChannel(ctx context.Context, collection string) (string, error) {
	u, _ := url.Parse(collection)
	if !strings.HasPrefix(u.Path, "/@") {
		return collection, nil
	}
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, collection, nil)
	if err != nil {
		return "", err
	}
	client := *api.HTTP
	client.CheckRedirect = func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }
	response, err := api.send(&client, request)
	if err != nil {
		return "", err
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusOK {
		return "", fmt.Errorf("YouTube channel page returned HTTP %d", response.StatusCode)
	}
	raw, err := ReadBounded(response.Body, playerResponseLimit)
	if err != nil {
		return "", fmt.Errorf("read YouTube channel page: %w", err)
	}
	match := initialDataPattern.FindIndex(raw)
	if match == nil {
		return "", errors.New("YouTube channel page has no metadata")
	}
	var data struct {
		Metadata struct {
			Channel struct {
				ID string `json:"externalId"`
			} `json:"channelMetadataRenderer"`
		} `json:"metadata"`
	}
	if err := json.NewDecoder(strings.NewReader(string(raw[match[1]:]))).Decode(&data); err != nil {
		return "", fmt.Errorf("invalid YouTube channel metadata: %w", err)
	}
	if !validChannelID(data.Metadata.Channel.ID) {
		return "", errors.New("YouTube channel metadata has no valid channel ID")
	}
	return "https://www.youtube.com/channel/" + data.Metadata.Channel.ID, nil
}

type ImportEvent struct {
	Stage                 string
	Scan                  *PlaylistScan
	Show                  *p.Show
	Existing              bool
	VideoRemainingSeconds *int64
}

func (e *Engine) Import(
	ctx context.Context,
	api *API,
	source, team, requestedSlug string,
	kind p.ShowSourceKind,
	event func(ImportEvent),
) (p.Show, Report, error) {
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	emit := func(value ImportEvent) {
		if event != nil {
			event(value)
		}
	}
	empty := p.Show{}
	var report Report
	source = strings.TrimSpace(source)
	admission, err := api.admitImport(ctx, source, team, requestedSlug, kind, emit)
	if err != nil {
		return empty, report, err
	}
	if admission.existing != nil {
		emit(ImportEvent{Show: admission.existing, Existing: true})
		return *admission.existing, report, nil
	}
	return e.importAdmitted(ctx, api, source, team, requestedSlug, kind, emit, admission)
}

func randomPlaylistSlug() string {
	return "youtube-" + strings.ReplaceAll(uuid.NewString(), "-", "")[:16]
}

func (e *Engine) importVideo(
	ctx context.Context,
	api *API,
	youtube *YouTube,
	journal *Database,
	show, collection string,
	video Video,
	audio bool,
	maxHeight int,
	language, id string,
) (string, error) {
	operation, err := journal.Operation(ctx, api.Config.APIOrigin, show, video.URL(), collection)
	if err != nil {
		return "", err
	}
	directory, err := journal.Directory(operation)
	if err != nil {
		return "", err
	}
	manifest, err := journal.Prepared(ctx, operation)
	if err != nil {
		return "", err
	}
	transfer := videoImport{
		engine: e, api: api, youtube: youtube, journal: journal,
		show: show, collection: collection, video: video, audio: audio, maxHeight: maxHeight,
		language: language, id: id, operation: operation, directory: directory,
	}
	if manifest == nil {
		var reason string
		manifest, reason, err = transfer.prepare(ctx)
		if err != nil || reason != "" {
			return reason, err
		}
	} else if Production != "1" {
		fmt.Fprintf(
			os.Stderr,
			"FFmpeg preparation reused show_slug=%s video_id=%s operation_id=%s\n",
			show,
			video.ID,
			operation,
		)
	}
	segments, total, err := packageUploadPlan(directory, manifest)
	if err != nil {
		return "", err
	}
	e.Downloads.update(id, func(item *Download) { item.UploadTotal = total; item.Uploaded = 0 })
	release, err := e.Downloads.acquireStage(ctx, id, UploadStage())
	if err != nil {
		return "", err
	}
	err = e.uploadPackage(ctx, api, journal, directory, *manifest, segments, id)
	release(err)
	if err != nil {
		return "", err
	}
	if err := journal.Forget(ctx, api.Config.APIOrigin, show, video.URL()); err != nil {
		return "", err
	}
	return "", nil
}

func packageInventory(root string, audio bool) ([]p.PreparedMediaObject, error) {
	var objects []p.PreparedMediaObject
	err := filepath.WalkDir(root, func(path string, entry os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if entry.IsDir() {
			return nil
		}
		if !entry.Type().IsRegular() {
			return errors.New("media package contains a non-regular object")
		}
		name, err := filepath.Rel(root, path)
		if err != nil {
			return err
		}
		name = filepath.ToSlash(name)
		if name != preparedAudioName &&
			(audio || (name != "video.mp4" && !strings.HasPrefix(name, "hls/"))) {
			return nil
		}
		if packageSegmentName(name) {
			return nil
		}
		contentType := packageContentType(name)
		info, err := entry.Info()
		if err != nil {
			return err
		}
		objects = append(
			objects,
			p.PreparedMediaObject{Name: name, ContentType: contentType, ByteLength: info.Size()},
		)
		return nil
	})
	slices.SortFunc(
		objects,
		func(a, b p.PreparedMediaObject) int { return strings.Compare(a.Name, b.Name) },
	)
	return objects, err
}

var packageRenditionDirectory = regexp.MustCompile(`^hls/[A-Za-z0-9_-]+/$`)

// packageSegmentName reports a normal rendition object. Segments are uploaded
// with single-request PUTs and have no manifest entry: their playlists, which
// are manifest objects, name them.
func packageSegmentName(name string) bool {
	directory, file := path.Split(name)
	if !packageRenditionDirectory.MatchString(directory) {
		return false
	}
	digits, numbered := strings.CutSuffix(file, ".m4s")
	if numbered {
		ordinal, err := strconv.Atoi(digits)
		numbered = err == nil && ordinal >= 0 && len(digits) <= 9 && strconv.Itoa(ordinal) == digits
	}
	return file == hlsInitName || numbered
}

// packageSegments lists the prepared segment objects in a stable order.
// packageUploadPlan returns the package's segments and the bytes of every
// object the upload sends.
func packageUploadPlan(
	root string,
	manifest *p.CreateEpisodePackage,
) ([]p.EpisodePackageSegment, int64, error) {
	segments, err := packageSegments(root)
	if err != nil {
		return nil, 0, err
	}
	total := int64(0)
	for _, object := range manifest.Objects {
		total += object.ByteLength
	}
	for _, segment := range segments {
		total += segment.ByteLength
	}
	return segments, total, nil
}

func packageSegments(root string) ([]p.EpisodePackageSegment, error) {
	var segments []p.EpisodePackageSegment
	hls := filepath.Join(root, "hls")
	err := filepath.WalkDir(hls, func(file string, entry os.DirEntry, err error) error {
		if errors.Is(err, fs.ErrNotExist) && file == hls {
			return fs.SkipDir
		}
		if err != nil || entry.IsDir() {
			return err
		}
		name, err := filepath.Rel(root, file)
		if err != nil {
			return err
		}
		name = filepath.ToSlash(name)
		if !packageSegmentName(name) {
			return nil
		}
		if !entry.Type().IsRegular() {
			return errors.New("media package contains a non-regular object")
		}
		info, err := entry.Info()
		if err != nil {
			return err
		}
		segments = append(segments, p.EpisodePackageSegment{
			Name: name, ContentType: packageContentType(name), ByteLength: info.Size(),
		})
		return nil
	})
	slices.SortFunc(
		segments,
		func(a, b p.EpisodePackageSegment) int { return strings.Compare(a.Name, b.Name) },
	)
	return segments, err
}

func (e *Engine) uploadPackage(
	ctx context.Context,
	api *API,
	journal *Database,
	directory string,
	manifest p.CreateEpisodePackage,
	segments []p.EpisodePackageSegment,
	id string,
) error {
	var session p.EpisodePackage
	if err := api.JSON(ctx, "POST", "/s/episode-packages", manifest, &session); err != nil {
		return err
	}
	if session.Status == completedStatus {
		return nil
	}
	if err := journal.Session(ctx, manifest.OperationId, session.UploadSessionId); err != nil {
		return err
	}
	if session.PartSize < 5<<20 {
		return errors.New("invalid package part size")
	}
	completed := completedPackageObjects(session)
	transfer := packageUpload{
		engine: e, api: api, journal: journal, directory: directory,
		manifest: manifest, id: id, session: session,
	}
	saved, err := transfer.savedBytes(ctx, completed)
	if err != nil {
		return err
	}
	pending, savedSegments, err := transfer.pendingSegments(ctx, segments)
	if err != nil {
		return err
	}
	e.Downloads.update(id, func(item *Download) { item.Uploaded = saved + savedSegments })
	var group errgroup.Group
	group.SetLimit(maximumPartWorkers)
	for ordinal, object := range manifest.Objects {
		if completed[ordinal] {
			continue
		}
		for n := int64(1); n <= (object.ByteLength+session.PartSize-1)/session.PartSize; n++ {
			number := int32(n)
			group.Go(func() error {
				return transfer.uploadPart(ctx, ordinal, object, number)
			})
		}
	}
	if err := group.Wait(); err != nil {
		return err
	}
	if err := transfer.uploadSegments(ctx, pending); err != nil {
		return err
	}
	e.Downloads.update(id, func(item *Download) { item.Phase = Publishing() })
	return api.completePackage(ctx, session.UploadSessionId)
}

func validCollectionURL(u *url.URL, source string) bool {
	return len(source) <= 2048 && u.Scheme == httpsScheme &&
		(u.Host == youtubeDomainName || u.Host == youtubeWebHost) && u.User == nil
}

func parseImportSource(source string) (string, string, error) {
	u, err := url.Parse(source)
	if err != nil || !validImportURL(source, u) {
		return "", "", errors.New("invalid YouTube source URL")
	}
	channel, err := channelPath(u)
	if err != nil {
		return "", "", err
	}
	collection := ""
	videoID := ""
	if collectionImportURL(u, channel) {
		collection, err = CollectionSource(source)
		if err != nil {
			return "", "", err
		}
	} else {
		switch {
		case u.Hostname() == "youtu.be":
			videoID = strings.Trim(u.Path, "/")
		case u.Query().Has("v"):
			videoID = u.Query().Get("v")
		case strings.HasPrefix(u.Path, "/shorts/"):
			videoID = strings.TrimPrefix(u.Path, "/shorts/")
		case strings.HasPrefix(u.Path, "/embed/"):
			videoID = strings.TrimPrefix(u.Path, "/embed/")
		}
		if !validVideoID(videoID) {
			return "", "", errors.New("invalid YouTube video ID")
		}
	}
	return collection, videoID, nil
}

func (yt *YouTube) importSnapshot(
	ctx context.Context,
	collection, videoID, source string,
	kind p.ShowSourceKind,
	preparation *ImportEvent,
	emit func(ImportEvent),
) (string, *youtubei.Playlist, PlaylistSnapshot, error) {
	title := ""
	var page *youtubei.Playlist
	var snapshot PlaylistSnapshot
	if collection != "" {
		return yt.importCollectionSnapshot(ctx, collection, kind, preparation, emit)
	}
	media, reason, err := yt.Media(ctx, videoID, source)
	if err != nil {
		return "", nil, PlaylistSnapshot{}, err
	}
	if reason != "" {
		return "", nil, PlaylistSnapshot{}, fmt.Errorf(
			"YouTube playback unavailable: %s",
			reason,
		)
	}
	title = media.Title
	snapshot = PlaylistSnapshot{
		Title:      title,
		ArtworkURL: media.ArtworkURL,
		Present: []Video{
			{
				ID:              videoID,
				Title:           title,
				SourceTitle:     &title,
				DurationSeconds: media.DurationSeconds,
			},
		},
		EstimatedSeconds: media.EstimatedSeconds,
		CanRemove:        true,
	}
	return title, page, snapshot, nil
}

func importShowSlug(requestedSlug, collection, title, source string) (string, bool) {
	showSlug := requestedSlug
	titleSlug := false
	if showSlug == "" {
		if collection != "" {
			showSlug = slug.Make(title)
			if len(showSlug) > maximumShowSlugLength {
				showSlug = strings.TrimRight(showSlug[:63], "-")
			}
			titleSlug = showSlug != ""
		} else {
			hash := sha256.Sum256([]byte(source))
			showSlug = fmt.Sprintf("youtube-%x", hash[:8])
		}
	}
	if showSlug == "" {
		showSlug = randomPlaylistSlug()
	}
	return showSlug, titleSlug
}

func (api *API) createImportShow(
	ctx context.Context,
	source, team, requestedSlug string,
	kind p.ShowSourceKind,
	title string,
	admission importAdmission,
) (p.Show, bool, error) {
	showSlug, titleSlug := importShowSlug(requestedSlug, admission.collection, title, source)
	body := p.CreateShow{
		Id:         "shw_" + strings.ReplaceAll(uuid.NewString(), "-", "")[:16],
		TeamId:     team,
		Title:      title,
		Slug:       showSlug,
		SourceKind: kind,
		Language:   "en",
		YoutubeUrl: &admission.canonical,
	}
	var show p.Show
	for {
		err := api.JSON(ctx, "POST", "/s/shows", body, &show)
		if err == nil {
			break
		}
		var failure *HTTPError
		if errors.As(err, &failure) && failure.Status == 409 {
			existing, conflictErr := api.importConflict(ctx, failure, body.TeamId)
			if conflictErr != nil {
				return p.Show{}, false, conflictErr
			}
			if existing != nil {
				return *existing, true, nil
			}
			if titleSlug {
				titleSlug = false
				body.Slug = randomPlaylistSlug()
				continue
			}
		}
		return p.Show{}, false, fmt.Errorf(
			"create podcast %q: %w. If creation completed but its reply was lost, reload your"+
				" podcasts and resume with sync",
			body.Slug,
			err,
		)
	}
	return show, false, nil
}

func (api *API) importConflict(
	ctx context.Context,
	failure *HTTPError,
	team string,
) (*p.Show, error) {
	var shows []p.Show
	var conflict p.PodcastCreationConflict
	if json.Unmarshal(failure.Body, &conflict) == nil &&
		conflict.ShowAlreadyImported != nil {
		existing := conflict.ShowAlreadyImported
		if existing.TeamId != team {
			return nil, errors.New(
				"import conflict returned another team's podcast",
			)
		}
		if err := api.JSON(ctx, "GET", "/s/shows", nil, &shows); err != nil {
			return nil, err
		}
		for _, candidate := range shows {
			if candidate.TeamId == team && candidate.Id == existing.ShowId {
				return &candidate, nil
			}
		}
		return nil, errors.New(
			"existing imported podcast is no longer available; reload your podcasts",
		)
	}
	return nil, nil
}

type videoImport struct {
	engine                             *Engine
	api                                *API
	youtube                            *YouTube
	journal                            *Database
	show, collection                   string
	video                              Video
	audio                              bool
	maxHeight                          int
	language, id, operation, directory string
}

func (t *videoImport) prepare(ctx context.Context) (*p.CreateEpisodePackage, string, error) {
	media, reason, err := t.download(ctx)
	if err == nil && reason != "" {
		// Nothing was downloaded for a skipped video, so nothing can resume.
		err = t.journal.Forget(ctx, t.api.Config.APIOrigin, t.show, t.video.URL())
	}
	if err != nil || reason != "" {
		return nil, reason, err
	}
	duration, err := t.prepareMedia(ctx, media)
	if err != nil {
		return nil, "", err
	}
	manifest, err := t.checkpoint(ctx, media, duration)
	return manifest, "", err
}

func (t *videoImport) download(ctx context.Context) (*Media, string, error) {
	for _, name := range []string{preparedAudioName, "video.mp4", "hls"} {
		if err := os.RemoveAll(filepath.Join(t.directory, name)); err != nil {
			return nil, "", err
		}
	}
	release, err := t.engine.Downloads.acquireStage(ctx, t.id, DownloadStage())
	if err != nil {
		return nil, "", err
	}
	media, reason, err := t.youtube.Media(ctx, t.video.ID, t.collection)
	if err == nil && reason == "" {
		t.engine.Downloads.update(t.id, func(item *Download) {
			item.Title = media.Title
			if media.DurationSeconds != nil {
				item.DurationSeconds = media.DurationSeconds
			}
		})
		err = t.downloadMedia(ctx, media)
	}
	release(err)
	if err != nil {
		return nil, "", err
	}
	if reason != "" {
		return nil, reason, nil
	}

	return media, "", nil
}

func (t *videoImport) downloadMedia(ctx context.Context, media *Media) error {
	var err error
	for {
		stream := &media.Video
		if t.audio {
			if media.Audio != nil {
				stream = media.Audio
			}
			err = t.api.download(
				ctx,
				*stream,
				filepath.Join(t.directory, "source-audio"),
				t.engine.Downloads,
				t.id,
				t.journal,
				t.operation,
			)
		} else {
			err = t.api.download(
				ctx,
				media.Video,
				filepath.Join(t.directory, "source-video"),
				t.engine.Downloads,
				t.id,
				t.journal,
				t.operation,
			)
			if err == nil && media.Audio != nil {
				stream = media.Audio
				err = t.api.download(
					ctx,
					*media.Audio,
					filepath.Join(t.directory, "source-audio"),
					t.engine.Downloads,
					t.id,
					t.journal,
					t.operation,
				)
			}
		}
		if !errors.Is(err, ErrMediaUnavailable) || ctx.Err() != nil {
			break
		}
		fallback, fallbackErr := t.youtube.FallbackMedia(ctx, t.video.ID, media, stream)
		if fallbackErr != nil {
			err = fallbackErr
			break
		}
		if !fallback {
			break
		}
	}
	return err
}

func (t *videoImport) prepareMedia(ctx context.Context, media *Media) (int64, error) {
	release, err := t.engine.Downloads.acquireStage(ctx, t.id, PrepareStage())
	if err != nil {
		return 0, err
	}
	started := time.Now()
	if Production != "1" {
		fmt.Fprintf(
			os.Stderr,
			"FFmpeg preparation started show_slug=%s video_id=%s operation_id=%s kind=%s title=%q\n",
			t.show,
			t.video.ID,
			t.operation,
			map[bool]string{true: "audio", false: "video"}[t.audio],
			media.Title,
		)
	}
	var duration int64
	if t.audio {
		err = PrepareAudio(
			ctx,
			filepath.Join(t.directory, "source-audio"),
			filepath.Join(t.directory, preparedAudioName),
		)
		if err == nil {
			duration, err = MediaDuration(filepath.Join(t.directory, preparedAudioName))
		}
	} else {
		duration, err = PrepareVideo(ctx, t.directory, media.Audio != nil, t.maxHeight, t.language)
	}
	if Production != "1" {
		result := "success"
		if err != nil {
			result = "failed"
		}
		fmt.Fprintf(
			os.Stderr,
			"FFmpeg preparation finished show_slug=%s video_id=%s operation_id=%s elapsed_ms="+
				"%d result=%s media_duration_seconds=%d error=%v\n",
			t.show,
			t.video.ID,
			t.operation,
			time.Since(started).Milliseconds(),
			result,
			duration,
			err,
		)
	}
	release(err)
	if err != nil {
		return 0, err
	}

	return duration, nil
}

func (t *videoImport) checkpoint(
	ctx context.Context,
	media *Media,
	duration int64,
) (*p.CreateEpisodePackage, error) {
	objects, err := packageInventory(t.directory, t.audio)
	if err != nil {
		return nil, err
	}
	prepared := p.CreateEpisodePackage{
		ShowSlug:        t.show,
		SourceUrl:       t.video.URL(),
		YoutubeUrl:      t.collection,
		OperationId:     t.operation,
		Title:           media.Title,
		Description:     &media.Description,
		DurationSeconds: duration,
		PublishedAt:     media.PublishedAt,
		Objects:         objects,
	}
	for _, object := range objects {
		file, err := os.Open(filepath.Join(t.directory, filepath.FromSlash(object.Name)))
		if err != nil {
			return nil, err
		}
		err = durableSync(file)
		file.Close()
		if err != nil {
			return nil, err
		}
	}
	if err := t.journal.SavePrepared(ctx, prepared); err != nil {
		return nil, err
	}
	return &prepared, nil
}

type packageUpload struct {
	engine        *Engine
	api           *API
	journal       *Database
	directory, id string
	manifest      p.CreateEpisodePackage
	session       p.EpisodePackage
}

func (t *packageUpload) savedBytes(ctx context.Context, completed map[int]bool) (int64, error) {
	saved := int64(0)
	for ordinal, object := range t.manifest.Objects {
		if completed[ordinal] {
			saved += object.ByteLength
			continue
		}
		count := (object.ByteLength + t.session.PartSize - 1) / t.session.PartSize
		for n := int64(1); n <= count; n++ {
			has, err := t.journal.HasPart(ctx, t.manifest.OperationId, ordinal, int32(n))
			if err != nil {
				return 0, err
			}
			if has {
				saved += min(t.session.PartSize, object.ByteLength-(n-1)*t.session.PartSize)
			}
		}
	}
	return saved, nil
}

func (t *packageUpload) uploadPart(
	ctx context.Context,
	ordinal int,
	object p.PreparedMediaObject,
	number int32,
) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	has, err := t.journal.HasPart(ctx, t.manifest.OperationId, ordinal, number)
	if err != nil || has {
		return err
	}
	signed, err := t.signedPart(ctx, ordinal, number)
	if err != nil {
		return err
	}
	offset := (int64(number) - 1) * t.session.PartSize
	length := min(t.session.PartSize, object.ByteLength-offset)
	if _, err := t.api.UploadPart(
		ctx,
		filepath.Join(t.directory, filepath.FromSlash(object.Name)),
		offset,
		length,
		"PUT",
		signed.UploadUrl,
		func(n int64) { t.engine.Downloads.uploadProgress(t.id, n) },
	); err != nil {
		return err
	}
	return t.journal.SavePart(ctx, t.manifest.OperationId, ordinal, number)
}

func (t *packageUpload) signedPart(
	ctx context.Context,
	ordinal int,
	number int32,
) (*p.PresignedEpisodeUploadPart, error) {
	var signed *p.PresignedEpisodeUploadPart
	for _, upload := range t.session.Uploads {
		if upload.ObjectIndex == int64(ordinal) {
			for _, part := range upload.Parts {
				if part.PartNumber == number &&
					part.ExpiresAt > t.api.Clock.Now().UnixMilli()+30000 {
					part := part
					signed = &part
					break
				}
			}
		}
	}
	if signed == nil {
		var refreshed p.PresignedEpisodeUploadParts
		if err := t.api.JSON(
			ctx,
			"POST",
			fmt.Sprintf(
				"/s/episode-packages/%s/objects/%d/parts",
				t.session.UploadSessionId,
				ordinal,
			),
			p.PresignEpisodeUploadSessionParts{PartNumbers: []int32{number}},
			&refreshed,
		); err != nil {
			return nil, err
		}
		if len(refreshed.Parts) != 1 || refreshed.Parts[0].PartNumber != number {
			return nil, errors.New("invalid signed package part")
		}
		signed = &refreshed.Parts[0]
	}
	return signed, nil
}

// packageSegmentBatch bounds how many segment URLs are signed at once, so a
// batch uploads well within the signatures' lifetime.
const packageSegmentBatch = 32

// pendingSegments returns the segments this session has not stored yet and
// the bytes of those it already has.
func (t *packageUpload) pendingSegments(
	ctx context.Context,
	segments []p.EpisodePackageSegment,
) ([]p.EpisodePackageSegment, int64, error) {
	pending := make([]p.EpisodePackageSegment, 0, len(segments))
	saved := int64(0)
	for _, segment := range segments {
		has, err := t.journal.HasSegment(ctx, t.manifest.OperationId, segment.Name)
		if err != nil {
			return nil, 0, err
		}
		if has {
			saved += segment.ByteLength
		} else {
			pending = append(pending, segment)
		}
	}
	return pending, saved, nil
}

func (t *packageUpload) uploadSegments(ctx context.Context, segments []p.EpisodePackageSegment) error {
	for batch := range slices.Chunk(segments, packageSegmentBatch) {
		var signed p.PresignedEpisodePackageSegments
		if err := t.api.JSON(
			ctx,
			"POST",
			fmt.Sprintf("/s/episode-packages/%s/segments", t.session.UploadSessionId),
			p.PresignEpisodePackageSegments{Segments: batch},
			&signed,
		); err != nil {
			return err
		}
		if len(signed.Segments) != len(batch) {
			return errors.New("invalid signed package segments")
		}
		var group errgroup.Group
		group.SetLimit(maximumPartWorkers)
		for index, segment := range batch {
			upload := signed.Segments[index]
			if upload.Name != segment.Name {
				return errors.New("invalid signed package segment")
			}
			group.Go(func() error {
				if err := t.api.UploadObject(
					ctx,
					filepath.Join(t.directory, filepath.FromSlash(segment.Name)),
					segment.ContentType,
					upload.Method,
					upload.UploadUrl,
					func(n int64) { t.engine.Downloads.uploadProgress(t.id, n) },
				); err != nil {
					return err
				}
				return t.journal.SaveSegment(ctx, t.manifest.OperationId, segment.Name)
			})
		}
		if err := group.Wait(); err != nil {
			return err
		}
	}
	return nil
}

func packageContentType(name string) string {
	contentType := mp4ContentType
	if strings.HasSuffix(name, ".m3u8") {
		contentType = "application/vnd.apple.mpegurl"
	} else if name == preparedAudioName || strings.HasPrefix(name, "hls/audio/") {
		contentType = "audio/mp4"
	}
	return contentType
}

func validateImportPlan(capacity p.ImportCapacity, kind p.ShowSourceKind) error {
	if !capacity.HasActiveSubscription {
		return &HTTPError{
			Status:  http.StatusPaymentRequired,
			Message: "A paid podcast plan is required for YouTube imports. Choose a plan, then try again.",
		}
	}
	if kind == p.ShowSourceKindVideo && !capacity.VideoAllowed {
		return &HTTPError{
			Status: http.StatusPaymentRequired,
			Message: "A video podcast plan is required to import this playlist as video. Upgrade your " +
				"plan, then try again.",
		}
	}
	return nil
}

func validImportURL(source string, u *url.URL) bool {
	return len(source) <= 2048 && IsYouTubeSource(source) && u.Scheme == httpsScheme &&
		u.User == nil && u.Port() == ""
}

func completedPackageObjects(session p.EpisodePackage) map[int]bool {
	completed := make(map[int]bool)
	for _, upload := range session.Uploads {
		if len(upload.Parts) == 0 {
			completed[int(upload.ObjectIndex)] = true
		}
	}
	return completed
}

func (yt *YouTube) importCollectionSnapshot(
	ctx context.Context,
	collection string,
	kind p.ShowSourceKind,
	preparation *ImportEvent,
	emit func(ImportEvent),
) (string, *youtubei.Playlist, PlaylistSnapshot, error) {
	var snapshot PlaylistSnapshot
	var page *youtubei.Playlist
	var title string
	cu, _ := url.Parse(collection)
	head, err := yt.PlaylistHead(ctx, collectionPlaylistID(cu))
	if err != nil {
		return "", nil, PlaylistSnapshot{}, err
	}
	title = head.Title
	if kind == p.ShowSourceKindVideo {
		snapshot, err = yt.CompletePlaylist(ctx, head, func(scan PlaylistScan) {
			preparation.Stage = "scanning_playlist"
			preparation.Scan = &scan
			emit(*preparation)
		})
		if err != nil {
			return "", nil, PlaylistSnapshot{}, err
		}
	} else {
		page = &head
	}
	return title, page, snapshot, nil
}

type importAdmission struct {
	collection, videoID, canonical string
	capacity                       p.ImportCapacity
	existing                       *p.Show
}

func (api *API) admitImport(
	ctx context.Context,
	source, team, requestedSlug string,
	kind p.ShowSourceKind,
	emit func(ImportEvent),
) (importAdmission, error) {
	collection, videoID, err := parseImportSource(source)
	if err != nil {
		return importAdmission{}, err
	}
	if requestedSlug != "" {
		if _, err := Slug(requestedSlug); err != nil {
			return importAdmission{}, err
		}
	}
	emit(ImportEvent{Stage: "checking_plan"})
	shows, capacity, err := api.importPlan(ctx, team)
	if err != nil {
		return importAdmission{}, err
	}
	if collection != "" {
		collection, err = api.resolveChannel(ctx, collection)
		if err != nil {
			return importAdmission{}, err
		}
	}
	canonical := collection
	if canonical == "" {
		canonical = "https://www.youtube.com/watch?v=" + videoID
	}
	existing, err := existingImportedShow(shows, team, canonical, requestedSlug)
	if err != nil {
		return importAdmission{}, err
	}
	if existing != nil {
		return importAdmission{existing: existing}, nil
	}
	if err := validateImportPlan(capacity, kind); err != nil {
		return importAdmission{}, err
	}
	return importAdmission{
		collection: collection,
		videoID:    videoID,
		canonical:  canonical,
		capacity:   capacity,
	}, nil
}

func (api *API) importPlan(ctx context.Context, team string) ([]p.Show, p.ImportCapacity, error) {
	var shows []p.Show
	var capacity p.ImportCapacity
	group, groupCtx := errgroup.WithContext(ctx)
	group.Go(func() error { return api.JSON(groupCtx, "GET", "/s/shows", nil, &shows) })
	group.Go(func() error {
		return api.JSON(
			groupCtx,
			"GET",
			"/s/import-capacity?team_id="+url.QueryEscape(team),
			nil,
			&capacity,
		)
	})
	if err := group.Wait(); err != nil {
		return shows, capacity, err
	}
	return shows, capacity, nil
}

func existingImportedShow(shows []p.Show, team, canonical, requestedSlug string) (*p.Show, error) {
	for _, show := range shows {
		if show.TeamId == team && stringValue(show.Youtube.Url) == canonical {
			return &show, nil
		}
		if requestedSlug != "" && show.Slug == requestedSlug {
			//nolint:revive,staticcheck // Preserve existing end-user guidance.
			return nil, fmt.Errorf(
				"Podcast %q already exists. Use sync to resume an imported podcast, or choose a new slug.",
				requestedSlug,
			)
		}
	}
	return nil, nil
}

func collectionImportURL(u *url.URL, channel string) bool {
	return u.Query().Has("list") || u.Path == "/playlist" || channel != ""
}

func (api *API) completePackage(ctx context.Context, sessionID string) error {
	var completedSession p.EpisodePackage
	if err := api.JSON(
		ctx,
		"POST",
		"/s/episode-packages/"+sessionID+"/complete",
		nil,
		&completedSession,
	); err != nil {
		return err
	}
	if completedSession.Status != completedStatus {
		return errors.New("prepared media did not complete")
	}
	return nil
}

func importPreparation(kind p.ShowSourceKind, capacity p.ImportCapacity) ImportEvent {
	preparation := ImportEvent{Stage: "reading_playlist"}
	if kind == p.ShowSourceKindVideo {
		preparation.VideoRemainingSeconds = &capacity.VideoRemainingSeconds
	}
	return preparation
}

func validateImportStorage(kind p.ShowSourceKind, snapshot PlaylistSnapshot, capacity p.ImportCapacity) error {
	if kind == p.ShowSourceKindVideo && snapshot.EstimatedSeconds > capacity.VideoRemainingSeconds {
		return &HTTPError{
			Status: http.StatusPaymentRequired,
			Message: fmt.Sprintf(
				"This playlist needs about %.2f hours of video storage. Your team has %.2f hours "+
					"available. Upgrade your plan, then try again.",
				float64(snapshot.EstimatedSeconds)/secondsPerHour,
				float64(capacity.VideoRemainingSeconds)/secondsPerHour,
			),
		}
	}
	return nil
}

func (e *Engine) syncImportedShow(
	ctx context.Context,
	api *API,
	slug string,
	page *youtubei.Playlist,
	snapshot PlaylistSnapshot,
	youtube *YouTube,
	admission importAdmission,
) (Report, error) {
	var err error
	if page != nil {
		snapshot, err = youtube.CompletePlaylist(ctx, *page, nil)
		if err != nil {
			return Report{}, err
		}
	}
	return e.sync(
		ctx,
		api,
		slug,
		&snapshot,
		youtube,
		admission.canonical,
		int(admission.capacity.VideoMaxHeight),
	)
}

func (e *Engine) importAdmitted(
	ctx context.Context,
	api *API,
	source, team, requestedSlug string,
	kind p.ShowSourceKind,
	emit func(ImportEvent),
	admission importAdmission,
) (p.Show, Report, error) {
	empty := p.Show{}
	var report Report
	collection, videoID := admission.collection, admission.videoID
	capacity := admission.capacity
	preparation := importPreparation(kind, capacity)
	emit(preparation)
	youtube, err := NewYouTube(ctx, api)
	if err != nil {
		return empty, report, err
	}
	defer youtube.Close()
	title, page, snapshot, err := youtube.importSnapshot(
		ctx,
		collection,
		videoID,
		source,
		kind,
		&preparation,
		emit,
	)
	if err != nil {
		return empty, report, err
	}
	if err := validateImportStorage(kind, snapshot, capacity); err != nil {
		return empty, report, err
	}
	preparation.Stage = "creating_podcast"
	emit(preparation)
	show, existing, err := api.createImportShow(ctx, source, team, requestedSlug, kind, title, admission)
	if err != nil {
		return empty, report, err
	}
	if existing {
		emit(ImportEvent{Show: &show, Existing: true})
		return show, report, nil
	}
	emit(ImportEvent{Show: &show})
	report, err = e.syncImportedShow(ctx, api, show.Slug, page, snapshot, youtube, admission)
	if err != nil {
		//nolint:staticcheck // Preserve existing end-user guidance.
		return show, report, fmt.Errorf(
			"Podcast %q was created. Resume it with sync: %w",
			show.Slug,
			err,
		)
	}
	return show, report, nil
}

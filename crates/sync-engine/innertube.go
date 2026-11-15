package syncengine

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"net/http"
	"net/url"
	"os"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/listenbox/client/crates/youtubei"
)

type Video struct {
	ID, Title       string
	SourceTitle     *string
	DurationSeconds *int64
}

const (
	youtubeResponseLimit = 32 << 20
	maximumPlaylistPages = 10000
	youtubeVideoIDLength = 11
	secondsPerMinute     = 60
)

func (v Video) URL() string { return "https://www.youtube.com/watch?v=" + v.ID }

type PlaylistSnapshot struct {
	Title, ArtworkURL string
	Present           []Video
	EstimatedSeconds  int64
	CanRemove         bool
}
type PlaylistScan struct {
	Title            string
	Videos           int
	EstimatedSeconds int64
	UnknownDurations int
}
type (
	MediaStream struct{ URL, UserAgent, Identity string }
	Media       struct {
		Title, Description, ArtworkURL string
		PublishedAt, EstimatedSeconds  int64
		DurationSeconds                *int64
		Video                          MediaStream
		Audio                          *MediaStream
		fallback                       []youtubei.Client
	}
)

type YouTube struct {
	engine                 *youtubei.Engine
	userAgent, visionAgent string
	playbackClient         youtubei.Client
}

func NewYouTube(ctx context.Context, api *API) (*YouTube, error) {
	jar := NewCookieJar(api.Config.Directory)
	initial, err := jar.snapshot()
	if err != nil {
		return nil, err
	}
	origin, _ := url.Parse("https://www.youtube.com/")
	cookie := initial.header(origin)
	client := youtubei.VisionOS()
	for pair := range strings.SplitSeq(cookie, "; ") {
		name, value, _ := strings.Cut(pair, "=")
		if value != "" &&
			(name == "SAPISID" || name == "__Secure-1PAPISID" || name == "__Secure-3PAPISID") {
			client = youtubei.EmbeddedWeb()
		}
	}
	transport := youtubeTransport{api: api, jar: jar, continuations: make(map[string]bool)}
	engine, err := youtubei.New(ctx, transport.fetch)
	if err != nil {
		return nil, err
	}
	session, err := engine.Initialize(
		ctx,
		youtubei.SessionOptions{
			Lang:           "en",
			Location:       "US",
			Timezone:       "UTC",
			Cookie:         cookie,
			FailFast:       true,
			RetrievePlayer: true,
		},
	)
	if err != nil {
		engine.Close()
		return nil, fmt.Errorf("initialize YouTube: %w", err)
	}
	return &YouTube{
		engine:         engine,
		userAgent:      session.UserAgent,
		visionAgent:    session.VisionOSUserAgent,
		playbackClient: client,
	}, nil
}

func allowedYouTubeURL(u *url.URL) bool {
	if u == nil || u.Scheme != httpsScheme {
		return false
	}
	host := u.Hostname()
	for _, base := range []string{youtubeDomainName, "google.com", "googleapis.com", "googlevideo.com", "ytimg.com"} {
		if host == base || strings.HasSuffix(host, "."+base) {
			return true
		}
	}
	return false
}
func (yt *YouTube) Close() { yt.engine.Close() }
func (yt *YouTube) PlaylistHead(ctx context.Context, id string) (youtubei.Playlist, error) {
	page, err := yt.engine.PlaylistHead(ctx, id)
	if err != nil {
		return page, err
	}
	if err := validPlaylist(page); err != nil {
		return page, err
	}
	if strings.TrimSpace(page.Title) == "" {
		return page, errors.New("YouTube playlist missing title")
	}
	return page, nil
}

func validPlaylist(page youtubei.Playlist) error {
	if !page.IsComplete {
		return errors.New("YouTube playlist could not be parsed completely; no changes were made")
	}
	for _, alert := range page.Alerts {
		if (alert.Type != "Alert" && alert.Type != "AlertWithButton") ||
			(alert.AlertType != "INFO" && alert.AlertType != "WARNING") {
			return errors.New("YouTube could not list this playlist")
		}
	}
	return nil
}

func (yt *YouTube) CompletePlaylist(
	ctx context.Context,
	page youtubei.Playlist,
	progress func(PlaylistScan),
) (PlaylistSnapshot, error) {
	snapshot := PlaylistSnapshot{CanRemove: true}
	seen := make(map[string]bool)
	unknown := 0
	var err error
	for pages := 1; ; pages++ {
		if pages > maximumPlaylistPages {
			return snapshot, errors.New("YouTube playlist exceeds the scan limit")
		}
		if err := validPlaylist(page); err != nil {
			return snapshot, err
		}
		if len(page.Alerts) > 0 {
			snapshot.CanRemove = false
		}
		if snapshot.Title == "" {
			snapshot.Title = page.Title
		}
		if snapshot.ArtworkURL == "" {
			snapshot.ArtworkURL = thumbnailURL(page.Thumbnails)
		}
		unknown, err = appendPlaylistPage(&snapshot, page, seen, unknown)
		if err != nil {
			return snapshot, err
		}
		if progress != nil {
			progress(
				PlaylistScan{
					snapshot.Title,
					len(snapshot.Present),
					snapshot.EstimatedSeconds,
					unknown,
				},
			)
		}
		if !page.HasContinuation {
			break
		}
		page, err = yt.engine.Continuation(ctx, page)
		if err != nil {
			return snapshot, err
		}
	}
	if snapshot.Title == "" {
		return snapshot, errors.New("YouTube playlist missing title")
	}
	return snapshot, nil
}

func (yt *YouTube) Snapshot(ctx context.Context, id string) (PlaylistSnapshot, error) {
	page, err := yt.PlaylistHead(ctx, id)
	if err != nil {
		return PlaylistSnapshot{}, err
	}
	return yt.CompletePlaylist(ctx, page, nil)
}

func (yt *YouTube) VideoSnapshot(ctx context.Context, id string) (PlaylistSnapshot, error) {
	info, _, err := yt.playerInfo(ctx, id, youtubei.Web())
	if err != nil {
		return PlaylistSnapshot{}, err
	}
	video := Video{ID: id, Title: info.BasicInfo.Title}
	if strings.TrimSpace(video.Title) != "" {
		video.SourceTitle = &video.Title
	} else {
		video.Title = "YouTube video " + id
	}
	if info.BasicInfo.Duration != nil {
		video.DurationSeconds = durationSeconds(*info.BasicInfo.Duration)
	}
	return PlaylistSnapshot{
		Title:      video.Title,
		ArtworkURL: thumbnailURL(info.BasicInfo.Thumbnail),
		Present:    []Video{video},
		CanRemove:  true,
	}, nil
}

func unavailableReason(status youtubei.PlayabilityStatus) string {
	reason := strings.TrimSpace(status.Reason)
	detail := strings.TrimSpace(status.Subreason)
	if detail != "" && detail != reason {
		if reason != "" {
			return reason + ": " + detail
		}
		return detail
	}
	if reason == "" {
		return "YouTube playback unavailable"
	}
	return reason
}

func (yt *YouTube) Media(ctx context.Context, id, collection string) (*Media, string, error) {
	metadataClient := youtubei.Web()
	if yt.playbackClient == youtubei.EmbeddedWeb() {
		metadataClient = youtubei.MobileWeb()
	}
	metadata, reason, err := yt.playerInfo(ctx, id, metadataClient)
	if err != nil || reason != "" {
		return nil, reason, err
	}
	if metadata.BasicInfo.IsLive || metadata.BasicInfo.IsUpcoming {
		return nil, "Live or upcoming video; sync after it has finished", nil
	}
	playback, client, reason, err := yt.playbackInfo(ctx, id)
	if err != nil || reason != "" {
		return nil, reason, err
	}
	video, audio, err := yt.selectStreams(
		ctx,
		id,
		playback.Formats,
		playback.CPN,
		metadata.BasicInfo.Duration,
		client,
	)
	if err != nil {
		return nil, "", err
	}
	published, err := mediaPublication(metadata, collection)
	if err != nil {
		return nil, "", err
	}
	return resolvedMedia(metadata, id, published, video, audio, client), "", nil
}

func (yt *YouTube) FallbackMedia(ctx context.Context, id string, media *Media) (bool, error) {
	for len(media.fallback) > 0 {
		client := media.fallback[0]
		media.fallback = media.fallback[1:]
		info, reason, err := yt.playerInfo(ctx, id, client)
		if reason != "" || errors.Is(err, ErrSignInRequired) {
			continue
		}
		if err != nil {
			return false, err
		}
		var duration *float64
		if media.DurationSeconds != nil {
			value := float64(*media.DurationSeconds)
			duration = &value
		}
		video, audio, err := yt.selectStreams(ctx, id, info.Formats, info.CPN, duration, client)
		if err != nil {
			return false, err
		}
		media.Video, media.Audio = video, audio
		return true, nil
	}
	return false, nil
}

func (yt *YouTube) playerInfo(
	ctx context.Context,
	id string,
	client youtubei.Client,
) (youtubei.VideoInfo, string, error) {
	info, err := yt.engine.VideoInfo(ctx, id, client)
	if err != nil {
		var upstream *youtubei.Error
		if errors.As(err, &upstream) && upstream.PlayabilityStatus != nil &&
			upstream.PlayabilityStatus.Status == "ERROR" {
			return info, unavailableReason(*upstream.PlayabilityStatus), nil
		}
		return info, "", err
	}
	if info.PlayabilityStatus == nil {
		return info, "", errors.New("YouTube player response missing playability status")
	}
	status := info.PlayabilityStatus
	switch status.Status {
	case "OK":
	case "UNPLAYABLE":
		return info, unavailableReason(*status), nil
	case "LOGIN_REQUIRED":
		return info, "", fmt.Errorf("%s (%s): %w", status.Reason, id, ErrSignInRequired)
	default:
		return info, "", fmt.Errorf(
			"YouTube could not resolve %s (%s): %s",
			id,
			status.Status,
			status.Reason,
		)
	}
	if info.BasicInfo.ID != id {
		return info, "", errors.New("YouTube metadata video ID differs from the requested video")
	}
	return info, "", nil
}

func (yt *YouTube) selectStreams(
	ctx context.Context,
	id string,
	formats []youtubei.Format,
	cpn string,
	duration *float64,
	client youtubei.Client,
) (MediaStream, *MediaStream, error) {
	usable, err := usableFormats(formats)
	if err != nil {
		return MediaStream{}, nil, err
	}
	audio := preferredAudio(usable)
	selection := streamSelection{audio: audio, duration: duration}
	videos, err := supportedVideos(usable, audio)
	if err != nil {
		return MediaStream{}, nil, err
	}
	sort.Slice(videos, func(i, j int) bool { return selection.prefer(videos[i], videos[j]) })
	selected := videos[0]
	selectedAudio := selection.separate(selected)
	audioInfo := selected
	if selectedAudio != nil {
		audioInfo = *selectedAudio
	}
	if Production != "1" {
		fmt.Fprintf(
			os.Stderr,
			"YouTube media selected video_id=%s video_itag=%d video_mime=%q audio_itag=%d aud"+
				"io_mime=%q audio_bitrate_bps=%.0f audio_has_video=%t\n",
			id,
			selected.ITag,
			selected.MimeType,
			audioInfo.ITag,
			audioInfo.MimeType,
			audioInfo.Bitrate,
			audioInfo.HasVideo,
		)
	}
	video, err := yt.stream(ctx, selected, cpn, client)
	if err != nil {
		return video, nil, err
	}
	if selectedAudio == nil {
		return video, nil, nil
	}
	resolved, err := yt.stream(ctx, *selectedAudio, cpn, client)
	return video, &resolved, err
}
func isAAC(format youtubei.Format) bool { return strings.Contains(format.MimeType, "mp4a.40.") }

func (yt *YouTube) stream(
	ctx context.Context,
	format youtubei.Format,
	cpn string,
	client youtubei.Client,
) (MediaStream, error) {
	resolved, err := yt.engine.Decipher(ctx, format)
	if err != nil {
		return MediaStream{}, err
	}
	u, err := url.Parse(resolved)
	if err != nil {
		return MediaStream{}, errors.New("invalid media URL")
	}
	query := u.Query()
	query.Add("cpn", cpn)
	u.RawQuery = query.Encode()
	agent := yt.userAgent
	if client == youtubei.VisionOS() {
		agent = yt.visionAgent
	}
	length := "None"
	if format.ContentLength != nil {
		value := strconv.FormatFloat(*format.ContentLength, 'f', -1, 64)
		if !strings.Contains(value, ".") {
			value += ".0"
		}
		length = "Some(" + value + ")"
	}
	return MediaStream{
		URL:       u.String(),
		UserAgent: agent,
		Identity:  fmt.Sprintf("%d:%s:%s", format.ITag, format.MimeType, length),
	}, nil
}

func validVideoID(id string) bool {
	if len(id) != youtubeVideoIDLength {
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

func knownSeconds(value float64) int64 {
	if math.IsNaN(value) || math.IsInf(value, 0) || value <= 0 || value >= float64(math.MaxInt64) {
		return 0
	}
	return int64(math.Ceil(value))
}

func durationSeconds(value float64) *int64 {
	seconds := knownSeconds(value)
	if seconds == 0 {
		return nil
	}
	return &seconds
}

func durationBadgeSeconds(value string) (int64, bool) {
	parts := strings.Split(value, ":")
	if len(parts) < 2 || len(parts) > 3 {
		return 0, false
	}
	var total int64
	for _, part := range parts {
		n, err := strconv.ParseInt(part, 10, 64)
		if err != nil || n < 0 || n >= 60 {
			return 0, false
		}
		total = total*secondsPerMinute + n
	}
	return total, total > 0
}

func thumbnailURL(thumbnails []youtubei.Thumbnail) string {
	var selected youtubei.Thumbnail
	for _, thumbnail := range thumbnails {
		if thumbnail.Width*thumbnail.Height >= selected.Width*selected.Height {
			selected = thumbnail
		}
	}
	return selected.URL
}

type youtubeTransport struct {
	api           *API
	jar           *CookieJar
	continuations map[string]bool
}

func (t *youtubeTransport) fetch(
	ctx context.Context,
	input youtubei.FetchRequest,
) (youtubei.FetchResponse, error) {
	u, err := url.Parse(input.URL)
	if err != nil || !allowedYouTubeURL(u) {
		return youtubei.FetchResponse{}, errors.New("unexpected YouTube API host")
	}
	var body map[string]any
	_ = json.Unmarshal(input.Body, &body)
	if err := t.checkContinuation(u, body); err != nil {
		return youtubei.FetchResponse{}, err
	}
	snapshot, err := t.jar.snapshot()
	if err != nil {
		return youtubei.FetchResponse{}, err
	}
	request, err := youtubeFetchRequest(ctx, input, body, snapshot.header(u), u)
	if err != nil {
		return youtubei.FetchResponse{}, err
	}
	response, err := t.api.send(t.api.HTTP, request)
	if err != nil {
		return youtubei.FetchResponse{}, err
	}
	defer response.Body.Close()
	if err := t.jar.update(
		ctx,
		snapshot,
		u,
		response.Header.Values("Set-Cookie"),
	); err != nil {
		return youtubei.FetchResponse{}, err
	}
	raw, err := ReadBounded(response.Body, youtubeResponseLimit)
	if err != nil {
		return youtubei.FetchResponse{}, err
	}
	headers := make([][2]string, 0, len(response.Header))
	for key, values := range response.Header {
		for _, value := range values {
			headers = append(headers, [2]string{key, value})
		}
	}
	return youtubei.FetchResponse{
		URL:     response.Request.URL.String(),
		Status:  response.StatusCode,
		Headers: headers,
		Body:    raw,
	}, nil
}

func youtubeFetchRequest(
	ctx context.Context,
	input youtubei.FetchRequest,
	body map[string]any,
	cookie string,
	u *url.URL,
) (*http.Request, error) {
	if contextBody, ok := body["context"].(map[string]any); ok {
		if clientBody, ok := contextBody["client"].(map[string]any); ok &&
			clientBody["clientName"] == "VISIONOS" {
			cookie = ""
		}
	}
	request, err := http.NewRequestWithContext(
		ctx,
		input.Method,
		input.URL,
		strings.NewReader(string(input.Body)),
	)
	if err != nil {
		return nil, err
	}
	for _, header := range input.Headers {
		switch strings.ToLower(header[0]) {
		case "cookie",
			"authorization",
			"x-goog-authuser",
			"x-goog-pageid",
			"x-origin",
			"x-youtube-bootstrap-logged-in",
			"host",
			"content-length":
			continue
		}
		request.Header.Add(header[0], header[1])
	}
	if cookie != "" {
		request.Header.Set("Cookie", cookie)
		if u.Hostname() == youtubeWebHost && strings.HasPrefix(u.Path, "/youtubei/") {
			authorization := cookieAuthorization(cookie, time.Now().UTC().Unix())
			if authorization != "" {
				request.Header.Set("Authorization", authorization)
				request.Header.Set("X-Origin", "https://www.youtube.com")
				request.Header.Set("X-Goog-Authuser", "0")
			}
		}
	}
	return request, nil
}

func playlistVideo(item youtubei.PlaylistItem) (Video, int64, error) {
	if !validVideoID(item.ID) {
		//nolint:staticcheck // Preserve existing end-user guidance.
		return Video{}, 0, errors.New(
			"Playlist contains an unidentified unavailable item",
		)
	}
	video := Video{ID: item.ID, Title: item.Title}
	seconds := knownSeconds(item.DurationSeconds)
	switch item.Type {
	case "PlaylistVideo":
		if item.IsPlayable && item.SourceTitle != nil &&
			strings.TrimSpace(*item.SourceTitle) != "" {
			video.SourceTitle = item.SourceTitle
		}
	case "LockupView":
		if item.ContentType != "VIDEO" && item.ContentType != "SHORT" {
			return Video{}, 0, incompletePlaylistItem()
		}
		video.SourceTitle = item.SourceTitle
		seconds = lockupSeconds(item, seconds)
	default:
		//nolint:staticcheck // Preserve existing end-user guidance.
		return Video{}, 0, errors.New(
			"Unsupported playlist item; listing is incomplete",
		)
	}
	if video.Title == "" {
		video.Title = "YouTube video " + video.ID
	}
	if seconds > 0 {
		video.DurationSeconds = &seconds
	}
	return video, seconds, nil
}

func appendPlaylistPage(
	snapshot *PlaylistSnapshot,
	page youtubei.Playlist,
	seen map[string]bool,
	unknown int,
) (int, error) {
	for _, item := range page.Items {
		video, seconds, err := playlistVideo(item)
		if err != nil {
			return unknown, err
		}
		if !seen[video.ID] {
			seen[video.ID] = true
			if seconds == 0 {
				unknown++
			}
			if math.MaxInt64-snapshot.EstimatedSeconds < seconds {
				//nolint:staticcheck // Preserve existing end-user guidance.
				return unknown, errors.New(
					"Playlist duration exceeds the supported range",
				)
			}
			snapshot.EstimatedSeconds += seconds
			snapshot.Present = append(snapshot.Present, video)
		}
	}
	return unknown, nil
}

func mediaPublication(metadata youtubei.VideoInfo, collection string) (time.Time, error) {
	date := metadata.PublishDate
	secondary := metadata.UploadDate
	if strings.HasPrefix(collection, "https://www.youtube.com/channel/") {
		date, secondary = secondary, date
	}
	if date == "" {
		date = secondary
	}
	if date == "" {
		return time.Time{}, errors.New("YouTube video has no publication date")
	}
	published, err := time.Parse(time.RFC3339Nano, date)
	if err != nil {
		published, err = time.Parse("2006-01-02", date)
	}
	if err != nil {
		return time.Time{}, errors.New("YouTube publication date is invalid")
	}
	return published, nil
}

func (yt *YouTube) playbackInfo(
	ctx context.Context,
	id string,
) (youtubei.VideoInfo, youtubei.Client, string, error) {
	client := yt.playbackClient
	playback, reason, err := yt.playerInfo(ctx, id, client)
	if err != nil {
		return playback, client, "", err
	}
	if reason != "" && client == youtubei.EmbeddedWeb() {
		client = youtubei.CreatorWeb()
		playback, reason, err = yt.playerInfo(ctx, id, client)
	}
	if err != nil || reason != "" {
		return playback, client, reason, err
	}
	return playback, client, reason, nil
}

func usableFormats(formats []youtubei.Format) ([]youtubei.Format, error) {
	usable := make([]youtubei.Format, 0, len(formats))
	for _, format := range formats {
		if len(format.DRMFamilies) == 0 && !format.IsTypeOTF &&
			(format.URL != "" || format.Cipher != "" || format.SignatureCipher != "") {
			usable = append(usable, format)
		}
	}
	if len(usable) == 0 {
		return nil, errors.New(
			"YouTube returned no downloadable stream URLs; video resolution is not the problem",
		)
	}
	return usable, nil
}

func preferredAudio(usable []youtubei.Format) *youtubei.Format {
	var audio *youtubei.Format
	for i := range usable {
		format := &usable[i]
		if !format.HasAudio || format.HasVideo ||
			(format.AudioTrack != nil && !format.AudioTrack.AudioIsDefault) {
			continue
		}
		if audio == nil || (isAAC(*format) && !isAAC(*audio)) ||
			(isAAC(*format) == isAAC(*audio) && format.Bitrate > audio.Bitrate) {
			audio = format
		}
	}
	return audio
}

func supportedVideos(usable []youtubei.Format, audio *youtubei.Format) ([]youtubei.Format, error) {
	var videos []youtubei.Format
	for _, format := range usable {
		supported := supportedVideoCodec(format.MimeType)
		if format.HasVideo && supported && format.Height != nil && *format.Height > 0 &&
			(format.AudioTrack == nil || format.AudioTrack.AudioIsDefault) &&
			(format.HasAudio || audio != nil) {
			videos = append(videos, format)
		}
	}
	if len(videos) == 0 {
		return nil, errors.New(
			"YouTube video has no supported video rendition with audio",
		)
	}
	return videos, nil
}

type streamSelection struct {
	audio    *youtubei.Format
	duration *float64
}

func (s streamSelection) separate(video youtubei.Format) *youtubei.Format {
	if !video.HasAudio {
		return s.audio
	}
	if isAAC(video) || s.audio == nil || !isAAC(*s.audio) {
		return nil
	}
	return s.audio
}

func (s streamSelection) copyAudio(video youtubei.Format) bool {
	return (video.HasAudio && isAAC(video)) || (s.audio != nil && isAAC(*s.audio))
}

func (s streamSelection) bitrate(format youtubei.Format) float64 {
	if s.duration != nil && *s.duration > 0 && format.ContentLength != nil &&
		*format.ContentLength > 0 {
		return *format.ContentLength * 8 / *s.duration
	}
	if format.Bitrate > 0 {
		return format.Bitrate
	}
	return math.Inf(1)
}

func (s streamSelection) cost(video youtubei.Format) float64 {
	rate := s.bitrate(video)
	if other := s.separate(video); other != nil {
		rate += s.bitrate(*other)
	}
	return rate
}

func streamFPS(f youtubei.Format) float64 {
	if f.FPS != nil && *f.FPS > 0 {
		return *f.FPS
	}
	return 0
}

func (s streamSelection) prefer(a, b youtubei.Format) bool {
	if *a.Height != *b.Height {
		return *a.Height > *b.Height
	}
	if streamFPS(a) != streamFPS(b) {
		return streamFPS(a) > streamFPS(b)
	}
	if s.copyAudio(a) != s.copyAudio(b) {
		return s.copyAudio(a)
	}
	if s.cost(a) != s.cost(b) {
		return s.cost(a) < s.cost(b)
	}
	return a.ITag < b.ITag
}

func resolvedMedia(
	metadata youtubei.VideoInfo,
	id string,
	published time.Time,
	video MediaStream,
	audio *MediaStream,
	client youtubei.Client,
) *Media {
	media := &Media{
		Title:       metadata.BasicInfo.Title,
		Description: metadata.BasicInfo.ShortDescription,
		ArtworkURL:  thumbnailURL(metadata.BasicInfo.Thumbnail),
		PublishedAt: published.UTC().UnixMilli(),
		Video:       video,
		Audio:       audio,
	}
	if media.Title == "" {
		media.Title = id
	}
	if metadata.BasicInfo.Duration != nil {
		media.EstimatedSeconds = knownSeconds(*metadata.BasicInfo.Duration)
		media.DurationSeconds = durationSeconds(*metadata.BasicInfo.Duration)
	}
	if client == youtubei.EmbeddedWeb() || client == youtubei.CreatorWeb() {
		media.fallback = []youtubei.Client{youtubei.VisionOS()}
	}
	return media
}

func (t *youtubeTransport) checkContinuation(u *url.URL, body map[string]any) error {
	if strings.Contains(u.Path, "/youtubei/v1/browse") {
		if token, ok := body["continuation"].(string); ok {
			if t.continuations[token] {
				//nolint:staticcheck // Preserve existing end-user guidance.
				return errors.New(
					"Repeated YouTube continuation",
				)
			}
			t.continuations[token] = true
		}
	}
	return nil
}

func lockupSeconds(item youtubei.PlaylistItem, seconds int64) int64 {
	for _, badge := range item.DurationBadges {
		if value, ok := durationBadgeSeconds(badge); ok {
			seconds = value
			break
		}
	}
	return seconds
}

func incompletePlaylistItem() error {
	//nolint:staticcheck // Preserve existing end-user guidance.
	return errors.New("Unsupported playlist item; listing is incomplete")
}

func supportedVideoCodec(mime string) bool {
	supported := false
	for _, codec := range []string{"avc1", "vp09", "hev1", "hvc1"} {
		supported = supported || strings.Contains(mime, codec)
	}
	return supported
}

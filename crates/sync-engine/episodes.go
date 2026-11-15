package syncengine

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"slices"
	"strings"

	p "github.com/listenbox/client/crates/sync-engine/publicapi"
)

type EpisodeCreate struct {
	Show, Title, File string
	Description       *string
	Publication       p.EpisodePublication
}
type episodeResume struct {
	Version        int                            `json:"version"`
	EpisodeID      string                         `json:"episode_id"`
	SessionID      string                         `json:"upload_session_id"`
	TeamID         string                         `json:"team_id"`
	ShowID         string                         `json:"show_id"`
	ManagementURL  string                         `json:"management_url"`
	FilePath       string                         `json:"file_path"`
	FileSHA256     string                         `json:"file_sha256"`
	FileByteLength int64                          `json:"file_byte_length"`
	CompletedParts []p.CompletedEpisodeUploadPart `json:"completed_parts"`
	ExpiresAt      int64                          `json:"expires_at"`
	Phase          string                         `json:"phase"`
}

const (
	maximumMultipartParts = 10000
	percentScale          = 100
	processingStatus      = "processing"
)

func FileHash(path string) (int64, string, error) {
	file, err := os.Open(path)
	if err != nil {
		return 0, "", fmt.Errorf("open --file %q: %w", path, err)
	}
	defer file.Close()
	info, err := file.Stat()
	if err != nil {
		return 0, "", err
	}
	if !info.Mode().IsRegular() || info.Size() <= 0 {
		return 0, "", errors.New("--file must be a non-empty regular file")
	}
	hash := sha256.New()
	if _, err := io.Copy(hash, file); err != nil {
		return 0, "", err
	}
	return info.Size(), hex.EncodeToString(hash.Sum(nil)), nil
}

func (api *API) CreateEpisode(
	ctx context.Context,
	args EpisodeCreate,
	output, diagnostics io.Writer,
) error {
	path, err := filepath.Abs(args.File)
	if err != nil {
		return err
	}
	length, hash, err := FileHash(path)
	if err != nil {
		return err
	}
	contentType, err := episodeContentType(path)
	if err != nil {
		return err
	}
	key := sha256.Sum256([]byte(args.Show + "\x00" + hash))
	resumePath := filepath.Join(api.Config.Directory, "episode-resume", fmt.Sprintf("%x.json", key))
	record := episodeResume{
		Version:        1,
		FilePath:       path,
		FileSHA256:     hash,
		FileByteLength: length,
		CompletedParts: []p.CompletedEpisodeUploadPart{},
		Phase:          "prepared",
	}
	if err := api.loadEpisodeResume(resumePath, &record, hash, length); err != nil {
		return err
	}
	if !slices.Contains(
		[]string{finalizingStatus, processingStatus, completedStatus},
		record.Phase,
	) {
		if err := api.uploadEpisodeSource(
			ctx,
			args,
			path,
			contentType,
			resumePath,
			&record,
			diagnostics,
		); err != nil {
			return err
		}
	}
	if err := api.completeEpisodeSource(ctx, args, resumePath, &record); err != nil {
		return err
	}
	return api.waitForEpisode(ctx, args, resumePath, record, output, diagnostics)
}

func savedEpisodeProgress(output io.Writer, record episodeResume) {
	saved := int64(0)
	for _, part := range record.CompletedParts {
		saved += part.Size
	}
	fmt.Fprintf(
		output,
		"Source upload: %d%% saved\n",
		min(percentScale, saved*percentScale/record.FileByteLength),
	)
}

func (api *API) uploadEpisodeSource(
	ctx context.Context,
	args EpisodeCreate,
	path string,
	contentType p.EpisodeUploadContentType,
	resumePath string,
	record *episodeResume,
	diagnostics io.Writer,
) error {
	var session p.EpisodeUploadSession
	if err := api.JSON(
		ctx,
		"POST",
		"/s/episode-upload-sessions",
		p.CreateEpisodeUploadSession{
			ShowSlug:     args.Show,
			Title:        args.Title,
			FileName:     filepath.Base(path),
			ContentType:  contentType,
			ByteLength:   record.FileByteLength,
			SourceSha256: record.FileSHA256,
			Description:  args.Description,
		},
		&session,
	); err != nil {
		return err
	}
	record.SessionID = session.UploadSessionId
	record.TeamID = session.TeamId
	record.ShowID = session.ShowId
	record.EpisodeID = stringValue(session.EpisodeId)
	record.ExpiresAt = session.ExpiresAt
	record.Phase = string(session.Phase)
	record.CompletedParts = session.CompletedParts
	if record.CompletedParts == nil {
		record.CompletedParts = []p.CompletedEpisodeUploadPart{}
	}
	if err := WritePrivateJSON(resumePath, *record); err != nil {
		return err
	}
	partSize := session.PartSize
	missing, err := missingEpisodeParts(record, partSize)
	if err != nil {
		return err
	}
	savedEpisodeProgress(diagnostics, *record)
	if len(missing) > 0 {
		if err := api.uploadMissingEpisodeParts(
			ctx,
			path,
			resumePath,
			record,
			partSize,
			missing,
			diagnostics,
		); err != nil {
			return err
		}
	}
	return finalizeEpisodeUpload(resumePath, record)
}

func (api *API) uploadMissingEpisodeParts(
	ctx context.Context,
	path, resumePath string,
	record *episodeResume,
	partSize int64,
	missing []int32,
	diagnostics io.Writer,
) error {
	var signed p.PresignedEpisodeUploadParts
	if err := api.JSON(
		ctx,
		"POST",
		"/s/episode-upload-sessions/"+record.SessionID+"/parts/presign",
		p.PresignEpisodeUploadSessionParts{PartNumbers: missing},
		&signed,
	); err != nil {
		return err
	}
	pending := make(map[int32]bool)
	for _, number := range missing {
		pending[number] = true
	}
	for _, part := range signed.Parts {
		number := part.PartNumber
		if !pending[number] {
			return fmt.Errorf("unexpected or duplicate signed part %d", number)
		}
		delete(pending, number)
		offset := (int64(number) - 1) * partSize
		size := min(partSize, record.FileByteLength-offset)
		etag, err := api.UploadPart(ctx, path, offset, size, "PUT", part.UploadUrl, nil)
		if err != nil {
			return err
		}
		record.CompletedParts = append(
			record.CompletedParts,
			p.CompletedEpisodeUploadPart{PartNumber: number, Etag: etag, Size: size},
		)
		slices.SortFunc(
			record.CompletedParts,
			func(a, b p.CompletedEpisodeUploadPart) int { return int(a.PartNumber - b.PartNumber) },
		)
		record.Phase = "uploading"
		if err := WritePrivateJSON(resumePath, *record); err != nil {
			return err
		}
		savedEpisodeProgress(diagnostics, *record)
	}
	if len(pending) > 0 {
		return errors.New("signed response omitted upload parts")
	}
	return nil
}

func (api *API) waitForEpisode(
	ctx context.Context,
	args EpisodeCreate,
	resumePath string,
	record episodeResume,
	output, diagnostics io.Writer,
) error {
	events, err := api.Events(ctx, "/s/episode-upload-sessions/"+record.SessionID+"/events")
	if err != nil {
		return err
	}
	defer events.Close()
	for {
		event, err := events.Next()
		if err != nil {
			return err
		}
		switch event["type"] {
		case "episode.processing.progress":
			if err := episodeProcessingProgress(event, diagnostics); err != nil {
				return err
			}
		case "episode.processing.safe_handoff":
			return finishEpisodeHandoff(args, resumePath, record, event, output)
		case "episode.processing.failed":
			return fmt.Errorf(
				"episode processing failed (%s): %s",
				event["error_code"],
				event["error_message"],
			)
		default:
			return fmt.Errorf("unknown episode processing event %q", event["type"])
		}
	}
}

func (api *API) loadEpisodeResume(
	resumePath string,
	record *episodeResume,
	hash string,
	length int64,
) error {
	if raw, err := os.ReadFile(resumePath); err == nil {
		if err := json.Unmarshal(raw, record); err != nil {
			return fmt.Errorf("decode episode resume record: %w", err)
		}
		if record.Version != 1 || record.FileSHA256 != hash || record.FileByteLength != length {
			return errors.New("invalid episode resume record")
		}
		if record.ExpiresAt != 0 && record.ExpiresAt <= api.Clock.Now().UnixMilli() &&
			!slices.Contains(
				[]string{finalizingStatus, processingStatus, completedStatus},
				record.Phase,
			) {
			return fmt.Errorf(
				"resumable upload expired; remove %s to restart explicitly",
				resumePath,
			)
		}
	} else if !os.IsNotExist(err) {
		return err
	}
	return nil
}

func episodeProcessingProgress(event map[string]any, diagnostics io.Writer) error {
	percent, ok := event["percent"].(float64)
	if !ok || percent < 0 || percent > 100 {
		return errors.New("invalid processing progress")
	}
	label := "Episode processing"
	switch event["current_step"] {
	case "queued", "processing_audio", "listenbox_processed":
		label = "Listenbox processing"
	case "video_preparation", "video_prepared":
		label = "Video preparation"
	case "youtube_upload":
		label = "YouTube upload"
	}
	fmt.Fprintf(diagnostics, "%s: %.0f%%\n", label, percent)
	return nil
}

func episodeContentType(path string) (p.EpisodeUploadContentType, error) {
	types := map[string]p.EpisodeUploadContentType{
		".mp3":  "audio/mpeg",
		".m4a":  "audio/mp4",
		".wav":  "audio/wav",
		".flac": "audio/flac",
		".mp4":  mp4ContentType,
		".m4v":  mp4ContentType,
		".mov":  "video/quicktime",
	}
	contentType, ok := types[strings.ToLower(filepath.Ext(path))]
	if !ok {
		return "", errors.New(
			"unsupported media upload file; accepted extensions: .mp3, .m4a, .wav, .flac, .mp" +
				"4, .m4v, .mov",
		)
	}
	return contentType, nil
}

func finishEpisodeHandoff(
	args EpisodeCreate,
	resumePath string,
	record episodeResume,
	event map[string]any,
	output io.Writer,
) error {
	record.Phase = completedStatus
	if err := WritePrivateJSON(resumePath, record); err != nil {
		return err
	}
	verb := "Created draft episode"
	if args.Publication == p.EpisodePublicationPublish {
		verb = "Published episode"
	}
	fmt.Fprintf(
		output,
		"%s %q\nOpen in Listenbox: %s\n",
		verb,
		args.Title,
		record.ManagementURL,
	)
	if id, ok := event["youtube_video_id"].(string); ok && id != "" {
		fmt.Fprintf(
			output,
			"YouTube (processing): https://www.youtube.com/watch?v=%s\n",
			id,
		)
	}
	return nil
}

func (api *API) completeEpisodeSource(
	ctx context.Context,
	args EpisodeCreate,
	resumePath string,
	record *episodeResume,
) error {
	var episode p.EpisodeListItem
	if err := api.JSON(
		ctx,
		"POST",
		"/s/episode-upload-sessions/"+record.SessionID+"/complete",
		p.CompleteEpisodeUploadSession{Publication: args.Publication},
		&episode,
	); err != nil {
		return err
	}
	record.EpisodeID = episode.Id
	record.ShowID = episode.ShowId
	if !ValidID(record.EpisodeID, "ep_") {
		return errors.New("invalid episode management identifier")
	}
	showURL, err := api.Config.ShowURL(record.TeamID, record.ShowID)
	if err != nil {
		return err
	}
	record.ManagementURL = showURL + "/episodes/" + record.EpisodeID + "/edit"
	record.Phase = processingStatus
	if err := WritePrivateJSON(resumePath, *record); err != nil {
		return err
	}
	return nil
}

func missingEpisodeParts(record *episodeResume, partSize int64) ([]int32, error) {
	if partSize < 5<<20 {
		return nil, fmt.Errorf("invalid upload part size %d", partSize)
	}
	count := (record.FileByteLength + partSize - 1) / partSize
	if count > maximumMultipartParts {
		return nil, errors.New("episode source requires too many parts")
	}
	accepted := make(map[int32]bool)
	for _, part := range record.CompletedParts {
		accepted[part.PartNumber] = true
	}
	var missing []int32
	for n := int64(1); n <= count; n++ {
		if !accepted[int32(n)] {
			missing = append(missing, int32(n))
		}
	}
	return missing, nil
}

func finalizeEpisodeUpload(resumePath string, record *episodeResume) error {
	record.Phase = finalizingStatus
	if err := WritePrivateJSON(resumePath, *record); err != nil {
		return err
	}
	return nil
}

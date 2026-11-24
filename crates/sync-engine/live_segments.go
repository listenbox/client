package syncengine

import (
	"context"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strconv"

	"golang.org/x/sync/errgroup"
)

// A stream that ended recently stays in YouTube's post-live DVR state until
// its VOD is processed. Its adaptive formats have no byte length: a range
// probe names the last segment in X-Head-Seqnum while its Content-Range total
// is meaningless (observed both `*` and `1` for the same URL), and `sq=N`
// returns segment N as a self-initializing fMP4 (ftyp, moov, then one
// fragment). Processed VODs never send X-Head-Seqnum. The segments are
// journaled one by one, so an interrupted download resumes, then joined into
// a single fragmented MP4.
const (
	maximumLiveSegments     = 1_000_000
	maximumLiveSegmentBytes = 256 << 20
	mp4BoxHeaderBytes       = 8
)

// liveHead returns the last segment number of a post-live probe response,
// or false when the response describes an ordinary byte-addressed file.
func liveHead(response *http.Response) (int64, bool) {
	value := response.Header.Get("X-Head-Seqnum")
	if value == "" {
		return 0, false
	}
	head, err := strconv.ParseInt(value, 10, 64)
	return head, err == nil && head >= 0 && head < maximumLiveSegments
}

func (api *API) downloadLiveSegments(
	ctx context.Context,
	source MediaStream,
	path string,
	manager *DownloadManager,
	id string,
	journal *Database,
	operation string,
	head int64,
) error {
	name := filepath.Base(path)
	identity := fmt.Sprintf("%s:live:%d", source.Identity, head)
	if err := journal.Download(ctx, operation, name, identity); err != nil {
		return err
	}
	manager.update(id, func(item *Download) {
		item.Phase = Downloading()
		item.Total, item.Received, item.Ranges = 0, 0, nil
	})
	joined, err := segmentsJoined(ctx, journal, operation, name, path, head)
	if err != nil || joined {
		return err
	}
	directory := path + ".segments"
	if err := os.MkdirAll(directory, profileDirectoryMode); err != nil {
		return err
	}
	segments := liveSegments{
		api: api, source: source, directory: directory, manager: manager,
		id: id, journal: journal, operation: operation, name: name,
	}
	group, groupCtx := errgroup.WithContext(ctx)
	group.SetLimit(maximumRangeWorkers)
	for sequence := int64(0); sequence <= head; sequence++ {
		group.Go(func() error { return segments.download(groupCtx, sequence, head) })
	}
	if err := group.Wait(); err != nil {
		return err
	}
	return segments.join(ctx, path, head)
}

// segmentsJoined reports whether an earlier attempt already joined every
// segment into path. The joined file's hash is journaled one past the head,
// after its segment files were removed.
func segmentsJoined(ctx context.Context, journal *Database, operation, name, path string, head int64) (bool, error) {
	hash, err := journal.RangeHash(ctx, operation, name, head+1)
	if err != nil || hash == "" {
		return false, err
	}
	file, err := os.Open(path)
	if errors.Is(err, os.ErrNotExist) {
		return false, nil
	}
	if err != nil {
		return false, err
	}
	defer file.Close()
	sum := sha256.New()
	if _, err := io.Copy(sum, file); err != nil {
		return false, err
	}
	return hex.EncodeToString(sum.Sum(nil)) == hash, nil
}

type liveSegments struct {
	api                            *API
	source                         MediaStream
	manager                        *DownloadManager
	journal                        *Database
	directory, id, operation, name string
}

func (s liveSegments) path(sequence int64) string {
	return filepath.Join(s.directory, strconv.FormatInt(sequence, 10))
}

func (s liveSegments) download(ctx context.Context, sequence, head int64) error {
	if saved, err := s.saved(ctx, sequence); err != nil || saved {
		return err
	}
	raw, err := s.fetch(ctx, sequence, head)
	if err != nil {
		return err
	}
	s.manager.streamProgress(s.id, int64(len(raw)), true)
	// Every segment repeats the stream's init boxes; keep only the first.
	payload, err := segmentPayload(raw, sequence > 0)
	if err != nil {
		return fmt.Errorf("live segment %d: %w", sequence, err)
	}
	if err := writeSynced(s.path(sequence), payload); err != nil {
		return err
	}
	sum := sha256.Sum256(payload)
	return s.journal.SaveRange(ctx, s.operation, s.name, sequence, hex.EncodeToString(sum[:]))
}

func (s liveSegments) fetch(ctx context.Context, sequence, head int64) ([]byte, error) {
	u, err := url.Parse(s.source.URL)
	if err != nil {
		return nil, errors.New("invalid media URL")
	}
	query := u.Query()
	query.Set("sq", strconv.FormatInt(sequence, 10))
	u.RawQuery = query.Encode()
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, u.String(), nil)
	if err != nil {
		return nil, err
	}
	request.Header.Set("Origin", "https://youtube.com")
	request.Header.Set("User-Agent", s.source.UserAgent)
	response, err := s.api.send(s.api.HTTP, request)
	if err != nil {
		return nil, err
	}
	switch response.StatusCode {
	case http.StatusOK:
		defer response.Body.Close()
		return ReadBounded(response.Body, maximumLiveSegmentBytes)
	case http.StatusForbidden:
		response.Body.Close()
		return nil, ErrMediaUnavailable
	case http.StatusNoContent:
		response.Body.Close()
		return nil, fmt.Errorf("live segment %d of %d is unavailable", sequence, head)
	default:
		return nil, errors.New("download source: " + responseError(response).Error())
	}
}

func (s liveSegments) saved(ctx context.Context, sequence int64) (bool, error) {
	hash, err := s.journal.RangeHash(ctx, s.operation, s.name, sequence)
	if err != nil || hash == "" {
		return false, err
	}
	saved, err := os.ReadFile(s.path(sequence))
	if err != nil {
		return false, nil //nolint:nilerr // A missing or unreadable segment is downloaded again.
	}
	sum := sha256.Sum256(saved)
	if hex.EncodeToString(sum[:]) != hash {
		return false, nil
	}
	s.manager.streamProgress(s.id, int64(len(saved)), false)
	return true, nil
}

func (s liveSegments) join(ctx context.Context, path string, head int64) error {
	file, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, privateFileMode)
	if err != nil {
		return err
	}
	defer file.Close()
	sum := sha256.New()
	for sequence := int64(0); sequence <= head; sequence++ {
		segment, err := os.ReadFile(s.path(sequence))
		if err != nil {
			return err
		}
		if _, err := io.MultiWriter(file, sum).Write(segment); err != nil {
			return err
		}
	}
	if err := file.Sync(); err != nil {
		return err
	}
	if err := s.journal.SaveRange(ctx, s.operation, s.name, head+1, hex.EncodeToString(sum.Sum(nil))); err != nil {
		return err
	}
	return os.RemoveAll(s.directory)
}

// segmentPayload validates a segment's top-level MP4 boxes and, when
// stripInit is set, drops its ftyp and moov.
func segmentPayload(raw []byte, stripInit bool) ([]byte, error) {
	payload := make([]byte, 0, len(raw))
	fragment := false
	for offset := 0; offset < len(raw); {
		if len(raw)-offset < mp4BoxHeaderBytes {
			return nil, errors.New("truncated MP4 box")
		}
		size := int(binary.BigEndian.Uint32(raw[offset:]))
		if size < mp4BoxHeaderBytes || size > len(raw)-offset {
			return nil, errors.New("invalid MP4 box size")
		}
		kind := string(raw[offset+4 : offset+mp4BoxHeaderBytes])
		fragment = fragment || kind == "moof"
		if !stripInit || (kind != "ftyp" && kind != "moov") {
			payload = append(payload, raw[offset:offset+size]...)
		}
		offset += size
	}
	if !fragment {
		return nil, errors.New("no media fragment")
	}
	return payload, nil
}

func writeSynced(path string, data []byte) error {
	file, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, privateFileMode)
	if err != nil {
		return err
	}
	if _, err := file.Write(data); err != nil {
		file.Close()
		return err
	}
	if err := file.Sync(); err != nil {
		file.Close()
		return err
	}
	return file.Close()
}

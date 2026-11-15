package syncengine

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/google/uuid"
	"golang.org/x/sync/errgroup"
)

const RangeBytes = int64(1024 * 1024)

const (
	maximumRangeWorkers = 4
	rangeCopyBufferSize = 64 << 10
)

func retryable(err error) bool {
	if failure, ok := errors.AsType[*HTTPError](err); ok {
		return failure.Status >= 500 && failure.Status < 600
	}
	if _, ok := errors.AsType[net.Error](err); ok {
		return true
	}
	return errors.Is(err, io.EOF) || errors.Is(err, io.ErrUnexpectedEOF) ||
		errors.Is(err, ErrUploadStalled)
}

func (api *API) download(
	ctx context.Context,
	source MediaStream,
	path string,
	manager *DownloadManager,
	id string,
	journal *Database,
	operation string,
) error {
	total, validator, err := api.probeMediaStream(ctx, source)
	if err != nil {
		return err
	}
	revision := validator
	if revision == "" {
		u, err := url.Parse(source.URL)
		if err != nil {
			return err
		}
		revision = u.Query().Get("lmt")
	}
	if revision == "" {
		revision = uuid.NewString()
	}
	identity := fmt.Sprintf("%s:%d:%s", source.Identity, total, revision)
	name := filepath.Base(path)
	if err := journal.Download(ctx, operation, name, identity); err != nil {
		return err
	}
	file, err := os.OpenFile(path, os.O_CREATE|os.O_RDWR, privateFileMode)
	if err != nil {
		return err
	}
	defer file.Close()
	if err := file.Truncate(total); err != nil {
		return err
	}
	manager.beginRanges(id, total)
	transfer := rangeDownload{
		api:       api,
		source:    source,
		file:      file,
		manager:   manager,
		id:        id,
		journal:   journal,
		operation: operation,
		name:      name,
		validator: validator,
		total:     total,
	}
	var group errgroup.Group
	group.SetLimit(maximumRangeWorkers)
	for start := int64(0); start < total; start += RangeBytes {
		group.Go(func() error {
			return transfer.downloadRange(ctx, start)
		})
	}
	return group.Wait()
}

func (api *API) probeMediaStream(ctx context.Context, source MediaStream) (int64, string, error) {
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, source.URL, nil)
	if err != nil {
		return 0, "", err
	}
	request.Header.Set("Range", "bytes=0-0")
	request.Header.Set("Origin", "https://youtube.com")
	request.Header.Set("User-Agent", source.UserAgent)
	response, err := api.send(api.HTTP, request)
	if err != nil {
		return 0, "", err
	}
	if response.StatusCode == http.StatusForbidden {
		response.Body.Close()
		return 0, "", ErrMediaUnavailable
	}
	if response.StatusCode != http.StatusPartialContent {
		return 0, "", errors.New("download source: " + responseError(response).Error())
	}
	value, ok := strings.CutPrefix(response.Header.Get("Content-Range"), "bytes 0-0/")
	total, parseErr := strconv.ParseInt(value, 10, 64)
	validator := response.Header.Get("ETag")
	if validator == "" || strings.HasPrefix(validator, "W/") {
		validator = response.Header.Get("Last-Modified")
	}
	response.Body.Close()
	if !ok || parseErr != nil || total <= 0 || total > 5_000_000_000_000 {
		return 0, "", errors.New("invalid initial media range")
	}
	return total, validator, nil
}

type rangeDownload struct {
	api                            *API
	source                         MediaStream
	file                           *os.File
	manager                        *DownloadManager
	id, operation, name, validator string
	journal                        *Database
	total                          int64
}

func (d *rangeDownload) downloadRange(ctx context.Context, start int64) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	end := min(start+RangeBytes, d.total) - 1
	length := end - start + 1
	saved, err := d.savedRange(ctx, start, length)
	if err != nil || saved {
		return err
	}
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, d.source.URL, nil)
	if err != nil {
		return err
	}
	request.Header.Set("Range", fmt.Sprintf("bytes=%d-%d", start, end))
	request.Header.Set("Origin", "https://youtube.com")
	request.Header.Set("User-Agent", d.source.UserAgent)
	if d.validator != "" {
		request.Header.Set("If-Range", d.validator)
	}
	response, err := d.api.send(d.api.HTTP, request)
	if err != nil {
		return err
	}
	defer response.Body.Close()
	if response.StatusCode == http.StatusForbidden {
		return ErrMediaUnavailable
	}
	if response.StatusCode != http.StatusPartialContent {
		return errors.New("download source: " + responseError(response).Error())
	}
	if response.Header.Get(
		"Content-Range",
	) != fmt.Sprintf(
		"bytes %d-%d/%d",
		start,
		end,
		d.total,
	) {
		return errors.New("media range differs from the requested bytes")
	}
	bytes, err := d.readRange(response.Body, start, length)
	if err != nil {
		return err
	}
	return d.checkpointRange(ctx, start, length, bytes)
}

func (d *rangeDownload) readRange(body io.Reader, start, length int64) ([]byte, error) {
	bytes := make([]byte, 0, length)
	buffer := make([]byte, rangeCopyBufferSize)
	for {
		n, err := body.Read(buffer)
		if n > 0 {
			if int64(len(bytes)+n) > length {
				return nil, errors.New("media range exceeded its declared length")
			}
			bytes = append(bytes, buffer[:n]...)
			d.manager.rangeProgress(d.id, start, int64(len(bytes)), false, true)
		}
		if err == io.EOF {
			break
		}
		if err != nil {
			return nil, err
		}
	}
	if int64(len(bytes)) != length {
		return nil, fmt.Errorf("media range was truncated: %w", io.ErrUnexpectedEOF)
	}
	return bytes, nil
}

func (d *rangeDownload) savedRange(ctx context.Context, start, length int64) (bool, error) {
	hash, err := d.journal.RangeHash(ctx, d.operation, d.name, start)
	if err != nil {
		return false, err
	}
	if hash != "" {
		saved := make([]byte, length)
		if _, err := d.file.ReadAt(saved, start); err == nil {
			sum := sha256.Sum256(saved)
			if hex.EncodeToString(sum[:]) == hash {
				d.manager.rangeProgress(d.id, start, length, true, false)
				return true, nil
			}
		}
	}
	return false, nil
}

func (d *rangeDownload) checkpointRange(
	ctx context.Context,
	start, length int64,
	bytes []byte,
) error {
	if _, err := d.file.WriteAt(bytes, start); err != nil {
		return err
	}
	if err := d.file.Sync(); err != nil {
		return err
	}
	sum := sha256.Sum256(bytes)
	if err := d.journal.SaveRange(ctx,
		d.operation,
		d.name,
		start,
		hex.EncodeToString(sum[:]),
	); err != nil {
		return err
	}
	d.manager.rangeProgress(d.id, start, length, true, false)
	return nil
}

func (manager *DownloadManager) beginRanges(id string, total int64) {
	manager.update(id, func(item *Download) {
		item.Phase = Downloading()
		item.Total = total
		item.Received = 0
		item.Ranges = nil
		for start := int64(0); start < total; start += RangeBytes {
			item.Ranges = append(
				item.Ranges,
				ByteRange{Start: start, End: min(start+RangeBytes, total) - 1},
			)
		}
	})
}

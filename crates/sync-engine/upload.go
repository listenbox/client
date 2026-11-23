package syncengine

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"sync"
	"time"
)

var ErrUploadStalled = errors.New("upload stalled; resumable source upload state preserved")

type uploadReader struct {
	reader   io.Reader
	mu       sync.Mutex
	last     time.Time
	api      *API
	progress func(int64)
}

const (
	uploadIdleTimeout = time.Minute
)

func (r *uploadReader) Read(bytes []byte) (int, error) {
	n, err := r.reader.Read(bytes)
	if n > 0 {
		r.mu.Lock()
		r.last = r.api.Clock.Now()
		r.mu.Unlock()
		if r.progress != nil {
			r.progress(int64(n))
		}
	}
	return n, err
}
func (r *uploadReader) lastProgress() time.Time { r.mu.Lock(); defer r.mu.Unlock(); return r.last }

func (api *API) UploadPart(
	ctx context.Context,
	path string,
	offset, length int64,
	method, uploadURL string,
	progress func(int64),
) (string, error) {
	return api.upload(ctx, path, offset, length, "", method, uploadURL, progress)
}

// UploadObject uploads a whole file with one signed request whose signature
// covers its content type and length.
func (api *API) UploadObject(
	ctx context.Context,
	path, contentType, method, uploadURL string,
	progress func(int64),
) error {
	info, err := os.Stat(path)
	if err != nil {
		return err
	}
	_, err = api.upload(ctx, path, 0, info.Size(), contentType, method, uploadURL, progress)
	return err
}

func (api *API) upload(
	ctx context.Context,
	path string,
	offset, length int64,
	contentType, method, uploadURL string,
	progress func(int64),
) (string, error) {
	file, err := openUploadRange(path, offset, length)
	if err != nil {
		return "", err
	}
	defer file.Close()
	requestCtx, cancel := context.WithCancelCause(ctx)
	defer cancel(nil)
	body := &uploadReader{
		reader:   io.NewSectionReader(file, offset, length),
		last:     api.Clock.Now(),
		api:      api,
		progress: progress,
	}
	request, err := http.NewRequestWithContext(requestCtx, method, uploadURL, body)
	if err != nil {
		return "", err
	}
	request.ContentLength = length
	if contentType != "" {
		request.Header.Set("Content-Type", contentType)
	}
	done, watchDone := api.watchUpload(requestCtx, body, cancel)
	response, err := api.send(api.UploadHTTP, request)
	if err == nil {
		defer response.Body.Close()
		err = readUploadResponse(response)
	}
	close(done)
	<-watchDone
	if errors.Is(context.Cause(requestCtx), ErrUploadStalled) {
		return "", ErrUploadStalled
	}
	if err != nil {
		return "", err
	}
	etag := response.Header.Get("ETag")
	if etag == "" {
		return "", errors.New("upload part returned no ETag")
	}
	return etag, nil
}

// openUploadRange opens path after checking the range lies inside it.
func openUploadRange(path string, offset, length int64) (*os.File, error) {
	if offset < 0 || length <= 0 {
		return nil, fmt.Errorf("invalid upload range offset=%d length=%d", offset, length)
	}
	file, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	info, err := file.Stat()
	if err == nil && (offset > info.Size() || length > info.Size()-offset) {
		err = fmt.Errorf("upload range offset=%d length=%d exceeds file", offset, length)
	}
	if err != nil {
		_ = file.Close()
		return nil, err
	}
	return file, nil
}

func (api *API) watchUpload(
	requestCtx context.Context,
	body *uploadReader,
	cancel context.CancelCauseFunc,
) (chan struct{}, chan struct{}) {
	done := make(chan struct{})
	watchDone := make(chan struct{})
	go func() {
		defer close(watchDone)
		for {
			remaining := uploadIdleTimeout - api.Clock.Now().Sub(body.lastProgress())
			if remaining <= 0 {
				cancel(ErrUploadStalled)
				return
			}
			timer := api.Clock.NewTimer(remaining)
			select {
			case <-done:
				timer.Stop()
				return
			case <-requestCtx.Done():
				timer.Stop()
				return
			case <-timer.Chan():
			}
		}
	}()
	return done, watchDone
}

func readUploadResponse(response *http.Response) error {
	if response.StatusCode < 200 || response.StatusCode >= 300 {
		// Preserve the prepared package for explicit resume after storage
		// rejects a part. API status retries belong to API requests.
		return errors.New("upload source part: " + responseError(response).Error())
	}
	_, err := ReadBounded(response.Body, errorResponseLimit)
	return err
}

package syncengine

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"image"
	_ "image/jpeg" // Register the JPEG decoder for uploaded artwork.
	"image/png"
	"net/http"
	"os"

	p "github.com/listenbox/client/crates/sync-engine/publicapi"
	_ "golang.org/x/image/webp" // Register YouTube's WebP artwork decoder.
)

const (
	artworkDownloadLimit = 20 << 20
)

func (api *API) UploadArtworkFile(ctx context.Context, show, path string) (string, error) {
	raw, err := os.ReadFile(path)
	if err != nil {
		return "", fmt.Errorf("read artwork %q: %w", path, err)
	}
	config, format, err := image.DecodeConfig(bytes.NewReader(raw))
	if err != nil {
		return "", fmt.Errorf("read artwork dimensions: %w", err)
	}
	if format != "jpeg" && format != "png" {
		return "", errors.New("artwork must be a JPEG or PNG image")
	}
	if config.Width != config.Height || config.Width < 1400 || config.Width > 3000 {
		return "", fmt.Errorf(
			"upload a square image between 1400 and 3000 pixels. Attempted resolution: %d × %d pixels",
			config.Width,
			config.Height,
		)
	}
	return api.UploadArtwork(ctx, show, raw)
}

func (api *API) UploadSourceArtwork(ctx context.Context, show, source string) (string, error) {
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, source, nil)
	if err != nil {
		return "", err
	}
	response, err := api.Do(request)
	if err != nil {
		return "", err
	}
	defer response.Body.Close()
	raw, err := ReadBounded(response.Body, artworkDownloadLimit)
	if err != nil {
		return "", err
	}
	return api.UploadArtwork(ctx, show, raw)
}

func (api *API) UploadArtwork(ctx context.Context, show string, raw []byte) (string, error) {
	raw, contentType, err := prepareArtwork(raw)
	if err != nil {
		return "", err
	}
	var signed p.CreatedImageUploadPresign
	if err := api.JSON(
		ctx,
		"POST",
		"/s/image-uploads/presign",
		p.CreateImageUploadPresign{
			ShowId:      show,
			ContentType: contentType,
			ByteLength:  int64(len(raw)),
		},
		&signed,
	); err != nil {
		return "", err
	}
	request, err := http.NewRequestWithContext(
		ctx,
		http.MethodPut,
		signed.UploadUrl,
		bytes.NewReader(raw),
	)
	if err != nil {
		return "", err
	}
	request.Header.Set("Content-Type", contentType)
	response, err := api.Do(request)
	if err != nil {
		return "", err
	}
	_, err = ReadBounded(response.Body, errorResponseLimit)
	response.Body.Close()
	if err != nil {
		return "", err
	}
	var completed p.ImageAsset
	err = api.JSON(
		ctx,
		"POST",
		"/s/image-uploads/"+signed.ImageAssetId+"/complete",
		p.CompleteImageUpload{ObjectKey: signed.ObjectKey},
		&completed,
	)
	return completed.Id, err
}

func prepareArtwork(raw []byte) ([]byte, string, error) {
	if len(raw) == 0 || len(raw) > artworkDownloadLimit {
		return nil, "", errors.New("artwork must contain at most 20 MiB")
	}
	config, format, err := image.DecodeConfig(bytes.NewReader(raw))
	if err != nil {
		return nil, "", err
	}
	contentType := "image/" + format
	if format == "webp" {
		if int64(config.Width)*int64(config.Height) > 16<<20 {
			return nil, "", errors.New("source artwork exceeds decoded image limit")
		}
		decoded, _, err := image.Decode(bytes.NewReader(raw))
		if err != nil {
			return nil, "", err
		}
		var output bytes.Buffer
		if err := png.Encode(&output, decoded); err != nil {
			return nil, "", err
		}
		raw = output.Bytes()
		contentType = "image/png"
	} else if format != "jpeg" && format != "png" {
		return nil, "", errors.New("artwork must be a JPEG, PNG or WebP image")
	}
	if len(raw) > artworkDownloadLimit {
		return nil, "", errors.New("converted artwork exceeds 20 MiB")
	}
	return raw, contentType, nil
}

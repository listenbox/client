package syncengine

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strings"
)

type Events struct {
	body  io.ReadCloser
	lines *bufio.Scanner
}

const (
	initialEventBuffer = 4096
	maximumEventSize   = 1 << 20
)

func (api *API) Events(ctx context.Context, path string) (*Events, error) {
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, api.Config.APIOrigin+path, nil)
	if err != nil {
		return nil, err
	}
	request.Header.Set("Accept", "text/event-stream")
	if api.Credential != "" {
		request.Header.Set("Authorization", "Bearer "+api.Credential)
	}
	response, err := api.Do(request)
	if err != nil {
		return nil, err
	}
	if !strings.HasPrefix(response.Header.Get("Content-Type"), "text/event-stream") {
		response.Body.Close()
		return nil, errors.New("expected an event stream")
	}
	scanner := bufio.NewScanner(response.Body)
	scanner.Buffer(make([]byte, initialEventBuffer), maximumEventSize)
	return &Events{response.Body, scanner}, nil
}
func (events *Events) Close() error { return events.body.Close() }
func (events *Events) Next() (map[string]any, error) {
	var data strings.Builder
	for events.lines.Scan() {
		line := strings.TrimSuffix(events.lines.Text(), "\r")
		if line == "" && data.Len() > 0 {
			var value map[string]any
			err := json.Unmarshal([]byte(data.String()), &value)
			return value, err
		}
		if value, ok := strings.CutPrefix(line, "data:"); ok {
			if data.Len() > 0 {
				data.WriteByte('\n')
			}
			data.WriteString(strings.TrimPrefix(value, " "))
			if data.Len() > 1<<20 {
				return nil, errors.New("event exceeds size limit")
			}
		}
	}
	if err := events.lines.Err(); err != nil {
		return nil, err
	}
	return nil, errors.New("event stream ended before a terminal event")
}

func EventString(event map[string]any, key string) (string, error) {
	value, ok := event[key].(string)
	if !ok || value == "" {
		return "", fmt.Errorf("response missing %s", key)
	}
	return value, nil
}

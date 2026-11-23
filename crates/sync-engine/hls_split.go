package syncengine

import (
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
)

// hlsInitName and hlsSegmentName are the API's storage contract for a normal
// (non trick-play) rendition: its playlist maps init.mp4 and lists
// <ordinal>.m4s in playlist order, so every object stays far below CDN cache
// limits and needs no per-object manifest row.
const hlsInitName = "init.mp4"

func hlsSegmentName(ordinal int) string { return strconv.Itoa(ordinal) + ".m4s" }

type hlsRange struct{ offset, length int64 }

var (
	hlsMapPattern = regexp.MustCompile(
		`^#EXT-X-MAP:URI="([^"]+)",BYTERANGE="([0-9]+)@([0-9]+)"$`,
	)
	hlsRangePattern = regexp.MustCompile(`^#EXT-X-BYTERANGE:([0-9]+)(?:@([0-9]+))?$`)
)

// hlsSingleFile is a parsed single-file byte-range media playlist.
type hlsSingleFile struct {
	path     string
	output   []string
	payload  string
	init     hlsRange
	segments []hlsRange
	pending  hlsRange
}

// splitHLSSingleFile rewrites a single-file byte-range media playlist into
// separate init.mp4 and <ordinal>.m4s files next to it, then removes the
// single payload. The ranges must tile the payload exactly from offset zero.
func splitHLSSingleFile(playlistPath string) error {
	body, err := os.ReadFile(playlistPath) // #nosec G304 -- generated package file.
	if err != nil {
		return err
	}
	playlist, err := parseHLSSingleFile(playlistPath, string(body))
	if err != nil {
		return err
	}
	directory := filepath.Dir(playlistPath)
	payload := filepath.Join(directory, playlist.payload)
	if err := writeHLSParts(payload, playlist); err != nil {
		return err
	}
	output := []byte(strings.Join(playlist.output, "\n") + "\n")
	// #nosec G703 -- These package files are generated inside the owned media directory.
	if err := os.WriteFile(playlistPath, output, privateFileMode); err != nil {
		return err
	}
	return os.Remove(payload)
}

// writeHLSParts copies the playlist's init and segment ranges out of payload.
// It closes payload before returning: Windows refuses to delete a file that
// any handle, including this one, still holds open.
func writeHLSParts(payload string, playlist *hlsSingleFile) error {
	source, err := os.Open(payload) // #nosec G304 -- generated package file.
	if err != nil {
		return err
	}
	defer func() { _ = source.Close() }()
	info, err := source.Stat()
	if err != nil {
		return err
	}
	last := playlist.segments[len(playlist.segments)-1]
	if last.offset+last.length != info.Size() {
		return fmt.Errorf("HLS playlist %q ranges do not cover its payload", playlist.path)
	}
	directory := filepath.Dir(payload)
	if err := writeHLSPart(source, filepath.Join(directory, hlsInitName), playlist.init); err != nil {
		return err
	}
	for ordinal, segment := range playlist.segments {
		if err := writeHLSPart(source, filepath.Join(directory, hlsSegmentName(ordinal)), segment); err != nil {
			return err
		}
	}
	return nil
}

func parseHLSSingleFile(playlistPath, body string) (*hlsSingleFile, error) {
	lines := strings.Split(strings.TrimSuffix(body, "\n"), "\n")
	playlist := &hlsSingleFile{
		path:    playlistPath,
		output:  make([]string, 0, len(lines)),
		pending: hlsRange{offset: -1},
	}
	for _, line := range lines {
		var err error
		switch {
		case strings.HasPrefix(line, "#EXT-X-MAP:"):
			err = playlist.mapLine(line)
		case strings.HasPrefix(line, "#EXT-X-BYTERANGE:"):
			err = playlist.rangeLine(line)
		case line != "" && !strings.HasPrefix(line, "#"):
			err = playlist.segmentLine(strings.TrimSpace(line))
		default:
			playlist.output = append(playlist.output, line)
		}
		if err != nil {
			return nil, err
		}
	}
	return playlist, playlist.complete()
}

func (p *hlsSingleFile) complete() error {
	if p.payload == "" || len(p.segments) == 0 || p.pending.offset >= 0 {
		return fmt.Errorf("HLS playlist %q has no complete media segments", p.path)
	}
	if p.payload == hlsInitName || strings.HasSuffix(p.payload, ".m4s") {
		return fmt.Errorf("HLS single-file payload %q collides with split names", p.payload)
	}
	return nil
}

func (p *hlsSingleFile) usePayload(name string) error {
	if name == "" || filepath.Base(name) != name || (p.payload != "" && p.payload != name) {
		return fmt.Errorf("HLS playlist %q has unexpected payload %q", p.path, name)
	}
	p.payload = name
	return nil
}

func (p *hlsSingleFile) mapLine(line string) error {
	match := hlsMapPattern.FindStringSubmatch(line)
	if match == nil || match[3] != "0" {
		return fmt.Errorf("HLS playlist %q has no leading initialization range", p.path)
	}
	if err := p.usePayload(match[1]); err != nil {
		return err
	}
	length, err := strconv.ParseInt(match[2], 10, 64)
	if err != nil || length <= 0 {
		return fmt.Errorf("HLS playlist %q has an invalid initialization range", p.path)
	}
	p.init = hlsRange{offset: 0, length: length}
	p.output = append(p.output, fmt.Sprintf("#EXT-X-MAP:URI=%q", hlsInitName))
	return nil
}

func (p *hlsSingleFile) rangeLine(line string) error {
	match := hlsRangePattern.FindStringSubmatch(line)
	if match == nil || p.pending.offset >= 0 {
		return fmt.Errorf("HLS playlist %q has an invalid byte range %q", p.path, line)
	}
	next := p.init.length
	if len(p.segments) > 0 {
		last := p.segments[len(p.segments)-1]
		next = last.offset + last.length
	}
	length, lengthErr := strconv.ParseInt(match[1], 10, 64)
	offset, offsetErr := next, error(nil)
	if match[2] != "" {
		offset, offsetErr = strconv.ParseInt(match[2], 10, 64)
	}
	if lengthErr != nil || offsetErr != nil || length <= 0 || offset != next {
		return fmt.Errorf("HLS playlist %q has a non-contiguous byte range %q", p.path, line)
	}
	p.pending = hlsRange{offset: offset, length: length}
	return nil
}

func (p *hlsSingleFile) segmentLine(uri string) error {
	if p.pending.offset < 0 || p.init.length == 0 {
		return fmt.Errorf("HLS playlist %q has unranged segment %q", p.path, uri)
	}
	if err := p.usePayload(uri); err != nil {
		return err
	}
	p.output = append(p.output, hlsSegmentName(len(p.segments)))
	p.segments = append(p.segments, p.pending)
	p.pending = hlsRange{offset: -1}
	return nil
}

func writeHLSPart(source *os.File, destination string, part hlsRange) error {
	// #nosec G304 -- caller owns the generated package directory.
	file, err := os.OpenFile(destination, os.O_WRONLY|os.O_CREATE|os.O_EXCL, privateFileMode)
	if err != nil {
		return err
	}
	written, copyErr := io.Copy(file, io.NewSectionReader(source, part.offset, part.length))
	copyErr = errors.Join(copyErr, file.Close())
	if copyErr == nil && written != part.length {
		copyErr = fmt.Errorf("split %q wrote %d of %d bytes", destination, written, part.length)
	}
	return copyErr
}

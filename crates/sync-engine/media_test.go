package syncengine

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	a "github.com/asticode/go-astiav"
)

func mediaFixture(name string) string { return filepath.Join("tests", "fixtures", "media", name) }
func TestLinkedFfmpegConfigurationIsVisibleInCi(t *testing.T) {
	t.Parallel()
	configuration := FFmpegConfiguration()
	t.Log("FFmpeg configure:", configuration)
	for _, required := range []string{"--enable-static", "--disable-everything", "--disable-network", "--disable-debug"} {
		if !strings.Contains(configuration, required) {
			t.Fatalf("linked configuration is missing %s", required)
		}
	}
	if a.FindEncoder(a.CodecIDMp3) != nil || strings.Contains(configuration, "--enable-gpl") ||
		strings.Contains(configuration, "--enable-nonfree") {
		t.Fatal("linked codecs differ from the distributable build")
	}
}

//nolint:cyclop,funlen // Inspect every decoded stream and packet from the independently opened media artifact.
func decodedMedia(t *testing.T, path string, kind a.MediaType) {
	t.Helper()
	input, err := openMedia(path)
	requireNoError(t, err)
	defer closeMedia(input)
	stream, err := mediaStream(input, kind)
	requireNoError(t, err)
	want := a.CodecIDAac
	if kind == a.MediaTypeVideo {
		want = a.CodecIDH264
	}
	if stream.CodecParameters().CodecID() != want {
		t.Fatalf("%s: codec=%s", path, stream.CodecParameters().CodecID())
	}
	decoder := a.AllocCodecContext(a.FindDecoder(want))
	if decoder == nil {
		t.Fatal("allocate decoder")
	}
	defer decoder.Free()
	requireNoError(t, stream.CodecParameters().ToCodecContext(decoder))
	requireNoError(t, decoder.Open(a.FindDecoder(want), nil))
	packet, frame := a.AllocPacket(), a.AllocFrame()
	defer packet.Free()
	defer frame.Free()
	var samples, frames int
	receive := func() {
		for {
			err := decoder.ReceiveFrame(frame)
			if waitingMedia(err) {
				return
			}
			requireNoError(t, err)
			samples += frame.NbSamples()
			frames++
			if kind == a.MediaTypeVideo && (frame.Width() != 1280 || frame.Height() != 720) {
				t.Fatalf("decoded %dx%d", frame.Width(), frame.Height())
			}
			frame.Unref()
		}
	}
	for {
		err := input.ReadFrame(packet)
		if errors.Is(err, a.ErrEof) {
			break
		}
		requireNoError(t, err)
		if packet.StreamIndex() == stream.Index() {
			requireNoError(t, decoder.SendPacket(packet))
			receive()
		}
		packet.Unref()
	}
	requireNoError(t, decoder.SendPacket(nil))
	receive()
	minimum, maximum := 1.8, 2.3
	if kind == a.MediaTypeAudio {
		seconds := float64(samples) / float64(decoder.SampleRate())
		if seconds < minimum || seconds > maximum {
			t.Fatalf("decoded %.2fs from %s", seconds, path)
		}
	} else {
		seconds := float64(frames) / 30
		if seconds < minimum-.3 || seconds > maximum+.3 {
			t.Fatalf("decoded %d frames from %s", frames, path)
		}
	}
}

func preparedAudio(t *testing.T, name string) {
	t.Helper()
	output := filepath.Join(t.TempDir(), preparedAudioName)
	requireNoError(t, PrepareAudio(t.Context(), mediaFixture(name), output))
	stat, err := os.Stat(output)
	requireNoError(t, err)
	if stat.Size() < 5000 || stat.Size() > 60000 {
		t.Fatalf("audio size=%d", stat.Size())
	}
	fast, err := FastStart(output)
	requireNoError(t, err)
	if !fast {
		t.Fatal("M4A does not start before its media data")
	}
	decodedMedia(t, output, a.MediaTypeAudio)
}

func TestAacIsRemuxedToPlayableFastStartM4A(
	t *testing.T,
) {
	t.Parallel()
	preparedAudio(t, "aac.m4a")
}

func TestOpusIsDecodedResampledAndEncodedToAac(t *testing.T) {
	t.Parallel()
	preparedAudio(t, "opus.webm")
}

func prepareVideoFixture(t *testing.T, separate, long bool) string {
	t.Helper()
	directory := filepath.Join(t.TempDir(), "native media ü")
	requireNoError(t, os.Mkdir(directory, 0o700))
	name := "avc-aac.mp4"
	audio := "opus.webm"
	if long {
		name = "avc-aac-long-gop.mp4"
		audio = "opus-long.webm"
	}
	copyFile := func(from, to string) {
		raw, err := os.ReadFile(mediaFixture(from))
		requireNoError(t, err)
		// #nosec G703 -- The test owns the configured temporary profile and artifact paths.
		requireNoError(t, os.WriteFile(filepath.Join(directory, to), raw, 0o600))
	}
	copyFile(name, "source-video")
	if separate {
		copyFile(audio, "source-audio")
	}
	ctx, cancel := context.WithTimeout(t.Context(), 8*time.Second)
	defer cancel()
	seconds, err := PrepareVideo(ctx, directory, separate, 1080, "en")
	requireNoError(t, err)
	if (!long && (seconds < 2 || seconds > 3)) || (long && (seconds < 8 || seconds > 9)) {
		t.Fatalf("duration=%d", seconds)
	}
	return directory
}

//nolint:cyclop // Verify all generated variants, codecs and playlist relationships together.
func assertVideoPackage(t *testing.T, separate bool) {
	t.Helper()
	directory := prepareVideoFixture(t, separate, false)
	for _, file := range []string{"video.mp4", preparedAudioName} {
		path := filepath.Join(directory, file)
		fast, err := FastStart(path)
		requireNoError(t, err)
		if !fast {
			t.Fatal(file, "is not fast start")
		}
		decodedMedia(t, path, a.MediaTypeAudio)
	}
	decodedMedia(t, filepath.Join(directory, "video.mp4"), a.MediaTypeVideo)
	master, err := os.ReadFile(filepath.Join(directory, "hls/master.m3u8"))
	requireNoError(t, err)
	for _, required := range []string{"RESOLUTION=1280x720", "audio/index.m3u8", "video/index.m3u8"} {
		if !strings.Contains(string(master), required) {
			t.Fatalf("master missing %s: %s", required, master)
		}
	}
	for _, kind := range []string{"audio", "video"} {
		root := filepath.Join(directory, "hls", kind)
		playlist, err := os.ReadFile(filepath.Join(root, "index.m3u8"))
		requireNoError(t, err)
		for _, required := range []string{"#EXT-X-ENDLIST", `#EXT-X-MAP:URI="media.mp4",BYTERANGE="`, "#EXT-X-BYTERANGE:"} {
			if !strings.Contains(string(playlist), required) {
				t.Fatalf("%s playlist missing %s", kind, required)
			}
		}
		segments := 0
		for line := range strings.SplitSeq(string(playlist), "\n") {
			if line != "" && !strings.HasPrefix(line, "#") {
				if line != "media.mp4" {
					t.Fatalf("non-relative segment %q", line)
				}
				segments++
			}
		}
		if segments < 1 || segments > 2 {
			t.Fatalf("segments=%d", segments)
		}
		typ := a.MediaTypeAudio
		if kind == "video" {
			typ = a.MediaTypeVideo
		}
		decodedMedia(t, filepath.Join(root, "media.mp4"), typ)
	}
}

func TestCombinedAvcAacProducesPlayableMp4AndHls(t *testing.T) {
	t.Parallel()
	assertVideoPackage(t, false)
}

func TestSeparateOpusAndAvcProducePlayableMp4AndHls(t *testing.T) {
	t.Parallel()
	assertVideoPackage(t, true)
}

func TestInvalidAudioFailsWithoutLeavingAnOutput(t *testing.T) {
	t.Parallel()
	directory := t.TempDir()
	source, output := filepath.Join(
		directory,
		"invalid",
	), filepath.Join(
		directory,
		preparedAudioName,
	)
	requireNoError(t, os.WriteFile(source, []byte("not a media container"), 0o600))
	if err := PrepareAudio(t.Context(), source, output); err == nil {
		t.Fatal("invalid audio succeeded")
	}
	if _, err := os.Stat(output); !os.IsNotExist(err) {
		t.Fatal("failed preparation left output")
	}
	_, err := os.Stat(source)
	requireNoError(t, err)
}

func TestCancelledPreparationDoesNotCreateOutput(t *testing.T) {
	t.Parallel()
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	output := filepath.Join(t.TempDir(), preparedAudioName)
	if err := PrepareAudio(
		ctx,
		mediaFixture("opus.webm"),
		output,
	); !errors.Is(
		err,
		context.Canceled,
	) {
		t.Fatalf("cancellation: %v", err)
	}
	if _, err := os.Stat(output); !os.IsNotExist(err) {
		t.Fatal("cancelled preparation left output")
	}
}

//nolint:cyclop,funlen // Verify every segment and finding returned by the independent platform validator.
func validateLongGOP(t *testing.T, separate bool) {
	t.Helper()
	if _, err := exec.LookPath("mediastreamvalidator"); err != nil {
		t.Skip("requires the Apple HLS tools used by CI")
	}
	ctx, cancel := context.WithTimeout(t.Context(), 9*time.Second)
	defer cancel()
	directory := prepareVideoFixture(t, separate, true)
	reportPath := filepath.Join(directory, "validation.json")
	command := exec.CommandContext(
		ctx,
		"mediastreamvalidator",
		// Keep the 1.x JSON, error codes and requirement levels read below.
		"--compatible-output=full",
		"--quiet",
		"--timeout",
		"5",
		"--validation-data-path",
		reportPath,
		filepath.Join(directory, "hls/master.m3u8"),
	)
	output, commandError := command.CombinedOutput()
	raw, err := os.ReadFile(reportPath)
	requireNoError(t, err)
	var report map[string]any
	requireNoError(t, json.Unmarshal(raw, &report))
	variants, ok := report["variants"].([]any)
	if !ok {
		t.Fatalf("no validated variants: %s\n%s", raw, output)
	}
	visited := 0
	for _, entry := range variants {
		variant := validationValue[map[string]any](t, entry)
		if _, ok := variant["url"].(string); !ok {
			continue
		}
		visited++
		parsed, _ := variant["parsedSegmentsCount"].(float64)
		processed, _ := variant["processedSegmentsCount"].(float64)
		if parsed <= 1 || processed != parsed {
			t.Fatalf("validator did not consume all segments: %+v", variant)
		}
		var measured float64
		for _, discontinuity := range validationValue[[]any](t, variant["discontinuities"]) {
			discontinuity := validationValue[map[string]any](t, discontinuity)
			measurements := validationValue[map[string]any](t, discontinuity["measurements"])
			measured += validationValue[float64](t, measurements["measuredSegments"])
		}
		if measured != processed {
			t.Fatalf("validator did not analyze media: %+v", variant)
		}
	}
	if visited != 4 {
		t.Fatalf("visited %d media variants", visited)
	}
	var findings []any
	var visit func(any)
	visit = func(value any) {
		switch value := value.(type) {
		case map[string]any:
			if level, ok := value["errorRequirementLevel"].(float64); ok &&
				(level == 1 || level >= 6) {
				findings = append(findings, value)
			}
			for _, child := range value {
				visit(child)
			}
		case []any:
			for _, child := range value {
				visit(child)
			}
		}
	}
	visit(report)
	if len(findings) > 0 {
		t.Fatalf("Apple HLS MUST-fix findings: %v", findings)
	}
	if commandError != nil {
		t.Fatal(fmt.Errorf("Apple HLS validation: %w\n%s", commandError, output))
	}
}

func TestAppleHlsCombinedLongGopIsPlayable(t *testing.T) { t.Parallel(); validateLongGOP(t, false) }

func TestAppleHlsSeparateOpusLongGopIsPlayable(
	t *testing.T,
) {
	t.Parallel()
	validateLongGOP(t, true)
}

//nolint:ireturn // Preserve the concrete type selected by the validator field contract.
func validationValue[T any](t *testing.T, value any) T {
	t.Helper()
	parsed, ok := value.(T)
	if !ok {
		t.Fatalf("media validator field has type %T, expected %T", value, *new(T))
	}
	return parsed
}

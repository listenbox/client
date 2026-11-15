package syncengine

import (
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"math"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	a "github.com/asticode/go-astiav"
)

type mediaOutput struct {
	format *a.FormatContext
	io     *a.IOContext
}

const (
	audioSampleRate       = 48000
	defaultAudioBitrate   = 128000
	mediaTimeBase         = 1000000
	peakRateNumerator     = 12
	peakRateDenominator   = 10
	videoThreadCount      = 2
	segmentSeconds        = 6
	frameBoundaryEpsilon  = 1e-7
	maximumVideoFrameRate = 60
	minimumVideoDimension = 2
	minimumSeekFrames     = 2
	millisecondsPerSecond = 1000
	seekPreviewBitrate    = 12000
	height180p            = 180
	height360p            = 360
	height720p            = 720
	height1080p           = 1080
	height1440p           = 1440
	videoBitrate180p      = 96000
	videoBitrate360p      = 365000
	videoBitrate720p      = 3000000
	videoBitrate1080p     = 6000000
	videoBitrate1440p     = 9000000
	videoBitrate2160p     = 16000000
	bitsPerByte           = 8
	muxStreamCount        = 2
)

const (
	finalizingStatus = "finalizing"
	movflagsOption   = "movflags"
)

func openMedia(path string) (*a.FormatContext, error) {
	f := a.AllocFormatContext()
	if f == nil {
		return nil, errors.New("allocate media input")
	}
	if err := f.OpenInput(filepath.ToSlash(path), nil, nil); err != nil {
		f.Free()
		return nil, err
	}
	if err := f.FindStreamInfo(nil); err != nil {
		f.CloseInput()
		f.Free()
		return nil, err
	}
	return f, nil
}
func closeMedia(f *a.FormatContext) { f.CloseInput(); f.Free() }
func mediaStream(f *a.FormatContext, kind a.MediaType) (*a.Stream, error) {
	for _, stream := range f.Streams() {
		if stream.CodecParameters().MediaType() == kind {
			return stream, nil
		}
	}
	return nil, fmt.Errorf("source missing required %s stream", kind.String())
}

func newMediaOutput(path, format string) (*mediaOutput, error) {
	f, err := a.AllocOutputFormatContext(nil, format, filepath.ToSlash(path))
	if err != nil {
		return nil, err
	}
	out := &mediaOutput{format: f}
	if !f.OutputFormat().Flags().Has(a.IOFormatFlagNofile) {
		pb, err := a.OpenIOContext(
			filepath.ToSlash(path),
			a.NewIOContextFlags(a.IOContextFlagWrite),
			nil,
			nil,
		)
		if err != nil {
			f.Free()
			return nil, err
		}
		out.io = pb
		f.SetPb(pb)
	}
	return out, nil
}

func (out *mediaOutput) close() {
	if out.io != nil {
		_ = out.io.Close()
	}
	out.format.Free()
}

func mediaOptions(values map[string]string) (*a.Dictionary, error) {
	d := a.NewDictionary()
	for key, value := range values {
		if err := d.Set(key, value, 0); err != nil {
			d.Free()
			return nil, err
		}
	}
	return d, nil
}

func writeMediaHeader(out *mediaOutput, values map[string]string) error {
	options, err := mediaOptions(values)
	if err != nil {
		return err
	}
	defer options.Free()
	return out.format.WriteHeader(options)
}

func addMediaStream(out *mediaOutput, source *a.Stream) (*a.Stream, error) {
	stream := out.format.NewStream(nil)
	if stream == nil {
		return nil, errors.New("allocate output media stream")
	}
	if err := source.CodecParameters().Copy(stream.CodecParameters()); err != nil {
		return nil, err
	}
	stream.CodecParameters().SetCodecTag(0)
	stream.SetTimeBase(source.TimeBase())
	return stream, nil
}

func copyPackets(
	ctx context.Context,
	input *a.FormatContext,
	source *a.Stream,
	out *mediaOutput,
	target *a.Stream,
) error {
	packet := a.AllocPacket()
	defer packet.Free()
	count := 0
	for {
		if err := ctx.Err(); err != nil {
			return err
		}
		err := input.ReadFrame(packet)
		if errors.Is(err, a.ErrEof) {
			break
		}
		if err != nil {
			return err
		}
		if packet.StreamIndex() == source.Index() {
			packet.RescaleTs(source.TimeBase(), target.TimeBase())
			packet.SetStreamIndex(target.Index())
			packet.SetPos(-1)
			if err := out.format.WriteInterleavedFrame(packet); err != nil {
				packet.Unref()
				return err
			}
			count++
		}
		packet.Unref()
	}
	if count == 0 {
		return errors.New("downloaded media stream was empty")
	}
	return nil
}
func waitingMedia(err error) bool { return errors.Is(err, a.ErrEof) || errors.Is(err, a.ErrEagain) }

func PrepareAudio(ctx context.Context, inputPath, outputPath string) (err error) {
	if err := ctx.Err(); err != nil {
		return err
	}
	if err := validateAudioDestination(inputPath, outputPath); err != nil {
		return err
	}
	defer func() {
		if err != nil {
			os.Remove(outputPath)
		}
	}()
	a.SetLogLevel(a.LogLevelError)
	input, err := openMedia(inputPath)
	if err != nil {
		return err
	}
	defer closeMedia(input)
	source, err := mediaStream(input, a.MediaTypeAudio)
	if err != nil {
		return err
	}
	out, err := newMediaOutput(outputPath, "ipod")
	if err != nil {
		return err
	}
	defer out.close()
	if err := writePreparedAudio(ctx, input, source, out, outputPath); err != nil {
		return err
	}
	fast, err := FastStart(outputPath)
	if err != nil {
		return err
	}
	if !fast {
		return errors.New("M4A fast-start validation failed")
	}
	return nil
}

func transcodeAudio(
	ctx context.Context,
	input *a.FormatContext,
	source *a.Stream,
	out *mediaOutput,
) error {
	decoder, err := newAudioDecoder(source)
	if err != nil {
		return err
	}
	defer decoder.Free()
	if decoder.SampleRate() <= 0 || decoder.ChannelLayout().Channels() <= 0 {
		return errors.New("invalid input audio format")
	}
	layout := a.ChannelLayoutStereo
	if decoder.ChannelLayout().Channels() == 1 {
		layout = a.ChannelLayoutMono
	}
	encoder, codec, err := newAACEncoder(layout)
	if err != nil {
		return err
	}
	defer encoder.Free()
	target := out.format.NewStream(codec)
	if target == nil {
		return errors.New("allocate AAC stream")
	}
	target.SetTimeBase(encoder.TimeBase())
	if err := target.CodecParameters().FromCodecContext(encoder); err != nil {
		return err
	}
	if err := writeMediaHeader(out, map[string]string{movflagsOption: faststartFlag}); err != nil {
		return err
	}
	resampler := a.AllocSoftwareResampleContext()
	if resampler == nil {
		return errors.New("allocate audio resampler")
	}
	defer resampler.Free()
	fifo := a.AllocAudioFifo(a.SampleFormatFltp, layout.Channels(), encoder.FrameSize())
	if fifo == nil {
		return errors.New("allocate audio sample buffer")
	}
	defer fifo.Free()
	packet := a.AllocPacket()
	defer packet.Free()
	decoded := a.AllocFrame()
	defer decoded.Free()
	encoded := a.AllocPacket()
	defer encoded.Free()
	transcoder := audioTranscoder{
		encoder: encoder, decoder: decoder, target: target, out: out,
		encoded: encoded, decoded: decoded, fifo: fifo, layout: layout, resampler: resampler,
	}
	if err := transcoder.decodePackets(ctx, input, source, packet); err != nil {
		return err
	}
	return transcoder.finish(ctx)
}

func FastStart(path string) (bool, error) {
	file, err := os.Open(path)
	if err != nil {
		return false, err
	}
	defer file.Close()
	info, err := file.Stat()
	if err != nil {
		return false, err
	}
	total := info.Size()
	for offset := int64(0); offset+8 <= total; {
		if _, err := file.Seek(offset, io.SeekStart); err != nil {
			return false, err
		}
		var header [8]byte
		if _, err := io.ReadFull(file, header[:]); err != nil {
			return false, err
		}
		size := int64(binary.BigEndian.Uint32(header[:4]))
		kind := string(header[4:])
		if size == 1 {
			size, err = extendedBoxSize(file)
			if err != nil {
				return false, err
			}
		}
		if size < 8 || size > total-offset {
			return false, nil
		}
		if kind == "moov" {
			return true, nil
		}
		if kind == "mdat" {
			return false, nil
		}
		offset += size
	}
	return false, nil
}

func MediaDuration(path string) (int64, error) {
	input, err := openMedia(path)
	if err != nil {
		return 0, err
	}
	defer closeMedia(input)
	seconds := math.Ceil(float64(input.Duration()) / mediaTimeBase)
	if math.IsNaN(seconds) || seconds < 1 || seconds > 604800 {
		return 0, errors.New("invalid prepared media duration")
	}
	return int64(seconds), nil
}

func hlsOptions(root string) map[string]string {
	return map[string]string{
		"hls_time":             "6",
		"hls_playlist_type":    "vod",
		"hls_segment_type":     "fmp4",
		"hls_flags":            "single_file",
		"hls_segment_filename": filepath.ToSlash(filepath.Join(root, "media.mp4")),
	}
}

func packageTrack(
	ctx context.Context,
	inputPath, directory, name string,
	kind a.MediaType,
) (string, error) {
	root := filepath.Join(directory, "hls", name)
	if err := os.MkdirAll(root, profileDirectoryMode); err != nil {
		return "", err
	}
	path := filepath.Join(root, "index.m3u8")
	input, err := openMedia(inputPath)
	if err != nil {
		return "", err
	}
	defer closeMedia(input)
	source, err := mediaStream(input, kind)
	if err != nil {
		return "", err
	}
	out, err := newMediaOutput(path, "hls")
	if err != nil {
		return "", err
	}
	defer out.close()
	target, err := addMediaStream(out, source)
	if err != nil {
		return "", err
	}
	if err := writeMediaHeader(out, hlsOptions(root)); err != nil {
		return "", err
	}
	if err := copyPackets(ctx, input, source, out, target); err != nil {
		return "", err
	}
	return path, out.format.WriteTrailer()
}

type videoEncoder struct {
	encoder          *a.CodecContext
	scaler           *a.SoftwareScaleContext
	output           *mediaOutput
	stream           *a.Stream
	rate, inputTime  a.Rational
	first            *int64
	previous         *a.Frame
	next, limit, gop int64
}

func newVideoEncoder(
	source *a.Stream,
	decoder *a.CodecContext,
	destination string,
	width, height int,
	rate a.Rational,
	bitrate int64,
	iframe bool,
	duration float64,
) (*videoEncoder, error) {
	codec := a.FindEncoderByName("libopenh264")
	if codec == nil {
		return nil, errors.New("bundled OpenH264 encoder unavailable")
	}
	encoder := a.AllocCodecContext(codec)
	if encoder == nil {
		return nil, errors.New("allocate video encoder")
	}
	state := &videoEncoder{
		encoder:   encoder,
		rate:      rate,
		inputTime: source.TimeBase(),
		limit:     int64(math.Max(1, math.Ceil(duration*rate.Float64()))),
	}
	success := false
	defer func() {
		if !success {
			state.close()
		}
	}()
	state.configure(width, height, rate, bitrate, iframe)
	options, err := mediaOptions(map[string]string{"allow_skip_frames": "0", "rc_mode": "bitrate"})
	if err != nil {
		return nil, err
	}
	defer options.Free()
	if err := encoder.Open(codec, options); err != nil {
		return nil, err
	}
	if err := state.scaleFrom(decoder, width, height); err != nil {
		return nil, err
	}
	state.output, err = newMediaOutput(destination, "hls")
	if err != nil {
		return nil, err
	}
	state.stream = state.output.format.NewStream(codec)
	if state.stream == nil {
		return nil, errors.New("allocate video stream")
	}
	if err := state.stream.CodecParameters().FromCodecContext(encoder); err != nil {
		return nil, err
	}
	state.stream.SetTimeBase(encoder.TimeBase())
	state.stream.SetAvgFrameRate(rate)
	state.stream.SetRFrameRate(rate)
	if err := writeMediaHeader(state.output, hlsOptions(filepath.Dir(destination))); err != nil {
		return nil, err
	}
	success = true
	return state, nil
}

func (v *videoEncoder) close() {
	if v.previous != nil {
		v.previous.Free()
	}
	if v.scaler != nil {
		v.scaler.Free()
	}
	if v.encoder != nil {
		v.encoder.Free()
	}
	if v.output != nil {
		v.output.close()
	}
}

func (v *videoEncoder) drain() error {
	packet := a.AllocPacket()
	defer packet.Free()
	for {
		err := v.encoder.ReceivePacket(packet)
		if waitingMedia(err) {
			return nil
		}
		if err != nil {
			return err
		}
		packet.SetDuration(1)
		packet.RescaleTs(v.encoder.TimeBase(), v.stream.TimeBase())
		packet.SetStreamIndex(v.stream.Index())
		packet.SetPos(-1)
		if err := v.output.format.WriteInterleavedFrame(packet); err != nil {
			return err
		}
		packet.Unref()
	}
}

func (v *videoEncoder) sendPrevious() error {
	if v.previous == nil {
		return errors.New("missing decoded video")
	}
	frame := v.previous.Clone()
	if frame == nil {
		return errors.New("clone decoded video")
	}
	defer frame.Free()
	frame.SetPts(v.next)
	boundary := int64(v.rate.Num()) * segmentSeconds
	ticks := int64(v.rate.Den())
	kind := a.PictureTypeNone
	if v.gop == 1 || v.next == 0 || v.next*ticks/boundary > (v.next-1)*ticks/boundary {
		kind = a.PictureTypeI
	}
	frame.SetPictureType(kind)
	if err := v.encoder.SendFrame(frame); err != nil {
		return err
	}
	v.next++
	return v.drain()
}

func (v *videoEncoder) accept(ctx context.Context, frame *a.Frame) error {
	timestamp := frame.Pts()
	if timestamp == math.MinInt64 {
		return errors.New("source video has no frame timestamp")
	}
	if v.first == nil {
		v.first = &timestamp
	}
	index := int64(
		math.Floor(
			float64(
				timestamp-*v.first,
			)*v.inputTime.Float64()*v.rate.Float64() + frameBoundaryEpsilon,
		),
	)
	if index < v.next || index >= v.limit {
		return nil
	}
	for v.next < index && v.previous != nil {
		if err := ctx.Err(); err != nil {
			return err
		}
		if err := v.sendPrevious(); err != nil {
			return err
		}
	}
	scaled := a.AllocFrame()
	if scaled == nil {
		return errors.New("allocate scaled video frame")
	}
	scaled.SetWidth(v.encoder.Width())
	scaled.SetHeight(v.encoder.Height())
	scaled.SetPixelFormat(a.PixelFormatYuv420P)
	if err := v.scaler.ScaleFrame(frame, scaled); err != nil {
		scaled.Free()
		return err
	}
	if v.previous != nil {
		v.previous.Free()
	}
	v.previous = scaled
	return v.sendPrevious()
}

func PrepareVideo(
	ctx context.Context,
	directory string,
	separateAudio bool,
	maxHeight int,
	language string,
) (int64, error) {
	if maxHeight < 2 || maxHeight > 2160 {
		return 0, fmt.Errorf("invalid video delivery ceiling %d", maxHeight)
	}
	if _, err := Language(language); err != nil {
		return 0, err
	}
	a.SetLogLevel(a.LogLevelError)
	videoPath := filepath.Join(directory, "source-video")
	audioPath := videoPath
	if separateAudio {
		audioPath = filepath.Join(directory, "source-audio")
	}
	preparedAudio := filepath.Join(directory, preparedAudioName)
	if err := PrepareAudio(ctx, audioPath, preparedAudio); err != nil {
		return 0, err
	}
	input, err := openMedia(videoPath)
	if err != nil {
		return 0, err
	}
	defer closeMedia(input)
	source, err := mediaStream(input, a.MediaTypeVideo)
	if err != nil {
		return 0, err
	}
	decoder, err := newVideoDecoder(source)
	if err != nil {
		return 0, err
	}
	defer decoder.Free()
	preparation, err := newVideoPreparation(
		source,
		decoder,
		input.Duration(),
		directory,
		language,
		maxHeight,
	)
	if err != nil {
		return 0, err
	}
	defer preparation.close()
	if err := preparation.createRenditions(ctx); err != nil {
		return 0, err
	}
	if err := preparation.encode(ctx, input); err != nil {
		return 0, err
	}
	return preparation.finish(ctx)
}

func videoBitrate(height int) int64 {
	switch {
	case height <= height180p:
		return videoBitrate180p
	case height <= height360p:
		return videoBitrate360p
	case height <= height720p:
		return videoBitrate720p
	case height <= height1080p:
		return videoBitrate1080p
	case height <= height1440p:
		return videoBitrate1440p
	default:
		return videoBitrate2160p
	}
}

func harmonizeTargetDurations(paths []string) error {
	target := int64(segmentSeconds)
	for _, path := range paths {
		raw, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		for line := range strings.SplitSeq(string(raw), "\n") {
			if value, ok := strings.CutPrefix(line, "#EXT-X-TARGETDURATION:"); ok {
				n, err := strconv.ParseInt(value, 10, 64)
				if err != nil {
					return err
				}
				target = max(target, n)
			}
		}
	}
	for _, path := range paths {
		raw, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		lines := strings.Split(string(raw), "\n")
		for i, line := range lines {
			if strings.HasPrefix(line, "#EXT-X-TARGETDURATION:") {
				lines[i] = fmt.Sprintf("#EXT-X-TARGETDURATION:%d", target)
			}
		}
		// #nosec G703 -- These package files are generated inside the owned media directory.
		if err := os.WriteFile(
			path,
			[]byte(strings.Join(lines, "\n")),
			privateFileMode,
		); err != nil {
			return err
		}
	}
	return nil
}

func playlistBandwidth(path string) (int64, int64, float64, error) {
	raw, err := os.ReadFile(path)
	if err != nil {
		return 0, 0, 0, err
	}
	var segments []measuredSegment
	target, duration, totalDuration := 0.0, 0.0, 0.0
	totalBytes := int64(0)
	for line := range strings.SplitSeq(string(raw), "\n") {
		if value, ok := strings.CutPrefix(line, "#EXT-X-TARGETDURATION:"); ok {
			target, err = strconv.ParseFloat(value, 64)
		} else if value, ok := strings.CutPrefix(line, "#EXTINF:"); ok {
			duration, err = strconv.ParseFloat(strings.Split(value, ",")[0], 64)
		} else if value, ok := strings.CutPrefix(line, "#EXT-X-BYTERANGE:"); ok {
			segment, err := measuredPlaylistSegment(value, duration)
			if err != nil {
				return 0, 0, 0, err
			}
			segments = append(segments, segment)
			length := segment.bytes
			totalBytes += length
			totalDuration += duration
			duration = 0
		}
		if err != nil {
			return 0, 0, 0, err
		}
	}
	if target <= 0 || totalBytes <= 0 || totalDuration <= 0 {
		return 0, 0, 0, errors.New("empty HLS measurement")
	}
	peak, average := measuredBandwidth(segments, target, totalBytes, totalDuration)
	return peak, average, totalDuration, nil
}

func videoFacts(path string) (int, int, float64, string, error) {
	input, err := openMedia(path)
	if err != nil {
		return 0, 0, 0, "", err
	}
	defer closeMedia(input)
	stream, err := mediaStream(input, a.MediaTypeVideo)
	if err != nil {
		return 0, 0, 0, "", err
	}
	params := stream.CodecParameters()
	extra := params.ExtraData()
	if len(extra) < 4 || extra[0] != 1 {
		return 0, 0, 0, "", errors.New("generated video has no AVC configuration record")
	}
	return params.Width(), params.Height(), stream.AvgFrameRate().
			Float64(),
		fmt.Sprintf(
			"avc1.%02X%02X%02X",
			extra[1],
			extra[2],
			extra[3],
		), nil
}

func muxMedia(ctx context.Context, videoPath, audioPath, destination string) error {
	video, err := openMedia(videoPath)
	if err != nil {
		return err
	}
	defer closeMedia(video)
	audio, err := openMedia(audioPath)
	if err != nil {
		return err
	}
	defer closeMedia(audio)
	out, err := newMediaOutput(destination, "mp4")
	if err != nil {
		return err
	}
	defer out.close()
	inputs := []*a.FormatContext{video, audio}
	kinds := []a.MediaType{a.MediaTypeVideo, a.MediaTypeAudio}
	sources := make([]*a.Stream, muxStreamCount)
	targets := make([]*a.Stream, muxStreamCount)
	for i := range inputs {
		sources[i], err = mediaStream(inputs[i], kinds[i])
		if err != nil {
			return err
		}
		targets[i], err = addMediaStream(out, sources[i])
		if err != nil {
			return err
		}
	}
	if err := writeMediaHeader(out, map[string]string{movflagsOption: faststartFlag}); err != nil {
		return err
	}
	packets := []*a.Packet{a.AllocPacket(), a.AllocPacket()}
	defer packets[0].Free()
	defer packets[1].Free()
	available := []bool{false, false}

	mux := mediaMux{
		inputs:    inputs,
		sources:   sources,
		targets:   targets,
		packets:   packets,
		available: available,
		out:       out,
	}
	if err := mux.prime(ctx); err != nil {
		return err
	}
	return mux.copy(ctx)
}

func packetTimestamp(packet *a.Packet, stream *a.Stream) float64 {
	return float64(packet.Dts()) * stream.TimeBase().Float64()
}

func (v *videoEncoder) configure(
	width, height int,
	rate a.Rational,
	bitrate int64,
	iframe bool,
) {
	encoder := v.encoder
	encoder.SetWidth(width)
	encoder.SetHeight(height)
	encoder.SetPixelFormat(a.PixelFormatYuv420P)
	encoder.SetTimeBase(a.NewRational(rate.Den(), rate.Num()))
	encoder.SetFramerate(rate)
	encoder.SetBitRate(bitrate)
	encoder.SetRateControlMaxRate(bitrate * peakRateNumerator / peakRateDenominator)
	encoder.SetMaxBFrames(0)
	encoder.SetThreadCount(videoThreadCount)
	encoder.SetFlags(a.NewCodecContextFlags(a.CodecContextFlagGlobalHeader))
	gop := int(math.Max(1, math.Ceil(rate.Float64()*segmentSeconds)))
	if iframe {
		gop = 1
	}
	encoder.SetGopSize(gop)
	v.gop = int64(gop)
}

func (v *videoEncoder) scaleFrom(decoder *a.CodecContext, width, height int) error {
	var err error
	v.scaler, err = a.CreateSoftwareScaleContext(
		decoder.Width(),
		decoder.Height(),
		decoder.PixelFormat(),
		width,
		height,
		a.PixelFormatYuv420P,
		a.NewSoftwareScaleContextFlags(a.SoftwareScaleContextFlagBilinear),
	)
	if err != nil {
		return err
	}
	return nil
}

func remuxAudio(
	ctx context.Context,
	input *a.FormatContext,
	source *a.Stream,
	out *mediaOutput,
) error {
	target, err := addMediaStream(out, source)
	if err != nil {
		return err
	}
	if err := writeMediaHeader(
		out,
		map[string]string{movflagsOption: faststartFlag},
	); err != nil {
		return err
	}
	if err := copyPackets(ctx, input, source, out, target); err != nil {
		return err
	}
	if err := out.format.WriteTrailer(); err != nil {
		return err
	}
	return nil
}

func validateAudioDestination(inputPath, outputPath string) error {
	if inputPath == outputPath {
		return fmt.Errorf("audio output %q must be a new file", outputPath)
	}
	if _, err := os.Stat(outputPath); err == nil {
		return fmt.Errorf("audio output %q already exists", outputPath)
	} else if !os.IsNotExist(err) {
		return err
	}
	return nil
}

func newAudioDecoder(source *a.Stream) (*a.CodecContext, error) {
	codec := a.FindDecoder(source.CodecParameters().CodecID())
	if codec == nil {
		return nil, errors.New("bundled audio decoder unavailable")
	}
	decoder := a.AllocCodecContext(codec)
	if decoder == nil {
		return nil, errors.New("allocate audio decoder")
	}
	if err := source.CodecParameters().ToCodecContext(decoder); err != nil {
		decoder.Free()
		return nil, err
	}
	if err := decoder.Open(codec, nil); err != nil {
		decoder.Free()
		return nil, err
	}
	return decoder, nil
}

func newAACEncoder(layout a.ChannelLayout) (*a.CodecContext, *a.Codec, error) {
	codec := a.FindEncoder(a.CodecIDAac)
	if codec == nil {
		return nil, nil, errors.New("bundled AAC encoder unavailable")
	}
	encoder := a.AllocCodecContext(codec)
	if encoder == nil {
		return nil, nil, errors.New("allocate AAC encoder")
	}
	encoder.SetSampleRate(audioSampleRate)
	encoder.SetSampleFormat(a.SampleFormatFltp)
	encoder.SetChannelLayout(layout)
	bitrate := int64(defaultAudioBitrate)
	if layout.Channels() == 1 {
		bitrate = 64000
	}
	encoder.SetBitRate(bitrate)
	encoder.SetTimeBase(a.NewRational(1, audioSampleRate))
	encoder.SetFlags(a.NewCodecContextFlags(a.CodecContextFlagGlobalHeader))
	if err := encoder.Open(codec, nil); err != nil {
		encoder.Free()
		return nil, nil, err
	}
	return encoder, codec, nil
}

type audioTranscoder struct {
	encoder, decoder *a.CodecContext
	target           *a.Stream
	out              *mediaOutput
	encoded          *a.Packet
	decoded          *a.Frame
	fifo             *a.AudioFifo
	layout           a.ChannelLayout
	resampler        *a.SoftwareResampleContext
	timestamp        int64
}

func (t *audioTranscoder) drainEncoder() error {
	for {
		err := t.encoder.ReceivePacket(t.encoded)
		if waitingMedia(err) {
			return nil
		}
		if err != nil {
			return err
		}
		t.encoded.RescaleTs(t.encoder.TimeBase(), t.target.TimeBase())
		t.encoded.SetStreamIndex(t.target.Index())
		t.encoded.SetPos(-1)
		if err := t.out.format.WriteInterleavedFrame(t.encoded); err != nil {
			t.encoded.Unref()
			return err
		}
		t.encoded.Unref()
	}
}

func (t *audioTranscoder) encodeReady(ctx context.Context, finish bool) error {
	for t.fifo.Size() >= t.encoder.FrameSize() || (finish && t.fifo.Size() > 0) {
		if err := ctx.Err(); err != nil {
			return err
		}
		count := min(t.fifo.Size(), t.encoder.FrameSize())
		frame := a.AllocFrame()
		if frame == nil {
			return errors.New("allocate AAC frame")
		}
		frame.SetChannelLayout(t.layout)
		frame.SetSampleFormat(a.SampleFormatFltp)
		frame.SetSampleRate(audioSampleRate)
		frame.SetNbSamples(count)
		if err := frame.AllocBuffer(0); err != nil {
			frame.Free()
			return err
		}
		if _, err := t.fifo.Read(frame); err != nil {
			frame.Free()
			return err
		}
		frame.SetPts(t.timestamp)
		t.timestamp += int64(count)
		err := t.encoder.SendFrame(frame)
		frame.Free()
		if err != nil {
			return err
		}
		if err := t.drainEncoder(); err != nil {
			return err
		}
	}
	return nil
}

func (t *audioTranscoder) convert(ctx context.Context, src *a.Frame) (int, error) {
	converted := a.AllocFrame()
	if converted == nil {
		return 0, errors.New("allocate resampled audio")
	}
	defer converted.Free()
	converted.SetChannelLayout(t.layout)
	converted.SetSampleFormat(a.SampleFormatFltp)
	converted.SetSampleRate(audioSampleRate)
	if err := t.resampler.ConvertFrame(src, converted); err != nil {
		return 0, err
	}
	samples := converted.NbSamples()
	if samples > 0 {
		if _, err := t.fifo.Write(converted); err != nil {
			return 0, err
		}
	}
	return samples, t.encodeReady(ctx, false)
}

func (t *audioTranscoder) drainDecoder(ctx context.Context) error {
	for {
		if err := ctx.Err(); err != nil {
			return err
		}
		err := t.decoder.ReceiveFrame(t.decoded)
		if waitingMedia(err) {
			return nil
		}
		if err != nil {
			return err
		}
		_, err = t.convert(ctx, t.decoded)
		t.decoded.Unref()
		if err != nil {
			return err
		}
	}
}

func extendedBoxSize(file io.Reader) (int64, error) {
	var header [8]byte
	if _, err := io.ReadFull(file, header[:]); err != nil {
		return 0, err
	}
	extended := binary.BigEndian.Uint64(header[:])
	if extended > math.MaxInt64 {
		return 0, nil
	}
	return int64(extended), nil
}

func newVideoDecoder(source *a.Stream) (*a.CodecContext, error) {
	codec := a.FindDecoder(source.CodecParameters().CodecID())
	if codec == nil {
		return nil, errors.New("bundled video decoder unavailable")
	}
	decoder := a.AllocCodecContext(codec)
	if decoder == nil {
		return nil, errors.New("allocate video decoder")
	}
	if err := source.CodecParameters().ToCodecContext(decoder); err != nil {
		decoder.Free()
		return nil, err
	}
	decoder.SetThreadCount(videoThreadCount)
	if err := decoder.Open(codec, nil); err != nil {
		decoder.Free()
		return nil, err
	}
	return decoder, nil
}

type videoPreparation struct {
	directory, language, videoPath, preparedAudio      string
	source                                             *a.Stream
	decoder                                            *a.CodecContext
	width, height, ceiling                             int
	rate                                               a.Rational
	fps, duration                                      float64
	heights                                            []int
	audioPlaylist, seekRoot, seekPlaylist, progressive string
	playlists, variants                                []string
	encoders                                           []*videoEncoder
}

func newVideoPreparation(
	source *a.Stream,
	decoder *a.CodecContext,
	inputDuration int64,
	directory, language string,
	maxHeight int,
) (*videoPreparation, error) {
	width, height := decoder.Width(), decoder.Height()
	if width < 2 || height < 2 || width > 16384 || height > 16384 {
		return nil, fmt.Errorf("invalid video dimensions %dx%d", width, height)
	}
	rate, fps, err := sourceVideoRate(source)
	if err != nil {
		return nil, err
	}
	duration := float64(inputDuration) / mediaTimeBase
	if duration <= 0 || duration > 604800 {
		return nil, fmt.Errorf("invalid video duration %g", duration)
	}
	ceiling := min(height, maxHeight) &^ 1
	var heights []int
	for _, h := range []int{720, 1080, 1440, 2160} {
		if h < ceiling {
			heights = append(heights, h)
		}
	}
	if len(heights) == 0 {
		heights = append(
			heights,
			max(minimumVideoDimension, min(height360p, ceiling/minimumVideoDimension))&^1,
		)
	}
	heights = append(heights, ceiling)
	return &videoPreparation{
		source:    source,
		decoder:   decoder,
		directory: directory,
		language:  language,
		videoPath: filepath.Join(
			directory,
			"source-video",
		),
		preparedAudio: filepath.Join(directory, preparedAudioName),
		width:         width,
		height:        height,
		rate:          rate,
		fps:           fps,
		duration:      duration,
		ceiling:       ceiling,
		heights:       heights,
	}, nil
}

func (p *videoPreparation) close() {
	for _, encoder := range p.encoders {
		encoder.close()
	}
}

func (p *videoPreparation) createRenditions(ctx context.Context) error {
	var err error
	p.audioPlaylist, err = packageTrack(
		ctx,
		p.preparedAudio,
		p.directory,
		"audio",
		a.MediaTypeAudio,
	)
	if err != nil {
		return err
	}
	p.playlists = []string{p.audioPlaylist}
	p.progressive = ""
	for index, targetHeight := range p.heights {
		targetWidth := max(minimumVideoDimension, (p.width*targetHeight/p.height)&^1)
		name := fmt.Sprintf("v%d-%d", targetHeight, index)
		if index == len(p.heights)-1 {
			name = "video"
		}
		root := filepath.Join(p.directory, "hls", name)
		if err := os.MkdirAll(root, profileDirectoryMode); err != nil {
			return err
		}
		playlist := filepath.Join(root, "index.m3u8")
		copyable := p.canCopyVideo(index, targetHeight, targetWidth)
		if copyable {
			if _, err := packageTrack(
				ctx,
				p.videoPath,
				p.directory,
				name,
				a.MediaTypeVideo,
			); err != nil {
				return err
			}
		} else {
			bitrate := renditionBitrate(index, targetHeight, p.ceiling)
			v, err := newVideoEncoder(
				p.source,
				p.decoder,
				playlist,
				targetWidth,
				targetHeight,
				p.rate,
				bitrate,
				false,
				p.duration,
			)
			if err != nil {
				return err
			}
			p.encoders = append(p.encoders, v)
		}
		p.variants = append(p.variants, name)
		p.playlists = append(p.playlists, playlist)
		if targetHeight <= height1080p {
			p.progressive = filepath.Join(root, "media.mp4")
		}
	}
	return p.createSeek()
}

func (p *videoPreparation) createSeek() error {
	p.seekRoot = filepath.Join(p.directory, "hls", "seek")
	if err := os.MkdirAll(p.seekRoot, profileDirectoryMode); err != nil {
		return err
	}
	p.seekPlaylist = filepath.Join(p.seekRoot, "index.m3u8")
	seekHeight := max(minimumVideoDimension, min(height180p, p.ceiling)&^1)
	seekWidth := max(minimumVideoDimension, (p.width*seekHeight/p.height)&^1)
	seekRate := a.NewRational(
		int(math.Max(minimumSeekFrames, math.Ceil(p.duration))*millisecondsPerSecond),
		int(p.duration*millisecondsPerSecond),
	)
	seek, err := newVideoEncoder(
		p.source,
		p.decoder,
		p.seekPlaylist,
		seekWidth,
		seekHeight,
		seekRate,
		seekPreviewBitrate,
		true,
		p.duration,
	)
	if err != nil {
		return err
	}
	p.encoders = append(p.encoders, seek)
	return nil
}

func (p *videoPreparation) encode(ctx context.Context, input *a.FormatContext) error {
	packet := a.AllocPacket()
	defer packet.Free()
	frame := a.AllocFrame()
	defer frame.Free()

	for {
		if err := ctx.Err(); err != nil {
			return err
		}
		err := input.ReadFrame(packet)
		if errors.Is(err, a.ErrEof) {
			break
		}
		if err != nil {
			return err
		}
		if packet.StreamIndex() == p.source.Index() {
			err = p.decoder.SendPacket(packet)
			if err == nil {
				err = p.acceptFrames(ctx, frame)
			}
		}
		packet.Unref()
		if err != nil {
			return err
		}
	}
	if err := p.decoder.SendPacket(nil); err != nil {
		return err
	}
	if err := p.acceptFrames(ctx, frame); err != nil {
		return err
	}
	return p.flush(ctx)
}

func (p *videoPreparation) acceptFrames(ctx context.Context, frame *a.Frame) error {
	for {
		if err := ctx.Err(); err != nil {
			return err
		}
		err := p.decoder.ReceiveFrame(frame)
		if waitingMedia(err) {
			return nil
		}
		if err != nil {
			return err
		}
		for _, v := range p.encoders {
			if err := v.accept(ctx, frame); err != nil {
				frame.Unref()
				return err
			}
		}
		frame.Unref()
	}
}

func (p *videoPreparation) flush(ctx context.Context) error {
	for _, v := range p.encoders {
		if v.previous == nil {
			return errors.New("downloaded video was empty")
		}
		for v.next < v.limit {
			if err := ctx.Err(); err != nil {
				return err
			}
			if err := v.sendPrevious(); err != nil {
				return err
			}
		}
		if err := v.encoder.SendFrame(nil); err != nil {
			return err
		}
		if err := v.drain(); err != nil {
			return err
		}
		if err := v.output.format.WriteTrailer(); err != nil {
			return err
		}
	}
	return nil
}

func (p *videoPreparation) finish(ctx context.Context) (int64, error) {
	if err := p.markSeekPlaylist(); err != nil {
		return 0, err
	}
	p.playlists = append(p.playlists, p.seekPlaylist)
	if err := harmonizeTargetDurations(p.playlists); err != nil {
		return 0, err
	}
	if err := p.writeMaster(); err != nil {
		return 0, err
	}
	if p.progressive == "" {
		return 0, errors.New("no progressive video rendition")
	}
	if err := muxMedia(
		ctx,
		p.progressive,
		p.preparedAudio,
		filepath.Join(p.directory, "video.mp4"),
	); err != nil {
		return 0, err
	}
	_, _, audioDuration, err := playlistBandwidth(p.audioPlaylist)
	return int64(math.Ceil(math.Min(audioDuration, p.duration))), err
}

func (p *videoPreparation) markSeekPlaylist() error {
	text, err := os.ReadFile(p.seekPlaylist)
	if err != nil {
		return err
	}
	// #nosec G703 -- These package files are generated inside the owned media directory.
	if err := os.WriteFile(
		p.seekPlaylist,
		[]byte(
			strings.Replace(
				string(text),
				"#EXT-X-VERSION:7",
				"#EXT-X-VERSION:7\n#EXT-X-I-FRAMES-ONLY",
				1,
			),
		),
		privateFileMode,
	); err != nil {
		return err
	}
	return nil
}

func (p *videoPreparation) writeMaster() error {
	audioInput, err := openMedia(p.preparedAudio)
	if err != nil {
		return err
	}
	audioStream, err := mediaStream(audioInput, a.MediaTypeAudio)
	if err != nil {
		closeMedia(audioInput)
		return err
	}
	channels := audioStream.CodecParameters().ChannelLayout().Channels()
	closeMedia(audioInput)
	audioPeak, audioAverage, _, err := playlistBandwidth(p.audioPlaylist)
	if err != nil {
		return err
	}
	var master strings.Builder
	fmt.Fprintf(
		&master,
		"#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-MEDIA:TYPE=AUDIO,"+
			"GROUP-ID=\"audio\",NAME=\"Audio\",LANGUAGE=\"%s\",CHANNELS=\"%d\",DEFAULT=YES,AU"+
			"TOSELECT=YES,URI=\"audio/index.m3u8\"\n",
		p.language,
		channels,
	)
	for _, name := range p.variants {
		if err := appendMasterVariant(
			&master,
			p.directory,
			name,
			audioPeak,
			audioAverage,
		); err != nil {
			return err
		}
	}
	if err := p.appendSeekVariant(&master); err != nil {
		return err
	}
	if err := os.WriteFile(
		filepath.Join(p.directory, "hls", "master.m3u8"),
		[]byte(master.String()),
		privateFileMode,
	); err != nil {
		return err
	}
	return nil
}

func appendMasterVariant(
	master *strings.Builder,
	directory, name string,
	audioPeak, audioAverage int64,
) error {
	path := filepath.Join(directory, "hls", name)
	w, h, fps, codec, err := videoFacts(filepath.Join(path, "media.mp4"))
	if err != nil {
		return err
	}
	peak, average, _, err := playlistBandwidth(filepath.Join(path, "index.m3u8"))
	if err != nil {
		return err
	}
	fmt.Fprintf(
		master,
		"#EXT-X-STREAM-INF:BANDWIDTH=%d,AVERAGE-BANDWIDTH=%d,RESOLUTION=%dx%d,FRAME-RATE="+
			"%.3f,CODECS=\"%s,mp4a.40.2\",AUDIO=\"audio\",CLOSED-CAPTIONS=NONE\n%s/index.m3u8"+
			"\n",
		peak+audioPeak,
		average+audioAverage,
		w,
		h,
		fps,
		codec,
		name,
	)
	return nil
}

func nextMuxPacket(
	ctx context.Context,
	input *a.FormatContext,
	packet *a.Packet,
	source *a.Stream,
	available *bool,
) error {
	for {
		if err := ctx.Err(); err != nil {
			return err
		}
		packet.Unref()
		err := input.ReadFrame(packet)
		if errors.Is(err, a.ErrEof) {
			*available = false
			return nil
		}
		if err != nil {
			return err
		}
		if packet.StreamIndex() == source.Index() {
			*available = true
			return nil
		}
	}
}

type measuredSegment struct {
	bytes    int64
	duration float64
}

func measuredBandwidth(
	segments []measuredSegment,
	target float64,
	totalBytes int64,
	totalDuration float64,
) (int64, int64) {
	average := float64(totalBytes) * bitsPerByte / totalDuration
	peak := 0.0
	for start := range segments {
		bytes := int64(0)
		seconds := 0.0
		for _, segment := range segments[start:] {
			bytes += segment.bytes
			seconds += segment.duration
			if seconds > target*1.5 {
				break
			}
			if seconds >= target*.5 {
				peak = math.Max(peak, float64(bytes)*8/seconds)
			}
		}
	}
	if peak == 0 {
		peak = average
	}
	return int64(math.Ceil(peak)), int64(math.Ceil(average))
}

func writePreparedAudio(
	ctx context.Context,
	input *a.FormatContext,
	source *a.Stream,
	out *mediaOutput,
	outputPath string,
) error {
	mode := "copy"
	if source.CodecParameters().CodecID() != a.CodecIDAac {
		mode = "transcode"
	}
	if Production != "1" {
		fmt.Fprintf(
			os.Stderr,
			"FFmpeg audio mode=%s input_codec=%s output_codec=AAC output=%q\n",
			mode,
			strings.ToUpper(source.CodecParameters().CodecID().String()),
			outputPath,
		)
	}
	var err error
	if mode == "copy" {
		err = remuxAudio(ctx, input, source, out)
	} else {
		err = transcodeAudio(ctx, input, source, out)
	}
	if err != nil {
		return err
	}
	return nil
}

func measuredPlaylistSegment(value string, duration float64) (measuredSegment, error) {
	length, parseErr := strconv.ParseInt(strings.Split(value, "@")[0], 10, 64)
	if parseErr != nil {
		return measuredSegment{}, parseErr
	}
	if duration <= 0 || length <= 0 {
		return measuredSegment{}, errors.New("invalid media range measurement")
	}

	return measuredSegment{length, duration}, nil
}

func sourceVideoRate(source *a.Stream) (a.Rational, float64, error) {
	rate := source.AvgFrameRate()
	if rate.Num() <= 0 || rate.Den() <= 0 {
		rate = source.RFrameRate()
	}
	fps := rate.Float64()
	if math.IsNaN(fps) || fps <= 0 || fps > 1000 {
		return a.Rational{}, 0, fmt.Errorf("invalid source frame rate %g", fps)
	}
	if fps > maximumVideoFrameRate {
		rate = a.NewRational(maximumVideoFrameRate, 1)
	}
	return rate, fps, nil
}

func (p *videoPreparation) canCopyVideo(index, targetHeight, targetWidth int) bool {
	return index > 0 && p.decoder.CodecID() == a.CodecIDH264 &&
		p.decoder.PixelFormat() == a.PixelFormatYuv420P &&
		targetHeight == p.height &&
		targetWidth == p.width &&
		p.fps <= 60 &&
		p.duration <= segmentSeconds
}

type mediaMux struct {
	inputs           []*a.FormatContext
	sources, targets []*a.Stream
	packets          []*a.Packet
	available        []bool
	out              *mediaOutput
}

func (m *mediaMux) prime(ctx context.Context) error {
	for i := range m.inputs {
		if err := nextMuxPacket(
			ctx,
			m.inputs[i],
			m.packets[i],
			m.sources[i],
			&m.available[i],
		); err != nil {
			return err
		}
		if !m.available[i] {
			return errors.New("downloaded media stream was empty")
		}
	}
	return nil
}

func (m *mediaMux) copy(ctx context.Context) error {
	for m.available[0] || m.available[1] {
		i := 0
		if !m.available[0] ||
			(m.available[1] && packetTimestamp(m.packets[1], m.sources[1]) < packetTimestamp(m.packets[0], m.sources[0])) {
			i = 1
		}
		packet := m.packets[i]
		packet.RescaleTs(m.sources[i].TimeBase(), m.targets[i].TimeBase())
		packet.SetStreamIndex(m.targets[i].Index())
		packet.SetPos(-1)
		if err := m.out.format.WriteInterleavedFrame(packet); err != nil {
			return err
		}
		if err := nextMuxPacket(
			ctx,
			m.inputs[i],
			m.packets[i],
			m.sources[i],
			&m.available[i],
		); err != nil {
			return err
		}
	}
	return m.out.format.WriteTrailer()
}

func (p *videoPreparation) appendSeekVariant(master *strings.Builder) error {
	w, h, _, codecName, err := videoFacts(filepath.Join(p.seekRoot, "media.mp4"))
	if err != nil {
		return err
	}
	peak, average, _, err := playlistBandwidth(p.seekPlaylist)
	if err != nil {
		return err
	}
	fmt.Fprintf(
		master,
		"#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=%d,AVERAGE-BANDWIDTH=%d,RESOLUTION=%dx%d,COD"+
			"ECS=\"%s\",URI=\"seek/index.m3u8\"\n",
		peak,
		average,
		w,
		h,
		codecName,
	)
	return nil
}

func (t *audioTranscoder) decodePackets(
	ctx context.Context,
	input *a.FormatContext,
	source *a.Stream,
	packet *a.Packet,
) error {
	for {
		if err := ctx.Err(); err != nil {
			return err
		}
		err := input.ReadFrame(packet)
		if errors.Is(err, a.ErrEof) {
			break
		}
		if err != nil {
			return err
		}
		if packet.StreamIndex() == source.Index() {
			err = t.decoder.SendPacket(packet)
			if err == nil {
				err = t.drainDecoder(ctx)
			}
		}
		packet.Unref()
		if err != nil {
			return err
		}
	}
	return nil
}

func (t *audioTranscoder) finish(ctx context.Context) error {
	if err := t.decoder.SendPacket(nil); err != nil {
		return err
	}
	if err := t.drainDecoder(ctx); err != nil {
		return err
	}
	for {
		count, err := t.convert(ctx, nil)
		if err != nil {
			return err
		}
		if count == 0 {
			break
		}
	}
	if err := t.encodeReady(ctx, true); err != nil {
		return err
	}
	if t.timestamp <= 0 {
		return errors.New("downloaded audio was empty")
	}
	if err := t.encoder.SendFrame(nil); err != nil {
		return err
	}
	if err := t.drainEncoder(); err != nil {
		return err
	}
	return t.out.format.WriteTrailer()
}

func renditionBitrate(index, targetHeight, ceiling int) int64 {
	bitrate := videoBitrate(targetHeight)
	if index == 0 && targetHeight == ceiling {
		bitrate /= 2
	}
	return bitrate
}

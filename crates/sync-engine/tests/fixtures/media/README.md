# Native media fixtures

Two-second synthetic fixtures copied from the parent Listenbox E2E suite:

- `aac.m4a`: `apps/fixtures/e2e/youtube/dash-range/720/manifest-stream1.mp4`
- `opus.webm`: `apps/fixtures/e2e/youtube/audio/opus.webm`
- `avc-aac.mp4`: `apps/fixtures/e2e/youtube/fallback/720.mp4`

The synthetic `avc-aac-long-gop.mp4` and `opus-long.webm` fixtures cover an
8.4-second recording whose eight-second video keyframe interval exceeds the
six-second audio segment interval. The desktop upload regression used this
same mismatch: its HLS video target duration was 11 seconds and audio was six.
Regenerate these fixtures with:

```sh
ffmpeg -f lavfi -i 'testsrc2=size=320x180:rate=25:duration=8.4' \
  -f lavfi -i 'sine=frequency=440:sample_rate=44100:duration=8.4' \
  -c:v libx264 -pix_fmt yuv420p -profile:v high -g 200 -keyint_min 200 \
  -sc_threshold 0 -bf 2 -b:v 160k -c:a aac -b:a 128k -ac 2 \
  -movflags +faststart -shortest avc-aac-long-gop.mp4
ffmpeg -i avc-aac-long-gop.mp4 -vn -c:a libopus -b:a 96k opus-long.webm
```

`hls-long-gop/` is an independently generated, single-file HLS package of the
same recording for the parent Dashboard browser regression. It exercises
real byte-range delivery and decoded frames, with Chrome advertising native
HLS support. Apple validation reports no MUST findings for this package, so
the browser regression isolates player selection and media CORS from the
producer's manifest errors. Regenerate its renditions with:

```sh
mkdir -p hls-long-gop/video hls-long-gop/audio
for kind in video audio; do
  stream=v
  [ "$kind" = audio ] && stream=a
  ffmpeg -i avc-aac-long-gop.mp4 -map "0:$stream" -c copy \
    -hls_time 6 -hls_playlist_type vod -hls_segment_type fmp4 \
    -hls_flags single_file \
    -hls_segment_filename "hls-long-gop/$kind/media.mp4" \
    "hls-long-gop/$kind/index.m3u8"
done
```

Set both rendition target durations to eight seconds and keep the checked-in
master playlist's relative audio/video URLs and `CLOSED-CAPTIONS=NONE`.

The client keeps these bytes locally so its native media tests run independently
on macOS, Linux, Windows x64, and Windows ARM64, without network services or an
installed FFmpeg executable. Tests invoke the production preparation code and
decode its M4A, MP4, and concatenated HLS initialization/media segments through
the linked FFmpeg libraries. Each test owns a temporary output directory.

`moon run client-engine:test-hls` runs the real preparation code on both
combined AAC/video and separate Opus/video, then asks Apple
`mediastreamvalidator` to traverse every byte-range segment. It fails on MUST
findings anywhere in the JSON report, including rendition-level findings, and
on incomplete traversal. Each external validation has an eight-second failure
guard; Nextest retains the hard 30-second test timeout. The tools must be
installed locally on macOS; Linux CI installs the checked-in, checksum-verified
Apple package with `tools/install-apple-hls-tools.sh`.

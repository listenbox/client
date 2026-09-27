# Native media fixtures

Two-second synthetic fixtures copied from the parent Listenbox E2E suite:

- `aac.m4a`: `apps/fixtures/e2e/youtube/dash-range/720/manifest-stream1.mp4`
- `opus.webm`: `apps/fixtures/e2e/youtube/audio/opus.webm`
- `avc-aac.mp4`: `apps/fixtures/e2e/youtube/fallback/720.mp4`

The client keeps these bytes locally so its native media tests run independently
on macOS, Linux, Windows x64, and Windows ARM64, without network services or an
installed FFmpeg executable. Tests invoke the production preparation code and
decode its M4A, MP4, and concatenated HLS initialization/media segments through
the linked FFmpeg libraries. Each test owns a temporary output directory.

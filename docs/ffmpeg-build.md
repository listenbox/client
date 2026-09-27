# FFmpeg build comparison

The existing checksum-pinned FFmpeg 9.0.2 builder remains in `tools/native-ffmpeg.rs`.
The sync engine uses the registry release of `ffmpeg-the-third` with `static`,
`format`, and `software-resampling`; no bindings are vendored or patched.

The upstream `build` feature was tested on Apple Silicon with Rust 1.98.1,
FFmpeg 9.0.2, identical CLI sources, and the existing stripped release profile:

| Measurement | Existing minimal build | Upstream Cargo builder |
| --- | ---: | ---: |
| CLI bytes | 21,930,720 | 34,650,448 (+58%) |
| Decoders / encoders | 4 / 1 | 477 / 166 |
| Demuxers / muxers | 4 / 8 | 359 / 183 |
| Protocols | 1 | 30 |
| Original media scenarios | 7 passed | 7 passed |

The wrapper's features do not provide a native codec allowlist. The upstream
builder enables many additional components and fetches a floating patch tag.
Its source-build path also does not configure native Windows/MSVC; upstream's
Windows tests use prebuilt libraries. There is no demonstrated build-time benefit:
the isolated upstream probe took 126 seconds, but the existing libraries were warm.

Moving preparation into the sync engine's own `build.rs` cannot configure the
previously built `ffmpeg-sys-the-third` dependency. A local vendored replacement
was rejected because its maintenance and source-size cost outweighed the cleanup.
Cargo integration is deferred until the dependency offers a suitable build hook.

This PR removes the unrelated Go-versus-Rust benchmark and adds PR acceptance on
macOS ARM64, Windows x64, and Windows ARM64: real media preparation and decoding,
application packaging, architecture/linkage checks, and Windows startup. Tests run
on cache hits too. Only master pushes publish releases. Existing native preparation
and caches remain; no Cargo migration or Windows success is claimed here.

Sources: [upstream builder](https://github.com/shssoichiro/ffmpeg-the-third/blob/47eb652418f56f8010e640946b105cd5614701f7/ffmpeg-sys-the-third/build/compile.rs),
[upstream Windows CI](https://github.com/shssoichiro/ffmpeg-the-third/blob/47eb652418f56f8010e640946b105cd5614701f7/.github/workflows/build.yml).

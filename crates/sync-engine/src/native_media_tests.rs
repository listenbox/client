//! Exercise the linked native libraries through the same preparation code as sync.
use crate::{audio, media};
use ffmpeg::{codec, format, frame};
use std::{fs, path::Path};
use tokio_util::sync::CancellationToken;

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/media")
        .join(name)
}

#[test]
fn linked_ffmpeg_configuration_is_visible_in_ci() {
    ffmpeg::init().unwrap();
    let configuration = codec::configuration();
    eprintln!("libavcodec version: {}", codec::version());
    eprintln!("FFmpeg configure: {configuration}");
    assert!(configuration.contains("--enable-static"));
    assert!(configuration.contains("--disable-everything"));
    assert!(configuration.contains("--disable-network"));
    assert!(configuration.contains("--disable-debug"));
    assert!(codec::decoder::find(codec::Id::HEVC).is_none());
    assert!(codec::encoder::find(codec::Id::MP3).is_none());
    assert!(!configuration.contains("--enable-gpl"));
    assert!(!configuration.contains("--enable-nonfree"));
}

fn decoded(result: Result<(), ffmpeg::Error>) -> bool {
    match result {
        Ok(()) => true,
        Err(ffmpeg::Error::Eof) => false,
        Err(ffmpeg::Error::Other { errno }) if errno == libc::EAGAIN => false,
        Err(error) => panic!("decoding prepared media failed: {error}"),
    }
}

fn assert_audio(path: &Path) {
    let mut input = format::input(path).unwrap();
    let stream = input.streams().best(ffmpeg::media::Type::Audio).unwrap();
    assert_eq!(stream.parameters().id(), codec::Id::AAC);
    let index = stream.index();
    let mut decoder = codec::Context::from_parameters(stream.parameters())
        .unwrap()
        .decoder()
        .audio()
        .unwrap();
    let mut samples = 0;
    let mut frame = frame::Audio::empty();
    for packet in input.packets() {
        let (stream, packet) = packet.unwrap();
        if stream.index() == index {
            decoder.send_packet(&packet).unwrap();
            while decoded(decoder.receive_frame(&mut frame)) {
                samples += frame.samples();
            }
        }
    }
    decoder.send_eof().unwrap();
    while decoded(decoder.receive_frame(&mut frame)) {
        samples += frame.samples();
    }
    // Two-second fixtures; require decoded audio, not merely readable headers.
    let seconds = samples as f64 / f64::from(decoder.rate());
    assert!(
        (1.8..2.3).contains(&seconds),
        "decoded {seconds}s from {path:?}"
    );
}

fn assert_video(path: &Path) {
    let mut input = format::input(path).unwrap();
    let stream = input.streams().best(ffmpeg::media::Type::Video).unwrap();
    assert_eq!(stream.parameters().id(), codec::Id::H264);
    let index = stream.index();
    let mut decoder = codec::Context::from_parameters(stream.parameters())
        .unwrap()
        .decoder()
        .video()
        .unwrap();
    assert_eq!((decoder.width(), decoder.height()), (1280, 720));
    let mut frames = 0;
    let mut frame = frame::Video::empty();
    for packet in input.packets() {
        let (stream, packet) = packet.unwrap();
        if stream.index() == index {
            decoder.send_packet(&packet).unwrap();
            while decoded(decoder.receive_frame(&mut frame)) {
                frames += 1;
            }
        }
    }
    decoder.send_eof().unwrap();
    while decoded(decoder.receive_frame(&mut frame)) {
        frames += 1;
    }
    assert!((45..=65).contains(&frames), "decoded {frames} video frames");
}

#[test]
fn aac_is_remuxed_to_playable_fast_start_m4a() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("audio.m4a");
    let bytes =
        audio::prepare_m4a(&fixture("aac.m4a"), &output, &CancellationToken::new()).unwrap();
    assert!((20_000..60_000).contains(&bytes));
    assert!(audio::fast_start(&output).unwrap());
    assert_audio(&output);
}

#[test]
fn opus_is_decoded_resampled_and_encoded_to_aac() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("audio.m4a");
    let bytes =
        audio::prepare_m4a(&fixture("opus.webm"), &output, &CancellationToken::new()).unwrap();
    assert!((5_000..60_000).contains(&bytes));
    assert!(audio::fast_start(&output).unwrap());
    assert_audio(&output);
}

fn prepare_video(separate_audio: bool) {
    let directory = tempfile::Builder::new()
        .prefix("native media ü-")
        .tempdir()
        .unwrap();
    let root = directory.path();
    fs::copy(fixture("avc-aac.mp4"), root.join("source-video")).unwrap();
    if separate_audio {
        fs::copy(fixture("opus.webm"), root.join("source-audio")).unwrap();
    }
    let duration = media::prepare(root, separate_audio, &CancellationToken::new()).unwrap();
    assert!((2..=3).contains(&duration));
    assert!(audio::fast_start(&root.join("video.mp4")).unwrap());
    assert_audio(&root.join("video.mp4"));
    assert_video(&root.join("video.mp4"));
    assert!(audio::fast_start(&root.join("audio.m4a")).unwrap());
    assert_audio(&root.join("audio.m4a"));
    let master = fs::read_to_string(root.join("hls/master.m3u8")).unwrap();
    assert!(master.contains("RESOLUTION=1280x720"));
    assert!(master.contains("audio/index.m3u8"));
    assert!(master.contains("video/index.m3u8"));
    for kind in ["audio", "video"] {
        let hls = root.join("hls").join(kind);
        let playlist = fs::read_to_string(hls.join("index.m3u8")).unwrap();
        assert!(playlist.contains("#EXT-X-ENDLIST"));
        assert!(playlist.contains("#EXT-X-MAP:URI=\"media.mp4\",BYTERANGE=\""));
        assert!(playlist.contains("#EXT-X-BYTERANGE:"));
        let payload = hls.join("media.mp4");
        let bytes =
            fs::read(&payload).unwrap_or_else(|error| panic!("reading {payload:?}: {error}"));
        let mut segments = 0;
        for line in playlist
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
        {
            assert!(
                line == "media.mp4",
                "segment must be relative to its playlist: {line:?}"
            );
            segments += 1;
        }
        assert!((1..=2).contains(&segments));
        assert!((5_000..1_500_000).contains(&bytes.len()));
        if kind == "audio" {
            assert_audio(&payload);
        } else {
            assert_video(&payload);
        }
    }
}

#[test]
fn combined_avc_aac_produces_playable_mp4_and_hls() {
    prepare_video(false);
}

#[test]
fn separate_opus_and_avc_produce_playable_mp4_and_hls() {
    prepare_video(true);
}

#[test]
fn invalid_audio_fails_without_leaving_an_output() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("invalid");
    let output = directory.path().join("audio.m4a");
    fs::write(&source, b"not a media container").unwrap();
    assert!(audio::prepare_m4a(&source, &output, &CancellationToken::new()).is_err());
    assert!(!output.exists());
    assert!(source.exists());
}

#[test]
fn cancelled_preparation_does_not_create_output() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("audio.m4a");
    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = audio::prepare_m4a(&fixture("opus.webm"), &output, &cancel).unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert!(!output.exists());
}

async fn validate_long_gop_hls(separate_audio: bool) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::copy(fixture("avc-aac-long-gop.mp4"), root.join("source-video")).unwrap();
    if separate_audio {
        fs::copy(fixture("opus-long.webm"), root.join("source-audio")).unwrap();
    }
    let duration = media::prepare(root, separate_audio, &CancellationToken::new()).unwrap();
    assert!((8..=9).contains(&duration));
    let report_path = root.join("validation.json");
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        tokio::process::Command::new("mediastreamvalidator")
            .kill_on_drop(true)
            .args(["--quiet", "--timeout", "5", "--validation-data-path"])
            .arg(&report_path)
            .arg(root.join("hls/master.m3u8"))
            .output(),
    )
    .await
    .expect("Apple HLS validation must finish within 8 seconds")
    .expect("mediastreamvalidator is required; run client-engine:test-hls with Apple HLS tools installed");
    let report: serde_json::Value = serde_json::from_slice(
        &fs::read(&report_path).expect("validator must produce its structured report"),
    )
    .unwrap();
    let variants = report["variants"].as_array().unwrap();
    let media_variants: Vec<_> = variants.iter().filter(|v| v["url"].is_string()).collect();
    assert_eq!(
        media_variants.len(),
        2,
        "validator must visit audio and video: {report}"
    );
    for variant in media_variants {
        let parsed = variant["parsedSegmentsCount"].as_u64().unwrap();
        let processed = variant["processedSegmentsCount"].as_u64().unwrap();
        assert!(
            parsed > 1,
            "fixture must exercise multiple byte-range segments: {variant}"
        );
        assert_eq!(
            processed, parsed,
            "validator must consume every media segment: {variant}"
        );
        let measured: u64 = variant["discontinuities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["measurements"]["measuredSegments"].as_u64().unwrap())
            .sum();
        assert_eq!(
            measured, processed,
            "validator must analyze media, not only playlists: {variant}"
        );
    }
    fn blocking_findings(value: &serde_json::Value, findings: &mut Vec<serde_json::Value>) {
        match value {
            serde_json::Value::Object(object) => {
                if let Some(level) = object.get("errorRequirementLevel").and_then(|v| v.as_u64())
                    && (level == 1 || level >= 6)
                {
                    findings.push(value.clone());
                }
                for child in object.values() {
                    blocking_findings(child, findings);
                }
            }
            serde_json::Value::Array(array) => {
                for child in array {
                    blocking_findings(child, findings);
                }
            }
            _ => {}
        }
    }
    let mut findings = Vec::new();
    blocking_findings(&report, &mut findings);
    assert!(
        findings.is_empty(),
        "Apple HLS MUST-fix findings: {}",
        serde_json::to_string_pretty(&findings).unwrap()
    );
    assert!(
        output.status.success(),
        "validator failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[ignore = "requires Apple HLS tools; run moon run client-engine:test-hls"]
async fn apple_hls_combined_long_gop_is_playable() {
    validate_long_gop_hls(false).await;
}

#[tokio::test]
#[ignore = "requires Apple HLS tools; run moon run client-engine:test-hls"]
async fn apple_hls_separate_opus_long_gop_is_playable() {
    validate_long_gop_hls(true).await;
}

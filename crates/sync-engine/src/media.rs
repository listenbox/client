use anyhow::{Context, Result, ensure};
use ffmpeg::{Dictionary, Packet, codec, encoder, format, media};
use std::path::Path;
use tokio_util::sync::CancellationToken;

pub fn prepare(directory: &Path, separate_audio: bool, cancel: &CancellationToken) -> Result<i64> {
    ffmpeg::init()?;
    ffmpeg::log::set_level(ffmpeg::log::Level::Error);
    let video = directory.join("source-video");
    let audio = directory.join(if separate_audio {
        "source-audio"
    } else {
        "source-video"
    });
    let prepared_audio = directory.join("audio.m4a");
    crate::audio::prepare_m4a(&audio, &prepared_audio, cancel)?;
    let mp4 = directory.join("video.mp4");
    mux(&video, &prepared_audio, &mp4, cancel)?;
    let mut input = format::input(&mp4)?;
    let stream = input
        .streams()
        .best(media::Type::Video)
        .context("prepared video has no video stream")?;
    let decoder = codec::Context::from_parameters(stream.parameters())?
        .decoder()
        .video()?;
    let (width, height) = (decoder.width(), decoder.height());
    ensure!(width > 0 && height > 0, "invalid video dimensions");
    for (name, kind) in [("video", media::Type::Video), ("audio", media::Type::Audio)] {
        let root = directory.join("hls").join(name);
        std::fs::create_dir_all(&root)?;
        let output = root.join("index.m3u8");
        let mut options = Dictionary::new();
        options.set("hls_time", "6");
        options.set("hls_playlist_type", "vod");
        options.set("hls_segment_type", "fmp4");
        options.set("hls_flags", "single_file");
        options.set("hls_segment_filename", &hls_path(&root.join("media.mp4"))?);
        remux(&mut input, &mp4, &output, kind, options, cancel)?;
    }
    std::fs::write(
        directory.join("hls/master.m3u8"),
        format!(
            "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",NAME=\"Audio\",DEFAULT=YES,AUTOSELECT=YES,URI=\"audio/index.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=12000000,RESOLUTION={width}x{height},AUDIO=\"audio\"\nvideo/index.m3u8\n"
        ),
    )?;
    let duration = playlist_duration(&directory.join("hls/video/index.m3u8"))?
        .min(playlist_duration(&directory.join("hls/audio/index.m3u8"))?);
    Ok(duration.ceil() as i64)
}

fn add_stream(
    input: &format::context::Input,
    output: &mut format::context::Output,
    kind: media::Type,
) -> Result<(usize, usize, ffmpeg::Rational)> {
    let source = input
        .streams()
        .best(kind)
        .context("source missing required media stream")?;
    let mut stream = output.add_stream(encoder::find(codec::Id::None))?;
    stream.set_parameters(source.parameters());
    stream.set_time_base(source.time_base());
    // SAFETY: the uniquely borrowed output stream owns these codec parameters.
    unsafe {
        (*stream.parameters_mut().as_mut_ptr()).codec_tag = 0;
    }
    Ok((source.index(), stream.index(), source.time_base()))
}

fn packets(
    input: &mut format::context::Input,
    output: &mut format::context::Output,
    mapping: (usize, usize, ffmpeg::Rational),
    cancel: &CancellationToken,
) -> Result<()> {
    let (source, target, time_base) = mapping;
    let output_time = output
        .stream(target)
        .context("missing output stream")?
        .time_base();
    let mut count = 0;
    loop {
        ensure!(!cancel.is_cancelled(), "media preparation interrupted");
        let mut packet = Packet::empty();
        match packet.read(input) {
            Ok(()) => {}
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(error.into()),
        }
        if packet.stream() == source {
            packet.rescale_ts(time_base, output_time);
            packet.set_stream(target);
            packet.set_position(-1);
            packet.write_interleaved(output)?;
            count += 1;
        }
    }
    ensure!(count > 0, "downloaded media stream was empty");
    Ok(())
}

fn mux(video: &Path, audio: &Path, destination: &Path, cancel: &CancellationToken) -> Result<()> {
    let mut video = format::input(video)?;
    let mut audio = format::input(audio)?;
    let mut output = format::output_as(destination, "mp4")?;
    let video_mapping = add_stream(&video, &mut output, media::Type::Video)?;
    let audio_mapping = add_stream(&audio, &mut output, media::Type::Audio)?;
    let mut options = Dictionary::new();
    options.set("movflags", "+faststart");
    output.write_header_with(options)?;
    // Keep only one packet per input while muxing in decoding timestamp order.
    // Feeding a whole input first can make the interleaver buffer long recordings.
    let mut video_packet = next_packet(&mut video, video_mapping.0, cancel)?;
    let mut audio_packet = next_packet(&mut audio, audio_mapping.0, cancel)?;
    ensure!(
        video_packet.is_some() && audio_packet.is_some(),
        "downloaded media stream was empty"
    );
    while video_packet.is_some() || audio_packet.is_some() {
        let take_video = match (&video_packet, &audio_packet) {
            (Some(video), Some(audio)) => {
                timestamp(video, video_mapping.2) <= timestamp(audio, audio_mapping.2)
            }
            (Some(_), None) => true,
            _ => false,
        };
        let (slot, input, mapping) = if take_video {
            (&mut video_packet, &mut video, video_mapping)
        } else {
            (&mut audio_packet, &mut audio, audio_mapping)
        };
        let mut packet = slot.take().context("missing mux packet")?;
        let output_time = output
            .stream(mapping.1)
            .context("missing output stream")?
            .time_base();
        packet.rescale_ts(mapping.2, output_time);
        packet.set_stream(mapping.1);
        packet.set_position(-1);
        packet.write_interleaved(&mut output)?;
        *slot = next_packet(input, mapping.0, cancel)?;
    }
    output.write_trailer()?;
    Ok(())
}

fn timestamp(packet: &Packet, time: ffmpeg::Rational) -> i64 {
    use ffmpeg::Rescale;
    packet
        .dts()
        .or_else(|| packet.pts())
        .unwrap_or(0)
        .rescale(time, (1, 1_000_000))
}

fn next_packet(
    input: &mut format::context::Input,
    stream: usize,
    cancel: &CancellationToken,
) -> Result<Option<Packet>> {
    loop {
        ensure!(!cancel.is_cancelled(), "media preparation interrupted");
        let mut packet = Packet::empty();
        match packet.read(input) {
            Ok(()) if packet.stream() == stream => return Ok(Some(packet)),
            Ok(()) => {}
            Err(ffmpeg::Error::Eof) => return Ok(None),
            Err(error) => return Err(error.into()),
        }
    }
}

fn hls_path(path: &Path) -> Result<String> {
    // FFmpeg's HLS muxer resolves payload paths using '/' even on Windows.
    Ok(path
        .to_str()
        .context("non-UTF-8 media path")?
        .replace(std::path::MAIN_SEPARATOR, "/"))
}

fn remux(
    input: &mut format::context::Input,
    source: &Path,
    destination: &Path,
    kind: media::Type,
    options: Dictionary,
    cancel: &CancellationToken,
) -> Result<()> {
    *input = format::input(source)?;
    let mut output = format::output_as(hls_path(destination)?, "hls")?;
    let mapping = add_stream(input, &mut output, kind)?;
    output.write_header_with(options)?;
    packets(input, &mut output, mapping, cancel)?;
    output.write_trailer()?;
    Ok(())
}

fn playlist_duration(path: &Path) -> Result<f64> {
    let text = std::fs::read_to_string(path)?;
    let mut total = 0.0;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("#EXTINF:") {
            let seconds: f64 = value
                .split(',')
                .next()
                .context("missing HLS duration")?
                .parse()?;
            ensure!(
                seconds.is_finite() && seconds > 0.0,
                "invalid HLS segment duration"
            );
            total += seconds;
        }
    }
    ensure!(
        total.is_finite() && total > 0.0,
        "playlist has no valid segments"
    );
    Ok(total)
}

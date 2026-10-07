use anyhow::{Context, Result, ensure};
use ffmpeg::{Dictionary, Packet, codec, encoder, format, media};
use std::path::Path;
use tokio_util::sync::CancellationToken;

pub fn prepare(
    directory: &Path,
    separate_audio: bool,
    max_height: u32,
    language: &str,
    cancel: &CancellationToken,
) -> Result<i64> {
    ffmpeg::init()?;
    ffmpeg::log::set_level(ffmpeg::log::Level::Error);
    ensure!(
        (2..=2160).contains(&max_height),
        "invalid video delivery ceiling: {max_height}"
    );
    let video = directory.join("source-video");
    let audio = directory.join(if separate_audio {
        "source-audio"
    } else {
        "source-video"
    });
    let prepared_audio = directory.join("audio.m4a");
    crate::audio::prepare_m4a(&audio, &prepared_audio, cancel)?;
    let mut input = format::input(&video)?;
    let stream = input
        .streams()
        .best(media::Type::Video)
        .context("source has no video stream")?;
    let decoder = codec::Context::from_parameters(stream.parameters())?
        .decoder()
        .video()?;
    let (width, height) = (decoder.width(), decoder.height());
    ensure!(
        (2..=16384).contains(&width) && (2..=16384).contains(&height),
        "invalid video dimensions: {width}x{height}"
    );
    let source_rate = stream.avg_frame_rate();
    let source_rate = if source_rate.0 > 0 && source_rate.1 > 0 {
        source_rate
    } else {
        stream.rate()
    };
    let fps = f64::from(source_rate);
    ensure!(
        fps.is_finite() && fps > 0.0 && fps <= 1000.0,
        "invalid source frame rate: {fps}"
    );
    let frame_rate = if fps > 60.0 {
        ffmpeg::Rational(60, 1)
    } else {
        source_rate
    };
    let duration = input.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);
    ensure!(
        duration.is_finite() && (0.0..=604800.0).contains(&duration) && duration > 0.0,
        "invalid video duration: {duration}"
    );
    let ceiling = height.min(max_height) & !1;
    let mut heights: Vec<u32> = [720, 1080, 1440, 2160]
        .into_iter()
        .filter(|h| *h < ceiling)
        .collect();
    if heights.is_empty() {
        heights.push((ceiling / 2).clamp(2, 360) & !1);
    }
    heights.push(ceiling);
    let mut playlists = Vec::new();
    let mut variants = Vec::new();
    let mut encodes = Vec::new();
    let audio_playlist = package_track(
        directory,
        "audio",
        &prepared_audio,
        media::Type::Audio,
        &mut input,
        cancel,
    )?;
    playlists.push(audio_playlist.clone());
    let mut progressive = None;
    for (index, &target_height) in heights.iter().enumerate() {
        let target_width =
            ((u64::from(width) * u64::from(target_height) / u64::from(height)) as u32 & !1).max(2);
        ensure!(
            target_width <= width && target_height <= height,
            "rendition would upscale source"
        );
        let name = if index == heights.len() - 1 {
            "video".to_owned()
        } else {
            format!("v{target_height}-{index}")
        };
        let root = directory.join("hls").join(&name);
        std::fs::create_dir_all(&root)?;
        let playlist = root.join("index.m3u8");
        let copy = index > 0
            && decoder.id() == codec::Id::H264
            && decoder.format() == format::Pixel::YUV420P
            && target_height == height
            && target_width == width
            && fps <= 60.0
            && duration <= 6.0;
        if copy {
            remux(
                &mut input,
                &video,
                &playlist,
                media::Type::Video,
                hls_options(&root)?,
                cancel,
            )?;
        } else {
            let bitrate = video_bitrate(target_height)
                / if index == 0 && target_height == ceiling {
                    2
                } else {
                    1
                };
            encodes.push(VideoOutput {
                destination: playlist.clone(),
                geometry: (target_width, target_height),
                rate: frame_rate,
                bitrate,
                iframe: false,
            });
        }
        variants.push(name);
        playlists.push(playlist);
        if target_height <= 1080 {
            progressive = Some(root.join("media.mp4"));
        }
    }
    let seek_root = directory.join("hls/seek");
    std::fs::create_dir_all(&seek_root)?;
    let seek_height = (ceiling.min(180) & !1).max(2);
    let seek_width =
        ((u64::from(width) * u64::from(seek_height) / u64::from(height)) as u32 & !1).max(2);
    let seek_playlist = seek_root.join("index.m3u8");
    let seek_rate = ffmpeg::Rational::from(duration.ceil().max(2.0) / duration);
    encodes.push(VideoOutput {
        destination: seek_playlist.clone(),
        geometry: (seek_width, seek_height),
        rate: seek_rate,
        bitrate: 12000,
        iframe: true,
    });
    encode_videos(&video, &encodes, duration, cancel)?;
    // All samples in this separately encoded payload are IDRs. Reusing a normal
    // single-file mux avoids FFmpeg's invalid single-file I-frame byte ranges.
    let text = std::fs::read_to_string(&seek_playlist)?
        .replace("#EXT-X-VERSION:7", "#EXT-X-VERSION:7\n#EXT-X-I-FRAMES-ONLY");
    std::fs::write(&seek_playlist, text)?;
    playlists.push(seek_playlist.clone());
    harmonize_target_durations(&playlists)?;
    let (audio_peak, audio_average) = playlist_bandwidth(&audio_playlist)?;
    let audio_input = format::input(&prepared_audio)?;
    let audio_stream = audio_input
        .streams()
        .best(media::Type::Audio)
        .context("missing prepared audio")?;
    let audio_decoder = codec::Context::from_parameters(audio_stream.parameters())?
        .decoder()
        .audio()?;
    let channels = audio_decoder.ch_layout().channels();
    ensure!(
        language
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !language.is_empty(),
        "invalid audio language: {language}"
    );
    let mut master = format!(
        "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",NAME=\"Audio\",LANGUAGE=\"{language}\",CHANNELS=\"{channels}\",DEFAULT=YES,AUTOSELECT=YES,URI=\"audio/index.m3u8\"\n"
    );
    for name in variants {
        let facts = video_facts(&directory.join("hls").join(&name).join("media.mp4"))?;
        let (peak, average) =
            playlist_bandwidth(&directory.join("hls").join(&name).join("index.m3u8"))?;
        master.push_str(&format!("#EXT-X-STREAM-INF:BANDWIDTH={},AVERAGE-BANDWIDTH={},RESOLUTION={}x{},FRAME-RATE={:.3},CODECS=\"{},mp4a.40.2\",AUDIO=\"audio\",CLOSED-CAPTIONS=NONE\n{name}/index.m3u8\n", peak + audio_peak, average + audio_average, facts.0, facts.1, facts.2, facts.3));
    }
    let (peak, average) = playlist_bandwidth(&seek_playlist)?;
    let seek = video_facts(&seek_root.join("media.mp4"))?;
    master.push_str(&format!("#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH={peak},AVERAGE-BANDWIDTH={average},RESOLUTION={}x{},CODECS=\"{}\",URI=\"seek/index.m3u8\"\n",seek.0,seek.1,seek.3));
    std::fs::write(directory.join("hls/master.m3u8"), master)?;
    mux(
        &progressive.context("no progressive video rendition")?,
        &prepared_audio,
        &directory.join("video.mp4"),
        cancel,
    )?;
    Ok(playlist_duration(&audio_playlist)?.min(duration).ceil() as i64)
}

fn video_bitrate(height: u32) -> usize {
    match height {
        0..=180 => 96000,
        181..=360 => 365000,
        361..=720 => 3000000,
        721..=1080 => 6000000,
        1081..=1440 => 9000000,
        _ => 16000000,
    }
}

fn hls_options(root: &Path) -> Result<Dictionary> {
    let mut options = Dictionary::new();
    options.set("hls_time", "6");
    options.set("hls_playlist_type", "vod");
    options.set("hls_segment_type", "fmp4");
    options.set("hls_flags", "single_file");
    options.set("hls_segment_filename", &hls_path(&root.join("media.mp4"))?);
    Ok(options)
}

fn package_track(
    directory: &Path,
    name: &str,
    source: &Path,
    kind: media::Type,
    input: &mut format::context::Input,
    cancel: &CancellationToken,
) -> Result<std::path::PathBuf> {
    let root = directory.join("hls").join(name);
    std::fs::create_dir_all(&root)?;
    let playlist = root.join("index.m3u8");
    remux(input, source, &playlist, kind, hls_options(&root)?, cancel)?;
    Ok(playlist)
}

fn harmonize_target_durations(playlists: &[std::path::PathBuf]) -> Result<()> {
    let mut target = 6;
    for path in playlists {
        let text = std::fs::read_to_string(path)?;
        let value: u64 = text
            .lines()
            .find_map(|l| l.strip_prefix("#EXT-X-TARGETDURATION:"))
            .context("missing target duration")?
            .parse()?;
        target = target.max(value);
    }
    for path in playlists {
        let text = std::fs::read_to_string(path)?;
        let text: String = text
            .lines()
            .map(|l| {
                if l.starts_with("#EXT-X-TARGETDURATION:") {
                    format!("#EXT-X-TARGETDURATION:{target}\n")
                } else {
                    format!("{l}\n")
                }
            })
            .collect();
        std::fs::write(path, text)?;
    }
    Ok(())
}

fn playlist_bandwidth(path: &Path) -> Result<(u64, u64)> {
    let text = std::fs::read_to_string(path)?;
    let target = text
        .lines()
        .find_map(|line| line.strip_prefix("#EXT-X-TARGETDURATION:"))
        .context("missing target duration")?
        .parse::<f64>()?;
    ensure!(
        target.is_finite() && target > 0.0,
        "invalid target duration"
    );
    let mut duration = 0.0;
    let mut total_duration = 0.0;
    let mut total_bytes = 0_u64;
    let mut segments = Vec::new();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("#EXTINF:") {
            duration = value
                .split(',')
                .next()
                .context("missing duration")?
                .parse::<f64>()?;
        } else if let Some(value) = line.strip_prefix("#EXT-X-BYTERANGE:") {
            let length = value
                .split('@')
                .next()
                .context("missing byte range")?
                .parse::<u64>()?;
            ensure!(
                duration.is_finite() && duration > 0.0 && length > 0,
                "invalid media range measurement"
            );
            segments.push((length, duration));
            total_bytes += length;
            total_duration += duration;
            duration = 0.0;
        }
    }
    ensure!(
        total_bytes > 0 && total_duration > 0.0,
        "empty HLS measurement"
    );
    let average = total_bytes as f64 * 8.0 / total_duration;
    let mut peak: f64 = 0.0;
    for start in 0..segments.len() {
        let mut bytes = 0;
        let mut seconds = 0.0;
        for &(length, duration) in &segments[start..] {
            bytes += length;
            seconds += duration;
            if seconds > target * 1.5 {
                break;
            }
            if seconds >= target * 0.5 {
                peak = peak.max(bytes as f64 * 8.0 / seconds);
            }
        }
    }
    if peak == 0.0 {
        peak = average;
    }
    Ok((peak.ceil() as u64, average.ceil() as u64))
}

fn video_facts(path: &Path) -> Result<(u32, u32, f64, String)> {
    let input = format::input(path)?;
    let stream = input
        .streams()
        .best(media::Type::Video)
        .context("missing generated video")?;
    let params = stream.parameters();
    let decoder = codec::Context::from_parameters(stream.parameters())?
        .decoder()
        .video()?;
    // SAFETY: parameters owns this immutable FFmpeg extradata for this scope.
    let data = unsafe {
        let p = params.as_ptr();
        ensure!(
            (*p).extradata_size >= 4 && !(*p).extradata.is_null(),
            "generated AVC has no configuration"
        );
        std::slice::from_raw_parts((*p).extradata, (*p).extradata_size as usize)
    };
    ensure!(
        data[0] == 1,
        "generated video has no AVC configuration record"
    );
    Ok((
        decoder.width(),
        decoder.height(),
        f64::from(stream.avg_frame_rate()),
        format!("avc1.{:02X}{:02X}{:02X}", data[1], data[2], data[3]),
    ))
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

fn waiting(error: ffmpeg::Error) -> bool {
    matches!(error, ffmpeg::Error::Eof)
        || matches!(error, ffmpeg::Error::Other { errno } if errno == libc::EAGAIN)
}

struct VideoEncoder {
    encoder: codec::encoder::Video,
    scaler: ffmpeg::software::scaling::Context,
    output: format::context::Output,
    input_time: ffmpeg::Rational,
    output_time: ffmpeg::Rational,
    rate: ffmpeg::Rational,
    first_timestamp: Option<i64>,
    previous: Option<ffmpeg::frame::Video>,
    next_frame: i64,
    frame_limit: i64,
    gop: i64,
}

impl VideoEncoder {
    fn drain_encoder(&mut self) -> Result<()> {
        loop {
            let mut packet = Packet::empty();
            match self.encoder.receive_packet(&mut packet) {
                Ok(()) => (),
                Err(error) if waiting(error) => break,
                Err(error) => return Err(error.into()),
            }
            packet.set_duration(1);
            packet.rescale_ts(self.rate.invert(), self.output_time);
            packet.set_stream(0);
            packet.set_position(-1);
            packet.write_interleaved(&mut self.output)?;
        }
        Ok(())
    }

    fn send_previous(&mut self) -> Result<()> {
        let mut frame = self
            .previous
            .as_ref()
            .context("missing decoded video")?
            .clone();
        frame.set_pts(Some(self.next_frame));
        // Force each six-second presentation boundary, including fractional
        // rates: a rounded GOP could land just before 6s and produce a 12s
        // first HLS segment. All rungs use this same absolute time grid.
        let boundary = i64::from(self.rate.0) * 6;
        let ticks = i64::from(self.rate.1);
        let keyframe = self.gop == 1
            || self.next_frame == 0
            || self.next_frame * ticks / boundary > (self.next_frame - 1) * ticks / boundary;
        frame.set_kind(if keyframe {
            ffmpeg::picture::Type::I
        } else {
            ffmpeg::picture::Type::None
        });
        self.encoder.send_frame(&frame)?;
        self.next_frame += 1;
        self.drain_encoder()
    }

    fn accept_frame(
        &mut self,
        frame: &ffmpeg::frame::Video,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let timestamp = frame
            .timestamp()
            .or_else(|| frame.pts())
            .context("source video has no frame timestamp")?;
        let first = *self.first_timestamp.get_or_insert(timestamp);
        let index = ((timestamp - first) as f64 * f64::from(self.input_time) * f64::from(self.rate)
            + 1e-7)
            .floor() as i64;
        if index < self.next_frame || index >= self.frame_limit {
            return Ok(());
        }
        while self.next_frame < index && self.previous.is_some() {
            ensure!(!cancel.is_cancelled(), "video encoding interrupted");
            self.send_previous()?;
        }
        let mut scaled = ffmpeg::frame::Video::empty();
        self.scaler.run(frame, &mut scaled)?;
        self.previous = Some(scaled);
        self.send_previous()
    }
}

struct VideoOutput {
    destination: std::path::PathBuf,
    geometry: (u32, u32),
    rate: ffmpeg::Rational,
    bitrate: usize,
    iframe: bool,
}

// One source decoder fans out to bounded, independently owned encoders. The
// joined preparation worker owns all frames, outputs and cancellation; no extra
// retry or cleanup owner is introduced. Each encoder retains only its last frame.
fn new_video_encoder(
    request: &VideoOutput,
    input_format: format::Pixel,
    input_geometry: (u32, u32),
    input_time: ffmpeg::Rational,
    duration: f64,
) -> Result<VideoEncoder> {
    let VideoOutput {
        geometry,
        rate,
        bitrate,
        iframe,
        destination,
    } = request;
    let (geometry, rate, bitrate, iframe) = (*geometry, *rate, *bitrate, *iframe);
    let codec =
        encoder::find_by_name("libopenh264").context("bundled OpenH264 encoder unavailable")?;
    let mut encoder = codec::Context::new_with_codec(codec).encoder().video()?;
    encoder.set_width(geometry.0);
    encoder.set_height(geometry.1);
    encoder.set_format(format::Pixel::YUV420P);
    encoder.set_time_base(rate.invert());
    encoder.set_frame_rate(Some(rate));
    encoder.set_bit_rate(bitrate);
    encoder.set_max_bit_rate(bitrate * 12 / 10);
    encoder.set_max_b_frames(0);
    encoder.set_threading(codec::threading::Config {
        count: 2,
        ..Default::default()
    });
    encoder.set_flags(codec::Flags::GLOBAL_HEADER);
    let gop = if iframe {
        1
    } else {
        (f64::from(rate) * 6.0).ceil().max(1.0) as u32
    };
    encoder.set_gop(gop);
    let mut options = Dictionary::new();
    options.set("allow_skip_frames", "0");
    options.set("rc_mode", "bitrate");
    let encoder = encoder.open_as_with(codec, options)?;
    let scaler = ffmpeg::software::scaling::Context::get(
        input_format,
        input_geometry.0,
        input_geometry.1,
        format::Pixel::YUV420P,
        geometry.0,
        geometry.1,
        ffmpeg::software::scaling::Flags::BILINEAR,
    )?;
    let mut output = format::output_as(hls_path(destination)?, "hls")?;
    {
        let mut stream = output.add_stream(codec)?;
        stream.set_parameters(codec::Parameters::from(&encoder));
        stream.set_time_base(rate.invert());
        stream.set_rate(rate);
    }
    output.write_header_with(hls_options(
        destination.parent().context("missing HLS directory")?,
    )?)?;
    let output_time = output
        .stream(0)
        .context("missing output stream")?
        .time_base();
    Ok(VideoEncoder {
        encoder,
        scaler,
        output,
        input_time,
        output_time,
        rate,
        first_timestamp: None,
        previous: None,
        next_frame: 0,
        frame_limit: (duration * f64::from(rate)).ceil().max(1.0) as i64,
        gop: i64::from(gop),
    })
}

fn encode_videos(
    source: &Path,
    requests: &[VideoOutput],
    duration: f64,
    cancel: &CancellationToken,
) -> Result<()> {
    let mut input = format::input(source)?;
    let stream = input
        .streams()
        .best(media::Type::Video)
        .context("missing source video")?;
    let input_index = stream.index();
    let input_time = stream.time_base();
    let mut context = codec::Context::from_parameters(stream.parameters())?;
    context.set_threading(codec::threading::Config {
        count: 2,
        ..Default::default()
    });
    let mut decoder = context.decoder().video()?;
    let source_geometry = (decoder.width(), decoder.height());
    let largest = requests
        .iter()
        .max_by_key(|request| u64::from(request.geometry.0) * u64::from(request.geometry.1))
        .context("video has no encoding outputs")?
        .geometry;
    // Reduce a source above the encoded ceiling once. Lower outputs then read
    // this bounded YUV frame, instead of repeatedly scaling the full 4K source.
    // The same joined worker owns the scaler and retains one shared frame only.
    let mut shared_scaler = if largest != source_geometry {
        Some(ffmpeg::software::scaling::Context::get(
            decoder.format(),
            source_geometry.0,
            source_geometry.1,
            format::Pixel::YUV420P,
            largest.0,
            largest.1,
            ffmpeg::software::scaling::Flags::BILINEAR,
        )?)
    } else {
        None
    };
    let (input_format, input_geometry) = if shared_scaler.is_some() {
        (format::Pixel::YUV420P, largest)
    } else {
        (decoder.format(), source_geometry)
    };
    let mut outputs = requests
        .iter()
        .map(|request| {
            new_video_encoder(request, input_format, input_geometry, input_time, duration)
        })
        .collect::<Result<Vec<_>>>()?;
    loop {
        ensure!(!cancel.is_cancelled(), "video encoding interrupted");
        let mut packet = Packet::empty();
        match packet.read(&mut input) {
            Ok(()) => (),
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(error.into()),
        }
        if packet.stream() == input_index {
            decoder.send_packet(&packet)?;
            drain_video_decoder(&mut decoder, shared_scaler.as_mut(), &mut outputs, cancel)?;
        }
    }
    decoder.send_eof()?;
    drain_video_decoder(&mut decoder, shared_scaler.as_mut(), &mut outputs, cancel)?;
    for state in &mut outputs {
        ensure!(state.previous.is_some(), "source video decoded no frames");
        while state.next_frame < state.frame_limit {
            ensure!(!cancel.is_cancelled(), "video encoding interrupted");
            state.send_previous()?;
        }
        state.encoder.send_eof()?;
        state.drain_encoder()?;
        state.output.write_trailer()?;
    }
    Ok(())
}

fn drain_video_decoder(
    decoder: &mut codec::decoder::Video,
    mut shared_scaler: Option<&mut ffmpeg::software::scaling::Context>,
    outputs: &mut [VideoEncoder],
    cancel: &CancellationToken,
) -> Result<()> {
    loop {
        ensure!(!cancel.is_cancelled(), "video encoding interrupted");
        let mut frame = ffmpeg::frame::Video::empty();
        match decoder.receive_frame(&mut frame) {
            Ok(()) => (),
            Err(error) if waiting(error) => break,
            Err(error) => return Err(error.into()),
        }
        let mut shared = ffmpeg::frame::Video::empty();
        let frame = if let Some(scaler) = shared_scaler.as_mut() {
            scaler.run(&frame, &mut shared)?;
            shared.set_pts(frame.timestamp().or_else(|| frame.pts()));
            &shared
        } else {
            &frame
        };
        for output in outputs.iter_mut() {
            output.accept_frame(frame, cancel)?;
        }
    }
    Ok(())
}

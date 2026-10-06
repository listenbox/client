//! FFmpeg objects stay on one worker thread and are owned by ffmpeg-the-third's Rust wrappers.
use anyhow::{Context, Result, ensure};
use ffmpeg::{
    ChannelLayout, ChannelLayoutMask, Dictionary, Packet, codec, format, frame, media,
    software::resampling,
};
use std::{
    collections::VecDeque,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};
use tokio_util::sync::CancellationToken;

pub fn prepare_m4a(input: &Path, output: &Path, cancelled: &CancellationToken) -> Result<u64> {
    ensure!(!cancelled.is_cancelled(), "Audio conversion cancelled");
    ensure!(
        input != output && !output.exists(),
        "Audio output must be a new file"
    );
    ffmpeg::init()?;
    ffmpeg::log::set_level(ffmpeg::log::Level::Error);
    let result = convert(input, output, cancelled).and_then(|()| {
        ensure!(fast_start(output)?, "M4A fast-start validation failed");
        Ok(output.metadata()?.len())
    });
    if result.is_err() {
        let _ = std::fs::remove_file(output);
    }
    result
}
fn convert(input: &Path, output: &Path, cancelled: &CancellationToken) -> Result<()> {
    let mut input = format::input(input)?;
    let source = input
        .streams()
        .best(media::Type::Audio)
        .context("No audio stream")?;
    let index = source.index();
    let time_base = source.time_base();
    let parameters = source.parameters();
    let mode = if parameters.id() == codec::Id::AAC {
        "copy"
    } else {
        "transcode"
    };
    if cfg!(debug_assertions) {
        eprintln!(
            "FFmpeg audio mode={mode} input_codec={:?} output_codec=AAC output={output:?}",
            parameters.id()
        );
    }
    let mut output = format::output_as(output, "ipod")?;
    if parameters.id() != codec::Id::AAC {
        let decoder = codec::Context::from_parameters(parameters)?
            .decoder()
            .audio()?;
        return transcode(input, output, index, decoder, cancelled);
    }

    {
        let mut stream = output.add_stream(ffmpeg::encoder::find(codec::Id::None))?;
        stream.set_parameters(parameters);
        stream.set_time_base(time_base);
        // Reset the source-container codec tag when writing an M4A container.
        // SAFETY: the uniquely borrowed output stream owns these codec parameters.
        unsafe {
            (*stream.parameters_mut().as_mut_ptr()).codec_tag = 0;
        }
    }
    header(&mut output)?;
    let destination_time = output.stream(0).context("No output stream")?.time_base();
    let mut count = 0;
    loop {
        ensure!(!cancelled.is_cancelled(), "Audio conversion cancelled");
        let mut packet = Packet::empty();
        match packet.read(&mut input) {
            Ok(()) => (),
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(error.into()),
        }
        if packet.stream() == index {
            packet.rescale_ts(time_base, destination_time);
            packet.set_stream(0);
            packet.set_position(-1);
            packet.write_interleaved(&mut output)?;
            count += 1;
        }
    }
    ensure!(count > 0, "Downloaded audio was empty");
    output.write_trailer()?;
    Ok(())
}
fn header(output: &mut format::context::Output) -> Result<()> {
    let mut options = Dictionary::new();
    options.set("movflags", "+faststart");
    output.write_header_with(options)?;
    Ok(())
}
fn waiting(error: ffmpeg::Error) -> bool {
    // FFmpeg returns C errno values, not the Win32 codes std::io::Error expects on Windows.
    matches!(error, ffmpeg::Error::Eof)
        || matches!(error, ffmpeg::Error::Other { errno } if errno == libc::EAGAIN)
}

struct Transcoder {
    decoder: codec::decoder::Audio,
    encoder: codec::encoder::Audio,
    resampler: resampling::Context,
    fifo: Vec<VecDeque<f32>>,
    output: format::context::Output,
    layout: ChannelLayoutMask,
    timestamp: i64,
    output_time: ffmpeg::Rational,
}
impl Transcoder {
    fn drain_encoder(&mut self) -> Result<()> {
        loop {
            let mut packet = Packet::empty();
            match self.encoder.receive_packet(&mut packet) {
                Ok(()) => (),
                Err(error) if waiting(error) => break,
                Err(error) => return Err(error.into()),
            }
            packet.set_stream(0);
            packet.rescale_ts((1, 48000), self.output_time);
            packet.write_interleaved(&mut self.output)?;
        }
        Ok(())
    }
    fn encode_ready(&mut self, finish: bool) -> Result<()> {
        let frame_size = self.encoder.frame_size() as usize;
        while self.fifo[0].len() >= frame_size || (finish && !self.fifo[0].is_empty()) {
            let count = frame_size.min(self.fifo[0].len());
            let mut frame = frame::Audio::new(self.encoder.format(), count, self.layout);
            frame.set_rate(48000);
            for (channel, samples) in self.fifo.iter_mut().enumerate() {
                for (target, value) in frame
                    .plane_mut::<f32>(channel)
                    .iter_mut()
                    .zip(samples.drain(..count))
                {
                    *target = value;
                }
            }
            frame.set_pts(Some(self.timestamp));
            self.timestamp += count as i64;
            self.encoder.send_frame(&frame)?;
            self.drain_encoder()?;
        }
        Ok(())
    }
    fn collect(&mut self, converted: &frame::Audio) -> Result<()> {
        for (channel, fifo) in self.fifo.iter_mut().enumerate() {
            fifo.extend(converted.plane::<f32>(channel));
        }
        self.encode_ready(false)
    }
    fn drain_decoder(&mut self) -> Result<()> {
        loop {
            let mut decoded = frame::Audio::empty();
            match self.decoder.receive_frame(&mut decoded) {
                Ok(()) => (),
                Err(error) if waiting(error) => break,
                Err(error) => return Err(error.into()),
            }
            let delay = self
                .resampler
                .delay()
                .map_or(0, |delay| delay.output.max(0) as usize);
            let capacity = (decoded.samples() as u64 * 48000)
                .div_ceil(u64::from(self.decoder.rate())) as usize
                + delay
                + 32;
            let mut converted = frame::Audio::new(self.encoder.format(), capacity, self.layout);
            self.resampler.run(&decoded, &mut converted)?;
            self.collect(&converted)?;
        }
        Ok(())
    }
}
fn transcode(
    mut input: format::context::Input,
    mut output: format::context::Output,
    index: usize,
    decoder: codec::decoder::Audio,
    cancelled: &CancellationToken,
) -> Result<()> {
    ensure!(
        decoder.rate() > 0 && decoder.ch_layout().channels() > 0,
        "Invalid input audio format"
    );
    let layout = if decoder.ch_layout().channels() == 1 {
        ChannelLayoutMask::MONO
    } else {
        ChannelLayoutMask::STEREO
    };
    let codec = ffmpeg::encoder::find(codec::Id::AAC).context("Bundled AAC encoder unavailable")?;
    let mut encoder = codec::Context::new_with_codec(codec).encoder().audio()?;
    encoder.set_rate(48000);
    encoder.set_format(format::Sample::F32(format::sample::Type::Planar));
    encoder.set_ch_layout(ChannelLayout::from_mask(layout).context("AAC channel layout")?);
    encoder.set_bit_rate(if layout == ChannelLayoutMask::MONO {
        64000
    } else {
        128000
    });
    encoder.set_time_base((1, 48000));
    encoder.set_flags(codec::Flags::GLOBAL_HEADER);
    let encoder = encoder.open_as(codec)?;
    {
        let mut stream = output.add_stream(codec)?;
        stream.set_time_base((1, 48000));
        stream.set_parameters(codec::Parameters::from(&encoder));
    }
    header(&mut output)?;
    let output_time = output
        .stream(0)
        .context("No AAC output stream")?
        .time_base();
    let resampler = resampling::Context::get2(
        decoder.format(),
        decoder.ch_layout(),
        decoder.rate(),
        encoder.format(),
        ChannelLayout::from_mask(layout).context("AAC channel layout")?,
        48000,
    )?;
    let mut state = Transcoder {
        decoder,
        encoder,
        resampler,
        output,
        layout,
        output_time,
        timestamp: 0,
        fifo: vec![VecDeque::new(); layout.bits().count_ones() as usize],
    };
    loop {
        ensure!(!cancelled.is_cancelled(), "Audio conversion cancelled");
        let mut packet = Packet::empty();
        match packet.read(&mut input) {
            Ok(()) => (),
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(error.into()),
        }
        if packet.stream() == index {
            state.decoder.send_packet(&packet)?;
            state.drain_decoder()?;
        }
    }
    state.decoder.send_eof()?;
    state.drain_decoder()?;
    loop {
        ensure!(!cancelled.is_cancelled(), "Audio conversion cancelled");
        let capacity = state
            .resampler
            .delay()
            .map_or(32, |delay| delay.output.max(0) as usize + 32);
        let mut converted = frame::Audio::new(state.encoder.format(), capacity, layout);
        let remaining = state.resampler.flush(&mut converted)?;
        state.collect(&converted)?;
        if remaining.is_none() || converted.samples() == 0 {
            break;
        }
    }
    state.encode_ready(true)?;
    ensure!(state.timestamp > 0, "Downloaded audio was empty");
    state.encoder.send_eof()?;
    state.drain_encoder()?;
    state.output.write_trailer()?;
    Ok(())
}

pub fn fast_start(path: &Path) -> Result<bool> {
    let mut file = File::open(path)?;
    let total = file.metadata()?.len();
    let mut offset = 0;
    while offset + 8 <= total {
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = [0; 8];
        file.read_exact(&mut bytes)?;
        let mut size = u64::from(u32::from_be_bytes(bytes[..4].try_into()?));
        let kind = bytes[4..8].to_vec();
        if size == 1 {
            file.read_exact(&mut bytes)?;
            size = u64::from_be_bytes(bytes);
        }
        if size < 8 || size > total - offset {
            return Ok(false);
        }
        if kind == b"moov" {
            return Ok(true);
        }
        if kind == b"mdat" {
            return Ok(false);
        }
        offset += size;
    }
    Ok(false)
}

/// Duration of the locally verified media, in whole seconds for admission.
pub fn duration(path: &Path) -> Result<i64> {
    let input = format::input(path)?;
    let seconds = (input.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE)).ceil();
    ensure!(
        seconds.is_finite() && (1.0..=604800.0).contains(&seconds),
        "Invalid prepared audio duration"
    );
    Ok(seconds as i64)
}

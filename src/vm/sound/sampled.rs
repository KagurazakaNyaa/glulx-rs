//! Decode only upcoming audio packets; repeat at natural EOF, never by time.

use std::{io::Cursor, sync::Arc, time::Duration};

use rodio::Source;
use symphonia::core::{
    audio::SampleBuffer,
    codecs::{Decoder, DecoderOptions},
    errors::Error,
    formats::{FormatOptions, FormatReader},
    io::MediaSourceStream,
    meta::MetadataOptions,
    probe::Hint,
};

struct PacketDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    samples: Option<SampleBuffer<i16>>,
    offset: usize,
    channels: u16,
    sample_rate: u32,
    duration: Option<Duration>,
    remaining_samples: Option<u64>,
}

impl PacketDecoder {
    fn new(bytes: Arc<[u8]>) -> Option<Self> {
        // Cursor exposes the encoded byte length. Rodio 0.20's adapter hides
        // it, preventing Ogg from inspecting the final granule and trimming
        // codec padding. The payload remains shared between repetitions.
        let input = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
        let options = FormatOptions {
            enable_gapless: true,
            ..Default::default()
        };
        let format = symphonia::default::get_probe()
            .format(&Hint::new(), input, &options, &MetadataOptions::default())
            .ok()?
            .format;
        let track = format.default_track()?;
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .ok()?;
        let duration = track
            .codec_params
            .time_base
            .zip(track.codec_params.n_frames)
            .map(|(base, frames)| {
                let time = base.calc_time(frames);
                Duration::from_secs(time.seconds) + Duration::from_secs_f64(time.frac)
            });
        let track_id = track.id;
        let frames = track.codec_params.n_frames;
        let mut result = Self {
            format,
            decoder,
            track_id,
            samples: None,
            offset: 0,
            channels: 0,
            sample_rate: 0,
            duration,
            remaining_samples: None,
        };
        result.refill()?;
        // The container's integral frame count excludes codec padding. Ogg
        // streams whose audio fits on one page can have their packets queued
        // before Symphonia has discovered the final granule's trim boundary.
        result.remaining_samples =
            frames.and_then(|frames| frames.checked_mul(result.channels as u64));
        Some(result)
    }

    fn refill(&mut self) -> Option<()> {
        let mut errors = 0;
        loop {
            let packet = self.format.next_packet().ok()?;
            if packet.track_id() != self.track_id {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(Error::DecodeError(_)) if errors < 3 => {
                    errors += 1;
                    continue;
                }
                Err(_) => return None,
            };
            // Vorbis has an empty priming packet, and trimmed packets can
            // also contain no output. Only the container's EOF ends audio.
            if decoded.frames() == 0 {
                continue;
            }
            let spec = *decoded.spec();
            let mut samples = SampleBuffer::new(decoded.capacity() as u64, spec);
            samples.copy_interleaved_ref(decoded);
            self.samples = Some(samples);
            self.offset = 0;
            self.channels = spec.channels.count().try_into().ok()?;
            self.sample_rate = spec.rate;
            return Some(());
        }
    }

    fn next(&mut self) -> Option<i16> {
        if self.remaining_samples == Some(0) {
            return None;
        }
        if self.offset >= self.samples.as_ref()?.len() {
            self.refill()?;
        }
        let sample = self.samples.as_ref()?.samples()[self.offset];
        self.offset += 1;
        if let Some(remaining) = &mut self.remaining_samples {
            *remaining -= 1;
        }
        Some(sample)
    }
}

pub(super) struct SampledSource {
    bytes: Arc<[u8]>,
    decoder: PacketDecoder,
    pending: Option<i16>,
    remaining: u32,
    duration: Option<Duration>,
}

impl SampledSource {
    pub(super) fn new(bytes: &[u8], repeats: u32, offset_ms: u64) -> Option<Self> {
        let bytes: Arc<[u8]> = bytes.into();
        let mut decoder = PacketDecoder::new(bytes.clone())?;
        if decoder.sample_rate == 0 || decoder.channels == 0 {
            return None;
        }
        let pending = Some(decoder.next()?);
        let duration = if repeats == u32::MAX {
            None
        } else {
            decoder
                .duration
                .map(|duration| duration.saturating_mul(repeats))
        };
        let frames = offset_ms as u128 * decoder.sample_rate as u128 / 1000;
        let samples = frames * decoder.channels as u128;
        let mut source = Self {
            bytes,
            decoder,
            pending,
            remaining: repeats,
            duration,
        };
        // Millisecond session offsets resume on a complete interleaved frame.
        // Using nanoseconds per sample would accumulate rounding drift at
        // 44.1 kHz and can swap left/right by stopping between the channels.
        for _ in 0..samples {
            if source.next().is_none() {
                break;
            }
        }
        source.duration = source
            .duration
            .map(|duration| duration.saturating_sub(Duration::from_millis(offset_ms)));
        Some(source)
    }

    fn advance(&mut self) {
        self.pending = self.decoder.next();
        if self.pending.is_some() {
            return;
        }
        if self.remaining != u32::MAX {
            self.remaining = self.remaining.saturating_sub(1);
        }
        if self.remaining == 0 {
            return;
        }
        let Some(mut decoder) = PacketDecoder::new(self.bytes.clone()) else {
            self.remaining = 0;
            return;
        };
        self.pending = decoder.next();
        if self.pending.is_none() {
            self.remaining = 0;
        }
        self.decoder = decoder;
    }
}

impl Iterator for SampledSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let sample = self.pending.take()?;
        self.advance();
        Some(sample as f32 / 32768.0)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (
            usize::from(self.remaining != 0 && self.pending.is_some()),
            None,
        )
    }
}

impl Source for SampledSource {
    fn current_frame_len(&self) -> Option<usize> {
        // Decoder packets and resource repetitions keep the same sample
        // format. Reporting them as separate Source frames would restart
        // Rodio's resampler and accumulate fractional-sample drift.
        None
    }
    fn channels(&self) -> u16 {
        self.decoder.channels
    }
    fn sample_rate(&self) -> u32 {
        self.decoder.sample_rate
    }
    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wave(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&(36 + samples.len() as u32 * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        for value in [1u16, channels] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
        for value in [channels * 2, 16] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(samples.len() as u32 * 2).to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn finite_repetition_preserves_every_sample_at_all_sample_rates() {
        for (rate, channels, length) in [
            (8000, 1, 8),
            (8000, 2, 16),
            (44100, 2, 88200),
            (48000, 2, 96000),
        ] {
            let samples: Vec<i16> = (0..length)
                .map(|i| if i % 2 == 0 { 8192 } else { -16384 })
                .collect();
            let bytes = wave(rate, channels, &samples);
            for repeats in [0, 1, 3] {
                let actual: Vec<_> = SampledSource::new(&bytes, repeats, 0).unwrap().collect();
                let expected: Vec<_> = samples
                    .iter()
                    .map(|sample| *sample as f32 / 32768.0)
                    .collect();
                assert_eq!(
                    actual,
                    expected.repeat(repeats as usize),
                    "{rate} Hz, {channels} channels, repeats {repeats}"
                );
            }
        }
    }

    #[test]
    fn resume_offset_crosses_repetition_boundaries_without_channel_drift() {
        let samples: Vec<i16> = (0..882)
            .map(|i| if i % 2 == 0 { i as i16 } else { -(i as i16) })
            .collect();
        let bytes = wave(44100, 2, &samples);
        let expected: Vec<f32> = samples
            .iter()
            .map(|sample| *sample as f32 / 32768.0)
            .collect::<Vec<_>>()
            .repeat(3);
        for offset in [0, 1, 10, 11, 20, 29, 30, 40] {
            let actual: Vec<_> = SampledSource::new(&bytes, 3, offset).unwrap().collect();
            let skipped = (offset * 44100 / 1000 * 2) as usize;
            assert_eq!(
                actual,
                expected[skipped.min(expected.len())..],
                "offset {offset} ms"
            );
        }
        let infinite: Vec<_> = SampledSource::new(&bytes, u32::MAX, 10)
            .unwrap()
            .take(expected.len())
            .collect();
        assert_eq!(infinite, expected);
    }

    #[test]
    fn empty_and_invalid_sampled_resources_fail_without_repeat_loops() {
        assert!(SampledSource::new(b"not audio", u32::MAX, 0).is_none());
        assert!(SampledSource::new(&wave(8000, 1, &[]), u32::MAX, 0).is_none());
    }

    #[test]
    fn streaming_resampling_preserves_native_rate_stereo_frames() {
        let samples: Vec<i16> = (0..4000)
            .map(|i| if i % 2 == 0 { 8192 } else { -16384 })
            .collect();
        let bytes = wave(44100, 2, &samples);
        let source = SampledSource::new(&bytes, 3, 0).unwrap();
        let converted: Vec<f32> =
            rodio::source::UniformSourceIterator::new(source, 2, 44100).collect();
        let expected: Vec<_> = samples
            .iter()
            .map(|sample| *sample as f32 / 32768.0)
            .collect();
        assert_eq!(converted, expected.repeat(3));
    }

    #[test]
    fn resampling_keeps_its_phase_across_packets_and_repetitions() {
        for rate in [8000, 22050, 48000] {
            let samples: Vec<i16> = (0..rate * 2)
                .map(|i| ((i % 200) as i16 - 100) * 256)
                .collect();
            let bytes = wave(rate, 2, &samples);
            let source = SampledSource::new(&bytes, 3, 0).unwrap();
            let actual: Vec<f32> =
                rodio::source::UniformSourceIterator::new(source, 2, 44100).collect();
            let expected_pcm: Vec<f32> = samples
                .iter()
                .map(|sample| *sample as f32 / 32768.0)
                .collect::<Vec<_>>()
                .repeat(3);
            let baseline = rodio::buffer::SamplesBuffer::new(2, rate, expected_pcm);
            let expected: Vec<f32> =
                rodio::source::UniformSourceIterator::new(baseline, 2, 44100).collect();
            assert_eq!(actual.len(), expected.len(), "{rate} Hz sample count");
            assert!(actual == expected, "{rate} Hz resampling phase");
        }
    }
}

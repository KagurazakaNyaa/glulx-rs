//! Blorb SONG: MOD patterns with AIFF samples from the same resource map.
//!
//! Preserve 16-bit PCM precision (wider samples lose their low bits), averaging
//! multichannel frames to mono. Playback pitch follows the MOD period/finetune,
//! as with Blorb's permitted conversion to ordinary MOD, not the AIFF rate.
//! AIFF sustain loops become ordinary tracker loops; release loops are unused.

use std::{collections::BTreeMap, sync::Arc};

use xmrs::prelude::{InstrumentType, LoopType, Module, SampleDataType};

const MAX_SONG_BYTES: usize = 1024 * 1024;
const MAX_PCM_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone)]
struct AiffSample {
    pcm: Arc<[i16]>,
    loop_kind: LoopType,
    loop_start: u32,
    loop_length: u32,
}

pub(super) fn assemble<'a>(
    bytes: &[u8],
    mut resource: impl FnMut(u32) -> Option<&'a [u8]>,
) -> Option<Module> {
    let (mut module, references) = template(bytes)?;
    let mut samples = BTreeMap::<u32, AiffSample>::new();
    let mut budget = MAX_PCM_BYTES;
    for (instrument, reference) in module.instrument.iter_mut().zip(references) {
        let Some(number) = reference else { continue };
        if let std::collections::btree_map::Entry::Vacant(entry) = samples.entry(number) {
            let sample = aiff(resource(number)?, budget)?;
            budget -= sample.pcm.len() * 2;
            entry.insert(sample);
        }
        let source = &samples[&number];
        let InstrumentType::Default(instrument) = &mut instrument.instr_type else {
            return None;
        };
        let sample = instrument.sample.first_mut()?.as_mut()?;
        sample.data = Some(SampleDataType::Mono16(source.pcm.clone()));
        sample.loop_flag = source.loop_kind;
        sample.loop_start = source.loop_start;
        sample.loop_length = source.loop_length;
    }
    Some(module)
}

fn reference(name: &[u8]) -> Option<Option<u32>> {
    let end = name
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(name.len());
    let name = &name[..end];
    let name = name.trim_ascii_end();
    if name.is_empty() {
        return Some(None);
    }
    let digits = name.strip_prefix(b"SND")?;
    if digits.is_empty() {
        return None;
    }
    let mut number = 0u32;
    for digit in digits {
        if !digit.is_ascii_digit() {
            return None;
        }
        number = number
            .checked_mul(10)?
            .checked_add(u32::from(digit - b'0'))?;
    }
    Some(Some(number))
}

fn template(bytes: &[u8]) -> Option<(Module, Vec<Option<u32>>)> {
    if bytes.len() > MAX_SONG_BYTES {
        return None;
    }
    // Try tagged 31-sample and original 15-sample MOD layouts. The importer
    // validates the actual signature, order table and pattern lengths; its
    // instrument count must agree with the header fields we cleared.
    for count in [31, 15] {
        let header_size = 20 + count * 30 + 130 + if count == 31 { 4 } else { 0 };
        if bytes.len() < header_size {
            continue;
        }
        let references: Option<Vec<_>> = (0..count)
            .map(|index| reference(&bytes[20 + index * 30..42 + index * 30]))
            .collect();
        let Some(references) = references else {
            continue;
        };
        let mut template = bytes.to_vec();
        for index in 0..count {
            let start = 20 + index * 30;
            // SONG's lengths and repeat points have no meaning. Preserve
            // name, finetune and volume for the ordinary MOD importer.
            template[start + 22..start + 24].fill(0);
            template[start + 26..start + 30].fill(0);
        }
        if let Ok(module) = Module::load_mod(&template)
            && module.instrument.len() == count
        {
            return Some((module, references));
        }
    }
    None
}

fn word(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn long(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn aiff(bytes: &[u8], budget: usize) -> Option<AiffSample> {
    if bytes.get(..4)? != b"FORM" || bytes.get(8..12)? != b"AIFF" {
        return None;
    }
    let end = (long(bytes, 4)? as usize).checked_add(8)?;
    if end != bytes.len() {
        return None;
    }
    let mut chunks = BTreeMap::new();
    let mut cursor = 12usize;
    while cursor < end {
        let kind: [u8; 4] = bytes.get(cursor..cursor.checked_add(4)?)?.try_into().ok()?;
        let length = long(bytes, cursor.checked_add(4)?)? as usize;
        let start = cursor.checked_add(8)?;
        let next = start.checked_add(length)?;
        let payload = bytes.get(start..next)?;
        if matches!(&kind, b"COMM" | b"SSND" | b"MARK" | b"INST")
            && chunks.insert(kind, payload).is_some()
        {
            return None;
        }
        cursor = next.checked_add(length % 2)?;
    }
    if cursor != end {
        return None;
    }
    let common = *chunks.get(b"COMM")?;
    if common.len() != 18 {
        return None;
    }
    let channels = word(common, 0)? as usize;
    let frames = long(common, 2)? as usize;
    let bits = word(common, 6)? as usize;
    let rate_exponent = word(common, 8)?;
    if channels == 0
        || channels > i16::MAX as usize
        || !(1..=32).contains(&bits)
        || frames.checked_mul(2)? > budget
        || rate_exponent & 0x8000 != 0
        || rate_exponent == 0x7fff
        || common[10..18].iter().all(|byte| *byte == 0)
    {
        return None;
    }
    let bytes_per_point = bits.div_ceil(8);
    let frame_bytes = channels.checked_mul(bytes_per_point)?;
    let data = if let Some(sound) = chunks.get(b"SSND") {
        let offset = long(sound, 0)? as usize;
        long(sound, 4)?; // Block alignment does not alter contiguous frames.
        let start = 8usize.checked_add(offset)?;
        sound.get(start..start.checked_add(frames.checked_mul(frame_bytes)?)?)?
    } else if frames == 0 {
        &[]
    } else {
        return None;
    };
    let (loop_kind, loop_start, loop_length) = sustain_loop(&chunks, frames as u32)?;
    let mut pcm = Vec::new();
    pcm.try_reserve_exact(frames).ok()?;
    for frame in data.chunks_exact(frame_bytes) {
        let mut sum = 0i64;
        for point in frame.chunks_exact(bytes_per_point) {
            let mut wide = [0u8; 4];
            wide[..bytes_per_point].copy_from_slice(point);
            sum += i64::from(i32::from_be_bytes(wide));
        }
        pcm.push(((sum / channels as i64) >> 16) as i16);
    }
    Some(AiffSample {
        pcm: pcm.into(),
        loop_kind,
        loop_start,
        loop_length,
    })
}

fn sustain_loop(chunks: &BTreeMap<[u8; 4], &[u8]>, frames: u32) -> Option<(LoopType, u32, u32)> {
    let no_loop = (LoopType::No, 0, 0);
    let Some(instrument) = chunks.get(b"INST") else {
        return Some(no_loop);
    };
    if instrument.len() != 20 {
        return None;
    }
    let kind = match word(instrument, 8)? {
        0 => return Some(no_loop),
        1 => LoopType::Forward,
        2 => LoopType::PingPong,
        _ => return None,
    };
    let markers = *chunks.get(b"MARK")?;
    let mut positions = BTreeMap::new();
    let mut cursor = 2usize;
    for _ in 0..word(markers, 0)? {
        let id = word(markers, cursor)?;
        let position = long(markers, cursor.checked_add(2)?)?;
        if id == 0
            || id > i16::MAX as u16
            || position > frames
            || positions.insert(id, position).is_some()
        {
            return None;
        }
        cursor = cursor.checked_add(6)?;
        let length = usize::from(*markers.get(cursor)?).checked_add(1)?;
        cursor = cursor.checked_add(length)?.checked_add(length % 2)?;
        markers.get(..cursor)?;
    }
    if cursor != markers.len() {
        return None;
    }
    let start = *positions.get(&word(instrument, 10)?)?;
    let end = *positions.get(&word(instrument, 12)?)?;
    Some(if start >= end {
        no_loop
    } else {
        (kind, start, end - start)
    })
}

#[cfg(test)]
mod fixtures;

#[cfg(test)]
mod tests {
    use super::super::tracker::ModSource;
    use super::*;
    use rodio::Source;
    use xmrs::prelude::Sample;

    fn sample(module: &Module, index: usize) -> &Sample {
        let InstrumentType::Default(instrument) = &module.instrument[index].instr_type else {
            panic!("expected PCM instrument");
        };
        instrument.sample[0].as_ref().unwrap()
    }

    fn waveform() -> Vec<i8> {
        (0..64)
            .map(|index| if index < 32 { 96 } else { -96 })
            .collect()
    }

    fn points(pcm: &[i8]) -> Vec<i32> {
        pcm.iter().map(|value| i32::from(*value) << 24).collect()
    }

    #[test]
    fn song_matches_assembled_mod_pcm_pitch_volume_and_repetition() {
        let pcm = waveform();
        for (mode, start, end) in [(0, 0, 64), (1, 4, 52)] {
            for (finetune, volume) in [(0, 64), (7, 32), (8, 48)] {
                let song = fixtures::song(&[("SND42", finetune, volume)]);
                let instrument = fixtures::aiff(16, 1, &points(&pcm), Some((mode, start, end)), 3);
                let ordinary = fixtures::module(
                    &pcm,
                    start as u16,
                    if mode == 0 { 0 } else { (end - start) as u16 },
                    finetune,
                    volume,
                );
                let expected: Vec<_> = ModSource::new(&ordinary, 3).unwrap().collect();
                let module = assemble(&song, |number| {
                    (number == 42).then_some(instrument.as_slice())
                })
                .unwrap();
                let actual: Vec<_> = ModSource::from_module(module, 3).collect();
                assert_eq!(
                    actual, expected,
                    "mode {mode}, finetune {finetune}, volume {volume}"
                );
                assert_eq!(actual.len(), 15876 * 3);
                assert!(actual.iter().any(|sample| sample.abs() > 0.01));
            }
        }
    }

    #[test]
    fn pingpong_markers_match_an_unfolded_forward_loop() {
        let pcm: Vec<i8> = (0..32)
            .map(|value| (value as i16 * 7 - 110) as i8)
            .collect();
        let song = fixtures::song(&[("SND42", 0, 64)]);
        let instrument = fixtures::aiff(8, 1, &points(&pcm), Some((2, 4, 12)), 0);
        let module = assemble(&song, |_| Some(instrument.as_slice())).unwrap();
        assert_eq!(sample(&module, 0).loop_flag, LoopType::PingPong);
        let mut unfolded = pcm[..12].to_vec();
        unfolded.extend(pcm[4..12].iter().rev());
        let expected: Vec<_> = ModSource::new(&fixtures::module(&unfolded, 4, 16, 0, 64), 1)
            .unwrap()
            .collect();
        let actual: Vec<_> = ModSource::from_module(module, 1).collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn shared_references_reuse_pcm_and_preserve_instrument_controls() {
        let song = fixtures::song(&[("SND42", 7, 16), ("SND42", 8, 64), ("SND0", 0, 32)]);
        let instrument = fixtures::aiff(24, 1, &points(&waveform()), Some((1, 3, 17)), 5);
        let mut requested = Vec::new();
        let module = assemble(&song, |number| {
            requested.push(number);
            Some(instrument.as_slice())
        })
        .unwrap();
        assert_eq!(requested, [42, 0]);
        let (Some(SampleDataType::Mono16(first)), Some(SampleDataType::Mono16(second))) =
            (&sample(&module, 0).data, &sample(&module, 1).data)
        else {
            panic!()
        };
        assert!(Arc::ptr_eq(first, second));
        assert_eq!(sample(&module, 0).loop_start, 3);
        assert_eq!(sample(&module, 0).loop_length, 14);
        assert_ne!(sample(&module, 0).finetune, sample(&module, 1).finetune);
        assert_ne!(sample(&module, 0).volume, sample(&module, 1).volume);
    }

    #[test]
    fn aiff_bit_depth_offset_and_multichannel_frames_decode_correctly() {
        for (bits, point, expected) in [
            (1, i32::MIN, i16::MIN),
            (8, 0x12000000, 0x1200),
            (12, 0x12300000, 0x1230),
            (16, 0x12340000, 0x1234),
            (20, 0x12345000, 0x1234),
            (24, 0x12345600, 0x1234),
            (32, 0x12345678, 0x1234),
        ] {
            let bytes = fixtures::aiff(bits, 1, &[point], None, 7);
            assert_eq!(
                &*aiff(&bytes, MAX_PCM_BYTES).unwrap().pcm,
                &[expected],
                "{bits} bits"
            );
        }
        let stereo = fixtures::aiff(
            16,
            2,
            &[0x40000000, -0x40000000, 0x60000000, 0x20000000],
            Some((1, 0, 2)),
            1,
        );
        let decoded = aiff(&stereo, MAX_PCM_BYTES).unwrap();
        assert_eq!(&*decoded.pcm, &[0, 16384]);
        assert_eq!(decoded.loop_length, 2); // Frames, not interleaved sample points.
    }

    #[test]
    fn absent_disabled_and_reversed_sustain_loops_do_not_repeat() {
        for looping in [None, Some((0, 0, 64)), Some((1, 12, 12)), Some((2, 50, 2))] {
            let bytes = fixtures::aiff(8, 1, &points(&waveform()), looping, 0);
            let decoded = aiff(&bytes, MAX_PCM_BYTES).unwrap();
            assert_eq!(
                (decoded.loop_kind, decoded.loop_start, decoded.loop_length),
                (LoopType::No, 0, 0)
            );
        }
        // MODE0 ignores marker IDs and the release loop must not substitute.
        let mut bytes = fixtures::aiff(8, 1, &points(&waveform()), Some((0, 0, 64)), 0);
        let inst = bytes.windows(4).position(|part| part == b"INST").unwrap() + 8;
        bytes[inst + 10..inst + 14].fill(0xff);
        assert_eq!(aiff(&bytes, MAX_PCM_BYTES).unwrap().loop_kind, LoopType::No);
    }

    #[test]
    fn malformed_references_aiff_chunks_and_lengths_fail_within_limits() {
        let valid = fixtures::aiff(16, 1, &points(&waveform()), Some((1, 0, 64)), 0);
        for name in ["SND", "SND-1", "SND4294967296", "SND1x", "not a reference"] {
            assert!(
                assemble(&fixtures::song(&[(name, 0, 64)]), |_| Some(
                    valid.as_slice()
                ))
                .is_none(),
                "{name}"
            );
        }
        let song = fixtures::song(&[("SND42", 0, 64)]);
        assert!(assemble(&song, |_| None).is_none());
        assert!(assemble(&song, |_| Some(song.as_slice())).is_none());
        for length in [0, 21, 42, 599, 1083, song.len() - 1] {
            assert!(
                assemble(&song[..length], |_| Some(valid.as_slice())).is_none(),
                "SONG prefix {length}"
            );
        }
        for length in 0..valid.len() {
            assert!(aiff(&valid[..length], MAX_PCM_BYTES).is_none());
        }
        for (kind, field, replacement) in [
            (b"COMM", 2, u32::MAX),  // Declared sample count, before allocation.
            (b"SSND", 0, u32::MAX),  // Sound offset.
            (b"MARK", 4, 65),        // Marker beyond the64-frame sample.
            (b"INST", 10, u32::MAX), // Missing marker IDs.
        ] {
            let mut broken = valid.clone();
            let offset = broken.windows(4).position(|part| part == kind).unwrap() + 8 + field;
            broken[offset..offset + 4].copy_from_slice(&replacement.to_be_bytes());
            assert!(aiff(&broken, MAX_PCM_BYTES).is_none(), "{kind:?}");
        }
        assert!(aiff(&valid, 127).is_none());
        assert!(assemble(&vec![0; MAX_SONG_BYTES + 1], |_| Some(valid.as_slice())).is_none());
    }

    #[test]
    fn song_uses_story_resources_and_resumes_on_stereo_frame_boundaries() {
        let song = fixtures::song(&[("SND42", 0, 64)]);
        let instrument = fixtures::aiff(16, 1, &points(&waveform()), Some((1, 0, 64)), 3);
        let story = fixtures::story(&song, &instrument);
        let source = |repeats, offset| {
            super::super::decode_sound(
                story.sound_resource(7).unwrap(),
                story.resource_type(*b"Snd ", 7).unwrap(),
                repeats,
                offset,
                |number| {
                    (story.resource_type(*b"Snd ", number)? == *b"FORM")
                        .then(|| story.sound_resource(number))?
                },
            )
            .unwrap()
        };
        let once: Vec<f32> = source(1, 0).collect();
        assert_eq!(once.len(), 15876);
        assert_eq!(source(0, 0).count(), 0);
        assert_eq!(source(3, 0).collect::<Vec<_>>(), once.repeat(3));
        assert_eq!(
            source(u32::MAX, 0).take(once.len() * 3).collect::<Vec<_>>(),
            once.repeat(3)
        );
        let repeated = once.repeat(3);
        for offset in [1, 180, 181, 359, 540, 600] {
            let skipped = (offset * 44100 / 1000 * 2) as usize;
            let resumed: Vec<_> = source(3, offset).collect();
            assert_eq!(resumed, repeated[skipped.min(repeated.len())..]);
        }
        let normalized: Vec<f32> =
            rodio::source::UniformSourceIterator::new(source(3, 0), 2, 44100).collect();
        assert_eq!(normalized, repeated);
        assert_eq!(source(1, 0).channels(), 2);
    }

    #[test]
    fn original_fifteen_sample_headers_and_full_width_names_are_supported() {
        let song = fixtures::song(&[("SND0000000000000000001", 0, 64)]);
        let mut original = song[..470].to_vec();
        original.extend_from_slice(&song[950..1080]);
        original.extend_from_slice(&song[1084..]);
        let instrument = fixtures::aiff(8, 1, &points(&waveform()), None, 0);
        let mut requests = Vec::new();
        let module = assemble(&original, |number| {
            requests.push(number);
            Some(instrument.as_slice())
        })
        .unwrap();
        assert_eq!(requests, [1]);
        assert_eq!(module.instrument.len(), 15);
        assert_eq!(sample(&module, 0).len(), 64);
        assert!(
            ModSource::from_module(module, 1)
                .take(1000)
                .any(|value| value.abs() > 0.01)
        );
    }

    #[test]
    fn song_pause_completion_and_stop_use_existing_glk_channel_semantics() {
        use super::super::{Channel, Sink};
        let song = fixtures::song(&[("SND42", 0, 64)]);
        let instrument = fixtures::aiff(16, 1, &points(&waveform()), Some((1, 0, 64)), 0);
        let mut vm = crate::Vm::new(fixtures::story(&song, &instrument)).unwrap();
        let new_source = |repeats| {
            ModSource::from_module(
                assemble(&song, |_| Some(instrument.as_slice())).unwrap(),
                repeats,
            )
        };
        let (sink, mut output) = Sink::new_idle();
        sink.pause();
        sink.append(new_source(2));
        vm.channels.insert(
            1,
            Channel {
                rock: 1,
                volume: 65536,
                paused: true,
                resource: 7,
                notify: 99,
                repeats: 2,
                position_ms: 0,
                active: true,
                fade_resume: None,
                offset_ms: 0,
                sink: Some(sink),
                fade: None,
            },
        );
        assert!(output.by_ref().take(1024).all(|value| value == 0.0));
        vm.poll_sound();
        assert!(vm.events.is_empty());
        assert!(vm.channels[&1].active);
        vm.sound_call(0xff, &[1]).unwrap();
        assert!(
            output
                .by_ref()
                .take(15876 * 2 + 4096)
                .any(|value| value.abs() > 0.01)
        );
        // Finish consuming after the early-exiting non-silence check above.
        output.by_ref().take(15876 * 2 + 4096).for_each(drop);
        vm.poll_sound();
        assert_eq!(vm.events.pop_front(), Some([7, 0, 7, 99]));
        vm.poll_sound();
        assert!(vm.events.is_empty());
        let (sink, mut output) = Sink::new_idle();
        sink.append(new_source(u32::MAX));
        let channel = vm.channels.get_mut(&1).unwrap();
        channel.sink = Some(sink);
        channel.active = true;
        channel.notify = 99;
        channel.repeats = u32::MAX;
        output.by_ref().take(2048).for_each(drop);
        vm.sound_call(0xfa, &[1]).unwrap();
        output.take(4096).for_each(drop);
        vm.poll_sound();
        assert!(vm.events.is_empty());
        assert!(!vm.channels[&1].active);
    }
}

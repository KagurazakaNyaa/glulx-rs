//! Blorb MOD/XM/S3M/IT resources, rendered incrementally to stereo PCM.
//! Keeping the module with its borrowing player avoids expanding minutes of
//! tracker music into a large PCM allocation before playback can begin.
use rodio::Source;
use std::time::Duration;
use xmrs::prelude::Module;
use xmrsplayer::xmrsplayer::XmrsPlayer;

#[cfg(test)]
mod fixtures;

pub(super) const SAMPLE_RATE: u32 = 44_100;

self_cell::self_cell! {
    struct OwnedPlayer {
        owner: Module,
        #[not_covariant]
        dependent: XmrsPlayer,
    }
}

pub(super) struct ModSource {
    player: OwnedPlayer,
    pending: Option<f32>,
    remaining: u32,
}

impl ModSource {
    pub(super) fn new(bytes: &[u8], repeats: u32) -> Option<Self> {
        let module = if bytes.starts_with(b"Extended Module: ") {
            Module::load_xm(bytes)
        } else if bytes.starts_with(b"IMPM") {
            Module::load_it(bytes)
        } else if bytes.get(44..48) == Some(b"SCRM") {
            Module::load_s3m(bytes)
        } else {
            Module::load_mod(bytes)
        }
        .ok()?;
        Some(Self::from_module(module, repeats))
    }

    pub(super) fn from_module(module: Module, repeats: u32) -> Self {
        let mut player = OwnedPlayer::new(module, Self::player);
        let pending = player
            .with_dependent_mut(|_, player| player.next())
            .map(|sample| sample as f32 / 32768.0);
        Self {
            player,
            pending,
            remaining: repeats,
        }
    }

    fn player(module: &Module) -> XmrsPlayer<'_> {
        let mut player = XmrsPlayer::new(module, SAMPLE_RATE, 0);
        // The library folds tracker pattern loops into one song traversal.
        // End the traversal at a song restart; Glk's repeat count then controls
        // how many complete, freshly initialized traversals we play.
        player.set_max_loop_count(1);
        player
    }

    pub(super) fn skip_millis(&mut self, millis: u64) {
        let frames = millis as u128 * SAMPLE_RATE as u128 / 1000;
        for _ in 0..frames * 2 {
            if self.next().is_none() {
                break;
            }
        }
    }
    fn advance(&mut self) {
        self.pending = self
            .player
            .with_dependent_mut(|_, player| player.next())
            .map(|sample| sample as f32 / 32768.0);
        if self.pending.is_some() {
            return;
        }
        if self.remaining != u32::MAX {
            self.remaining -= 1;
        }
        if self.remaining == 0 {
            return;
        }
        self.player
            .with_dependent_mut(|module, player| *player = Self::player(module));
        // An empty song must terminate even for infinite repetition.
        self.pending = self
            .player
            .with_dependent_mut(|_, player| player.next())
            .map(|sample| sample as f32 / 32768.0);
        if self.pending.is_none() {
            self.remaining = 0;
        }
    }
}

impl Iterator for ModSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let sample = self.pending.take()?;
        self.advance();
        Some(sample)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (
            usize::from(self.remaining != 0 && self.pending.is_some()),
            None,
        )
    }
}

impl Source for ModSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> rodio::ChannelCount {
        rodio::ChannelCount::new(2).unwrap()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        rodio::SampleRate::new(SAMPLE_RATE).unwrap()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A generated, original four-channel M.K. song with one square-wave sample.
    // No copyrighted music or binary fixtures are needed for the replay tests.
    fn module() -> Vec<u8> {
        let mut bytes = vec![0; 1084 + 1024 + 64];
        bytes[..14].copy_from_slice(b"Glulx MOD test");
        bytes[42..44].copy_from_slice(&32u16.to_be_bytes());
        bytes[45] = 64;
        bytes[48..50].copy_from_slice(&32u16.to_be_bytes());
        bytes[950] = 1;
        bytes[1080..1084].copy_from_slice(b"M.K.");
        bytes[2108..2140].fill(96);
        bytes[2140..2172].fill(160);
        // Row 0: instrument 1, period 428 (C-3), F03 (three ticks/row).
        bytes[1084..1088].copy_from_slice(&[0x01, 0xac, 0x1f, 0x03]);
        // Row 1: C00 (mute); row 2: B00 (restart song).
        bytes[1100..1104].copy_from_slice(&[0, 0, 0x0c, 0]);
        bytes[1116..1120].copy_from_slice(&[0, 0, 0x0b, 0]);
        bytes
    }

    #[test]
    fn mod_renders_pcm_speed_volume_and_song_end() {
        let source = ModSource::new(&module(), 1).unwrap();
        assert_eq!(source.channels().get(), 2);
        assert_eq!(source.sample_rate().get(), SAMPLE_RATE);
        let samples: Vec<_> = source.collect();
        // Default 125 BPM: 20 ms/tick, three 3-tick rows = 180 ms.
        assert_eq!(samples.len(), 15_876);
        assert!(samples[..4410].iter().any(|sample| sample.abs() > 0.01));
        assert!(samples[7056..].iter().all(|sample| *sample == 0.0));
        // PAL Amiga sample clock / period 428 / 64-sample waveform is
        // approximately 129.5 Hz: twelve or thirteen half-cycles in 50 ms.
        let left: Vec<_> = samples[..4410].iter().step_by(2).collect();
        let crossings = left
            .windows(2)
            .filter(|pair| pair[0].is_sign_negative() != pair[1].is_sign_negative())
            .count();
        assert!((12..=14).contains(&crossings), "{crossings} crossings");
    }

    #[test]
    fn mod_tempo_changes_and_pattern_loops_have_correct_duration() {
        let mut faster = module();
        // FFA changes BPM to 250: 60 ms + 30 ms + 30 ms = 120 ms.
        faster[1100..1104].copy_from_slice(&[0, 0, 0x0f, 0xfa]);
        assert_eq!(ModSource::new(&faster, 1).unwrap().count(), 10_584);

        let mut looping = module();
        // E60/E62 repeats the first two rows twice more before B00 ends
        // the song: seven rows total. Pattern repeats are part of one
        // Glk playback, not additional repetitions of the whole resource.
        looping[1088..1092].copy_from_slice(&[0, 0, 0x0e, 0x60]);
        looping[1104..1108].copy_from_slice(&[0, 0, 0x0e, 0x62]);
        assert_eq!(ModSource::new(&looping, 1).unwrap().count(), 37_044);
    }

    #[test]
    fn mod_repeats_restart_the_song_and_zero_repeats_are_empty() {
        let bytes = module();
        let once: Vec<_> = ModSource::new(&bytes, 1).unwrap().collect();
        let twice: Vec<_> = ModSource::new(&bytes, 2).unwrap().collect();
        assert_eq!(twice, once.repeat(2));
        assert_eq!(
            ModSource::new(&bytes, u32::MAX)
                .unwrap()
                .take(once.len() * 3)
                .collect::<Vec<_>>(),
            once.repeat(3)
        );
        assert!(ModSource::new(&bytes, 0).unwrap().next().is_none());
        assert_eq!(
            ModSource::new(&bytes, 2)
                .unwrap()
                .skip_duration(Duration::from_millis(100))
                .collect::<Vec<_>>(),
            twice[8820..]
        );
    }

    #[test]
    fn mod_rejects_truncated_headers_patterns_and_invalid_resources() {
        for size in [0, 20, 599, 1083, 2107] {
            assert!(
                ModSource::new(&module()[..size], 1).is_none(),
                "length {size}"
            );
        }
        assert!(ModSource::new(&[0; 1084], 1).is_none());
    }

    #[test]
    fn blorb_mod_chunk_uses_the_tracker_decoder() {
        let bytes = module();
        assert!(super::super::decode_sound(&bytes, *b"MOD ", 1, 0, |_| None).is_some());
        assert!(
            super::super::decode_sound_with_limits(
                &bytes,
                *b"MOD ",
                1,
                0,
                |_| None,
                crate::memory::ResourceLimits {
                    audio_resource_mib: 0,
                    ..Default::default()
                }
            )
            .is_none()
        );
        assert!(super::super::decode_sound(&bytes, *b"OGGV", 1, 0, |_| None).is_none());
        assert!(super::super::decode_sound(&bytes[..600], *b"MOD ", 1, 0, |_| None).is_none());
    }

    #[test]
    fn all_standard_blorb_tracker_formats_render_and_repeat_complete_songs() {
        for (name, bytes) in [
            ("MOD", module()),
            ("XM", fixtures::xm()),
            ("S3M", fixtures::s3m()),
            ("IT", fixtures::it()),
        ] {
            let source = ModSource::new(&bytes, 1).unwrap_or_else(|| panic!("cannot load {name}"));
            let once: Vec<_> = source.take(100_000).collect();
            assert_eq!(once.len(), 15_876, "{name}: three 3-tick rows at125BPM");
            assert!(
                once[..4410].iter().any(|sample| sample.abs() > 0.001),
                "{name} is silent"
            );
            let twice: Vec<_> = ModSource::new(&bytes, 2).unwrap().take(100_000).collect();
            assert_eq!(twice, once.repeat(2), "{name} repeated playback");
            let converted: Vec<f32> = rodio::source::UniformSourceIterator::new(
                ModSource::new(&bytes, 2).unwrap(),
                rodio::ChannelCount::new(2).unwrap(),
                rodio::SampleRate::new(SAMPLE_RATE).unwrap(),
            )
            .collect();
            assert_eq!(converted, twice, "{name} playback conversion");
            let mut resumed = ModSource::new(&bytes, 1).unwrap();
            resumed.skip_millis(5);
            assert_eq!(
                resumed.collect::<Vec<_>>(),
                once[440..],
                "{name} stereo resume at5ms"
            );
            assert!(
                super::super::decode_sound(&bytes, *b"MOD ", 1, 0, |_| None).is_some(),
                "{name} inMOD chunk"
            );
        }
    }

    #[test]
    fn s3m_adlib_instruments_use_the_fm_synthesizer() {
        let bytes = fixtures::s3m_adlib();
        let pcm: Vec<_> = ModSource::new(&bytes, 1).unwrap().take(100_000).collect();
        assert_eq!(pcm.len(), 15_876);
        assert!(pcm[..4410].iter().any(|sample| sample.abs() > 0.001));
        let loud = pcm[..4410]
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        let quiet = pcm[7056..]
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        // S3M volume zero maps to the OPL chip's finite total-level
        // attenuation, so it is very quiet rather than digitally silent.
        assert!(quiet < loud * 0.01, "FM volume: loud {loud}, quiet {quiet}");
    }
}

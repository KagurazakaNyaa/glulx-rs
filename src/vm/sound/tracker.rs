//! ProTracker/SoundTracker MOD resources, rendered incrementally to stereo PCM.
//! Keeping the module with its borrowing player avoids expanding minutes of
//! tracker music into a large PCM allocation before playback can begin.
use rodio::Source;
use std::time::Duration;
use xmrs::prelude::Module;
use xmrsplayer::xmrsplayer::XmrsPlayer;

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
    remaining: u32,
}

impl ModSource {
    pub(super) fn new(bytes: &[u8], repeats: u32) -> Option<Self> {
        let module = Module::load_mod(bytes).ok()?;
        let player = OwnedPlayer::new(module, Self::player);
        Some(Self {
            player,
            remaining: repeats,
        })
    }

    fn player(module: &Module) -> XmrsPlayer<'_> {
        let mut player = XmrsPlayer::new(module, SAMPLE_RATE, 0);
        // The library folds tracker pattern loops into one song traversal.
        // End the traversal at a song restart; Glk's repeat count then controls
        // how many complete, freshly initialized traversals we play.
        player.set_max_loop_count(1);
        player
    }
}

impl Iterator for ModSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        if let Some(sample) = self.player.with_dependent_mut(|_, player| player.next()) {
            return Some(sample as f32 / 32768.0);
        }
        if self.remaining != u32::MAX {
            self.remaining -= 1;
        }
        if self.remaining == 0 {
            return None;
        }
        self.player
            .with_dependent_mut(|module, player| *player = Self::player(module));
        // An empty song must terminate even for infinite repetition.
        let sample = self.player.with_dependent_mut(|_, player| player.next());
        if sample.is_none() {
            self.remaining = 0;
        }
        sample.map(|sample| sample as f32 / 32768.0)
    }
}

impl Source for ModSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        2
    }
    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
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
        assert_eq!(source.channels(), 2);
        assert_eq!(source.sample_rate(), SAMPLE_RATE);
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
        assert!(super::super::decode_sound(&bytes, *b"MOD ", 1, 0).is_some());
        assert!(super::super::decode_sound(&bytes, *b"OGGV", 1, 0).is_none());
        assert!(super::super::decode_sound(&bytes[..600], *b"MOD ", 1, 0).is_none());
    }
}

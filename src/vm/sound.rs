use super::*;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::{
    io::Cursor,
    time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct AudioDevice {
    stream: Option<OutputStream>,
    handle: Option<OutputStreamHandle>,
}
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Channel {
    rock: u32,
    volume: u32,
    paused: bool,
    resource: u32,
    notify: u32,
    repeats: u32,
    position_ms: u64,
    active: bool,
    fade_resume: Option<(u32, u32, u32)>,
    #[serde(skip)]
    offset_ms: u64,
    #[serde(skip)]
    sink: Option<Sink>,
    #[serde(skip)]
    fade: Option<(Instant, Duration, u32, u32, u32)>,
}
impl Vm {
    pub fn enable_audio(&mut self) {
        if let Ok((stream, handle)) = OutputStream::try_default() {
            self.audio.stream = Some(stream);
            self.audio.handle = Some(handle);
        }
        let resumable: Vec<_> = self
            .channels
            .iter()
            .filter(|(_, c)| c.active)
            .map(|(&id, c)| (id, c.resource, c.repeats, c.notify, c.position_ms))
            .collect();
        for (id, resource, repeats, notify, position) in resumable {
            if !self.play_sound_at(id, resource, repeats, notify, position) && notify != 0 {
                self.events.push_back([7, 0, resource, notify]);
            }
        }
        for channel in self.channels.values_mut() {
            if let Some((millis, to, notify)) = channel.fade_resume.take() {
                channel.fade = Some((
                    Instant::now(),
                    Duration::from_millis(millis as u64),
                    channel.volume,
                    to,
                    notify,
                ));
            }
        }
    }
    pub(super) fn sound_available(&self) -> bool {
        self.audio.handle.is_some()
    }
    pub(super) fn poll_sound(&mut self) {
        for channel in self.channels.values_mut() {
            if let Some(sink) = &channel.sink {
                channel.position_ms = channel
                    .offset_ms
                    .saturating_add(sink.get_pos().as_millis().min(u64::MAX as u128) as u64);
            }
            if let Some((start, duration, from, to, notify)) = channel.fade {
                let elapsed = start.elapsed();
                let fraction = (elapsed.as_secs_f64() / duration.as_secs_f64()).min(1.0);
                channel.volume = (from as f64 + (to as f64 - from as f64) * fraction) as u32;
                if let Some(sink) = &channel.sink {
                    sink.set_volume(channel.volume as f32 / 65536.0);
                }
                channel.fade_resume = Some((
                    duration
                        .saturating_sub(elapsed)
                        .as_millis()
                        .min(u32::MAX as u128) as u32,
                    to,
                    notify,
                ));
                if elapsed >= duration {
                    channel.fade_resume = None;
                    channel.fade = None;
                    if notify != 0 {
                        self.events.push_back([9, 0, 0, notify]);
                    }
                }
            }
            if channel.sink.as_ref().is_some_and(Sink::empty) {
                channel.sink = None;
                channel.active = false;
                if channel.notify != 0 {
                    self.events
                        .push_back([7, 0, channel.resource, channel.notify]);
                    channel.notify = 0;
                }
            }
        }
    }
    fn play_sound(&mut self, id: u32, resource: u32, repeats: u32, notify: u32) -> bool {
        self.play_sound_at(id, resource, repeats, notify, 0)
    }
    fn play_sound_at(
        &mut self,
        id: u32,
        resource: u32,
        repeats: u32,
        notify: u32,
        offset_ms: u64,
    ) -> bool {
        let Some(channel) = self.channels.get_mut(&id) else {
            return false;
        };
        if let Some(sink) = channel.sink.take() {
            sink.stop();
        }
        channel.notify = 0;
        channel.active = false;
        if repeats == 0 {
            return true;
        }
        let Some(handle) = &self.audio.handle else {
            return false;
        };
        let Some(bytes) = self.story.sound_resource(resource) else {
            return false;
        };
        let Ok(decoder) = Decoder::new(Cursor::new(bytes.to_vec())) else {
            return false;
        };
        let sample_rate = decoder.sample_rate();
        let channels = decoder.channels();
        let samples: Vec<f32> = decoder.convert_samples().collect();
        if samples.is_empty() || sample_rate == 0 || channels == 0 {
            return false;
        }
        let duration =
            Duration::from_secs_f64(samples.len() as f64 / sample_rate as f64 / channels as f64);
        let source =
            rodio::buffer::SamplesBuffer::new(channels, sample_rate, samples).repeat_infinite();
        let Ok(sink) = Sink::try_new(handle) else {
            return false;
        };
        sink.set_volume(channel.volume as f32 / 65536.0);
        if channel.paused {
            sink.pause();
        }
        if repeats == u32::MAX {
            sink.append(source.skip_duration(Duration::from_millis(offset_ms)));
        } else {
            sink.append(
                source
                    .take_duration(duration.saturating_mul(repeats))
                    .skip_duration(Duration::from_millis(offset_ms)),
            );
        }
        channel.repeats = repeats;
        channel.position_ms = offset_ms;
        channel.offset_ms = offset_ms;
        channel.active = true;
        channel.resource = resource;
        channel.notify = if repeats == u32::MAX { 0 } else { notify };
        channel.sink = Some(sink);
        true
    }
    pub(super) fn sound_call(&mut self, selector: u32, args: &[u32]) -> Result<u32, VmError> {
        let arg = |n: usize| args.get(n).copied().unwrap_or(0);
        Ok(match selector {
            0xf0 => {
                let next = self
                    .channels
                    .range(arg(0).saturating_add(1)..)
                    .next()
                    .map(|(&id, c)| (id, c.rock))
                    .unwrap_or((0, 0));
                self.write_glk_reference(arg(1), next.1)?;
                next.0
            }
            0xf1 => self.channels.get(&arg(0)).map_or(0, |c| c.rock),
            0xf2 | 0xf4 => {
                if !self.sound_available() {
                    0
                } else {
                    let id = self.next_channel;
                    self.next_channel += 1;
                    self.channels.insert(
                        id,
                        Channel {
                            rock: arg(0),
                            volume: if selector == 0xf4 { arg(1) } else { 65536 },
                            paused: false,
                            resource: 0,
                            notify: 0,
                            repeats: 0,
                            position_ms: 0,
                            offset_ms: 0,
                            active: false,
                            fade_resume: None,
                            sink: None,
                            fade: None,
                        },
                    );
                    id
                }
            }
            0xf3 => {
                self.channels.remove(&arg(0));
                0
            }
            0xf8 | 0xf9 => u32::from(self.play_sound(
                arg(0),
                arg(1),
                if selector == 0xf8 { 1 } else { arg(2) },
                if selector == 0xf8 { 0 } else { arg(3) },
            )),
            0xf7 => {
                if arg(1) != arg(3) {
                    return Ok(0);
                }
                let mut sounds = Vec::new();
                for i in 0..arg(1) {
                    let channel = if arg(0) == u32::MAX {
                        self.stack.pop_u32()?
                    } else {
                        self.memory.read32(arg(0).wrapping_add(i * 4))?
                    };
                    sounds.push((channel, 0));
                }
                for (i, sound) in sounds.iter_mut().enumerate() {
                    sound.1 = if arg(2) == u32::MAX {
                        self.stack.pop_u32()?
                    } else {
                        self.memory.read32(arg(2).wrapping_add(i as u32 * 4))?
                    };
                }
                let states: Vec<_> = sounds
                    .iter()
                    .filter_map(|(id, _)| {
                        self.channels.get_mut(id).map(|channel| {
                            let paused = channel.paused;
                            channel.paused = true;
                            (*id, paused)
                        })
                    })
                    .collect();
                let count = sounds
                    .into_iter()
                    .filter(|(channel, sound)| self.play_sound(*channel, *sound, 1, arg(4)))
                    .count() as u32;
                for (id, paused) in states {
                    if let Some(channel) = self.channels.get_mut(&id) {
                        channel.paused = paused;
                        if !paused && let Some(sink) = &channel.sink {
                            sink.play();
                        }
                    }
                }
                count
            }
            0xfa => {
                if let Some(channel) = self.channels.get_mut(&arg(0)) {
                    channel.sink = None;
                    channel.active = false;
                    channel.notify = 0;
                }
                0
            }
            0xfb | 0xfd => {
                if let Some(channel) = self.channels.get_mut(&arg(0)) {
                    channel.fade = None;
                    channel.fade_resume = None;
                    if selector == 0xfd && arg(2) != 0 {
                        channel.fade = Some((
                            Instant::now(),
                            Duration::from_millis(arg(2) as u64),
                            channel.volume,
                            arg(1),
                            arg(3),
                        ));
                    } else {
                        channel.volume = arg(1);
                        if let Some(sink) = &channel.sink {
                            sink.set_volume(channel.volume as f32 / 65536.0);
                        }
                        if selector == 0xfd && arg(3) != 0 {
                            self.events.push_back([9, 0, 0, arg(3)]);
                        }
                    }
                }
                0
            }
            0xfe | 0xff => {
                if let Some(channel) = self.channels.get_mut(&arg(0)) {
                    channel.paused = selector == 0xfe;
                    if let Some(sink) = &channel.sink {
                        if channel.paused {
                            sink.pause();
                        } else {
                            sink.play();
                        }
                    }
                }
                0
            }
            _ => 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn channel() -> Channel {
        Channel {
            rock: 42,
            volume: 65536,
            paused: false,
            resource: 7,
            notify: 99,
            repeats: 1,
            position_ms: 0,
            active: true,
            fade_resume: None,
            offset_ms: 0,
            sink: None,
            fade: None,
        }
    }
    fn vm() -> Vm {
        Vm::new(
            Story::from_bytes(
                &super::super::tests::image_with_program(&[0x81, 0x20]),
                None,
            )
            .unwrap(),
        )
        .unwrap()
    }
    #[test]
    fn completion_stop_and_volume_notifications() {
        let mut vm = vm();
        let (sink, mut output) = Sink::new_idle();
        sink.append(rodio::buffer::SamplesBuffer::new(1, 8000, vec![0.0f32; 8]));
        let mut channel = channel();
        channel.sink = Some(sink);
        vm.channels.insert(1, channel);
        for _ in 0..64 {
            output.next();
        }
        vm.poll_sound();
        assert!(vm.events.contains(&[7, 0, 7, 99]));
        vm.events.clear();
        vm.sound_call(0xfa, &[1]).unwrap();
        vm.poll_sound();
        assert!(vm.events.is_empty());
        vm.channels.get_mut(&1).unwrap().fade = Some((
            Instant::now() - Duration::from_secs(1),
            Duration::from_millis(10),
            65536,
            0,
            12,
        ));
        vm.poll_sound();
        assert_eq!(vm.channels[&1].volume, 0);
        assert_eq!(vm.events.pop_front(), Some([9, 0, 0, 12]));
        assert_eq!(vm.sound_call(0xf8, &[1, 999]).unwrap(), 0);
        assert_eq!(vm.sound_call(0xf9, &[1, 999, 0, 55]).unwrap(), 1);
    }
}

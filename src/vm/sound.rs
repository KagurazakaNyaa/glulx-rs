use super::*;
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player as Sink, Source};
use std::{
    collections::BTreeMap,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

mod sampled;
mod song;
mod tracker;

type SoundOutput = rodio::queue::SourcesQueueOutput;
type PreparedSource = Box<dyn Source<Item = f32> + Send>;
const AUDIO_QUEUE_CAPACITY: usize = 2;

struct AudioDecodeWorker {
    sender: mpsc::SyncSender<AudioDecodeTask>,
    results: mpsc::Receiver<AudioDecodeResult>,
    next_id: u64,
}

struct AudioDecodeTask {
    id: u64,
    bytes: Vec<u8>,
    format: [u8; 4],
    repeats: u32,
    offset_ms: u64,
    limits: crate::memory::ResourceLimits,
}

struct AudioDecodeResult {
    id: u64,
    source: Result<PreparedSource, String>,
}

impl Default for AudioDecodeWorker {
    fn default() -> Self {
        let (task_sender, task_receiver) =
            mpsc::sync_channel::<AudioDecodeTask>(AUDIO_QUEUE_CAPACITY);
        let (result_sender, result_receiver) =
            mpsc::sync_channel::<AudioDecodeResult>(AUDIO_QUEUE_CAPACITY);
        thread::Builder::new()
            .name("glulx-audio-decode".to_owned())
            .spawn(move || {
                while let Ok(task) = task_receiver.recv() {
                    let source = decode_sound_with_limits(
                        &task.bytes,
                        task.format,
                        task.repeats,
                        task.offset_ms,
                        |_: u32| None,
                        task.limits,
                    )
                    .ok_or_else(|| "unsupported or invalid sound resource".to_owned());
                    if result_sender
                        .send(AudioDecodeResult {
                            id: task.id,
                            source,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .expect("audio decoder worker must start");
        Self {
            sender: task_sender,
            results: result_receiver,
            next_id: 0,
        }
    }
}

impl AudioDecodeWorker {
    fn submit(
        &mut self,
        bytes: Vec<u8>,
        format: [u8; 4],
        repeats: u32,
        offset_ms: u64,
        limits: crate::memory::ResourceLimits,
    ) -> Option<u64> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.sender
            .try_send(AudioDecodeTask {
                id,
                bytes,
                format,
                repeats,
                offset_ms,
                limits,
            })
            .ok()
            .map(|_| id)
    }

    fn poll(&self) -> impl Iterator<Item = AudioDecodeResult> + '_ {
        self.results.try_iter()
    }
}

/// One source submitted to the device for a whole play_multi call. Each idle
/// sink supplies exactly one sample at every mixer step, so decoding/setup time
/// cannot cause channels to start at different device positions.
struct AlignedSounds {
    outputs: Vec<SoundOutput>,
}

impl Iterator for AlignedSounds {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let mut mixed = 0.0;
        self.outputs.retain_mut(|output| {
            if let Some(sample) = output.next() {
                mixed += sample;
                true
            } else {
                false
            }
        });
        (!self.outputs.is_empty()).then_some(mixed)
    }
}

impl Source for AlignedSounds {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> rodio::ChannelCount {
        rodio::ChannelCount::new(2).unwrap()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        rodio::SampleRate::new(tracker::SAMPLE_RATE).unwrap()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
fn decode_sound<'a>(
    bytes: &[u8],
    format: [u8; 4],
    repeats: u32,
    offset_ms: u64,
    resource: impl FnMut(u32) -> Option<&'a [u8]>,
) -> Option<Box<dyn Source<Item = f32> + Send>> {
    decode_sound_with_limits(
        bytes,
        format,
        repeats,
        offset_ms,
        resource,
        Default::default(),
    )
}

fn decode_sound_with_limits<'a>(
    bytes: &[u8],
    format: [u8; 4],
    repeats: u32,
    offset_ms: u64,
    resource: impl FnMut(u32) -> Option<&'a [u8]>,
    limits: crate::memory::ResourceLimits,
) -> Option<Box<dyn Source<Item = f32> + Send>> {
    if bytes.len() > crate::memory::ResourceLimits::bytes(limits.audio_resource_mib) {
        return None;
    }
    if matches!(&format, b"MOD " | b"SONG") {
        let mut source = if format == *b"SONG" {
            tracker::ModSource::from_module(
                song::assemble_with_limit(
                    bytes,
                    resource,
                    crate::memory::ResourceLimits::bytes(limits.song_pcm_mib),
                )?,
                repeats,
            )
        } else {
            tracker::ModSource::new(bytes, repeats)?
        };
        source.skip_millis(offset_ms);
        return Some(Box::new(source));
    }
    Some(Box::new(sampled::SampledSource::new(
        bytes, repeats, offset_ms,
    )?))
}

#[derive(Default)]
pub(super) struct AudioDevice {
    stream: Option<MixerDeviceSink>,
    worker: Option<AudioDecodeWorker>,
    pending: BTreeMap<u64, PendingSound>,
}

struct PendingSound {
    channel: u32,
    resource: u32,
    repeats: u32,
    notify: u32,
    offset_ms: u64,
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
    #[serde(skip)]
    pending: Option<u64>,
}
impl Vm {
    pub fn enable_audio(&mut self) {
        if let Ok(mut stream) = DeviceSinkBuilder::open_default_sink() {
            stream.log_on_drop(false);
            self.audio.stream = Some(stream);
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
        self.audio.stream.is_some()
    }
    pub(crate) fn audio_needs_poll(&self) -> bool {
        self.channels
            .values()
            .any(|channel| channel.active || channel.fade.is_some())
    }
    pub(super) fn poll_sound(&mut self) {
        let results: Vec<_> = self
            .audio
            .worker
            .as_ref()
            .into_iter()
            .flat_map(AudioDecodeWorker::poll)
            .collect();
        for result in results {
            let Some(pending) = self.audio.pending.remove(&result.id) else {
                continue;
            };
            let mut output_to_add = None;
            let mut failed_notification = None;
            if let Some(channel) = self.channels.get_mut(&pending.channel) {
                if channel.pending != Some(result.id) {
                    continue;
                }
                channel.pending = None;
                match result.source {
                    Ok(source) => {
                        let (sink, output) = Sink::new();
                        sink.set_volume(channel.volume as f32 / 65536.0);
                        if channel.paused {
                            sink.pause();
                        }
                        sink.append(rodio::source::UniformSourceIterator::new(
                            source,
                            rodio::ChannelCount::new(2).unwrap(),
                            rodio::SampleRate::new(tracker::SAMPLE_RATE).unwrap(),
                        ));
                        channel.repeats = pending.repeats;
                        channel.position_ms = pending.offset_ms;
                        channel.offset_ms = pending.offset_ms;
                        channel.active = true;
                        channel.resource = pending.resource;
                        channel.notify = if pending.repeats == u32::MAX {
                            0
                        } else {
                            pending.notify
                        };
                        channel.sink = Some(sink);
                        output_to_add = Some(output);
                    }
                    Err(_) => {
                        channel.active = false;
                        channel.sink = None;
                        if channel.notify != 0 {
                            failed_notification = Some((channel.resource, channel.notify));
                        }
                        channel.notify = 0;
                    }
                }
            }
            if let Some(output) = output_to_add {
                if let Some(stream) = &self.audio.stream {
                    stream.mixer().add(AlignedSounds {
                        outputs: vec![output],
                    });
                } else {
                    self.stop_sound(pending.channel);
                }
            }
            if let Some((resource, notify)) = failed_notification {
                self.events.push_back([7, 0, resource, notify]);
            }
        }
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
        if let Some(format) = self.story.resource_type(*b"Snd ", resource)
            && format != *b"SONG"
        {
            return self.queue_sound_at(id, resource, format, repeats, notify, offset_ms);
        }
        let Ok(output) = self.prepare_sound_at(id, resource, repeats, notify, offset_ms) else {
            return false;
        };
        let Some(output) = output else {
            return true;
        };
        if let Some(stream) = &self.audio.stream {
            stream.mixer().add(AlignedSounds {
                outputs: vec![output],
            });
            true
        } else {
            self.stop_sound(id);
            false
        }
    }

    fn queue_sound_at(
        &mut self,
        id: u32,
        resource: u32,
        format: [u8; 4],
        repeats: u32,
        notify: u32,
        offset_ms: u64,
    ) -> bool {
        if !self.channels.contains_key(&id) {
            return false;
        }
        self.stop_sound(id);
        if repeats == 0 {
            return true;
        }
        if self.audio.stream.is_none() {
            return false;
        }
        let Some(bytes) = self.story.sound_resource(resource) else {
            return false;
        };
        let limits = self.resource_limits;
        let worker = self.audio.worker.get_or_insert_with(Default::default);
        let Some(request) = worker.submit(bytes.to_vec(), format, repeats, offset_ms, limits)
        else {
            return false;
        };
        let Some(channel) = self.channels.get_mut(&id) else {
            return false;
        };
        channel.repeats = repeats;
        channel.position_ms = offset_ms;
        channel.offset_ms = offset_ms;
        channel.active = true;
        channel.resource = resource;
        channel.notify = if repeats == u32::MAX { 0 } else { notify };
        channel.pending = Some(request);
        self.audio.pending.insert(
            request,
            PendingSound {
                channel: id,
                resource,
                repeats,
                notify,
                offset_ms,
            },
        );
        true
    }

    fn stop_sound(&mut self, id: u32) {
        let pending = self
            .channels
            .get_mut(&id)
            .and_then(|channel| channel.pending.take());
        if let Some(pending) = pending {
            self.audio.pending.remove(&pending);
        }
        if let Some(channel) = self.channels.get_mut(&id) {
            channel.sink = None;
            channel.active = false;
            channel.notify = 0;
        }
    }

    fn prepare_sound_at(
        &mut self,
        id: u32,
        resource: u32,
        repeats: u32,
        notify: u32,
        offset_ms: u64,
    ) -> Result<Option<SoundOutput>, ()> {
        let pending = self
            .channels
            .get_mut(&id)
            .and_then(|channel| channel.pending.take());
        if let Some(pending) = pending {
            self.audio.pending.remove(&pending);
        }
        let Some(channel) = self.channels.get_mut(&id) else {
            return Err(());
        };
        if let Some(sink) = channel.sink.take() {
            sink.stop();
        }
        channel.notify = 0;
        channel.active = false;
        if repeats == 0 {
            return Ok(None);
        }
        if self.audio.stream.is_none() {
            return Err(());
        }
        let bytes = self.story.sound_resource(resource).ok_or(())?;
        let format = self.story.resource_type(*b"Snd ", resource).ok_or(())?;
        let _stage = crate::diagnostics::stage("audio-prepare");
        let source = decode_sound_with_limits(
            bytes,
            format,
            repeats,
            offset_ms,
            |number| {
                if self.story.resource_type(*b"Snd ", number)? != *b"FORM" {
                    return None;
                }
                self.story.sound_resource(number).filter(|data| {
                    data.len()
                        <= crate::memory::ResourceLimits::bytes(
                            self.resource_limits.audio_resource_mib,
                        )
                })
            },
            self.resource_limits,
        )
        .ok_or(())?;
        let (sink, output) = Sink::new();
        sink.set_volume(channel.volume as f32 / 65536.0);
        if channel.paused {
            sink.pause();
        }
        sink.append(rodio::source::UniformSourceIterator::new(
            source,
            rodio::ChannelCount::new(2).unwrap(),
            rodio::SampleRate::new(tracker::SAMPLE_RATE).unwrap(),
        ));
        channel.repeats = repeats;
        channel.position_ms = offset_ms;
        channel.offset_ms = offset_ms;
        channel.active = true;
        channel.resource = resource;
        channel.notify = if repeats == u32::MAX { 0 } else { notify };
        channel.sink = Some(sink);
        channel.pending = None;
        Ok(Some(output))
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
                            pending: None,
                        },
                    );
                    id
                }
            }
            0xf3 => {
                self.stop_sound(arg(0));
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
                let mut started = Vec::new();
                let mut outputs = Vec::new();
                for (id, resource) in sounds {
                    if let Ok(Some(output)) = self.prepare_sound_at(id, resource, 1, arg(4), 0) {
                        started.push(id);
                        outputs.push(output);
                    }
                }
                if !outputs.is_empty()
                    && self.audio.stream.as_ref().is_some_and(|stream| {
                        stream.mixer().add(AlignedSounds { outputs });
                        true
                    })
                {
                    started.len() as u32
                } else {
                    for id in started {
                        self.stop_sound(id);
                    }
                    0
                }
            }
            0xfa => {
                self.stop_sound(arg(0));
                0
            }
            0xfb | 0xfd => {
                // Settle elapsed fades before replacing them: the new fade starts
                // at the current volume, and an already completed fade keeps its event.
                self.poll_sound();
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
            pending: None,
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
    fn sound_gestalts_all_require_an_audio_device() {
        let vm = vm();
        assert!(!vm.sound_available());
        for selector in [8, 9, 10, 13, 21] {
            assert_eq!(vm.glk_gestalt(selector, 0), 0, "gestalt {selector}");
        }
    }

    #[test]
    fn active_audio_requests_frequent_host_polls() {
        let mut vm = vm();
        assert!(!vm.audio_needs_poll());
        let mut channel = channel();
        channel.active = true;
        vm.channels.insert(1, channel);
        assert!(vm.audio_needs_poll());
        vm.channels.get_mut(&1).unwrap().active = false;
        vm.channels.get_mut(&1).unwrap().fade =
            Some((Instant::now(), Duration::from_secs(1), 1, 2, 0));
        assert!(vm.audio_needs_poll());
    }

    #[test]
    fn multi_sounds_begin_on_the_same_stereo_frame() {
        let (left_sink, left) = Sink::new();
        let (right_sink, right) = Sink::new();
        left_sink.append(rodio::buffer::SamplesBuffer::new(
            rodio::ChannelCount::new(2).unwrap(),
            rodio::SampleRate::new(tracker::SAMPLE_RATE).unwrap(),
            vec![0.25f32, 0.0, 0.0, 0.0],
        ));
        right_sink.append(rodio::buffer::SamplesBuffer::new(
            rodio::ChannelCount::new(2).unwrap(),
            rodio::SampleRate::new(tracker::SAMPLE_RATE).unwrap(),
            vec![0.0f32, 0.5, 0.0, 0.0],
        ));
        let mut mixed = AlignedSounds {
            outputs: vec![left, right],
        };
        assert_eq!(
            mixed.by_ref().take(4).collect::<Vec<_>>(),
            [0.25, 0.5, 0.0, 0.0]
        );
        mixed.next();
        assert!(left_sink.empty());
        assert!(right_sink.empty());
        drop((left_sink, right_sink));
        assert!(mixed.take(4096).count() < 4096);
    }

    #[test]
    fn multi_sounds_keep_independent_pause_and_volume_controls() {
        let (quiet_sink, quiet) = Sink::new();
        let (paused_sink, paused) = Sink::new();
        quiet_sink.set_volume(0.5);
        paused_sink.pause();
        for sink in [&quiet_sink, &paused_sink] {
            sink.append(rodio::buffer::SamplesBuffer::new(
                rodio::ChannelCount::new(2).unwrap(),
                rodio::SampleRate::new(tracker::SAMPLE_RATE).unwrap(),
                vec![0.5f32; 2048],
            ));
        }
        let mut mixed = AlignedSounds {
            outputs: vec![quiet, paused],
        };
        assert_eq!(mixed.next(), Some(0.25));
        paused_sink.play();
        // Rodio refreshes each channel's controls every five milliseconds.
        assert_eq!(mixed.nth(1023), Some(0.75));
        drop(quiet_sink);
        assert_eq!(mixed.nth(1023), Some(0.5));
    }
    #[test]
    fn completion_stop_and_volume_notifications() {
        let mut vm = vm();
        let (sink, mut output) = Sink::new();
        sink.append(rodio::buffer::SamplesBuffer::new(
            rodio::ChannelCount::new(1).unwrap(),
            rodio::SampleRate::new(8000).unwrap(),
            vec![0.0f32; 8],
        ));
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

    #[test]
    fn interrupted_fade_starts_at_elapsed_volume_and_only_notifies_for_replacement() {
        let mut vm = vm();
        let mut channel = channel();
        channel.fade = Some((
            Instant::now() - Duration::from_secs(25),
            Duration::from_secs(100),
            65536,
            0,
            11,
        ));
        vm.channels.insert(1, channel);

        vm.sound_call(0xfd, &[1, 16384, 10000, 22]).unwrap();
        let channel = &vm.channels[&1];
        let (_, duration, from, to, notify) = channel.fade.unwrap();
        // A quarter-complete fade from full volume to silence is at 75%.
        assert!(from.abs_diff(49152) < 256, "new fade starts at {from}");
        assert_eq!(from, channel.volume);
        assert_eq!((duration, to, notify), (Duration::from_secs(10), 16384, 22));
        assert!(vm.events.is_empty());

        vm.channels.get_mut(&1).unwrap().fade.as_mut().unwrap().0 =
            Instant::now() - Duration::from_secs(20);
        vm.poll_sound();
        assert_eq!(vm.channels[&1].volume, 16384);
        assert_eq!(vm.events.into_iter().collect::<Vec<_>>(), [[9, 0, 0, 22]]);
    }

    #[test]
    fn completed_fade_keeps_notification_when_volume_is_replaced_before_poll() {
        for selector in [0xfb, 0xfd] {
            let mut vm = vm();
            let mut channel = channel();
            channel.fade = Some((
                Instant::now() - Duration::from_secs(1),
                Duration::from_millis(10),
                65536,
                32768,
                11,
            ));
            vm.channels.insert(1, channel);

            vm.sound_call(selector, &[1, 1000, 0, 22]).unwrap();
            assert_eq!(vm.channels[&1].volume, 1000);
            assert!(vm.channels[&1].fade.is_none());
            let mut expected = vec![[9, 0, 0, 11]];
            if selector == 0xfd {
                expected.push([9, 0, 0, 22]);
            }
            assert_eq!(vm.events.iter().copied().collect::<Vec<_>>(), expected);
            vm.poll_sound();
            assert_eq!(vm.events.into_iter().collect::<Vec<_>>(), expected);
        }
    }
}

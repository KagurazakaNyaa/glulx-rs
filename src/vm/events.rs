use super::*;
use std::time::{Duration, Instant};

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub(super) enum Request {
    Line(LineRequest),
    Character { unicode: bool },
}

impl Vm {
    pub fn pending_input_windows(&self) -> Vec<u32> {
        self.requests.keys().copied().collect()
    }
    pub fn select_input_window(&mut self, window: u32) {
        if self.pending_select.is_some()
            && let Some(request) = self.requests.get(&window)
        {
            self.input_window = window;
            self.state = match request {
                Request::Line(_) => RunState::WaitingForLine,
                Request::Character { .. } => RunState::WaitingForChar,
            };
        }
    }
    pub fn update_line_input(&mut self, text: &str) -> Result<(), VmError> {
        if let Some(Request::Line(request)) = self.requests.get_mut(&self.input_window) {
            request.initial = text.chars().take(request.max_len as usize).collect();
            for (i, c) in request.initial.chars().enumerate() {
                if request.unicode {
                    self.memory
                        .write32(request.buffer + i as u32 * 4, c as u32)?;
                } else {
                    self.memory.write8(
                        request.buffer + i as u32,
                        if c as u32 > 255 { b'?' } else { c as u8 },
                    )?;
                }
            }
        }
        Ok(())
    }
    pub fn input_window(&self) -> u32 {
        self.input_window
    }
    pub fn initial_input(&self) -> String {
        match self.requests.get(&self.input_window) {
            Some(Request::Line(request)) => request.initial.clone(),
            _ => String::new(),
        }
    }
    pub fn line_terminators(&self) -> &[u32] {
        self.glk_windows
            .get(&self.input_window)
            .map_or(&[], |w| w.terminators.as_slice())
    }
    pub fn provide_terminated_input(&mut self, text: &str, terminator: u32) -> Result<(), VmError> {
        if terminator != 0 && !self.line_terminators().contains(&terminator) {
            return Err(VmError::UnexpectedInput);
        }
        self.complete_window_input(self.input_window, text, terminator)
    }
    pub fn provide_window_input(&mut self, window: u32, text: &str) -> Result<(), VmError> {
        self.complete_window_input(window, text, 0)
    }
    fn complete_window_input(
        &mut self,
        window: u32,
        text: &str,
        terminator: u32,
    ) -> Result<(), VmError> {
        if self.pending_select.is_none() {
            return Err(VmError::UnexpectedInput);
        }
        let request = self
            .requests
            .remove(&window)
            .ok_or(VmError::UnexpectedInput)?;
        let event = match request {
            Request::Line(request) => {
                let mut length = 0;
                for character in text.chars().take(request.max_len as usize) {
                    if request.unicode {
                        self.memory
                            .write32(request.buffer + length * 4, character as u32)?;
                    } else {
                        self.memory.write8(
                            request.buffer + length,
                            if character as u32 > 255 {
                                b'?'
                            } else {
                                character as u8
                            },
                        )?;
                    }
                    length += 1;
                }
                if request.echo
                    && let Some(stream) = self.glk_windows.get(&window).map(|w| w.stream)
                {
                    let output_len = self.output.len();
                    let old_style = self.glk_windows.get(&window).map_or(0, |w| w.style);
                    self.style_call(0x87, &[stream, 8])?;
                    for character in text.chars().take(request.max_len as usize) {
                        self.glk_write_char(stream, character);
                    }
                    self.glk_write_char(stream, '\n');
                    self.style_call(0x87, &[stream, old_style])?;
                    self.output.truncate(output_len);
                }
                [3, window, length, terminator]
            }
            Request::Character { unicode } => {
                let character = text.chars().next().unwrap_or('\n') as u32;
                let key = if character == 10 || character == 13 {
                    0xffff_fffa
                } else if !unicode && character > 255 {
                    b'?' as u32
                } else {
                    character
                };
                [2, window, key, 0]
            }
        };
        self.events.push_back(event);
        self.poll_events()
    }
    pub fn poll_events(&mut self) -> Result<(), VmError> {
        self.poll_sound();
        if let Some((interval, next)) = &mut self.timer
            && Instant::now() >= *next
        {
            if !self.events.iter().any(|e| e[0] == 1) {
                self.events.push_back([1, 0, 0, 0]);
            }
            *next = Instant::now() + *interval;
        }
        if self.pending_select.is_some()
            && let Some(event) = self.events.pop_front()
        {
            let select = self.pending_select.take().unwrap();
            self.write_event(select.event_address, event)?;
            self.store_destination(&select.destination, 0, Width::Word)?;
            self.state = RunState::Running;
        }
        Ok(())
    }
    pub fn resize_windows(&mut self, width: u32, height: u32) {
        let size = [width.min(16384), height.min(16384)];
        if self.viewport_size == size {
            return;
        }
        self.viewport_size = size;
        self.layout_windows();
        if self.glk_root != 0 && !self.events.iter().any(|e| e[0] == 5) {
            self.events.push_back([5, 0, 0, 0]);
        }
    }
    pub(super) fn write_event(&mut self, address: u32, event: [u32; 4]) -> Result<(), VmError> {
        for (i, value) in event.into_iter().enumerate() {
            self.write_glk_reference(
                if matches!(address, 0 | u32::MAX) {
                    address
                } else {
                    address.wrapping_add(i as u32 * 4)
                },
                value,
            )?;
        }
        Ok(())
    }
    pub(super) fn select_event(
        &mut self,
        address: u32,
        destination: Destination,
    ) -> Result<(), VmError> {
        self.pending_select = Some(PendingSelect {
            event_address: address,
            destination,
        });
        self.state = RunState::WaitingForEvent;
        if let Some((&window, request)) = self.requests.first_key_value() {
            self.input_window = window;
            self.state = match request {
                Request::Line(_) => RunState::WaitingForLine,
                Request::Character { .. } => RunState::WaitingForChar,
            };
        }
        self.poll_events()
    }
    pub(super) fn request_line(&mut self, args: &[u32], unicode: bool) -> Result<(), VmError> {
        let window = args.first().copied().unwrap_or(0);
        let buffer = args.get(1).copied().unwrap_or(0);
        let max_len = args.get(2).copied().unwrap_or(0);
        let init_len = args.get(3).copied().unwrap_or(0).min(max_len);
        let mut initial = String::new();
        for i in 0..init_len {
            let value = if unicode {
                self.memory.read32(buffer + i * 4)?
            } else {
                self.memory.read8(buffer + i)? as u32
            };
            initial.push(char::from_u32(value).unwrap_or('\u{fffd}'));
        }
        self.requests
            .entry(window)
            .or_insert(Request::Line(LineRequest {
                buffer,
                max_len,
                unicode,
                initial,
                echo: self.glk_windows.get(&window).is_none_or(|w| w.echo_line),
            }));
        Ok(())
    }
    pub(super) fn cancel_line(&mut self, args: &[u32]) -> Result<(), VmError> {
        let window = args.first().copied().unwrap_or(0);
        let event = if let Some(Request::Line(request)) = self.requests.get(&window) {
            let length = request.initial.chars().count() as u32;
            self.requests.remove(&window);
            [3, window, length, 0]
        } else {
            [0, 0, 0, 0]
        };
        self.write_event(args.get(1).copied().unwrap_or(0), event)
    }
    pub(super) fn request_timer(&mut self, millis: u32) {
        self.timer = if millis == 0 {
            None
        } else {
            let duration = Duration::from_millis(millis as u64);
            Some((duration, Instant::now() + duration))
        };
        self.events.retain(|e| e[0] != 1);
    }
}

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
        let maximum = self.line_input_max_len();
        if let Some(Request::Line(request)) = self.requests.get_mut(&self.input_window) {
            request.initial = text.chars().take(maximum as usize).collect();
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
        match self.requests.get(&self.input_window) {
            Some(Request::Line(request)) => &request.terminators,
            _ => &[],
        }
    }
    pub fn line_input_max_len(&self) -> u32 {
        match self.requests.get(&self.input_window) {
            Some(Request::Line(request)) => {
                self.effective_line_max_len(self.input_window, request.max_len)
            }
            _ => 0,
        }
    }
    fn effective_line_max_len(&self, window: u32, maximum: u32) -> u32 {
        match self.glk_windows.get(&window) {
            Some(w) if w.kind == WINTYPE_TEXT_GRID => maximum.min(if w.cursor_y < w.height {
                w.width.saturating_sub(w.cursor_x.saturating_add(1))
            } else {
                0
            }),
            _ => maximum,
        }
    }
    pub fn is_grid_line_input(&self) -> bool {
        matches!(self.input_request(), Some(InputRequest::Line { .. }))
            && self
                .glk_windows
                .get(&self.input_window)
                .is_some_and(|w| w.kind == WINTYPE_TEXT_GRID)
    }
    /// Deliver a character or one of Glk's special keycodes to the selected
    /// window. Special keys stay intact for both Latin-1 and Unicode requests.
    pub fn provide_key(&mut self, key: u32) -> Result<(), VmError> {
        self.provide_window_key(self.input_window, key)
    }
    pub fn provide_window_key(&mut self, window: u32, key: u32) -> Result<(), VmError> {
        if self.pending_select.is_none() {
            return Err(VmError::UnexpectedInput);
        }
        let Some(Request::Character { unicode }) = self.requests.get(&window) else {
            return Err(VmError::UnexpectedInput);
        };
        let key = if matches!(key, 10 | 13) {
            0xffff_fffa
        } else if is_special_key(key) {
            key
        } else if char::from_u32(key).is_none() {
            0xffff_ffff // keycode_Unknown
        } else if !unicode && key > 255 {
            b'?' as u32
        } else {
            key
        };
        self.requests.remove(&window);
        self.events.push_back([2, window, key, 0]);
        self.poll_events()
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
        if matches!(self.requests.get(&window), Some(Request::Character { .. })) {
            return self.provide_window_key(window, text.chars().next().unwrap_or('\n') as u32);
        }
        let Some(Request::Line(request)) = self.requests.remove(&window) else {
            return Err(VmError::UnexpectedInput);
        };
        let event = self.finish_line(window, request, text, terminator)?;
        self.events.push_back(event);
        self.poll_events()
    }
    fn finish_line(
        &mut self,
        window: u32,
        request: LineRequest,
        text: &str,
        terminator: u32,
    ) -> Result<[u32; 4], VmError> {
        let maximum = self.effective_line_max_len(window, request.max_len);
        let text: String = text.chars().take(maximum as usize).collect();
        let mut length = 0;
        for character in text.chars() {
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
        if let Some(w) = self.glk_windows.get(&window) {
            let (stream, echo, old_style, old_link, old_count) =
                (w.stream, w.echo_stream, w.style, w.hyperlink, w.write_count);
            // Input display is not a put_char call on the window stream. Its
            // count and current style/link must survive input unchanged.
            self.glk_windows.get_mut(&window).unwrap().echo_stream = 0;
            let output_len = self.output.len();
            if request.echo {
                self.style_call(0x87, &[stream, 8])?;
                self.glk_windows.get_mut(&window).unwrap().hyperlink = 0;
                for character in text.chars() {
                    self.glk_write_char(stream, character);
                }
                self.style_call(0x87, &[stream, old_style])?;
                self.glk_write_char(stream, '\n');
            }
            let w = self.glk_windows.get_mut(&window).unwrap();
            w.echo_stream = echo;
            w.hyperlink = old_link;
            w.write_count = old_count;
            self.output.truncate(output_len);
            // A transcript still receives input when display echo is disabled.
            if echo != 0 {
                self.style_call(0x87, &[echo, 8])?;
                for character in text.chars() {
                    self.glk_write_char(echo, character);
                }
                self.style_call(0x87, &[echo, old_style])?;
                self.glk_write_char(echo, '\n');
            }
        }
        Ok([3, window, length, terminator])
    }
    pub(super) fn select_poll(&mut self, address: u32) -> Result<(), VmError> {
        self.poll_events()?;
        let event = self
            .events
            .iter()
            .position(|event| !matches!(event[0], 2 | 3 | 4 | 8))
            .and_then(|index| self.events.remove(index))
            .unwrap_or([0, 0, 0, 0]);
        self.write_event(address, event)
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
        let limits: Vec<_> = self
            .requests
            .iter()
            .filter_map(|(&window, request)| {
                if let Request::Line(request) = request {
                    Some((window, self.effective_line_max_len(window, request.max_len)))
                } else {
                    None
                }
            })
            .collect();
        for (window, maximum) in limits {
            if let Some(Request::Line(request)) = self.requests.get_mut(&window) {
                request.initial = request.initial.chars().take(maximum as usize).collect();
            }
        }
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
        if let Some(w) = self.glk_windows.get_mut(&window)
            && w.kind == WINTYPE_TEXT_GRID
        {
            w.normalize_grid_cursor();
        }
        if buffer < self.memory.ram_start()
            || max_len
                .checked_mul(if unicode { 4 } else { 1 })
                .and_then(|length| buffer.checked_add(length))
                .is_none_or(|end| end > self.memory.len())
        {
            return Err(VmError::MemoryWrite(buffer));
        }
        let init_len = args
            .get(3)
            .copied()
            .unwrap_or(0)
            .min(self.effective_line_max_len(window, max_len));
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
                echo: self
                    .glk_windows
                    .get(&window)
                    .is_none_or(|w| w.kind != WINTYPE_TEXT_BUFFER || w.echo_line),
                terminators: self
                    .glk_windows
                    .get(&window)
                    .map_or_else(Vec::new, |w| w.terminators.clone()),
            }));
        Ok(())
    }
    pub(super) fn cancel_line(&mut self, args: &[u32]) -> Result<(), VmError> {
        let window = args.first().copied().unwrap_or(0);
        let event = if let Some(Request::Line(request)) = self.requests.get(&window).cloned() {
            self.requests.remove(&window);
            let text = request.initial.clone();
            self.finish_line(window, request, &text, 0)?
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

/// Glk reserves named special keycodes independently of Unicode validity.
pub(super) fn is_special_key(key: u32) -> bool {
    matches!(key, 0xffff_fff3..=0xffff_ffff | 0xffff_ffe4..=0xffff_ffef)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::tests::{image_with_program, push_glk_arguments};

    fn vm() -> Vm {
        Vm::new(Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap()).unwrap()
    }
    fn glk(vm: &mut Vm, selector: u32, args: &[u32]) -> u32 {
        push_glk_arguments(vm, args);
        vm.glk(selector, args.len() as u32, Destination::Stack)
            .unwrap();
        vm.stack.pop_u32().unwrap()
    }

    #[test]
    fn poll_returns_internal_events_and_preserves_pending_player_events() {
        let mut vm = vm();
        let players = [[4, 1, 2, 3], [8, 1, 99, 0], [3, 2, 5, 0], [2, 1, 65, 0]];
        vm.events.extend(players);
        vm.events.extend([[1, 0, 0, 0], [7, 0, 8, 42]]);
        for expected in [1, 7, 0] {
            glk(&mut vm, 0xc1, &[0x100]);
            assert_eq!(vm.memory.read32(0x100).unwrap(), expected);
        }
        assert_eq!(vm.events.iter().copied().collect::<Vec<_>>(), players);
        for event in players {
            vm.select_event(0x100, Destination::Discard).unwrap();
            for (index, value) in event.into_iter().enumerate() {
                assert_eq!(vm.memory.read32(0x100 + index as u32 * 4).unwrap(), value);
            }
        }
    }

    #[test]
    fn cancelled_input_stores_composition_and_echoes_without_counting_as_output() {
        let mut vm = vm();
        let window = glk(&mut vm, 0x23, &[0, 0, 0, 3, 0]);
        let stream = vm.glk_windows[&window].stream;
        let transcript = glk(&mut vm, 0x43, &[0x180, 32, 1, 0]);
        glk(&mut vm, 0x2d, &[window, transcript]);
        glk(&mut vm, 0x87, &[stream, 1]);
        glk(&mut vm, 0x101, &[stream, 99]);
        vm.glk_write_char(stream, '>');
        vm.request_line(&[window, 0x100, 8, 0], false).unwrap();
        vm.select_event(0x140, Destination::Discard).unwrap();
        vm.update_line_input("é中").unwrap();
        vm.cancel_line(&[window, 0x120]).unwrap();
        assert_eq!(vm.memory.read8(0x100).unwrap(), 0xe9);
        assert_eq!(vm.memory.read8(0x101).unwrap(), b'?');
        assert_eq!(vm.memory.read32(0x128).unwrap(), 2);
        let window = &vm.glk_windows[&window];
        assert_eq!(
            (window.write_count, window.style, window.hyperlink),
            (1, 1, 99)
        );
        assert_eq!(
            window
                .runs
                .iter()
                .map(|r| r.text.as_str())
                .collect::<String>(),
            ">é中\n"
        );
        assert_eq!((window.runs[1].style, window.runs[1].hyperlink), (8, 0));
        assert_eq!(window.runs.last().unwrap().style, 1);
        assert_eq!(vm.close_stream(transcript), (0, 4));
        assert_eq!(
            (0..4)
                .map(|i| vm.memory.read8(0x180 + i).unwrap())
                .collect::<Vec<_>>(),
            b">\xe9?\n"
        );
    }

    #[test]
    fn echo_and_terminator_configuration_apply_to_subsequent_requests() {
        let mut vm = vm();
        let window = glk(&mut vm, 0x23, &[0, 0, 0, 3, 0]);
        let transcript = glk(&mut vm, 0x43, &[0x180, 32, 1, 0]);
        glk(&mut vm, 0x2d, &[window, transcript]);
        vm.glk_windows.get_mut(&window).unwrap().terminators = vec![0xffff_fff8];
        vm.request_line(&[window, 0x100, 8, 0], true).unwrap();
        vm.select_event(0x140, Destination::Discard).unwrap();
        glk(&mut vm, 0x150, &[window, 0]);
        glk(&mut vm, 0x151, &[window, 0, 0]);
        assert_eq!(vm.line_terminators(), &[0xffff_fff8]);
        vm.provide_terminated_input("first", 0xffff_fff8).unwrap();
        vm.request_line(&[window, 0x100, 8, 0], true).unwrap();
        vm.select_event(0x140, Destination::Discard).unwrap();
        assert!(vm.line_terminators().is_empty());
        vm.update_line_input("second").unwrap();
        vm.cancel_line(&[window, 0]).unwrap();
        assert_eq!(
            vm.glk_windows[&window]
                .runs
                .iter()
                .map(|r| r.text.as_str())
                .collect::<String>(),
            "first\n"
        );
        assert_eq!(vm.close_stream(transcript), (0, 13));
        assert_eq!(
            (0..13)
                .map(|i| vm.memory.read8(0x180 + i).unwrap())
                .collect::<Vec<_>>(),
            b"first\nsecond\n"
        );
    }

    #[test]
    fn grid_input_stops_at_right_edge_and_cancellation_advances_row() {
        let mut vm = vm();
        vm.resize_windows(80, 48);
        let window = glk(&mut vm, 0x23, &[0, 0, 0, 4, 0]);
        let width = vm.glk_windows[&window].width;
        glk(&mut vm, 0x2b, &[window, width - 4, 0]);
        glk(&mut vm, 0x150, &[window, 0]);
        vm.request_line(&[window, 0x100, 8, 0], false).unwrap();
        vm.select_event(0x140, Destination::Discard).unwrap();
        assert!(vm.is_grid_line_input());
        assert_eq!(vm.line_input_max_len(), 3);
        vm.update_line_input("abcdef").unwrap();
        vm.cancel_line(&[window, 0x120]).unwrap();
        assert_eq!(vm.memory.read32(0x128).unwrap(), 3);
        assert_eq!(
            (0..3)
                .map(|i| vm.memory.read8(0x100 + i).unwrap())
                .collect::<Vec<_>>(),
            b"abc"
        );
        let window = &vm.glk_windows[&window];
        assert_eq!((window.cursor_x, window.cursor_y), (0, 1));
        assert_eq!(
            window.grid[(width - 4) as usize..(width - 1) as usize],
            ['a', 'b', 'c']
        );
    }

    #[test]
    fn character_special_keys_and_latin1_conversion_keep_event_contract() {
        let mut vm = vm();
        let window = glk(&mut vm, 0x23, &[0, 0, 0, 5, 0]);
        for key in [
            0xffff_fffe,
            0xffff_fff9,
            0xffff_fff3,
            0xffff_ffe4,
            'é' as u32,
        ] {
            assert_eq!(vm.glk_gestalt(1, key), 1);
            glk(&mut vm, 0xd2, &[window]);
            vm.select_event(0x100, Destination::Discard).unwrap();
            vm.provide_key(key).unwrap();
            assert_eq!(vm.memory.read32(0x100).unwrap(), 2);
            assert_eq!(vm.memory.read32(0x108).unwrap(), key);
            assert_eq!(vm.memory.read32(0x10c).unwrap(), 0);
        }
        for (selector, expected) in [(0xd2, b'?' as u32), (0x140, '中' as u32)] {
            glk(&mut vm, selector, &[window]);
            vm.select_event(0x100, Destination::Discard).unwrap();
            vm.provide_key('中' as u32).unwrap();
            assert_eq!(vm.memory.read32(0x108).unwrap(), expected);
        }
        vm.set_graphical_host(false);
        assert_eq!(vm.glk_gestalt(1, 0xffff_fffe), 0);
        assert_eq!(vm.glk_gestalt(1, 0xffff_fffa), 1);
    }

    #[test]
    fn line_request_rejects_wrapping_buffer_before_retaining_it() {
        let mut vm = vm();
        assert!(vm.request_line(&[1, 0x100, u32::MAX, 0], true).is_err());
        assert!(vm.requests.is_empty());
    }
    #[test]
    fn resizing_a_grid_updates_composition_limit_without_forgetting_requested_capacity() {
        let mut vm = vm();
        vm.resize_windows(80, 48);
        let window = glk(&mut vm, 0x23, &[0, 0, 0, 4, 0]);
        vm.request_line(&[window, 0x100, 20, 0], false).unwrap();
        vm.select_event(0x140, Destination::Discard).unwrap();
        vm.update_line_input("abcdefghi").unwrap();
        vm.resize_windows(40, 48);
        assert_eq!(vm.line_input_max_len(), 4);
        assert_eq!(vm.initial_input(), "abcd");
        vm.resize_windows(120, 48);
        assert_eq!(vm.line_input_max_len(), 14);
        vm.update_line_input("abcdefghijklmn").unwrap();
        vm.cancel_line(&[window, 0x120]).unwrap();
        assert_eq!(vm.memory.read32(0x128).unwrap(), 14);
    }
}

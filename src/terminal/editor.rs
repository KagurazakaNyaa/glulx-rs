use super::*;
use crossterm::event::KeyEvent;

pub(super) struct Editor {
    pub window: u32,
    pub text: String,
    pub cursor: usize,
    file: bool,
    maximum: usize,
}

impl Editor {
    pub fn synchronize(vm: &Vm, editor: &mut Option<Self>) -> bool {
        let request = vm.input_request();
        let file = matches!(request, Some(InputRequest::File { .. }));
        let maximum = match request {
            Some(InputRequest::Line { maximum_length }) => maximum_length as usize,
            Some(InputRequest::File { .. }) => 16384,
            _ => return editor.take().is_some(),
        };
        let initial = if file {
            String::new()
        } else {
            vm.initial_input()
        };
        if let Some(current) = editor
            && current.window == vm.input_window()
            && current.file == file
        {
            current.maximum = maximum;
            if file || current.text == initial {
                return false;
            }
            current.text = initial;
            current.cursor = current.text.chars().count();
        } else {
            *editor = Some(Self {
                window: vm.input_window(),
                cursor: initial.chars().count(),
                text: initial,
                file,
                maximum,
            });
        }
        true
    }

    fn update(&self, vm: &mut Vm) -> Result<(), crate::VmError> {
        if !self.file {
            vm.update_line_input(&self.text)?;
        }
        Ok(())
    }

    pub fn paste(&mut self, vm: &mut Vm, text: &str) -> Result<(), crate::VmError> {
        let mut characters: Vec<_> = self.text.chars().collect();
        let inserted: Vec<_> = text
            .chars()
            .filter(|character| !character.is_control())
            .take(self.maximum.saturating_sub(characters.len()))
            .collect();
        let length = inserted.len();
        characters.splice(self.cursor..self.cursor, inserted);
        self.cursor += length;
        self.text = characters.into_iter().collect();
        self.update(vm)
    }

    pub fn key(&mut self, vm: &mut Vm, key: KeyEvent) -> Result<bool, crate::VmError> {
        let terminator = character_key(key).filter(|code| vm.line_terminators().contains(code));
        if key.code == KeyCode::Enter || (!self.file && terminator.is_some()) {
            if self.file {
                vm.provide_input(&self.text)?;
            } else {
                vm.provide_terminated_input(&self.text, terminator.unwrap_or(0))?;
            }
            return Ok(true);
        }
        if self.file && key.code == KeyCode::Esc {
            vm.provide_input("")?;
            return Ok(true);
        }
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let mut characters: Vec<_> = self.text.chars().collect();
        match key.code {
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(characters.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = characters.len(),
            KeyCode::Char('a') if control => self.cursor = 0,
            KeyCode::Char('e') if control => self.cursor = characters.len(),
            KeyCode::Char('u') if control => {
                characters.drain(..self.cursor);
                self.cursor = 0;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                characters.remove(self.cursor);
            }
            KeyCode::Delete | KeyCode::Char('d')
                if (key.code == KeyCode::Delete || control) && self.cursor < characters.len() =>
            {
                characters.remove(self.cursor);
            }
            KeyCode::Char(character)
                if !control && !character.is_control() && characters.len() < self.maximum =>
            {
                characters.insert(self.cursor, character);
                self.cursor += 1;
            }
            _ => {}
        }
        self.text = characters.into_iter().collect();
        self.update(vm)?;
        Ok(false)
    }
}

pub(super) fn character_key(key: KeyEvent) -> Option<u32> {
    Some(match key.code {
        KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            character as u32
        }
        KeyCode::Left => 0xffff_fffe,
        KeyCode::Right => 0xffff_fffd,
        KeyCode::Up => 0xffff_fffc,
        KeyCode::Down => 0xffff_fffb,
        KeyCode::Enter => 0xffff_fffa,
        KeyCode::Delete | KeyCode::Backspace => 0xffff_fff9,
        KeyCode::Esc => 0xffff_fff8,
        KeyCode::Tab | KeyCode::BackTab => 0xffff_fff7,
        KeyCode::PageUp => 0xffff_fff6,
        KeyCode::PageDown => 0xffff_fff5,
        KeyCode::Home => 0xffff_fff4,
        KeyCode::End => 0xffff_fff3,
        KeyCode::F(number @ 1..=12) => 0xffff_fff0 - u32::from(number),
        _ => return None,
    })
}

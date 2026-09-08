use super::*;
use crossterm::{
    cursor::MoveTo,
    queue,
    style::Print,
    terminal::{Clear, ClearType},
};
use std::collections::VecDeque;

pub(super) struct Screen {
    width: usize,
    height: usize,
    previous: Vec<String>,
}

impl Screen {
    pub fn new((width, height): (u16, u16)) -> Self {
        Self {
            width: usize::from(width.clamp(1, 1024)),
            height: usize::from(height.clamp(1, 512)),
            previous: Vec::new(),
        }
    }

    pub fn resize_vm(&self, vm: &mut Vm) {
        vm.resize_windows(
            self.width as u32 * 8,
            self.height.saturating_sub(2) as u32 * 16,
        );
    }

    pub fn dimensions(&self) -> (u16, u16) {
        (self.width as u16, self.height as u16)
    }

    pub fn draw(&mut self, vm: &Vm, editor: Option<&Editor>) -> io::Result<()> {
        let mut cells = vec![vec![' '; self.width]; self.height];
        let mut cursor = None;
        for view in vm.window_views() {
            let [x, y, width, height] = [
                view.rect[0] / 8,
                view.rect[1] / 16,
                view.rect[2] / 8,
                view.rect[3] / 16,
            ]
            .map(|value| value as usize);
            if width == 0 || height == 0 {
                continue;
            }
            if view.kind == 4 {
                for row in 0..view.grid_size[1] as usize {
                    for column in 0..view.grid_size[0] as usize {
                        if let Some(cell) = view
                            .grid_cells
                            .get(row * view.grid_size[0] as usize + column)
                        {
                            put(&mut cells, x + column, y + row, printable(cell.character));
                        }
                    }
                }
                if let Some(editor) = editor.filter(|editor| {
                    editor.window == view.id
                        && matches!(vm.input_request(), Some(InputRequest::Line { .. }))
                }) {
                    let start = view.grid_cursor.map(|value| value as usize);
                    for (index, character) in editor
                        .text
                        .chars()
                        .take(width.saturating_sub(start[0]))
                        .enumerate()
                    {
                        put(
                            &mut cells,
                            x + start[0] + index,
                            y + start[1],
                            printable(character),
                        );
                    }
                    cursor = Some((x + start[0] + editor.cursor, y + start[1]));
                }
            } else if view.kind == 3 {
                let editor = editor.filter(|editor| {
                    editor.window == view.id
                        && matches!(vm.input_request(), Some(InputRequest::Line { .. }))
                });
                let output = view.runs.iter().flat_map(|run| run.text.chars());
                let mut wrapped = Wrapped::new(width, height);
                for character in output {
                    wrapped.push(character);
                }
                if let Some(editor) = editor {
                    for (index, character) in editor.text.chars().enumerate() {
                        if index == editor.cursor {
                            wrapped.mark_cursor();
                        }
                        wrapped.push(character);
                    }
                    if editor.cursor == editor.text.chars().count() {
                        wrapped.mark_cursor();
                    }
                } else if vm.input_window() == view.id
                    && vm.input_request() == Some(InputRequest::Character)
                {
                    wrapped.mark_cursor();
                }
                for (row, line) in wrapped.lines.iter().enumerate() {
                    for (column, character) in line.iter().enumerate() {
                        put(&mut cells, x + column, y + row, *character);
                    }
                }
                if let Some((column, row)) = wrapped.cursor {
                    cursor = Some((x + column, y + row));
                }
            }
        }
        let footer = self.height.saturating_sub(2);
        let message = if matches!(vm.input_request(), Some(InputRequest::File { .. })) {
            vm.file_prompt_message()
        } else if vm.input_request() == Some(InputRequest::Character) {
            format!("Window {}: press a key", vm.input_window())
        } else if vm.state() == RunState::Halted {
            "Story finished".to_owned()
        } else {
            format!(
                "Window {} | Ctrl+N: switch input | Ctrl+C: quit",
                vm.input_window()
            )
        };
        for (column, character) in message.chars().take(self.width).enumerate() {
            put(&mut cells, column, footer, printable(character));
        }
        if matches!(vm.input_request(), Some(InputRequest::File { .. }))
            && let Some(editor) = editor
        {
            let start = editor.cursor.saturating_sub(self.width.saturating_sub(1));
            for (column, character) in editor.text.chars().skip(start).take(self.width).enumerate()
            {
                put(&mut cells, column, self.height - 1, printable(character));
            }
            cursor = Some((editor.cursor - start, self.height - 1));
        }
        let lines: Vec<String> = cells
            .into_iter()
            .map(|row| row.into_iter().collect())
            .collect();
        let mut stdout = io::stdout().lock();
        queue!(stdout, Hide)?;
        if self.previous.is_empty() {
            queue!(stdout, Clear(ClearType::All))?;
        }
        for (row, line) in lines.iter().enumerate() {
            if self.previous.get(row) != Some(line) {
                queue!(stdout, MoveTo(0, row as u16), Print(line))?;
            }
        }
        if let Some((column, row)) =
            cursor.filter(|(column, row)| *column < self.width && *row < footer)
        {
            queue!(stdout, MoveTo(column as u16, row as u16), Show)?;
        } else if matches!(vm.input_request(), Some(InputRequest::File { .. }))
            && let Some((column, row)) = cursor
        {
            queue!(stdout, MoveTo(column as u16, row as u16), Show)?;
        }
        self.previous = lines;
        stdout.flush()
    }

    pub fn plain_text(&self) -> String {
        self.previous
            .iter()
            .map(|line| line.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn printable(character: char) -> char {
    if character.width() == Some(1) {
        character
    } else {
        '?'
    }
}

fn put(cells: &mut [Vec<char>], x: usize, y: usize, character: char) {
    if let Some(cell) = cells.get_mut(y).and_then(|row| row.get_mut(x)) {
        *cell = character;
    }
}

struct Wrapped {
    lines: VecDeque<Vec<char>>,
    width: usize,
    height: usize,
    cursor: Option<(usize, usize)>,
}

impl Wrapped {
    fn new(width: usize, height: usize) -> Self {
        Self {
            lines: VecDeque::from([Vec::new()]),
            width,
            height,
            cursor: None,
        }
    }
    fn newline(&mut self) {
        self.lines.push_back(Vec::new());
        if self.lines.len() > self.height {
            self.lines.pop_front();
            self.cursor = self
                .cursor
                .and_then(|(x, y)| y.checked_sub(1).map(|y| (x, y)));
        }
    }
    fn push(&mut self, character: char) {
        if character == '\n' {
            self.newline();
            return;
        }
        if self.lines.back().unwrap().len() == self.width {
            self.newline();
        }
        self.lines.back_mut().unwrap().push(printable(character));
    }
    fn mark_cursor(&mut self) {
        if self.lines.back().unwrap().len() == self.width {
            self.newline();
        }
        self.cursor = Some((self.lines.back().unwrap().len(), self.lines.len() - 1));
    }
}

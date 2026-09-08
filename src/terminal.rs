//! Interactive terminal host, with a separate stable protocol for pipes.
use std::{
    io::{self, IsTerminal, Write},
    sync::Arc,
    time::{Duration, Instant},
};

use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use unicode_width::UnicodeWidthChar;

use crate::{InputRequest, RunState, Vm};

mod editor;
mod pipe;
mod screen;

use editor::Editor;
use screen::Screen;

pub fn run(vm: Vm) -> Result<(), Box<dyn std::error::Error>> {
    if io::stdin().is_terminal() && io::stdout().is_terminal() {
        run_interactive(vm)
    } else {
        pipe::run(vm)
    }
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let guard = Self;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            Hide
        )?;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            Show,
            LeaveAlternateScreen
        );
        let _ = terminal::disable_raw_mode();
    }
}

fn run_interactive(mut vm: Vm) -> Result<(), Box<dyn std::error::Error>> {
    let guard = TerminalGuard::enter()?;
    vm.set_terminal_host(true);
    // A Glk grid cell has a single terminal column. Unsupported glyph widths
    // are visibly replaced and reported as CannotPrint, including wide CJK.
    vm.set_glyph_support(Arc::new(|character| character.width() == Some(1)));
    let mut screen = Screen::new(terminal::size()?);
    screen.resize_vm(&mut vm);
    let mut editor = None;
    let mut repaint = true;
    let mut last_paint = Instant::now();
    let mut last_size_check = Instant::now();
    loop {
        // Some terminal multiplexers do not deliver every resize signal.
        // Recheck the actual dimensions as well as handling Resize events.
        if last_size_check.elapsed() >= Duration::from_millis(250) {
            let size = terminal::size()?;
            if screen.dimensions() != size {
                screen = Screen::new(size);
                screen.resize_vm(&mut vm);
                repaint = true;
            }
            last_size_check = Instant::now();
        }
        let state = vm
            .run_steps(20_000)
            .map_err(|error| format!("{error} at program counter {:#010x}", vm.pc()))?;
        let output = vm.take_output();
        let input_changed = Editor::synchronize(&vm, &mut editor);
        repaint |= !output.is_empty() || input_changed;
        if repaint || last_paint.elapsed() >= Duration::from_millis(50) {
            screen.draw(&vm, editor.as_ref())?;
            repaint = false;
            last_paint = Instant::now();
        }
        if state == RunState::Halted {
            break;
        }
        let timeout = if state == RunState::Running {
            Duration::ZERO
        } else {
            Duration::from_millis(10)
        };
        if !event::poll(timeout)? {
            continue;
        }
        match event::read()? {
            Event::Resize(width, height) => {
                screen = Screen::new((width, height));
                screen.resize_vm(&mut vm);
                repaint = true;
            }
            Event::Paste(text) => {
                if let Some(editor) = &mut editor {
                    editor.paste(&mut vm, &text)?;
                    repaint = true;
                } else if vm.input_request() == Some(InputRequest::Character)
                    && let Some(character) = text.chars().find(|character| !character.is_control())
                {
                    vm.provide_key(character as u32)?;
                    repaint = true;
                }
            }
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                let control = key.modifiers.contains(KeyModifiers::CONTROL);
                if control
                    && (key.code == KeyCode::Char('c')
                        || (key.code == KeyCode::Char('d')
                            && editor.as_ref().is_none_or(|editor| editor.text.is_empty())))
                {
                    vm.stop();
                    break;
                }
                if control && key.code == KeyCode::Char('n') {
                    let windows = vm.pending_input_windows();
                    if let Some(index) = windows
                        .iter()
                        .position(|window| *window == vm.input_window())
                    {
                        vm.select_input_window(windows[(index + 1) % windows.len()]);
                    }
                } else if vm.input_request() == Some(InputRequest::Character) {
                    if let Some(code) = editor::character_key(key) {
                        vm.provide_key(code)?;
                    }
                } else if let Some(current) = &mut editor
                    && current.key(&mut vm, key)?
                {
                    editor = None;
                }
                repaint = true;
            }
            _ => {}
        }
    }
    // Preserve the last visible scene after restoring the shell's screen,
    // including a game's final text before it exits without another prompt.
    screen.draw(&vm, editor.as_ref())?;
    let final_text = screen.plain_text();
    drop(guard);
    println!("{final_text}");
    Ok(())
}

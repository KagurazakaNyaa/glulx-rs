#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

use glulx_rs::app::PlayerApp;
use glulx_rs::{RunState, Story, Vm};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments
        .first()
        .is_some_and(|argument| argument == "--headless")
    {
        let path = arguments
            .get(1)
            .map(PathBuf::from)
            .ok_or("usage: glulx-rs --headless STORY")?;
        return run_headless(&path);
    }
    let initial_story = arguments.first().map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Glulx Player")
            .with_inner_size([1100.0, 760.0])
            .with_min_inner_size([720.0, 480.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "Glulx Player",
        options,
        Box::new(move |creation| Ok(Box::new(PlayerApp::new(creation, initial_story)))),
    )?;
    Ok(())
}

fn run_headless(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut vm = Vm::new(Story::open(path)?)?;
    vm.set_graphical_host(false);
    let (sender, input) = std::sync::mpsc::sync_channel(16);
    std::thread::spawn(move || {
        loop {
            let mut line = String::new();
            match io::stdin().read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if sender.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = sender.send(Err(error));
                    break;
                }
            }
        }
    });
    let mut file_prompt_shown = false;
    loop {
        let state = vm
            .run_steps(100_000)
            .map_err(|error| format!("{error} at program counter {:#010x}", vm.pc()))?;
        print!("{}", vm.take_output());
        io::stdout().flush()?;
        if state != RunState::WaitingForFile {
            file_prompt_shown = false;
        }
        match state {
            RunState::Running => continue,
            RunState::WaitingForLine | RunState::WaitingForChar | RunState::WaitingForFile => {
                if state == RunState::WaitingForFile && !file_prompt_shown {
                    print!("{} ", vm.file_prompt_message());
                    io::stdout().flush()?;
                    file_prompt_shown = true;
                }
                match input.recv_timeout(std::time::Duration::from_millis(10)) {
                    Ok(line) => {
                        vm.provide_input(line?.trim_end_matches(['\r', '\n']))?;
                        file_prompt_shown = false;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        vm.stop();
                        return Ok(());
                    }
                }
            }
            RunState::WaitingForEvent => std::thread::sleep(std::time::Duration::from_millis(10)),
            RunState::Halted => return Ok(()),
        }
    }
}

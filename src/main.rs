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
    loop {
        let state = vm.run_steps(100_000)?;
        print!("{}", vm.take_output());
        io::stdout().flush()?;
        match state {
            RunState::Running => continue,
            RunState::WaitingForLine | RunState::WaitingForChar => {
                let mut input = String::new();
                if io::stdin().read_line(&mut input)? == 0 {
                    return Ok(());
                }
                vm.provide_input(input.trim_end_matches(['\r', '\n']))?;
            }
            RunState::Halted => return Ok(()),
        }
    }
}

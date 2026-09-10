//! Stable text-only protocol for redirected stdin/stdout and reference tools.
use super::*;

pub(super) fn run(mut vm: Vm) -> Result<(), Box<dyn std::error::Error>> {
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
        let state = {
            let _stage = crate::diagnostics::stage("vm-slice");
            let state = vm
                .run_steps(100_000)
                .map_err(|error| format!("{error} at program counter {:#010x}", vm.pc()))?;
            crate::diagnostics::vm(&vm);
            state
        };
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
                match input.recv_timeout(Duration::from_millis(10)) {
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
            RunState::WaitingForEvent => std::thread::sleep(Duration::from_millis(10)),
            RunState::Halted => return Ok(()),
        }
    }
}

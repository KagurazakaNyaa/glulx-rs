//! Opt-in, flushed diagnostics that remain useful when the UI thread stalls.
use std::{
    fs::File,
    io::{self, Write},
    path::Path,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

struct State {
    stage: &'static str,
    since: Instant,
    vm: String,
    frames: u64,
    slices: u64,
    vm_time: Duration,
    ui_time: Duration,
}

struct Logger {
    file: Mutex<File>,
    state: Mutex<State>,
    started: Instant,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// Create the requested log before starting the GUI. No game text or credentials
/// are recorded. A worker reports the current stage even if the GUI stops moving.
pub fn start(path: &Path) -> io::Result<()> {
    let file = File::create(path)?;
    let now = Instant::now();
    LOGGER
        .set(Logger {
            file: Mutex::new(file),
            state: Mutex::new(State {
                stage: "event-loop",
                since: now,
                vm: String::new(),
                frames: 0,
                slices: 0,
                vm_time: Duration::ZERO,
                ui_time: Duration::ZERO,
            }),
            started: now,
        })
        .map_err(|_| io::Error::other("diagnostics already started"))?;
    record(format_args!(
        "start version={} os={} arch={} debug_assertions={} opt_level={}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        cfg!(debug_assertions),
        env!("GLULX_BUILD_OPT_LEVEL")
    ));
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        record(format_args!("panic: {info}"));
        previous(info);
    }));
    std::thread::Builder::new()
        .name("diagnostic-heartbeat".into())
        .spawn(|| {
            let mut previous = (Instant::now(), Duration::ZERO, Duration::ZERO, 0, 0);
            loop {
                std::thread::sleep(Duration::from_secs(2));
                let logger = LOGGER.get().unwrap();
                let message = {
                    let state = logger.state.lock().unwrap_or_else(|e| e.into_inner());
                    let now = Instant::now();
                    let message = format!(
                        "heartbeat stage={} stage_ms={} frames={} slices={} interval_ms={} vm_ms={} ui_ms={} frame_delta={} slice_delta={} {}",
                        state.stage, state.since.elapsed().as_millis(), state.frames, state.slices,
                        now.duration_since(previous.0).as_millis(),
                        state.vm_time.saturating_sub(previous.1).as_millis(),
                        state.ui_time.saturating_sub(previous.2).as_millis(),
                        state.frames - previous.3, state.slices - previous.4, state.vm
                    );
                    previous = (now, state.vm_time, state.ui_time, state.frames, state.slices);
                    message
                };
                record(format_args!("{message}"));
            }
        })?;
    Ok(())
}

pub(crate) fn record(message: std::fmt::Arguments<'_>) {
    if let Some(logger) = LOGGER.get() {
        let mut file = logger.file.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writeln!(
            file,
            "[{}ms] {message}",
            logger.started.elapsed().as_millis()
        );
        let _ = file.flush();
    }
}

pub(crate) fn vm(vm: &crate::Vm) {
    if let Some(logger) = LOGGER.get() {
        logger.state.lock().unwrap_or_else(|e| e.into_inner()).vm = vm.diagnostic_summary();
    }
}

pub(crate) struct Stage(Option<(&'static str, Instant, &'static str, Instant)>);

pub(crate) fn stage(name: &'static str) -> Stage {
    let Some(logger) = LOGGER.get() else {
        return Stage(None);
    };
    let mut state = logger.state.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    match name {
        "ui" => state.frames += 1,
        "vm-slice" => state.slices += 1,
        _ => {}
    }
    let previous = (state.stage, state.since, name, now);
    state.stage = name;
    state.since = now;
    Stage(Some(previous))
}

impl Drop for Stage {
    fn drop(&mut self) {
        if let Some((previous, since, name, started)) = self.0 {
            let elapsed = started.elapsed();
            {
                let mut state = LOGGER
                    .get()
                    .unwrap()
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                match name {
                    "vm-slice" => state.vm_time += elapsed,
                    "ui" => state.ui_time += elapsed,
                    _ => {}
                }
                state.stage = previous;
                state.since = since;
            }
            if elapsed >= Duration::from_millis(250) {
                record(format_args!(
                    "slow stage={name} elapsed_ms={}",
                    elapsed.as_millis()
                ));
            }
        }
    }
}

//! Flushed diagnostics that remain useful when the UI thread stalls.
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Write},
    path::Path,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use crate::vm::VmDiagnosticSnapshot;

struct State {
    stage: &'static str,
    since: Instant,
    vm: String,
    vm_snapshot: Option<VmDiagnosticSnapshot>,
    frames: u64,
    slices: u64,
    vm_time: Duration,
    ui_time: Duration,
    named_time: BTreeMap<&'static str, Duration>,
}

struct Logger {
    file: Option<Mutex<File>>,
    state: Mutex<State>,
    started: Instant,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

fn new_logger(file: Option<File>) -> Logger {
    let now = Instant::now();
    Logger {
        file: file.map(Mutex::new),
        state: Mutex::new(State {
            stage: "event-loop",
            since: now,
            vm: String::new(),
            vm_snapshot: None,
            frames: 0,
            slices: 0,
            vm_time: Duration::ZERO,
            ui_time: Duration::ZERO,
            named_time: BTreeMap::new(),
        }),
        started: now,
    }
}

/// Initialize the in-process observation state without creating a log file.
/// Profiling builds use this so the HTTP metrics endpoint works independently
/// from the optional diagnostic heartbeat log.
pub(crate) fn initialize() {
    let _ = LOGGER.set(new_logger(None));
}

/// Create the requested log before starting the GUI. No game text or credentials
/// are recorded. A worker reports the current stage even if the GUI stops moving.
pub fn start(path: &Path) -> io::Result<()> {
    let file = File::create(path)?;
    let logger = new_logger(Some(file));
    LOGGER
        .set(logger)
        .map_err(|_| io::Error::other("diagnostics already started"))?;
    record(format_args!(
        "start pid={} version={} os={} arch={} debug_assertions={} opt_level={}",
        std::process::id(),
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
            let mut previous_named = BTreeMap::new();
            loop {
                std::thread::sleep(Duration::from_secs(2));
                let logger = LOGGER.get().unwrap();
                let message = {
                    let state = logger.state.lock().unwrap_or_else(|e| e.into_inner());
                    let now = Instant::now();
                    let timings = state
                        .named_time
                        .iter()
                        .map(|(name, elapsed)| {
                            let before = previous_named.get(name).copied().unwrap_or_default();
                            format!("{name}_ms={}", elapsed.saturating_sub(before).as_millis())
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    let message = format!(
                        "heartbeat stage={} stage_ms={} frames={} slices={} interval_ms={} vm_ms={} ui_ms={} frame_delta={} slice_delta={} timings={} {}",
                        state.stage, state.since.elapsed().as_millis(), state.frames, state.slices,
                        now.duration_since(previous.0).as_millis(),
                        state.vm_time.saturating_sub(previous.1).as_millis(),
                        state.ui_time.saturating_sub(previous.2).as_millis(),
                        state.frames - previous.3, state.slices - previous.4,
                        if timings.is_empty() { "-" } else { &timings },
                        state.vm
                    );
                    previous = (now, state.vm_time, state.ui_time, state.frames, state.slices);
                    previous_named = state.named_time.clone();
                    message
                };
                record(format_args!("{message}"));
            }
        })?;
    Ok(())
}

pub fn record(message: std::fmt::Arguments<'_>) {
    if let Some(logger) = LOGGER.get()
        && let Some(file) = &logger.file
    {
        let mut file = file.lock().unwrap_or_else(|e| e.into_inner());
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
        let snapshot = vm.diagnostic_snapshot();
        {
            let mut state = logger.state.lock().unwrap_or_else(|e| e.into_inner());
            state.vm = vm.diagnostic_summary();
            state.vm_snapshot = Some(snapshot.clone());
        }
        crate::profiling::record_vm(&snapshot);
    }
}

#[derive(serde::Serialize)]
struct Snapshot {
    version: &'static str,
    os: &'static str,
    arch: &'static str,
    debug_assertions: bool,
    opt_level: &'static str,
    etw_enabled: bool,
    uptime_ms: u128,
    stage: &'static str,
    stage_ms: u128,
    frames: u64,
    slices: u64,
    vm_ms: u128,
    ui_ms: u128,
    named_ms: BTreeMap<String, u128>,
    vm: Option<VmDiagnosticSnapshot>,
}

/// Return a consistent JSON snapshot for the local profiling endpoint.
pub(crate) fn snapshot_json() -> Vec<u8> {
    let Some(logger) = LOGGER.get() else {
        let snapshot = Snapshot {
            version: env!("CARGO_PKG_VERSION"),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            debug_assertions: cfg!(debug_assertions),
            opt_level: env!("GLULX_BUILD_OPT_LEVEL"),
            etw_enabled: crate::profiling::etw_enabled(),
            uptime_ms: 0,
            stage: "uninitialized",
            stage_ms: 0,
            frames: 0,
            slices: 0,
            vm_ms: 0,
            ui_ms: 0,
            named_ms: BTreeMap::new(),
            vm: None,
        };
        return serde_json::to_vec(&snapshot).unwrap_or_else(|_| b"{}".to_vec());
    };
    let state = logger.state.lock().unwrap_or_else(|e| e.into_inner());
    let snapshot = Snapshot {
        version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        debug_assertions: cfg!(debug_assertions),
        opt_level: env!("GLULX_BUILD_OPT_LEVEL"),
        etw_enabled: crate::profiling::etw_enabled(),
        uptime_ms: logger.started.elapsed().as_millis(),
        stage: state.stage,
        stage_ms: state.since.elapsed().as_millis(),
        frames: state.frames,
        slices: state.slices,
        vm_ms: state.vm_time.as_millis(),
        ui_ms: state.ui_time.as_millis(),
        named_ms: state
            .named_time
            .iter()
            .map(|(name, elapsed)| ((*name).to_owned(), elapsed.as_millis()))
            .collect(),
        vm: state.vm_snapshot.clone(),
    };
    serde_json::to_vec(&snapshot).unwrap_or_else(|_| b"{}".to_vec())
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
                *state.named_time.entry(name).or_default() += elapsed;
                state.stage = previous;
                state.since = since;
            }
            crate::profiling::record_stage(name, elapsed);
            if elapsed >= Duration::from_millis(250) {
                record(format_args!(
                    "slow stage={name} elapsed_ms={}",
                    elapsed.as_millis()
                ));
            }
        }
    }
}

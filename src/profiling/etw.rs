//! Windows ETW markers for process-scoped performance traces.
//!
//! The player registers a PID-scoped provider and emits low-rate phase markers.
//! HTTP captures use a WPR profile with that provider, ProcessExeFilter, and
//! provider call stacks; they intentionally do not enable the machine-wide
//! SampledProfile keyword.

use std::{
    ffi::OsStr,
    fs,
    mem::size_of,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use windows_sys::{
    Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::Diagnostics::Etw::{EventRegister, EventWriteString, REGHANDLE},
    },
    core::GUID,
};

const PROVIDER_BASE_ID: u128 = 0x4f9f6d5e_2c8b_4f51_9d3e_7a1e6f7c2b40;
const INFO_LEVEL: u8 = 4;
const MAX_ETL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ETW_SECONDS: u64 = 60;
const HELPER_SWITCH: &str = "--internal-etw-capture";
const ETL_NAME: &str = "capture.etl";
const WPR_PROFILE_FILE: &str = "capture.wprp";
const WPR_PROFILE_NAME: &str = "GlulxProcess";
const CANCEL_NAME: &str = "cancel";
const ERROR_NAME: &str = "error.txt";

static PROCESS_PROVIDER_ID: OnceLock<GUID> = OnceLock::new();
static REGISTRATION_STATUS: OnceLock<u32> = OnceLock::new();
static HANDLE: AtomicI64 = AtomicI64::new(0);

pub(crate) fn start() -> Result<(), u32> {
    let status = *REGISTRATION_STATUS.get_or_init(|| {
        let mut handle: REGHANDLE = 0;
        // A TraceLogging consumer can enable this provider without a manifest;
        // EventWriteString carries the human-readable marker payload.
        let status = unsafe { EventRegister(provider_id(), None, std::ptr::null(), &mut handle) };
        if status == 0 {
            HANDLE.store(handle, Ordering::Release);
        }
        status
    });
    (status == 0).then_some(()).ok_or(status)
}

pub(crate) fn enabled() -> bool {
    HANDLE.load(Ordering::Acquire) != 0
}

pub(crate) fn record_stage(name: &str, elapsed: Duration) {
    record(&format!(
        "stage name={name} elapsed_us={}",
        elapsed.as_micros()
    ));
}

pub(crate) fn record_vm(snapshot: &crate::vm::VmDiagnosticSnapshot) {
    let top = snapshot
        .opcode_counts
        .iter()
        .take(4)
        .map(|opcode| format!("{:#x}:{}", opcode.opcode, opcode.count))
        .collect::<Vec<_>>()
        .join(",");
    record(&format!(
        "vm_slice state={:?} pc={:#x} instructions={} polls={} stack_bytes={} memory_bytes={} decode_hit_rate={:.6} top_opcodes={top}",
        snapshot.state,
        snapshot.pc,
        snapshot.instructions,
        snapshot.poll_calls,
        snapshot.stack_bytes,
        snapshot.memory_bytes,
        snapshot.decode_cache_hit_rate,
    ));
}

pub(crate) fn status_json() -> Vec<u8> {
    let process_id = std::process::id();
    let provider_guid = format!("{{{}}}", format_guid(provider_id()));
    format!(
        r#"{{"enabled":{},"provider":"{}","process_id":{},"events":["stage","vm_slice"],"sampling":"WPR PID-scoped TraceLogging markers and call stacks","scope":"current PID provider with ProcessExeFilter defense-in-depth","markers":"Enable this provider in the ETW session to correlate VM phases","uac":"Prompts when WPR reports missing system-profile privilege or access denied","capture_endpoint":"/debug/etw/profile?seconds=10"}}"#,
        enabled(), provider_guid, process_id
    )
    .into_bytes()
}

pub(crate) fn capture_profile(query: &str, stop: &AtomicBool) -> Result<Vec<u8>, String> {
    let seconds = super::etw_capture_seconds(query)?;
    let process_id = std::process::id();
    let work_dir = create_work_dir()?;
    let etl_path = work_dir.join(ETL_NAME);
    let cancel_path = work_dir.join(CANCEL_NAME);

    let result = match start_wpr(&work_dir, process_id) {
        Ok(()) => capture_started_wpr(seconds, &etl_path, stop, || false)
            .and_then(|()| read_profile(&etl_path)),
        Err(error) if error.needs_elevation => {
            launch_elevated_helper(seconds, process_id, &work_dir, &cancel_path, stop).and_then(
                |()| {
                    if cancel_path.exists() {
                        Err("ETW capture canceled during shutdown".to_owned())
                    } else if etl_path.exists() {
                        read_profile(&etl_path)
                    } else {
                        let helper_error = fs::read_to_string(work_dir.join(ERROR_NAME))
                            .unwrap_or_else(|_| {
                                "elevated capture helper produced no ETL".to_owned()
                            });
                        Err(helper_error)
                    }
                },
            )
        }
        Err(error) => Err(error.message),
    };
    cleanup_work_dir(&work_dir);
    result
}

/// Entrypoint used only by the short-lived UAC-elevated child process.
pub(crate) fn run_elevated_helper(
    seconds: &str,
    work_dir: &Path,
    process_id: u32,
) -> Result<(), String> {
    let result = run_elevated_helper_inner(seconds, work_dir, process_id);
    if let Err(error) = &result
        && let Ok(work_dir) = validate_work_dir(work_dir)
    {
        let _ = fs::write(work_dir.join(ERROR_NAME), error.as_bytes());
    }
    result
}

fn run_elevated_helper_inner(
    seconds: &str,
    work_dir: &Path,
    process_id: u32,
) -> Result<(), String> {
    let seconds = seconds
        .parse::<u64>()
        .map_err(|_| "capture duration must be an integer".to_owned())?;
    if !(1..=MAX_ETW_SECONDS).contains(&seconds) {
        return Err(format!(
            "capture duration must be from 1 to {MAX_ETW_SECONDS} seconds"
        ));
    }
    let work_dir = validate_work_dir(work_dir)?;
    let cancel_path = work_dir.join(CANCEL_NAME);
    let etl_path = work_dir.join(ETL_NAME);
    if etl_path.exists() || cancel_path.exists() || work_dir.join(ERROR_NAME).exists() {
        return Err("capture work directory is not empty".to_owned());
    }
    start_wpr(&work_dir, process_id).map_err(|error| error.message)?;
    capture_started_wpr(seconds, &etl_path, &AtomicBool::new(false), || {
        cancel_path.exists()
    })
}

fn create_work_dir() -> Result<PathBuf, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let work_dir =
        std::env::temp_dir().join(format!("glulx-rs-etw-{}-{stamp}", std::process::id()));
    fs::create_dir(&work_dir)
        .map_err(|error| format!("could not create capture directory: {error}"))?;
    Ok(work_dir)
}

fn validate_work_dir(path: &Path) -> Result<PathBuf, String> {
    let temp_dir = std::env::temp_dir()
        .canonicalize()
        .map_err(|error| format!("could not resolve temporary directory: {error}"))?;
    let work_dir = path
        .canonicalize()
        .map_err(|error| format!("could not resolve capture directory: {error}"))?;
    let name = work_dir
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or("invalid capture directory name")?;
    if !work_dir.starts_with(&temp_dir) || !name.starts_with("glulx-rs-etw-") {
        return Err("capture helper only accepts a glulx-rs ETW temp directory".to_owned());
    }
    if !work_dir.is_dir() {
        return Err("capture directory does not exist".to_owned());
    }
    Ok(work_dir)
}

fn cleanup_work_dir(work_dir: &Path) {
    let _ = fs::remove_file(work_dir.join(ETL_NAME));
    let _ = fs::remove_file(work_dir.join(WPR_PROFILE_FILE));
    let _ = fs::remove_file(work_dir.join(CANCEL_NAME));
    let _ = fs::remove_file(work_dir.join(ERROR_NAME));
    let _ = fs::remove_dir(work_dir);
}

fn start_wpr(work_dir: &Path, process_id: u32) -> Result<(), WprFailure> {
    let profile_path = create_wpr_profile(work_dir, process_id).map_err(|message| WprFailure {
        message,
        needs_elevation: false,
    })?;
    let profile = format!(
        "{}!{WPR_PROFILE_NAME}.Verbose",
        wpr_command_path(&profile_path).to_string_lossy()
    );
    let output = Command::new("wpr.exe")
        .args(["-start", profile.as_str(), "-filemode"])
        .output()
        .map_err(|error| WprFailure {
            message: format!("could not launch wpr.exe: {error}"),
            needs_elevation: false,
        })?;
    if output.status.success() {
        return Ok(());
    }
    let message = command_failure("wpr -start process-filtered profile", &output);
    let combined = format!(
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .to_ascii_lowercase();
    let needs_elevation = wpr_needs_elevation(output.status.code(), &combined);
    Err(WprFailure {
        message,
        needs_elevation,
    })
}

fn create_wpr_profile(work_dir: &Path, process_id: u32) -> Result<PathBuf, String> {
    let process_name = std::env::current_exe()
        .map_err(|error| format!("could not locate current executable: {error}"))?
        .file_name()
        .and_then(OsStr::to_str)
        .map(str::to_owned)
        .ok_or_else(|| "current executable has no UTF-8 file name".to_owned())?;
    let provider_guid = format_guid(&provider_id_for_process(process_id));
    let profile_path = work_dir.join(WPR_PROFILE_FILE);
    fs::write(
        &profile_path,
        wpr_profile_xml(&provider_guid, &process_name, process_id),
    )
    .map_err(|error| format!("could not write WPR profile: {error}"))?;
    Ok(profile_path)
}

fn wpr_profile_xml(provider_guid: &str, process_name: &str, process_id: u32) -> String {
    let process_name = xml_attribute(process_name);
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<WindowsPerformanceRecorder Version="1.0" Author="glulx-rs">
  <Profiles>
    <EventCollector Id="GlulxEventCollector" Name="Glulx ETW Event Collector">
      <BufferSize Value="64"/>
      <Buffers Value="64"/>
    </EventCollector>
    <EventProvider Id="GlulxTraceLogging" Name="{provider_guid}" Level="{INFO_LEVEL}" Stack="true" Strict="true" ProcessExeFilter="{process_name}"/>
    <Profile Id="{WPR_PROFILE_NAME}.Verbose.File" Name="{WPR_PROFILE_NAME}" DetailLevel="Verbose" LoggingMode="File" Description="PID {process_id} Glulx TraceLogging markers">
      <Collectors>
        <EventCollectorId Value="GlulxEventCollector">
          <EventProviders>
            <EventProviderId Value="GlulxTraceLogging"/>
          </EventProviders>
        </EventCollectorId>
      </Collectors>
    </Profile>
    <Profile Id="{WPR_PROFILE_NAME}.Verbose.Memory" Name="{WPR_PROFILE_NAME}" DetailLevel="Verbose" LoggingMode="Memory" Description="PID {process_id} Glulx TraceLogging markers">
      <Collectors>
        <EventCollectorId Value="GlulxEventCollector">
          <EventProviders>
            <EventProviderId Value="GlulxTraceLogging"/>
          </EventProviders>
        </EventCollectorId>
      </Collectors>
    </Profile>
  </Profiles>
</WindowsPerformanceRecorder>
"#
    )
}

fn xml_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn provider_id() -> &'static GUID {
    PROCESS_PROVIDER_ID.get_or_init(|| provider_id_for_process(std::process::id()))
}

fn provider_id_for_process(process_id: u32) -> GUID {
    GUID::from_u128(PROVIDER_BASE_ID ^ u128::from(process_id))
}

fn format_guid(guid: &GUID) -> String {
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4[0],
        guid.data4[1],
        guid.data4[2],
        guid.data4[3],
        guid.data4[4],
        guid.data4[5],
        guid.data4[6],
        guid.data4[7]
    )
}

fn wpr_needs_elevation(exit_code: Option<i32>, output: &str) -> bool {
    exit_code.is_some_and(|code| matches!(code as u32, 1314 | 0xc558_5011 | 0x8007_0005))
        || output.contains("0xc5585011")
        || output.contains("0x80070005")
        || output.contains("failed to enable the policy to profile system performance")
        || output.contains("privilege not held")
        || output.contains("access is denied")
}

struct WprFailure {
    message: String,
    needs_elevation: bool,
}

fn capture_started_wpr(
    seconds: u64,
    etl_path: &Path,
    stop: &AtomicBool,
    helper_cancelled: impl Fn() -> bool,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !stop.load(Ordering::Acquire) && !helper_cancelled() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        std::thread::sleep(remaining.min(Duration::from_millis(100)));
    }
    if stop.load(Ordering::Acquire) || helper_cancelled() {
        cancel_capture();
        return Err("ETW capture canceled during shutdown".to_owned());
    }

    let output = Command::new("wpr.exe")
        .arg("-stop")
        .arg(wpr_command_path(etl_path))
        .output()
        .map_err(|error| format!("could not launch wpr.exe -stop: {error}"))?;
    if !output.status.success() {
        cancel_capture();
        return Err(command_failure("wpr -stop", &output));
    }
    validate_profile_size(etl_path)
}

fn validate_profile_size(path: &Path) -> Result<(), String> {
    let size = fs::metadata(path)
        .map_err(|error| format!("could not inspect ETL output: {error}"))?
        .len();
    if size > MAX_ETL_BYTES {
        return Err(format!(
            "ETL output is too large ({size} bytes, limit {MAX_ETL_BYTES})"
        ));
    }
    Ok(())
}

fn read_profile(path: &Path) -> Result<Vec<u8>, String> {
    validate_profile_size(path)?;
    fs::read(path).map_err(|error| format!("could not read ETL output: {error}"))
}

fn wpr_command_path(path: &Path) -> PathBuf {
    let path = path.to_string_lossy();
    if let Some(path) = path.strip_prefix("\\\\?\\UNC\\") {
        return PathBuf::from(format!("\\\\{path}"));
    }
    if let Some(path) = path.strip_prefix("\\\\?\\") {
        return PathBuf::from(path);
    }
    PathBuf::from(path.as_ref())
}

fn launch_elevated_helper(
    seconds: u64,
    process_id: u32,
    work_dir: &Path,
    cancel_path: &Path,
    stop: &AtomicBool,
) -> Result<(), String> {
    use windows_sys::Win32::{
        Foundation::{ERROR_CANCELLED, GetLastError, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Threading::{GetExitCodeProcess, WaitForSingleObject},
        UI::{
            Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW},
            WindowsAndMessaging::SW_HIDE,
        },
    };

    let executable = std::env::current_exe()
        .map_err(|error| format!("could not locate the player executable: {error}"))?;
    let parameters = elevated_helper_parameters(seconds, process_id, work_dir);
    let verb = wide_null("runas");
    let executable = wide_null(&executable.to_string_lossy());
    let parameters = wide_null(&parameters);
    let mut execute = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: verb.as_ptr(),
        lpFile: executable.as_ptr(),
        lpParameters: parameters.as_ptr(),
        nShow: SW_HIDE,
        ..Default::default()
    };
    if unsafe { ShellExecuteExW(&mut execute) } == 0 {
        let error = unsafe { GetLastError() };
        return if error == ERROR_CANCELLED {
            Err("UAC consent was canceled".to_owned())
        } else {
            Err(format!(
                "could not launch the elevated capture helper (Windows error {error})"
            ))
        };
    }
    if execute.hProcess.is_null() {
        return Err("Windows did not return the elevated helper process handle".to_owned());
    }
    let process = OwnedHandle(execute.hProcess);
    let mut cancel_sent = false;
    loop {
        let result = unsafe { WaitForSingleObject(process.0, 100) };
        if result == WAIT_OBJECT_0 {
            break;
        }
        if result != WAIT_TIMEOUT {
            return Err(format!(
                "waiting for elevated capture helper failed (Windows result {result})"
            ));
        }
        if stop.load(Ordering::Acquire) && !cancel_sent {
            let _ = fs::write(cancel_path, b"cancel");
            cancel_sent = true;
        }
    }
    let mut exit_code = 1u32;
    if unsafe { GetExitCodeProcess(process.0, &mut exit_code) } == 0 {
        return Err("could not read elevated helper exit status".to_owned());
    }
    if exit_code != 0 {
        return Err(fs::read_to_string(work_dir.join(ERROR_NAME))
            .unwrap_or_else(|_| format!("elevated capture helper exited with {exit_code}")));
    }
    Ok(())
}

fn elevated_helper_parameters(seconds: u64, process_id: u32, work_dir: &Path) -> String {
    [
        HELPER_SWITCH.to_owned(),
        seconds.to_string(),
        work_dir.to_string_lossy().into_owned(),
        process_id.to_string(),
    ]
    .iter()
    .map(|argument| quote_windows_argument(argument))
    .collect::<Vec<_>>()
    .join(" ")
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn quote_windows_argument(argument: &str) -> String {
    let mut quoted = String::from("\"");
    let mut slashes = 0;
    for character in argument.chars() {
        match character {
            '\\' => slashes += 1,
            '"' => {
                quoted.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                quoted.push('"');
                slashes = 0;
            }
            _ => {
                quoted.extend(std::iter::repeat_n('\\', slashes));
                quoted.push(character);
                slashes = 0;
            }
        }
    }
    quoted.extend(std::iter::repeat_n('\\', slashes * 2));
    quoted.push('"');
    quoted
}

fn wide_null(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain(Some(0)).collect()
}

fn cancel_capture() {
    let _ = Command::new("wpr.exe").arg("-cancel").output();
}

fn command_failure(action: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        format!("{action} exited with {}", output.status)
    } else {
        format!("{action} exited with {}: {stderr}", output.status)
    }
}

fn record(message: &str) {
    let handle = HANDLE.load(Ordering::Acquire);
    if handle == 0 {
        return;
    }
    let mut wide = message.encode_utf16().collect::<Vec<_>>();
    wide.push(0);
    unsafe {
        let _ = EventWriteString(handle, INFO_LEVEL, 0, wide.as_ptr());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wpr_privilege_failure_is_selected_for_uac_retry() {
        assert!(wpr_needs_elevation(
            None,
            "failed to enable the policy to profile system performance"
        ));
        assert!(wpr_needs_elevation(Some(0xc558_5011u32 as i32), ""));
        assert!(wpr_needs_elevation(Some(0x8007_0005u32 as i32), ""));
        assert!(wpr_needs_elevation(Some(1314), ""));
        assert!(wpr_needs_elevation(
            None,
            "a required privilege is not held by the client"
        ));
        assert!(wpr_needs_elevation(None, "access is denied"));
        assert!(!wpr_needs_elevation(
            Some(1),
            "a WPR session is already running"
        ));
    }

    #[test]
    fn wpr_profile_filters_the_glulx_provider_to_the_process_executable() {
        let provider_guid = format_guid(&provider_id_for_process(1234));
        let profile = wpr_profile_xml(&provider_guid, "glulx-rs.exe", 1234);

        assert!(profile.contains(&format!("Name=\"{provider_guid}\"")));
        assert!(profile.contains("ProcessExeFilter=\"glulx-rs.exe\""));
        assert!(profile.contains("Stack=\"true\""));
        assert!(profile.contains("Description=\"PID 1234"));
        assert!(!profile.contains("SystemProvider"));
        assert!(!profile.contains("SampledProfile"));
        assert_eq!(profile.matches("LoggingMode=").count(), 2);
    }

    #[test]
    fn wpr_profile_escapes_process_executable_xml_attributes() {
        let profile = wpr_profile_xml(
            "4f9f6d5e-2c8b-4f51-9d3e-7a1e6f7c2b40",
            "glulx&\"<>.exe",
            1234,
        );

        assert!(profile.contains("ProcessExeFilter=\"glulx&amp;&quot;&lt;&gt;.exe\""));
    }

    #[test]
    fn provider_guid_is_scoped_to_the_target_process_id() {
        assert_ne!(
            format_guid(&provider_id_for_process(1234)),
            format_guid(&provider_id_for_process(1235))
        );
    }

    #[test]
    fn wpr_command_paths_drop_windows_extended_path_prefixes() {
        assert_eq!(
            wpr_command_path(Path::new(r"\\?\C:\capture\capture.wprp")),
            Path::new(r"C:\capture\capture.wprp")
        );
        assert_eq!(
            wpr_command_path(Path::new(r"\\?\UNC\host\share\capture.etl")),
            Path::new(r"\\host\share\capture.etl")
        );
    }

    #[test]
    fn elevated_helper_arguments_follow_windows_quoting_rules() {
        assert_eq!(
            quote_windows_argument("C:\\Program Files\\glulx-rs.exe"),
            "\"C:\\Program Files\\glulx-rs.exe\""
        );
        assert_eq!(quote_windows_argument("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(
            quote_windows_argument("C:\\capture dir\\"),
            "\"C:\\capture dir\\\\\""
        );
        assert_eq!(
            elevated_helper_parameters(10, 1234, Path::new("C:\\capture dir")),
            "\"--internal-etw-capture\" \"10\" \"C:\\capture dir\" \"1234\""
        );
    }
}

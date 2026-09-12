//! Local HTTP profiling and VM observability.
//!
//! The HTTP server owns the native pprof guard so the VM remains owned by its
//! event-loop thread. On Windows, ETW markers are registered for external
//! WPR/WPA CPU sampling; the metrics and VM counters remain available there.

use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::time::Instant;

#[cfg(windows)]
mod etw;

const MAX_REQUEST_BYTES: usize = 16 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const SHUTDOWN_POLL: Duration = Duration::from_millis(25);
#[cfg(any(windows, test))]
const MAX_ETW_CAPTURE_SECONDS: u64 = 60;

/// Hidden entrypoint used only for the UAC-elevated Windows capture helper.
#[cfg(windows)]
#[doc(hidden)]
pub fn run_etw_capture_helper(arguments: &[std::ffi::OsString]) -> i32 {
    if arguments.len() != 2 {
        return 2;
    }
    let Some(seconds) = arguments[0].to_str() else {
        return 2;
    };
    let result = etw::run_elevated_helper(seconds, std::path::Path::new(&arguments[1]));
    i32::from(result.is_err())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
type NativeGuard = pprof::ProfilerGuard<'static>;

/// A local profiling server. Dropping it stops the listener and native sampler.
pub struct Server {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl Server {
    pub fn address(&self) -> SocketAddr {
        self.address
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        crate::diagnostics::record(format_args!(
            "profile_http stopping address={} reason=server_drop",
            self.address
        ));
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Start a local profiling endpoint at `address`.
pub fn start(address: &str) -> io::Result<Server> {
    let listener = TcpListener::bind(address)?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    crate::diagnostics::initialize();
    #[cfg(windows)]
    if let Err(error) = etw::start() {
        crate::diagnostics::record(format_args!("etw provider registration failed={error}"));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let (ready_sender, ready_receiver) = mpsc::sync_channel::<Option<String>>(1);
    let join = thread::Builder::new()
        .name("profiling-http".to_owned())
        .spawn(move || {
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            {
                let (guard, native_error) = match build_native_guard() {
                    Ok(guard) => (Some(guard), None),
                    Err(error) => (None, Some(error)),
                };
                let _ = ready_sender.send(native_error);
                serve(listener, thread_stop, guard, Arc::new(Mutex::new(())));
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            {
                let _ = ready_sender.send(None);
                serve(listener, thread_stop, Arc::new(Mutex::new(())));
            }
        })?;
    match ready_receiver.recv_timeout(Duration::from_secs(5)) {
        Ok(native_error) => {
            if let Some(error) = &native_error {
                crate::diagnostics::record(format_args!(
                    "profile_http native_sampler_unavailable={error}"
                ));
            }
            crate::diagnostics::record(format_args!(
                "profile_http address={address} native_sampler={}",
                native_error.is_none() && cfg!(any(target_os = "linux", target_os = "macos"))
            ));
            crate::diagnostics::record(format_args!("etw_provider_enabled={}", etw_enabled()));
            Ok(Server {
                address,
                stop,
                join: Some(join),
            })
        }
        Err(error) => {
            stop.store(true, Ordering::Release);
            let _ = join.join();
            Err(io::Error::other(format!(
                "profiling server did not start: {error}"
            )))
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn serve(
    listener: TcpListener,
    stop: Arc<AtomicBool>,
    guard: Option<NativeGuard>,
    capture: Arc<Mutex<()>>,
) {
    let guard = Arc::new(Mutex::new(guard));
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let guard = Arc::clone(&guard);
                let stop = Arc::clone(&stop);
                let capture = Arc::clone(&capture);
                let _ = thread::spawn(move || handle_connection(stream, guard, stop, capture));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(SHUTDOWN_POLL);
            }
            Err(error) => {
                crate::diagnostics::record(format_args!("profile_http accept_error={error}"));
                break;
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn serve(listener: TcpListener, stop: Arc<AtomicBool>, capture: Arc<Mutex<()>>) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let stop = Arc::clone(&stop);
                let capture = Arc::clone(&capture);
                let _ = thread::spawn(move || handle_connection(stream, stop, capture));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(SHUTDOWN_POLL);
            }
            Err(error) => {
                crate::diagnostics::record(format_args!("profile_http accept_error={error}"));
                break;
            }
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn handle_connection(
    mut stream: TcpStream,
    guard: Arc<Mutex<Option<NativeGuard>>>,
    stop: Arc<AtomicBool>,
    capture: Arc<Mutex<()>>,
) {
    if let Err(error) = stream.set_read_timeout(Some(REQUEST_TIMEOUT)) {
        crate::diagnostics::record(format_args!("profile_http timeout_error={error}"));
        return;
    }
    let request = match read_request(&mut stream) {
        Ok(request) => request,
        Err(error) => {
            let _ = write_response(
                &mut stream,
                400,
                "text/plain; charset=utf-8",
                error.to_string().as_bytes(),
            );
            return;
        }
    };
    dispatch(&mut stream, &request, &guard, &stop, &capture);
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn handle_connection(mut stream: TcpStream, stop: Arc<AtomicBool>, capture: Arc<Mutex<()>>) {
    if let Err(error) = stream.set_read_timeout(Some(REQUEST_TIMEOUT)) {
        crate::diagnostics::record(format_args!("profile_http timeout_error={error}"));
        return;
    }
    let request = match read_request(&mut stream) {
        Ok(request) => request,
        Err(error) => {
            let _ = write_response(
                &mut stream,
                400,
                "text/plain; charset=utf-8",
                error.to_string().as_bytes(),
            );
            return;
        }
    };
    dispatch(&mut stream, &request, &stop, &capture);
}

fn read_request(stream: &mut TcpStream) -> io::Result<String> {
    let mut request = Vec::with_capacity(1024);
    let mut buffer = [0u8; 1024];
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..count]);
        if request.windows(4).any(|window| window == b"\r\n\r\n")
            || request.windows(2).any(|window| window == b"\n\n")
        {
            break;
        }
        if request.len() >= MAX_REQUEST_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "request headers are too large",
            ));
        }
    }
    String::from_utf8(request)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "request is not UTF-8"))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn dispatch(
    stream: &mut TcpStream,
    request: &str,
    guard: &Arc<Mutex<Option<NativeGuard>>>,
    stop: &AtomicBool,
    capture: &Mutex<()>,
) {
    let (method, target) = request_line(request);
    if method != Some("GET") {
        let _ = write_response(stream, 405, "text/plain; charset=utf-8", b"GET required\n");
        return;
    }
    let Some(target) = target else {
        let _ = write_response(
            stream,
            400,
            "text/plain; charset=utf-8",
            b"invalid request\n",
        );
        return;
    };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    match path {
        "/" | "/debug/pprof/" => write_index(stream),
        "/debug/metrics" | "/debug/vm" | "/debug/opcodes" => {
            let body = crate::diagnostics::snapshot_json();
            let _ = write_response(stream, 200, "application/json; charset=utf-8", &body);
        }
        "/debug/etw" => {
            let body = etw_status_json();
            let _ = write_response(stream, 200, "application/json; charset=utf-8", &body);
        }
        "/debug/etw/profile" => write_etw_profile(stream, query, stop),
        "/debug/pprof/profile" => {
            let _capture = capture.lock().unwrap_or_else(|e| e.into_inner());
            let mut guard = guard.lock().unwrap_or_else(|e| e.into_inner());
            write_profile_or_unavailable(stream, &mut guard, query, false, stop);
        }
        "/debug/pprof/flamegraph" => {
            let _capture = capture.lock().unwrap_or_else(|e| e.into_inner());
            let mut guard = guard.lock().unwrap_or_else(|e| e.into_inner());
            write_profile_or_unavailable(stream, &mut guard, query, true, stop);
        }
        _ => {
            let _ = write_response(stream, 404, "text/plain; charset=utf-8", b"not found\n");
        }
    };
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn dispatch(stream: &mut TcpStream, request: &str, stop: &AtomicBool, capture: &Mutex<()>) {
    let (method, target) = request_line(request);
    if method != Some("GET") {
        let _ = write_response(stream, 405, "text/plain; charset=utf-8", b"GET required\n");
        return;
    }
    let Some(target) = target else {
        let _ = write_response(
            stream,
            400,
            "text/plain; charset=utf-8",
            b"invalid request\n",
        );
        return;
    };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    match path {
        "/" | "/debug/pprof/" => write_index(stream),
        "/debug/metrics" | "/debug/vm" | "/debug/opcodes" => {
            let body = crate::diagnostics::snapshot_json();
            let _ = write_response(stream, 200, "application/json; charset=utf-8", &body);
        }
        "/debug/etw" => {
            let body = etw_status_json();
            let _ = write_response(stream, 200, "application/json; charset=utf-8", &body);
        }
        "/debug/etw/profile" => {
            let _capture = capture.lock().unwrap_or_else(|e| e.into_inner());
            write_etw_profile(stream, query, stop);
        }
        "/debug/pprof/profile" | "/debug/pprof/flamegraph" => {
            let body = br#"{"error":"native pprof-rs sampling is unavailable on Windows; use /debug/etw/profile or /debug/metrics"}"#;
            let _ = write_response(stream, 501, "application/json; charset=utf-8", body);
        }
        _ => {
            let _ = write_response(stream, 404, "text/plain; charset=utf-8", b"not found\n");
        }
    };
}

fn request_line(request: &str) -> (Option<&str>, Option<&str>) {
    let line = request.lines().next().unwrap_or_default();
    let mut parts = line.split_ascii_whitespace();
    (parts.next(), parts.next())
}

#[cfg(windows)]
pub(crate) fn etw_enabled() -> bool {
    etw::enabled()
}

#[cfg(not(windows))]
pub(crate) const fn etw_enabled() -> bool {
    false
}

#[cfg(windows)]
pub(crate) fn etw_status_json() -> Vec<u8> {
    etw::status_json()
}

#[cfg(not(windows))]
pub(crate) fn etw_status_json() -> Vec<u8> {
    br#"{"enabled":false,"provider":"not-supported","sampling":"pprof-rs on Linux/macOS","markers":"Windows ETW only"}"#.to_vec()
}

#[cfg(windows)]
pub(crate) fn record_stage(name: &str, elapsed: Duration) {
    etw::record_stage(name, elapsed);
}

#[cfg(not(windows))]
pub(crate) fn record_stage(_name: &str, _elapsed: Duration) {}

#[cfg(windows)]
pub(crate) fn record_vm(snapshot: &crate::vm::VmDiagnosticSnapshot) {
    etw::record_vm(snapshot);
}

#[cfg(not(windows))]
pub(crate) fn record_vm(_snapshot: &crate::vm::VmDiagnosticSnapshot) {}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn build_native_guard() -> Result<NativeGuard, String> {
    pprof::ProfilerGuardBuilder::default()
        .frequency(99)
        .build()
        .map_err(|error| error.to_string())
}

fn profile_seconds(query: &str) -> Result<Option<u64>, String> {
    query
        .split('&')
        .find_map(|part| part.strip_prefix("seconds="))
        .map(|value| {
            value
                .parse::<u64>()
                .map(|seconds| seconds.min(300))
                .map(Some)
                .map_err(|_| "seconds must be an integer".to_owned())
        })
        .unwrap_or(Ok(None))
}

#[cfg(any(windows, test))]
fn etw_capture_seconds(query: &str) -> Result<u64, String> {
    let seconds = profile_seconds(query)?.unwrap_or(10);
    if seconds == 0 {
        return Err("ETW capture duration must be at least one second".to_owned());
    }
    Ok(seconds.min(MAX_ETW_CAPTURE_SECONDS))
}

#[cfg(windows)]
fn write_etw_profile(stream: &mut TcpStream, query: &str, stop: &AtomicBool) {
    match etw::capture_profile(query, stop) {
        Ok(body) => {
            let _ = write_response(stream, 200, "application/vnd.ms-wpr.etl", &body);
        }
        Err(error) => {
            let body = format!("ETW capture failed: {error}\n");
            let status = if error == "UAC consent was canceled" {
                403
            } else {
                500
            };
            let _ = write_response(stream, status, "text/plain; charset=utf-8", body.as_bytes());
        }
    }
}

#[cfg(not(windows))]
fn write_etw_profile(stream: &mut TcpStream, _query: &str, _stop: &AtomicBool) {
    let body = b"ETW CPU capture is only available on Windows\n";
    let _ = write_response(stream, 501, "text/plain; charset=utf-8", body);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn write_profile_or_unavailable(
    stream: &mut TcpStream,
    guard: &mut Option<NativeGuard>,
    query: &str,
    flamegraph: bool,
    stop: &AtomicBool,
) {
    let Some(_) = guard.as_ref() else {
        let body = br#"{"error":"native pprof-rs sampling is unavailable; use /debug/metrics"}"#;
        let _ = write_response(stream, 501, "application/json; charset=utf-8", body);
        return;
    };
    let seconds = match profile_seconds(query) {
        Ok(seconds) => seconds,
        Err(error) => {
            let _ = write_response(stream, 400, "text/plain; charset=utf-8", error.as_bytes());
            return;
        }
    };
    if let Some(seconds) = seconds {
        drop(guard.take());
        match build_native_guard() {
            Ok(new_guard) => *guard = Some(new_guard),
            Err(error) => {
                let body = format!("profiling sampler failed to restart: {error}\n");
                let _ = write_response(stream, 500, "text/plain; charset=utf-8", body.as_bytes());
                return;
            }
        }
        let deadline = Instant::now() + Duration::from_secs(seconds);
        while !stop.load(Ordering::Acquire) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            thread::sleep(remaining.min(SHUTDOWN_POLL));
        }
        if stop.load(Ordering::Acquire) {
            return;
        }
    }
    let Some(guard) = guard.as_ref() else {
        let body = br#"{"error":"native pprof-rs sampling is unavailable; use /debug/metrics"}"#;
        let _ = write_response(stream, 501, "application/json; charset=utf-8", body);
        return;
    };
    write_profile(stream, guard, query, flamegraph);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn write_profile(stream: &mut TcpStream, guard: &NativeGuard, query: &str, flamegraph: bool) {
    let svg = flamegraph || query.split('&').any(|part| part == "format=svg");
    let result = guard
        .report()
        .build()
        .map_err(|error| error.to_string())
        .and_then(|report| {
            let mut body = Vec::new();
            if svg {
                report
                    .flamegraph(&mut body)
                    .map_err(|error| error.to_string())?;
            } else {
                use pprof::protos::Message;
                report
                    .pprof()
                    .map_err(|error| error.to_string())?
                    .encode(&mut body)
                    .map_err(|error| error.to_string())?;
            }
            Ok(body)
        });
    match result {
        Ok(body) => {
            let content_type = if svg {
                "image/svg+xml; charset=utf-8"
            } else {
                "application/octet-stream"
            };
            let _ = write_response(stream, 200, content_type, &body);
        }
        Err(error) => {
            let body = format!("profiling report failed: {error}\n");
            let _ = write_response(stream, 500, "text/plain; charset=utf-8", body.as_bytes());
        }
    }
}

fn write_index(stream: &mut TcpStream) {
    let body = br#"<!doctype html>
<meta charset="utf-8">
<title>Glulx profiler</title>
<h1>Glulx profiler</h1>
<ul>
<li><a href="/debug/metrics">/debug/metrics</a> - VM and host metrics (JSON)</li>
<li><a href="/debug/vm">/debug/vm</a> - current VM snapshot (JSON)</li>
<li><a href="/debug/opcodes">/debug/opcodes</a> - opcode counters (JSON)</li>
<li><a href="/debug/etw">/debug/etw</a> - Windows ETW provider status</li>
<li><a href="/debug/etw/profile?seconds=10">/debug/etw/profile?seconds=10</a> - Windows ETL CPU profile</li>
<li><a href="/debug/pprof/profile">/debug/pprof/profile</a> - pprof protobuf snapshot</li>
<li><a href="/debug/pprof/flamegraph">/debug/pprof/flamegraph</a> - SVG flamegraph</li>
</ul>
"#;
    let _ = write_response(stream, 200, "text/html; charset=utf-8", body);
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        403 => "Forbidden",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        _ => "Response",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_line_accepts_only_the_first_request_line() {
        assert_eq!(
            request_line("GET /debug/metrics HTTP/1.1\r\nHost: localhost\r\n"),
            (Some("GET"), Some("/debug/metrics"))
        );
        assert_eq!(
            request_line("POST / HTTP/1.1\n\n"),
            (Some("POST"), Some("/"))
        );
        assert_eq!(request_line("garbage"), (Some("garbage"), None));
    }

    #[test]
    fn request_target_can_split_query_without_decoding_paths() {
        let target = "/debug/pprof/profile?format=svg&seconds=10";
        assert_eq!(
            target.split_once('?'),
            Some(("/debug/pprof/profile", "format=svg&seconds=10"))
        );
    }

    #[test]
    fn etw_capture_duration_is_bounded() {
        assert_eq!(etw_capture_seconds("").unwrap(), 10);
        assert_eq!(etw_capture_seconds("seconds=20").unwrap(), 20);
        assert_eq!(etw_capture_seconds("seconds=90").unwrap(), 60);
        assert!(etw_capture_seconds("seconds=0").is_err());
        assert!(etw_capture_seconds("seconds=bad").is_err());
    }

    #[test]
    fn profile_duration_is_bounded_and_validated() {
        assert_eq!(profile_seconds("format=svg&seconds=10").unwrap(), Some(10));
        assert_eq!(profile_seconds("seconds=999").unwrap(), Some(300));
        assert_eq!(profile_seconds("format=svg").unwrap(), None);
        assert!(profile_seconds("seconds=bad").is_err());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    #[ignore = "requires local socket permissions"]
    fn server_serves_a_json_metrics_snapshot() {
        let server = start("127.0.0.1:0").expect("native profiler should start");
        let mut stream = TcpStream::connect(server.address()).unwrap();
        stream
            .write_all(b"GET /debug/metrics HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        let response = String::from_utf8(response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("Content-Type: application/json; charset=utf-8\r\n"));
        let body = response.split_once("\r\n\r\n").unwrap().1;
        let json: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(json["stage"], "event-loop");
    }
}

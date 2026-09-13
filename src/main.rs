#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::{ffi::OsString, path::PathBuf};

use glulx_rs::app::PlayerApp;
use glulx_rs::memory_budget::{Budget, MemoryPolicy, Overrides, startup_snapshot};
use glulx_rs::{ResourceSelection, Story, Vm};

const USAGE: &str = "Usage: glulx-rs [--headless] [--strict-glk] [--trace-events PATH] [--profile-http ADDRESS] [--profile-token TOKEN] [--max-memory MIB] [--max-process-memory MIB] [--diagnostics LOG] [--resources PATH] [--no-auto-resources] [STORY]\n\n--headless          Play in the terminal (plain text when input or output is piped)\n--strict-glk        Fail when a story calls an unknown Glk selector\n--trace-events PATH Write delivered Glk events as JSON after headless playback\n--profile-http ADDRESS  Listen for profiling and metrics HTTP requests\n--profile-token TOKEN   Require Authorization: Bearer TOKEN for profiling requests\n--max-memory MIB   Game VM limit: MiB or percentage (e.g. 1024 or 25%)\n--max-process-memory MIB  OS hard limit: MiB or percentage (0 disables)\n--max-undo-memory SIZE       Undo payload budget\n--max-graphics-cache SIZE    Graphics cache budget\n--max-text-image-cache SIZE  Text image cache budget\n--max-decoded-image SIZE     Per-picture RGBA budget\n--max-audio-resource SIZE    Per-audio encoded budget\n--max-song-pcm SIZE          SONG PCM budget\nAll SIZE values accept MiB or 1%–100%. CLI overrides JSON for this run only.\n--diagnostics LOG   Write diagnostic heartbeats and slow operations to LOG\n--resources PATH    Use this Blorb archive or loose resource directory\n--no-auto-resources Disable discovery of same-name external resource archives\n--help              Show this help\n\nDebug builds default to ./glulx-debug.log and the profiling endpoint 127.0.0.1:6060.\nRelease builds enable profiling only with --profile-http. Non-loopback profiling addresses require a token; GLULX_PROFILE_TOKEN is also accepted.\n\nAn explicit --resources path takes priority over --no-auto-resources.";

#[derive(Debug)]
struct Arguments {
    headless: bool,
    strict_glk: bool,
    trace_events: Option<PathBuf>,
    max_memory_mib: Option<Budget>,
    max_process_memory_mib: Option<Budget>,
    resource_overrides: std::collections::BTreeMap<String, Budget>,
    story: Option<PathBuf>,
    resources: ResourceSelection,
    diagnostics: Option<PathBuf>,
    profile_http: Option<String>,
    profile_token: Option<String>,
}

fn parse_arguments(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<Option<Arguments>, String> {
    let mut arguments = arguments.into_iter();
    let mut diagnostics = None;
    let mut profile_http = None;
    let mut profile_http_explicit = false;
    let mut profile_token = None;
    let mut profile_token_explicit = false;
    let mut resource_overrides = std::collections::BTreeMap::new();
    let mut max_memory_mib = None;
    let mut max_process_memory_mib = None;
    let mut strict_glk = false;
    let mut trace_events = None;
    let (mut headless, mut story, mut explicit, mut automatic, mut positional) =
        (false, None, None, true, false);
    while let Some(argument) = arguments.next() {
        if !positional && argument == "--help" {
            return Ok(None);
        } else if !positional && argument == "--" {
            positional = true;
        } else if !positional && argument == "--headless" {
            headless = true;
        } else if !positional && argument == "--strict-glk" {
            strict_glk = true;
        } else if !positional && argument == "--trace-events" {
            let path = arguments
                .next()
                .ok_or("--trace-events requires a JSON path")?;
            if trace_events.replace(PathBuf::from(path)).is_some() {
                return Err("Specify --trace-events only once".to_owned());
            }
        } else if !positional
            && matches!(
                argument.to_str(),
                Some(
                    "--max-memory"
                        | "--max-process-memory"
                        | "--max-undo-memory"
                        | "--max-graphics-cache"
                        | "--max-text-image-cache"
                        | "--max-decoded-image"
                        | "--max-audio-resource"
                        | "--max-song-pcm"
                )
            )
        {
            let name = argument.to_str().unwrap();
            let value = arguments
                .next()
                .ok_or_else(|| format!("{name} requires a MiB value or percentage"))?;
            let value: Budget = value.to_str().ok_or("invalid memory value")?.parse()?;
            if name == "--max-memory" && matches!(value, Budget::Fixed(0)) {
                return Err("game memory limit must be positive".to_owned());
            }
            let duplicate = match name {
                "--max-memory" => max_memory_mib.replace(value).is_some(),
                "--max-process-memory" => max_process_memory_mib.replace(value).is_some(),
                _ => resource_overrides.insert(name.to_owned(), value).is_some(),
            };
            if duplicate {
                return Err(format!("Specify {name} only once"));
            }
        } else if !positional && argument == "--diagnostics" {
            let path = arguments
                .next()
                .ok_or("--diagnostics requires a log path")?;
            if diagnostics.replace(PathBuf::from(path)).is_some() {
                return Err("Specify --diagnostics only once".to_owned());
            }
        } else if !positional && argument == "--profile-http" {
            let address = arguments
                .next()
                .ok_or("--profile-http requires an address such as 127.0.0.1:6060")?;
            if profile_http_explicit {
                return Err("Specify --profile-http only once".to_owned());
            }
            profile_http_explicit = true;
            profile_http = Some(
                address
                    .to_str()
                    .ok_or("--profile-http address must be valid UTF-8")?
                    .to_owned(),
            );
        } else if !positional && argument == "--profile-token" {
            let token = arguments.next().ok_or("--profile-token requires a token")?;
            if profile_token_explicit {
                return Err("Specify --profile-token only once".to_owned());
            }
            profile_token_explicit = true;
            let token = token
                .to_str()
                .ok_or("--profile-token must be valid UTF-8")?;
            if token.is_empty() {
                return Err("--profile-token must not be empty".to_owned());
            }
            profile_token = Some(token.to_owned());
        } else if !positional && argument == "--no-auto-resources" {
            automatic = false;
        } else if !positional && argument == "--resources" {
            let path = arguments.next().ok_or("--resources requires a path")?;
            if explicit.replace(PathBuf::from(path)).is_some() {
                return Err("Specify --resources only once".to_owned());
            }
        } else if !positional && argument.to_string_lossy().starts_with('-') {
            return Err(format!("Unknown option: {}", argument.to_string_lossy()));
        } else if story.replace(PathBuf::from(argument)).is_some() {
            return Err("Specify only one story".to_owned());
        }
    }
    if headless && story.is_none() {
        return Err("--headless requires a story".to_owned());
    }
    if trace_events.is_some() && !headless {
        return Err("--trace-events requires --headless".to_owned());
    }
    if story.is_none() && (explicit.is_some() || !automatic) {
        return Err("Resource options require a story".to_owned());
    }
    let profile_http =
        profile_http.or_else(|| cfg!(debug_assertions).then(|| "127.0.0.1:6060".to_owned()));
    if profile_token.is_some() && profile_http.is_none() {
        return Err("--profile-token requires --profile-http".to_owned());
    }
    Ok(Some(Arguments {
        headless,
        strict_glk,
        trace_events,
        max_memory_mib,
        resource_overrides,
        max_process_memory_mib,
        diagnostics: diagnostics
            .or_else(|| cfg!(debug_assertions).then(|| PathBuf::from("glulx-debug.log"))),
        profile_http,
        profile_token,
        story,
        resources: explicit.map_or_else(
            || {
                if automatic {
                    ResourceSelection::Auto
                } else {
                    ResourceSelection::None
                }
            },
            ResourceSelection::Path,
        ),
    }))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    #[cfg(windows)]
    if arguments
        .first()
        .is_some_and(|argument| argument == "--internal-etw-capture")
    {
        std::process::exit(glulx_rs::profiling::run_etw_capture_helper(&arguments[1..]));
    }
    #[cfg(windows)]
    attach_console(
        arguments
            .iter()
            .any(|argument| argument == "--headless" || argument == "--help"),
    );
    let Some(arguments) =
        parse_arguments(arguments).map_err(|error| format!("{error}\n\n{USAGE}"))?
    else {
        println!("{USAGE}");
        return Ok(());
    };
    // Snapshot before limiting our own address space or initializing the GUI.
    let snapshot = startup_snapshot();
    let overrides = Overrides {
        game: arguments.max_memory_mib,
        process: arguments.max_process_memory_mib,
        resources: arguments.resource_overrides,
    };
    let policy = overrides.apply(MemoryPolicy::load()?);
    let game_limit = policy.max_memory_mib.vm_bytes(snapshot)?;
    let resources = policy.resource_limits.resolve(snapshot)?;
    let process_limit = policy.max_process_memory_mib.resolve(snapshot)?;
    glulx_rs::process_memory::apply_limit_bytes(process_limit)
        .map_err(|error| format!("Could not enforce process memory limit: {error}"))?;
    if let Some(path) = &arguments.diagnostics {
        glulx_rs::diagnostics::start(path)?;
    }
    let _profile_server = arguments
        .profile_http
        .as_deref()
        .map(|address| glulx_rs::profiling::start(address, arguments.profile_token.as_deref()))
        .transpose()
        .map_err(|error| format!("Could not start profiling HTTP server: {error}"))?;
    if arguments.headless {
        let story = Story::open_with_resources(arguments.story.unwrap(), arguments.resources)?;
        let mut vm = Vm::new_with_memory_limit(story, game_limit)?;
        vm.set_strict_glk(arguments.strict_glk);
        vm.set_resource_limits(resources);
        return glulx_rs::terminal::run_with_event_trace(vm, arguments.trace_events);
    }
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Glulx Player")
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!(
                "../assets/icon.png"
            ))?)
            .with_inner_size([1100.0, 760.0])
            .with_min_inner_size([720.0, 480.0])
            .with_drag_and_drop(true),
        glow_options: eframe::egui_glow::GlowConfiguration {
            vsync: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let gui_result = eframe::run_native(
        "Glulx Player",
        options,
        Box::new(move |creation| {
            Ok(Box::new(PlayerApp::new_with_memory_policy_and_strict_glk(
                creation,
                arguments.story,
                arguments.resources,
                overrides,
                arguments.strict_glk,
            )))
        }),
    );
    match gui_result {
        Ok(()) => {
            glulx_rs::diagnostics::record(format_args!("gui_exit status=ok"));
            Ok(())
        }
        Err(error) => {
            glulx_rs::diagnostics::record(format_args!("gui_exit status=error error={error}"));
            Err(error.into())
        }
    }
}

#[cfg(windows)]
fn attach_console(allocate: bool) {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AllocConsole, AttachConsole};
    // GUI-subsystem executables do not automatically inherit a console.
    // These Win32 calls take no pointers and preserve explicitly redirected
    // handles; an existing console makes both calls harmlessly fail.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 && allocate {
            AllocConsole();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<Option<Arguments>, String> {
        parse_arguments(arguments.iter().map(OsString::from))
    }

    #[test]
    fn memory_limit_accepts_mib_and_rejects_invalid_values() {
        assert_eq!(
            parse(&["--max-memory", "512"])
                .unwrap()
                .unwrap()
                .max_memory_mib,
            Some(Budget::Fixed(512))
        );
        assert_eq!(parse(&[]).unwrap().unwrap().max_memory_mib, None);
        for value in ["0", "-1", "1.5", "garbage", "4294967296"] {
            assert!(parse(&["--max-memory", value]).is_err());
        }
        assert_eq!(
            parse(&["--max-process-memory", "2048"])
                .unwrap()
                .unwrap()
                .max_process_memory_mib,
            Some(Budget::Fixed(2048))
        );
        assert_eq!(
            parse(&["--max-process-memory", "0"])
                .unwrap()
                .unwrap()
                .max_process_memory_mib,
            Some(Budget::Fixed(0))
        );
        assert!(parse(&["--max-process-memory", "101%"]).is_err());
        assert!(parse(&["--max-process-memory", "1", "--max-process-memory", "2"]).is_err());
        assert!(parse(&["--max-process-memory"]).is_err());
        assert!(parse(&["--max-memory"]).is_err());
        assert!(parse(&["--max-memory", "1", "--max-memory", "2"]).is_err());
    }

    #[test]
    fn command_line_percentages_override_all_memory_categories() {
        let arguments = parse(&[
            "--headless",
            "game.ulx",
            "--max-memory",
            "25%",
            "--max-process-memory",
            "75%",
            "--max-undo-memory",
            "10%",
            "--max-graphics-cache",
            "8192",
            "--max-text-image-cache",
            "5%",
            "--max-decoded-image",
            "2048",
            "--max-audio-resource",
            "4096",
            "--max-song-pcm",
            "1%",
        ])
        .unwrap()
        .unwrap();
        let policy = Overrides {
            game: arguments.max_memory_mib,
            process: arguments.max_process_memory_mib,
            resources: arguments.resource_overrides,
        }
        .apply(MemoryPolicy::default());
        assert_eq!(policy.max_memory_mib, Budget::Percent { percent: 25 });
        assert_eq!(
            policy.max_process_memory_mib,
            Budget::Percent { percent: 75 }
        );
        assert_eq!(
            policy.resource_limits.graphics_cache_mib,
            Budget::Fixed(8192)
        );
        assert_eq!(
            policy.resource_limits.undo_mib,
            Budget::Percent { percent: 10 }
        );
        assert!(parse(&["--max-undo-memory", "1", "--max-undo-memory", "2%"]).is_err());
    }

    #[test]
    fn debug_builds_default_to_a_log_in_the_working_directory() {
        let args = parse(&["game.gblorb"]).unwrap().unwrap();
        let expected = cfg!(debug_assertions).then(|| PathBuf::from("glulx-debug.log"));
        assert_eq!(args.diagnostics, expected);
        let expected_profile = cfg!(debug_assertions).then(|| "127.0.0.1:6060".to_owned());
        assert_eq!(args.profile_http, expected_profile);
        assert_eq!(args.profile_token, None);
    }

    #[test]
    fn diagnostics_accepts_a_log_path_and_rejects_ambiguous_options() {
        let args = parse(&["--diagnostics", "player log.txt", "game.gblorb"])
            .unwrap()
            .unwrap();
        assert_eq!(args.diagnostics, Some(PathBuf::from("player log.txt")));
        assert_eq!(args.story, Some(PathBuf::from("game.gblorb")));
        assert!(!args.headless);
        assert!(parse(&["--diagnostics"]).is_err());
        assert!(parse(&["--diagnostics", "a", "--diagnostics", "b"]).is_err());
    }

    #[test]
    fn profiling_endpoint_accepts_an_explicit_address_and_rejects_duplicates() {
        let args = parse(&["--profile-http", "127.0.0.1:0", "game.gblorb"])
            .unwrap()
            .unwrap();
        assert_eq!(args.profile_http, Some("127.0.0.1:0".to_owned()));
        assert!(parse(&["--profile-http"]).is_err());
        assert!(
            parse(&[
                "--profile-http",
                "127.0.0.1:1",
                "--profile-http",
                "127.0.0.1:2"
            ])
            .is_err()
        );
        let args = parse(&[
            "--profile-http",
            "0.0.0.0:6060",
            "--profile-token",
            "secret",
            "game.gblorb",
        ])
        .unwrap()
        .unwrap();
        assert_eq!(args.profile_token, Some("secret".to_owned()));
        assert!(parse(&["--profile-token"]).is_err());
        assert!(parse(&["--profile-token", ""]).is_err());
        assert!(parse(&["--profile-token", "a", "--profile-token", "b"]).is_err());
    }

    #[test]
    fn strict_glk_mode_is_parsed_for_reference_checks() {
        let args = parse(&["--strict-glk", "game.ulx"]).unwrap().unwrap();
        assert!(args.strict_glk);
        assert!(!parse(&["game.ulx"]).unwrap().unwrap().strict_glk);
    }

    #[test]
    fn event_trace_path_is_parsed_without_becoming_a_story_argument() {
        let args = parse(&["--headless", "--trace-events", "events.json", "game.ulx"])
            .unwrap()
            .unwrap();
        assert_eq!(args.trace_events, Some(PathBuf::from("events.json")));
        assert_eq!(args.story, Some(PathBuf::from("game.ulx")));
    }

    #[test]
    fn explicit_resources_override_discovery_flags_in_either_order() {
        for arguments in [
            vec![
                "--headless",
                "story.ulx",
                "--resources",
                "media",
                "--no-auto-resources",
            ],
            vec![
                "--no-auto-resources",
                "--resources",
                "media",
                "story.ulx",
                "--headless",
            ],
        ] {
            let result = parse(&arguments).unwrap().unwrap();
            assert!(result.headless);
            assert_eq!(result.story, Some(PathBuf::from("story.ulx")));
            assert!(
                matches!(result.resources, ResourceSelection::Path(path) if path == std::path::Path::new("media"))
            );
        }
        assert!(matches!(
            parse(&["story.ulx"]).unwrap().unwrap().resources,
            ResourceSelection::Auto
        ));
        assert!(matches!(
            parse(&["story.ulx", "--no-auto-resources"])
                .unwrap()
                .unwrap()
                .resources,
            ResourceSelection::None
        ));
    }

    #[test]
    fn incomplete_and_ambiguous_arguments_fail_before_opening_the_gui() {
        for arguments in [
            vec!["--headless"],
            vec!["--resources"],
            vec!["--resources", "media"],
            vec!["--no-auto-resources"],
            vec!["--profile-http"],
            vec!["one.ulx", "two.ulx"],
            vec!["--typo"],
            vec!["story.ulx", "--resources", "one", "--resources", "two"],
        ] {
            assert!(parse(&arguments).is_err(), "{arguments:?}");
        }
        assert!(parse(&["--help"]).unwrap().is_none());
        assert_eq!(
            parse(&["--", "-story.ulx"]).unwrap().unwrap().story,
            Some(PathBuf::from("-story.ulx"))
        );
    }
}

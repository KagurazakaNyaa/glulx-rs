#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::{ffi::OsString, path::PathBuf};

use glulx_rs::app::PlayerApp;
use glulx_rs::memory_budget::{Budget, MemoryPolicy, Overrides, startup_snapshot};
use glulx_rs::{ResourceSelection, Story, Vm};

const USAGE: &str = "Usage: glulx-rs [--headless] [--max-memory MIB] [--max-process-memory MIB] [--diagnostics LOG] [--resources PATH] [--no-auto-resources] [STORY]\n\n--headless          Play in the terminal (plain text when input or output is piped)\n--max-memory MIB   Game VM limit: MiB or percentage (e.g. 1024 or 25%)\n--max-process-memory MIB  OS hard limit: MiB or percentage (0 disables)\n--max-undo-memory SIZE       Undo payload budget\n--max-graphics-cache SIZE    Graphics cache budget\n--max-text-image-cache SIZE  Text image cache budget\n--max-decoded-image SIZE     Per-picture RGBA budget\n--max-audio-resource SIZE    Per-audio encoded budget\n--max-song-pcm SIZE          SONG PCM budget\nAll SIZE values accept MiB or 1%–100%. CLI overrides JSON for this run only.\n--diagnostics LOG   Write diagnostic heartbeats and slow operations to LOG\n--resources PATH    Use this Blorb archive or loose resource directory\n--no-auto-resources Disable discovery of same-name external resource archives\n--help              Show this help\n\nDebug builds default to ./glulx-debug.log; --diagnostics overrides it.\n\nAn explicit --resources path takes priority over --no-auto-resources.";

#[derive(Debug)]
struct Arguments {
    headless: bool,
    max_memory_mib: Option<Budget>,
    max_process_memory_mib: Option<Budget>,
    resource_overrides: std::collections::BTreeMap<String, Budget>,
    story: Option<PathBuf>,
    resources: ResourceSelection,
    diagnostics: Option<PathBuf>,
}

fn parse_arguments(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<Option<Arguments>, String> {
    let mut arguments = arguments.into_iter();
    let mut diagnostics = None;
    let mut resource_overrides = std::collections::BTreeMap::new();
    let mut max_memory_mib = None;
    let mut max_process_memory_mib = None;
    let (mut headless, mut story, mut explicit, mut automatic, mut positional) =
        (false, None, None, true, false);
    while let Some(argument) = arguments.next() {
        if !positional && argument == "--help" {
            return Ok(None);
        } else if !positional && argument == "--" {
            positional = true;
        } else if !positional && argument == "--headless" {
            headless = true;
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
    if story.is_none() && (explicit.is_some() || !automatic) {
        return Err("Resource options require a story".to_owned());
    }
    Ok(Some(Arguments {
        headless,
        max_memory_mib,
        resource_overrides,
        max_process_memory_mib,
        diagnostics: diagnostics
            .or_else(|| cfg!(debug_assertions).then(|| PathBuf::from("glulx-debug.log"))),
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
    if arguments.headless {
        let story = Story::open_with_resources(arguments.story.unwrap(), arguments.resources)?;
        let mut vm = Vm::new_with_memory_limit(story, game_limit)?;
        vm.set_resource_limits(resources);
        return glulx_rs::terminal::run(vm);
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
        ..Default::default()
    };
    eframe::run_native(
        "Glulx Player",
        options,
        Box::new(move |creation| {
            Ok(Box::new(PlayerApp::new_with_memory_policy(
                creation,
                arguments.story,
                arguments.resources,
                overrides,
            )))
        }),
    )?;
    Ok(())
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

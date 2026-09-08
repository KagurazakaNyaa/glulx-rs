#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::{ffi::OsString, path::PathBuf};

use glulx_rs::app::PlayerApp;
use glulx_rs::{ResourceSelection, Story, Vm};

const USAGE: &str = "Usage: glulx-rs [--headless] [--resources PATH] [--no-auto-resources] [STORY]\n\n--headless          Play in the terminal (plain text when input or output is piped)\n--resources PATH    Use this Blorb archive or loose resource directory\n--no-auto-resources Disable discovery of same-name external resource archives\n--help              Show this help\n\nAn explicit --resources path takes priority over --no-auto-resources.";

#[derive(Debug)]
struct Arguments {
    headless: bool,
    story: Option<PathBuf>,
    resources: ResourceSelection,
}

fn parse_arguments(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<Option<Arguments>, String> {
    let mut arguments = arguments.into_iter();
    let (mut headless, mut story, mut explicit, mut automatic, mut positional) =
        (false, None, None, true, false);
    while let Some(argument) = arguments.next() {
        if !positional && argument == "--help" {
            return Ok(None);
        } else if !positional && argument == "--" {
            positional = true;
        } else if !positional && argument == "--headless" {
            headless = true;
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
    if arguments.headless {
        let story = Story::open_with_resources(arguments.story.unwrap(), arguments.resources)?;
        return glulx_rs::terminal::run(Vm::new(story)?);
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
            Ok(Box::new(PlayerApp::new_with_resources(
                creation,
                arguments.story,
                arguments.resources,
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

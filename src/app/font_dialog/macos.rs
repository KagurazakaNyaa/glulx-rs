use super::super::i18n::Language;
use std::{path::PathBuf, process::Command};

fn dialog(script: &str, argument: &str, language: Language) -> Result<Option<String>, String> {
    let labels = serde_json::json!({
        "family": language.text("ui.choose_a_font_family"),
        "instructions": language.text("ui.choose_in_the_fonts_panel_then_click_use"),
        "use": language.text("ui.use_font"),
        "cancel": language.text("ui.cancel"),
        "file": language.text("ui.choose_font_file_label"),
    })
    .to_string();
    let output = Command::new("/usr/bin/osascript")
        .args(["-l", "JavaScript", "-e", script, "--", argument, &labels])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let result = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
    Ok((!result.trim().is_empty()).then(|| result.trim_end_matches('\n').to_owned()))
}

pub(crate) fn choose_family(current: &str, language: Language) -> Result<Option<String>, String> {
    // JXA ships with macOS. AppKit owns the actual system font panel; the
    // confirmation alert supplies an explicit accept/cancel boundary.
    dialog(
        r#"
ObjC.import('AppKit');
function run(argv) {
    const labels = JSON.parse(argv[1]);
    const app = $.NSApplication.sharedApplication;
    app.setActivationPolicy($.NSApplicationActivationPolicyAccessory);
    app.activateIgnoringOtherApps(true);
    const font = $.NSFont.systemFontOfSize(18);
    const panel = $.NSFontPanel.sharedFontPanel;
    panel.setPanelFontIsMultiple(font, false);
    panel.setWorksWhenModal(true);
    panel.setLevel($.NSModalPanelWindowLevel + 1);
    panel.orderFrontRegardless;
    const alert = $.NSAlert.alloc.init;
    alert.messageText = labels.family;
    alert.informativeText = labels.instructions;
    alert.addButtonWithTitle(labels.use);
    alert.addButtonWithTitle(labels.cancel);
    const accepted = alert.runModal === $.NSAlertFirstButtonReturn;
    const chosen = accepted ? ObjC.unwrap(panel.panelConvertFont(font).familyName) : '';
    panel.orderOut(null);
    return chosen.startsWith('.') ? '' : chosen;
}
"#,
        current,
        language,
    )
}

pub(crate) fn choose_file(language: Language) -> Result<Option<PathBuf>, String> {
    dialog(
        r#"
ObjC.import('AppKit');
function run(argv) {
    const labels = JSON.parse(argv[1]);
    const app = $.NSApplication.sharedApplication;
    app.setActivationPolicy($.NSApplicationActivationPolicyAccessory);
    app.activateIgnoringOtherApps(true);
    const panel = $.NSOpenPanel.openPanel;
    panel.title = labels.file;
    panel.allowedFileTypes = ['ttf', 'otf', 'ttc'];
    panel.canChooseDirectories = false;
    panel.allowsMultipleSelection = false;
    return panel.runModal === $.NSModalResponseOK ? ObjC.unwrap(panel.URL.path) : '';
}
"#,
        "",
        language,
    )
    .map(|path| path.map(PathBuf::from))
}

pub(crate) fn font_bytes(family: &str) -> Result<Vec<u8>, String> {
    let output = Command::new("/usr/sbin/system_profiler")
        .args(["SPFontsDataType", "-json"])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("Could not query installed macOS fonts".into());
    }
    let fonts: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    fn find(value: &serde_json::Value, inherited: Option<&str>, family: &str) -> Option<PathBuf> {
        match value {
            serde_json::Value::Object(object) => {
                let path = object.get("path").and_then(|v| v.as_str()).or(inherited);
                if ["family", "full_name", "_name"].iter().any(|key| {
                    object
                        .get(*key)
                        .and_then(|v| v.as_str())
                        .is_some_and(|name| name.to_lowercase() == family.to_lowercase())
                }) {
                    if let Some(path) = path {
                        return Some(PathBuf::from(path));
                    }
                }
                object.values().find_map(|child| find(child, path, family))
            }
            serde_json::Value::Array(items) => {
                items.iter().find_map(|item| find(item, inherited, family))
            }
            _ => None,
        }
    }
    let path = find(&fonts, None, family).ok_or("Could not locate the selected font file")?;
    if path.metadata().map_err(|e| e.to_string())?.len() > 64 * 1024 * 1024 {
        return Err("Font file exceeds 64 MiB".into());
    }
    std::fs::read(path).map_err(|e| e.to_string())
}

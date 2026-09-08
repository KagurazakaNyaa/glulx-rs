use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use eframe::egui::{self, Color32, RichText};
use serde::{Deserialize, Serialize};

use crate::{
    InputRequest, RunState, Story, Vm,
    translation::{Submission, TranslationSettings, Translator},
};

const STORAGE_KEY: &str = "glulx-rs-settings";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerSettings {
    pub font_size: f32,
    pub text_color: [u8; 3],
    pub background_color: [u8; 3],
    pub hyperlink_color: [u8; 3],
    pub show_chrome: bool,
    pub window_borders: bool,
    pub translation: TranslationSettings,
}

impl Default for PlayerSettings {
    fn default() -> Self {
        Self {
            font_size: 18.0,
            text_color: [32, 34, 37],
            background_color: [248, 248, 246],
            hyperlink_color: [20, 94, 150],
            show_chrome: true,
            window_borders: true,
            translation: TranslationSettings::default(),
        }
    }
}

#[derive(Debug, Default)]
struct Turn {
    original: String,
    translation: Option<Result<String, String>>,
}

struct FileBrowser {
    open: bool,
    directory: PathBuf,
    typed_path: String,
    error: Option<String>,
}

impl FileBrowser {
    fn new() -> Self {
        let directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            open: false,
            typed_path: directory.display().to_string(),
            directory,
            error: None,
        }
    }

    fn show(&mut self, context: &egui::Context) -> Option<PathBuf> {
        if !self.open {
            return None;
        }
        let mut selected = None;
        let mut keep_open = self.open;
        egui::Window::new("Open Glulx story")
            .collapsible(false)
            .resizable(true)
            .default_size([660.0, 440.0])
            .open(&mut keep_open)
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Up").clicked()
                        && let Some(parent) = self.directory.parent()
                    {
                        self.directory = parent.to_path_buf();
                        self.typed_path = self.directory.display().to_string();
                    }
                    let response = ui.text_edit_singleline(&mut self.typed_path);
                    if response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    {
                        let path = PathBuf::from(&self.typed_path);
                        if path.is_dir() {
                            self.directory = path;
                        } else if path.is_file() {
                            selected = Some(path);
                        }
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let mut entries = std::fs::read_dir(&self.directory)
                        .map(|entries| entries.filter_map(Result::ok).collect::<Vec<_>>())
                        .unwrap_or_default();
                    entries.sort_by_key(|entry| {
                        (
                            !entry.path().is_dir(),
                            entry.file_name().to_string_lossy().to_lowercase(),
                        )
                    });
                    for entry in entries {
                        let path = entry.path();
                        if !path.is_dir() && !is_story_path(&path) {
                            continue;
                        }
                        let name = entry.file_name().to_string_lossy().into_owned();
                        let label = if path.is_dir() {
                            format!("[DIR] {name}")
                        } else {
                            name
                        };
                        if ui.selectable_label(false, label).double_clicked() {
                            if path.is_dir() {
                                self.directory = path;
                                self.typed_path = self.directory.display().to_string();
                            } else {
                                selected = Some(path);
                            }
                        }
                    }
                });
                if let Some(error) = &self.error {
                    ui.colored_label(Color32::from_rgb(180, 40, 40), error);
                }
            });
        self.open = keep_open && selected.is_none();
        selected
    }
}

pub struct PlayerApp {
    settings: PlayerSettings,
    vm: Option<Vm>,
    story_path: Option<PathBuf>,
    story_title: String,
    transcript: String,
    turn_buffer: String,
    turns: Vec<Turn>,
    input: String,
    status: String,
    error: Option<String>,
    show_options: bool,
    show_about: bool,
    show_scrollback: bool,
    file_browser: FileBrowser,
    translator: Translator,
    pending_translations: HashMap<u64, usize>,
    last_state: RunState,
}

impl PlayerApp {
    pub fn new(creation: &eframe::CreationContext<'_>, initial_story: Option<PathBuf>) -> Self {
        let settings = creation
            .storage
            .and_then(|storage| eframe::get_value(storage, STORAGE_KEY))
            .unwrap_or_default();
        let mut app = Self {
            settings,
            vm: None,
            story_path: None,
            story_title: "Glulx Player".to_owned(),
            transcript: String::new(),
            turn_buffer: String::new(),
            turns: Vec::new(),
            input: String::new(),
            status: "Open a .ulx or .gblorb story to begin".to_owned(),
            error: None,
            show_options: false,
            show_about: false,
            show_scrollback: false,
            file_browser: FileBrowser::new(),
            translator: Translator::new(),
            pending_translations: HashMap::new(),
            last_state: RunState::Halted,
        };
        if let Some(path) = initial_story {
            app.load_story(path);
        }
        app
    }

    fn load_story(&mut self, path: PathBuf) {
        let loaded = Story::open(&path)
            .map_err(|error| error.to_string())
            .and_then(|story| {
                self.story_title = story.title.clone();
                Vm::new(story).map_err(|error| error.to_string())
            });
        match loaded {
            Ok(vm) => {
                self.vm = Some(vm);
                self.story_path = Some(path.clone());
                self.transcript.clear();
                self.turn_buffer.clear();
                self.turns.clear();
                self.pending_translations.clear();
                self.input.clear();
                self.error = None;
                self.status = format!("Running {}", path.display());
                self.last_state = RunState::Running;
            }
            Err(error) => {
                self.error = Some(error.to_string());
                self.status = "Could not open story".to_owned();
            }
        }
    }

    fn restart_story(&mut self) {
        if let Some(vm) = &mut self.vm {
            match vm.restart() {
                Ok(()) => {
                    self.transcript.clear();
                    self.turn_buffer.clear();
                    self.turns.clear();
                    self.error = None;
                    self.status = "Story restarted".to_owned();
                    self.last_state = RunState::Running;
                }
                Err(error) => self.fail(error.to_string()),
            }
        }
    }

    fn run_vm(&mut self) {
        let Some(vm) = &mut self.vm else {
            return;
        };
        if vm.state() == RunState::Running
            && let Err(error) = vm.run_steps(25_000)
        {
            let pc = vm.pc();
            vm.stop();
            self.error = Some(format!("{error}\nProgram counter: {pc:#010x}"));
            self.status = "VM stopped after an error".to_owned();
        }
        let output = vm.take_output();
        if !output.is_empty() {
            self.transcript.push_str(&output);
            self.turn_buffer.push_str(&output);
        }
        let state = vm.state();
        if matches!(state, RunState::WaitingForLine | RunState::WaitingForChar)
            && !matches!(
                self.last_state,
                RunState::WaitingForLine | RunState::WaitingForChar
            )
        {
            self.finish_turn();
        }
        self.status = match state {
            RunState::Running => "Running".to_owned(),
            RunState::WaitingForLine => "Waiting for a command".to_owned(),
            RunState::WaitingForChar => "Waiting for a key".to_owned(),
            RunState::Halted => "Story finished".to_owned(),
        };
        self.last_state = state;
    }

    fn finish_turn(&mut self) {
        let source = std::mem::take(&mut self.turn_buffer);
        if source.trim().is_empty() {
            return;
        }
        let index = self.turns.len();
        self.turns.push(Turn {
            original: source.clone(),
            translation: None,
        });
        if self.settings.translation.enabled {
            match self.translator.submit(source, &self.settings.translation) {
                Submission::Cached(value) => self.turns[index].translation = Some(Ok(value)),
                Submission::Queued(id) => {
                    self.pending_translations.insert(id, index);
                }
            }
        }
    }

    fn poll_translations(&mut self) {
        for result in self.translator.poll() {
            if let Some(index) = self.pending_translations.remove(&result.id)
                && let Some(turn) = self.turns.get_mut(index)
            {
                turn.translation = Some(result.result);
            }
        }
        if self.settings.translation.enabled {
            self.queue_untranslated_turns();
        }
    }

    fn queue_untranslated_turns(&mut self) {
        for index in 0..self.turns.len() {
            if self.turns[index].translation.is_some()
                || self
                    .pending_translations
                    .values()
                    .any(|pending| *pending == index)
            {
                continue;
            }
            let source = self.turns[index].original.clone();
            match self.translator.submit(source, &self.settings.translation) {
                Submission::Cached(value) => self.turns[index].translation = Some(Ok(value)),
                Submission::Queued(id) => {
                    self.pending_translations.insert(id, index);
                }
            }
        }
    }

    fn submit_input(&mut self) {
        let Some(vm) = &mut self.vm else {
            return;
        };
        if vm.input_request().is_none() {
            return;
        }
        let input = std::mem::take(&mut self.input);
        self.transcript.push_str(&format!("> {input}\n"));
        if let Err(error) = vm.provide_input(&input) {
            self.fail(error.to_string());
        } else {
            self.last_state = RunState::Running;
        }
    }

    fn fail(&mut self, message: String) {
        self.error = Some(message);
        self.status = "Error".to_owned();
    }

    fn menu_bar(&mut self, context: &egui::Context) {
        if !self.settings.show_chrome {
            return;
        }
        egui::TopBottomPanel::top("menu").show(context, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open story...").clicked() {
                        self.file_browser.open = true;
                        ui.close();
                    }
                    if ui
                        .add_enabled(self.vm.is_some(), egui::Button::new("Restart"))
                        .clicked()
                    {
                        self.restart_story();
                        ui.close();
                    }
                    if ui
                        .add_enabled(self.vm.is_some(), egui::Button::new("Stop"))
                        .clicked()
                    {
                        if let Some(vm) = &mut self.vm {
                            vm.stop();
                        }
                        ui.close();
                    }
                });
                ui.menu_button("View", |ui| {
                    if ui.button("Scrollback").clicked() {
                        self.show_scrollback = true;
                        ui.close();
                    }
                    if ui.button("Options...").clicked() {
                        self.show_options = true;
                        ui.close();
                    }
                    ui.checkbox(&mut self.settings.translation.enabled, "Translation panel");
                });
                ui.menu_button("Help", |ui| {
                    if ui.button("About Glulx Player").clicked() {
                        self.show_about = true;
                        ui.close();
                    }
                });
            });
        });
        egui::TopBottomPanel::top("toolbar").show(context, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button("Open")
                    .on_hover_text("Open a Glulx story")
                    .clicked()
                {
                    self.file_browser.open = true;
                }
                if ui
                    .add_enabled(self.vm.is_some(), egui::Button::new("Restart"))
                    .on_hover_text("Restart the current story")
                    .clicked()
                {
                    self.restart_story();
                }
                if ui
                    .add_enabled(self.vm.is_some(), egui::Button::new("Stop"))
                    .on_hover_text("Stop execution")
                    .clicked()
                    && let Some(vm) = &mut self.vm
                {
                    vm.stop();
                }
                ui.separator();
                if ui
                    .selectable_label(self.settings.translation.enabled, "Translate")
                    .on_hover_text("Show translated turns")
                    .clicked()
                {
                    self.settings.translation.enabled = !self.settings.translation.enabled;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(&self.story_title).strong());
                });
            });
        });
    }

    fn story_view(&mut self, context: &egui::Context) {
        let background = rgb(self.settings.background_color);
        let text = rgb(self.settings.text_color);
        if self.settings.translation.enabled {
            egui::SidePanel::right("translation")
                .default_width(360.0)
                .width_range(240.0..=640.0)
                .frame(
                    egui::Frame::new()
                        .fill(Color32::from_rgb(242, 245, 247))
                        .inner_margin(16.0),
                )
                .show(context, |ui| {
                    ui.heading("Translation");
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for turn in &self.turns {
                                ui.collapsing("Original", |ui| {
                                    ui.weak(turn.original.trim());
                                });
                                match &turn.translation {
                                    Some(Ok(value)) => {
                                        ui.label(
                                            RichText::new(value).size(self.settings.font_size),
                                        );
                                    }
                                    Some(Err(error)) => {
                                        ui.colored_label(Color32::from_rgb(170, 50, 45), error);
                                    }
                                    None => {
                                        ui.weak("Translating...");
                                    }
                                }
                                ui.add_space(14.0);
                            }
                        });
                });
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(background)
                    .inner_margin(egui::Margin::symmetric(28, 22)),
            )
            .show(context, |ui| {
                if self.vm.is_none() {
                    ui.vertical_centered(|ui| {
                        ui.add_space((ui.available_height() * 0.28).max(40.0));
                        ui.heading("Glulx Player");
                        ui.add_space(8.0);
                        ui.label("Open a Glulx executable or Blorb story file.");
                        ui.add_space(16.0);
                        if ui.button("Open story...").clicked() {
                            self.file_browser.open = true;
                        }
                    });
                    return;
                }
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(&self.transcript)
                                .size(self.settings.font_size)
                                .color(text),
                        );
                        ui.add_space(16.0);
                    });
            });
    }

    fn input_bar(&mut self, context: &egui::Context) {
        let request = self.vm.as_ref().and_then(Vm::input_request);
        if request.is_none() {
            return;
        }
        egui::TopBottomPanel::bottom("input")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(235, 237, 238))
                    .inner_margin(10.0),
            )
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    ui.label(match request.unwrap() {
                        InputRequest::Line { .. } => ">",
                        InputRequest::Character => "Key",
                    });
                    let response = ui.add_sized(
                        [ui.available_width() - 72.0, 30.0],
                        egui::TextEdit::singleline(&mut self.input)
                            .font(egui::TextStyle::Monospace),
                    );
                    response.request_focus();
                    let enter = response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    if ui.button("Send").clicked() || enter {
                        self.submit_input();
                    }
                });
            });
    }

    fn status_bar(&mut self, context: &egui::Context) {
        if !self.settings.show_chrome {
            return;
        }
        egui::TopBottomPanel::bottom("status").show(context, |ui| {
            ui.horizontal(|ui| {
                ui.small(&self.status);
                if let Some(path) = &self.story_path {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.small(path.display().to_string());
                    });
                }
            });
        });
    }

    fn dialogs(&mut self, context: &egui::Context) {
        if let Some(error) = self.error.clone() {
            let mut open = true;
            egui::Window::new("Glulx error")
                .collapsible(false)
                .resizable(true)
                .open(&mut open)
                .show(context, |ui| {
                    ui.colored_label(Color32::from_rgb(170, 45, 40), error);
                    if ui.button("Dismiss").clicked() {
                        self.error = None;
                    }
                });
            if !open {
                self.error = None;
            }
        }
        egui::Window::new("Options")
            .open(&mut self.show_options)
            .resizable(true)
            .default_width(520.0)
            .show(context, |ui| {
                ui.heading("Display");
                ui.add(
                    egui::Slider::new(&mut self.settings.font_size, 12.0..=32.0).text("Text size"),
                );
                color_setting(ui, "Text", &mut self.settings.text_color);
                color_setting(ui, "Background", &mut self.settings.background_color);
                color_setting(ui, "Hyperlinks", &mut self.settings.hyperlink_color);
                ui.checkbox(
                    &mut self.settings.window_borders,
                    "Borders between game windows",
                );
                ui.checkbox(
                    &mut self.settings.show_chrome,
                    "Menus, toolbar and status bar",
                );
                ui.separator();
                ui.heading("Translation");
                ui.checkbox(
                    &mut self.settings.translation.enabled,
                    "Enable turn translation",
                );
                ui.label("OpenAI-compatible endpoint");
                ui.text_edit_singleline(&mut self.settings.translation.endpoint);
                ui.label("Model");
                ui.text_edit_singleline(&mut self.settings.translation.model);
                ui.label("Target language");
                ui.text_edit_singleline(&mut self.settings.translation.target_language);
                ui.label("API key (kept in local app settings)");
                ui.add(
                    egui::TextEdit::singleline(&mut self.settings.translation.api_key)
                        .password(true),
                );
                ui.label("System prompt");
                ui.add(
                    egui::TextEdit::multiline(&mut self.settings.translation.system_prompt)
                        .desired_rows(4),
                );
            });
        egui::Window::new("Scrollback")
            .open(&mut self.show_scrollback)
            .default_size([720.0, 560.0])
            .show(context, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.label(&self.transcript);
                });
            });
        egui::Window::new("About Glulx Player")
            .open(&mut self.show_about)
            .collapsible(false)
            .resizable(false)
            .show(context, |ui| {
                ui.heading("Glulx Player 0.1.0");
                ui.label("A pure Rust Glulx VM with a cross-platform graphical interface.");
                ui.label("VM behavior is based on the Glulx specification and David Kinder's Git.");
                ui.label("The interface follows the Windows Git player workflow.");
            });
    }
}

impl eframe::App for PlayerApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, STORAGE_KEY, &self.settings);
    }

    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        if context.input(|input| {
            input.modifiers.ctrl && input.modifiers.alt && input.key_pressed(egui::Key::L)
        }) {
            self.settings.translation.enabled = !self.settings.translation.enabled;
        }
        for path in context.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect::<Vec<_>>()
        }) {
            if is_story_path(&path) {
                self.load_story(path);
                break;
            }
        }
        self.run_vm();
        self.poll_translations();
        self.menu_bar(context);
        self.status_bar(context);
        self.input_bar(context);
        self.story_view(context);
        self.dialogs(context);
        if let Some(path) = self.file_browser.show(context) {
            self.load_story(path);
        }
        if self
            .vm
            .as_ref()
            .is_some_and(|vm| vm.state() == RunState::Running)
        {
            context.request_repaint();
        } else {
            context.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

fn color_setting(ui: &mut egui::Ui, label: &str, value: &mut [u8; 3]) {
    ui.horizontal(|ui| {
        ui.label(label);
        let mut color = [
            value[0] as f32 / 255.0,
            value[1] as f32 / 255.0,
            value[2] as f32 / 255.0,
        ];
        if ui.color_edit_button_rgb(&mut color).changed() {
            *value = color.map(|channel| (channel * 255.0).round() as u8);
        }
    });
}

fn rgb(value: [u8; 3]) -> Color32 {
    Color32::from_rgb(value[0], value[1], value[2])
}

fn is_story_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "ulx" | "blb" | "blorb" | "glb" | "gblorb"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_windows_git_story_extensions_case_insensitively() {
        assert!(is_story_path(Path::new("story.ULX")));
        assert!(is_story_path(Path::new("story.gblorb")));
        assert!(!is_story_path(Path::new("story.z5")));
    }
}

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};

use eframe::egui::{self, Color32, RichText};
use serde::{Deserialize, Serialize};

use crate::story::ResourceSelection;
use crate::{
    GraphicsRequest, InputRequest, RunState, Story, Vm,
    translation::{Submission, TranslationSettings, Translator},
};

mod i18n;
use i18n::Language;
pub use i18n::LanguagePreference;

mod fonts;
mod text_buffer;
mod text_grid;

const STORAGE_KEY: &str = "glulx-rs-settings";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerSettings {
    pub language: LanguagePreference,
    pub font_size: f32,
    pub fallback_font: String,
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
            language: LanguagePreference::System,
            font_size: 18.0,
            fallback_font: String::new(),
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

#[derive(Serialize, Deserialize)]
struct SavedCanvas {
    window: u32,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}
#[derive(Serialize)]
struct SessionRef<'a> {
    version: u32,
    vm: &'a Vm,
    transcript: &'a str,
    input: &'a str,
    timer: Option<u32>,
    canvases: Vec<SavedCanvas>,
}
#[derive(Deserialize)]
struct Session {
    version: u32,
    vm: Vm,
    transcript: String,
    input: String,
    timer: Option<u32>,
    canvases: Vec<SavedCanvas>,
}

struct DisplayedGraphics {
    pixels: image::RgbaImage,
    texture: egui::TextureHandle,
}

impl DisplayedGraphics {
    fn new(context: &egui::Context, window: u32, size: [u32; 2]) -> Self {
        let size = [size[0].max(1), size[1].max(1)];
        let pixels = image::RgbaImage::from_pixel(size[0], size[1], image::Rgba([255; 4]));
        let texture = context.load_texture(
            format!("glk-graphics-window-{window}"),
            texture_image(context, &pixels),
            egui::TextureOptions::LINEAR,
        );
        Self { pixels, texture }
    }

    fn upload(&mut self, context: &egui::Context) {
        self.texture.set(
            texture_image(context, &self.pixels),
            egui::TextureOptions::LINEAR,
        );
    }
}

struct FileBrowser {
    open: bool,
    directory: PathBuf,
    typed_path: String,
    error: Option<String>,
    resources: bool,
}

impl FileBrowser {
    fn new() -> Self {
        let directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            open: false,
            typed_path: directory.display().to_string(),
            directory,
            error: None,
            resources: false,
        }
    }

    fn show(&mut self, context: &egui::Context, language: Language) -> Option<PathBuf> {
        if !self.open {
            return None;
        }
        let mut selected = None;
        let mut keep_open = self.open;
        egui::Window::new(if self.resources {
            language.text("ui.choose_resource_archive_or_directory")
        } else {
            language.text("ui.open_glulx_story")
        })
        .id(egui::Id::new("file-browser"))
        .collapsible(false)
        .resizable(true)
        .default_size([660.0, 440.0])
        .open(&mut keep_open)
        .show(context, |ui| {
            ui.horizontal(|ui| {
                if ui.button(language.text("ui.up")).clicked()
                    && let Some(parent) = self.directory.parent()
                {
                    self.directory = parent.to_path_buf();
                    self.typed_path = self.directory.display().to_string();
                }
                let response = ui.text_edit_singleline(&mut self.typed_path);
                if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                    let path = PathBuf::from(&self.typed_path);
                    if path.is_dir() {
                        self.directory = path;
                    } else if path.is_file() {
                        selected = Some(path);
                    }
                }
            });
            if self.resources && ui.button(language.text("ui.use_this_directory")).clicked() {
                selected = Some(self.directory.clone());
            }
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
                        language.format("ui.dir", &[&name])
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
    game_status: String,
    graphics: BTreeMap<u32, DisplayedGraphics>,
    image_cache: HashMap<u32, image::RgbaImage>,
    buffer_images: text_buffer::ImageCache,
    status: String,
    error: Option<String>,
    show_options: bool,
    show_about: bool,
    show_story_info: bool,
    cover: Option<egui::TextureHandle>,
    show_scrollback: bool,
    file_browser: FileBrowser,
    translator: Translator,
    pending_translations: HashMap<u64, usize>,
    last_state: RunState,
    fonts: fonts::Fonts,
    show_resources: bool,
    resource_choice: u8,
    resource_path: String,
}

impl PlayerApp {
    pub fn new(creation: &eframe::CreationContext<'_>, initial_story: Option<PathBuf>) -> Self {
        Self::new_with_resources(creation, initial_story, ResourceSelection::Auto)
    }

    pub fn new_with_resources(
        creation: &eframe::CreationContext<'_>,
        initial_story: Option<PathBuf>,
        selection: ResourceSelection,
    ) -> Self {
        let settings: PlayerSettings = creation
            .storage
            .and_then(|storage| eframe::get_value(storage, STORAGE_KEY))
            .unwrap_or_default();
        let fonts = fonts::Fonts::new(&creation.egui_ctx, &settings.fallback_font);
        let mut app = Self {
            settings,
            vm: None,
            story_path: None,
            story_title: "Glulx Player".to_owned(),
            transcript: String::new(),
            turn_buffer: String::new(),
            turns: Vec::new(),
            input: String::new(),
            game_status: String::new(),
            graphics: BTreeMap::new(),
            image_cache: HashMap::new(),
            buffer_images: text_buffer::ImageCache::default(),
            status: "ui.open_a_ulx_or_gblorb_story_to_begin".to_owned(),
            error: None,
            show_options: false,
            show_about: false,
            show_story_info: false,
            cover: None,
            show_scrollback: false,
            file_browser: FileBrowser::new(),
            translator: Translator::new(),
            pending_translations: HashMap::new(),
            last_state: RunState::Halted,
            fonts,
            show_resources: false,
            resource_choice: 0,
            resource_path: String::new(),
        };
        if let Some(path) = initial_story {
            app.load_story_with_resources(path, selection);
        } else if let Some(session) = creation
            .storage
            .and_then(|storage| eframe::get_value::<Session>(storage, "glulx-session-v1"))
            && session.version == 1
        {
            match session.vm.validate_session() {
                Ok(mut vm) => {
                    vm.enable_audio();
                    vm.resume_timer(session.timer);
                    app.story_title = vm.story_title().to_owned();
                    app.story_path = vm.story_path().map(Path::to_owned);
                    app.last_state = vm.state();
                    app.vm = Some(vm);
                    app.transcript = session.transcript;
                    app.input = session.input;
                    for canvas in session.canvases {
                        if let Some(pixels) =
                            image::RgbaImage::from_raw(canvas.width, canvas.height, canvas.pixels)
                        {
                            let texture = creation.egui_ctx.load_texture(
                                format!("glk-graphics-window-{}", canvas.window),
                                texture_image(&creation.egui_ctx, &pixels),
                                egui::TextureOptions::LINEAR,
                            );
                            app.graphics
                                .insert(canvas.window, DisplayedGraphics { pixels, texture });
                        }
                    }
                    app.status = "ui.previous_session_restored".to_owned();
                }
                Err(error) => {
                    app.error = Some(format!("Could not restore previous session: {error}"))
                }
            }
        }
        app
    }

    fn load_story(&mut self, path: PathBuf) {
        self.load_story_with_resources(path, ResourceSelection::Auto);
    }

    fn load_story_with_resources(&mut self, path: PathBuf, selection: ResourceSelection) {
        let loaded = Story::open_with_resources(&path, selection)
            .map_err(|error| error.to_string())
            .and_then(|story| Vm::new(story).map_err(|error| error.to_string()));
        match loaded {
            Ok(mut vm) => {
                vm.enable_audio();
                self.story_title = vm.story_title().to_owned();
                self.show_resources = false;
                self.resource_choice = 0;
                self.resource_path.clear();
                self.file_browser.open = false;
                self.file_browser.resources = false;
                self.vm = Some(vm);
                self.story_path = Some(path.clone());
                self.transcript.clear();
                self.turn_buffer.clear();
                self.turns.clear();
                self.pending_translations.clear();
                self.input.clear();
                self.game_status.clear();
                self.graphics.clear();
                self.image_cache.clear();
                self.buffer_images.clear();
                self.cover = None;
                self.error = None;
                self.status = format!("Running {}", path.display());
                self.last_state = RunState::Running;
            }
            Err(error) => {
                self.error = Some(error.to_string());
                self.status = "ui.could_not_open_story".to_owned();
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
                    self.game_status.clear();
                    self.graphics.clear();
                    self.image_cache.clear();
                    self.buffer_images.clear();
                    self.error = None;
                    self.status = "ui.story_restarted".to_owned();
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
        let word = |c: [u8; 3]| u32::from_be_bytes([0, c[0], c[1], c[2]]);
        vm.set_light_fonts(self.fonts.light_fonts);
        vm.set_glyph_support(self.fonts.support.clone());
        vm.set_text_metrics(self.fonts.metrics.clone());
        vm.set_text_appearance(
            self.settings.font_size,
            word(self.settings.text_color),
            word(self.settings.background_color),
        );
        if let Err(error) = vm.run_steps(25_000) {
            let pc = vm.pc();
            vm.stop();
            self.error = Some(format!("{error}\nProgram counter: {pc:#010x}"));
            self.status = "ui.vm_stopped_after_an_error".to_owned();
        }
        let output = vm.take_output();
        self.game_status = vm.status_text();
        if !output.is_empty() {
            self.transcript.push_str(&output);
            self.turn_buffer.push_str(&output);
        }
        let state = vm.state();
        // A timer can cancel and re-request input during one VM slice, leaving
        // the same RunState but supplying a different prefilled line.
        if state == RunState::WaitingForLine {
            self.input = vm.initial_input();
        }
        if matches!(state, RunState::WaitingForLine | RunState::WaitingForChar)
            && !matches!(
                self.last_state,
                RunState::WaitingForLine | RunState::WaitingForChar
            )
        {
            self.finish_turn();
        }
        self.status = match state {
            RunState::Running => "ui.running".to_owned(),
            RunState::WaitingForLine => "ui.waiting_for_a_command".to_owned(),
            RunState::WaitingForChar => "ui.waiting_for_a_key".to_owned(),
            RunState::WaitingForFile => "ui.enter_a_file_path_or_submit_empty_to".to_owned(),
            RunState::WaitingForEvent => "ui.waiting_for_an_event".to_owned(),
            RunState::Halted => "ui.story_finished".to_owned(),
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

    fn poll_graphics(&mut self, context: &egui::Context) {
        let requests = self.vm.as_mut().map(Vm::take_graphics).unwrap_or_default();
        let mut dirty = HashSet::new();
        for request in requests {
            match request {
                GraphicsRequest::Resize {
                    window,
                    background,
                    canvas_size,
                } => {
                    if canvas_size.contains(&0) {
                        self.graphics.remove(&window);
                    } else {
                        ensure_canvas(context, &mut self.graphics, window, canvas_size, background);
                        dirty.insert(window);
                    }
                }
                GraphicsRequest::Draw(request) => {
                    if request.canvas_size.contains(&0) {
                        continue;
                    }
                    if let std::collections::hash_map::Entry::Vacant(entry) =
                        self.image_cache.entry(request.resource)
                    {
                        match crate::picture::decode(&request.data) {
                            Ok(decoded) => {
                                entry.insert(decoded);
                            }
                            Err(error) => {
                                self.status = format!(
                                    "Could not decode picture {}: {error}",
                                    request.resource
                                );
                                continue;
                            }
                        }
                    }
                    let source = &self.image_cache[&request.resource];
                    let size = request
                        .requested_size
                        .unwrap_or([source.width(), source.height()]);
                    let canvas = ensure_canvas(
                        context,
                        &mut self.graphics,
                        request.window,
                        request.canvas_size,
                        0xffffff,
                    );
                    crate::picture::draw_scaled(&mut canvas.pixels, source, request.position, size);
                    dirty.insert(request.window);
                }
                GraphicsRequest::Fill {
                    window,
                    color,
                    rect,
                    canvas_size,
                } => {
                    if canvas_size.contains(&0) {
                        continue;
                    }
                    let canvas =
                        ensure_canvas(context, &mut self.graphics, window, canvas_size, 0xffffff);
                    fill_rect(&mut canvas.pixels, rect, color);
                    dirty.insert(window);
                }
                GraphicsRequest::Clear {
                    window,
                    color,
                    canvas_size,
                } => {
                    if canvas_size.contains(&0) {
                        continue;
                    }
                    let canvas =
                        ensure_canvas(context, &mut self.graphics, window, canvas_size, color);
                    let color = rgba(color);
                    for pixel in canvas.pixels.pixels_mut() {
                        *pixel = color;
                    }
                    dirty.insert(window);
                }
                GraphicsRequest::Close { window } => {
                    self.graphics.remove(&window);
                }
            }
        }
        for window in dirty {
            if let Some(canvas) = self.graphics.get_mut(&window) {
                canvas.upload(context);
            }
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
        self.submit_terminated_input(0);
    }

    fn submit_terminated_input(&mut self, terminator: u32) {
        let Some(vm) = &mut self.vm else {
            return;
        };
        if vm.input_request().is_none() {
            return;
        }
        let input = std::mem::take(&mut self.input);
        if !matches!(vm.input_request(), Some(InputRequest::File { .. })) {
            self.transcript.push_str(&format!("> {input}\n"));
        }
        let result = if terminator == 0 {
            vm.provide_input(&input)
        } else {
            vm.provide_terminated_input(&input, terminator)
        };
        if let Err(error) = result {
            self.fail(error.to_string());
        } else {
            self.last_state = RunState::Running;
        }
    }

    fn submit_key(&mut self, key: u32) {
        let Some(vm) = &mut self.vm else {
            return;
        };
        self.input.clear();
        if let Err(error) = vm.provide_key(key) {
            self.fail(error.to_string());
        } else {
            self.last_state = RunState::Running;
        }
    }

    fn fail(&mut self, message: String) {
        self.error = Some(message);
        self.status = "ui.error".to_owned();
    }

    fn menu_bar(&mut self, root: &mut egui::Ui) {
        let language = self.settings.language.resolve();
        if !self.settings.show_chrome {
            return;
        }
        egui::Panel::top("menu").show(root, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button(language.text("ui.file"), |ui| {
                    if ui.button(language.text("ui.open_story")).clicked() {
                        self.file_browser.resources = false;
                        self.file_browser.open = true;
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            self.story_path.is_some(),
                            egui::Button::new(language.text("ui.choose_resources")),
                        )
                        .clicked()
                    {
                        self.show_resources = true;
                        self.resource_choice = 0;
                        self.resource_path = self
                            .vm
                            .as_ref()
                            .and_then(Vm::resource_path)
                            .map_or_else(String::new, |path| path.display().to_string());
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            self.vm.is_some(),
                            egui::Button::new(language.text("ui.restart")),
                        )
                        .clicked()
                    {
                        self.restart_story();
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            self.vm.is_some(),
                            egui::Button::new(language.text("ui.stop")),
                        )
                        .clicked()
                    {
                        if let Some(vm) = &mut self.vm {
                            vm.stop();
                        }
                        ui.close();
                    }
                });
                ui.menu_button(language.text("ui.view"), |ui| {
                    if ui.button(language.text("ui.story_information")).clicked() {
                        self.show_story_info = true;
                        ui.close();
                    }
                    if ui.button(language.text("ui.scrollback")).clicked() {
                        self.show_scrollback = true;
                        ui.close();
                    }
                    if ui.button(language.text("ui.options")).clicked() {
                        self.show_options = true;
                        ui.close();
                    }
                    ui.checkbox(
                        &mut self.settings.translation.enabled,
                        language.text("ui.translation_panel"),
                    );
                });
                ui.menu_button(language.text("ui.help"), |ui| {
                    if ui.button(language.text("ui.about_glulx_player")).clicked() {
                        self.show_about = true;
                        ui.close();
                    }
                });
            });
        });
        egui::Panel::top("toolbar").show(root, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button(language.text("ui.open"))
                    .on_hover_text(language.text("ui.open_a_glulx_story"))
                    .clicked()
                {
                    self.file_browser.resources = false;
                    self.file_browser.open = true;
                }
                if ui
                    .add_enabled(
                        self.vm.is_some(),
                        egui::Button::new(language.text("ui.restart")),
                    )
                    .on_hover_text(language.text("ui.restart_the_current_story"))
                    .clicked()
                {
                    self.restart_story();
                }
                if ui
                    .add_enabled(
                        self.vm.is_some(),
                        egui::Button::new(language.text("ui.stop")),
                    )
                    .on_hover_text(language.text("ui.stop_execution"))
                    .clicked()
                    && let Some(vm) = &mut self.vm
                {
                    vm.stop();
                }
                ui.separator();
                if ui
                    .selectable_label(
                        self.settings.translation.enabled,
                        language.text("ui.translate"),
                    )
                    .on_hover_text(language.text("ui.show_translated_turns"))
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

    fn story_view(&mut self, root: &mut egui::Ui) {
        let language = self.settings.language.resolve();
        let context = root.ctx().clone();
        let accept_input = !self.dialog_open(&context);
        let background = rgb(self.settings.background_color);
        if self.settings.translation.enabled {
            egui::Panel::right("translation")
                .default_size(360.0)
                .size_range(240.0..=640.0)
                .frame(
                    egui::Frame::new()
                        .fill(Color32::from_rgb(242, 245, 247))
                        .inner_margin(16.0),
                )
                .show(root, |ui| {
                    ui.heading(language.text("ui.translation"));
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for turn in &self.turns {
                                ui.collapsing(language.text("ui.original"), |ui| {
                                    ui.weak(turn.original.trim());
                                });
                                match &turn.translation {
                                    Some(Ok(value)) => {
                                        ui.label(
                                            RichText::new(value).size(self.settings.font_size),
                                        );
                                    }
                                    Some(Err(error)) => {
                                        ui.colored_label(
                                            Color32::from_rgb(170, 50, 45),
                                            language.message(error),
                                        );
                                    }
                                    None => {
                                        ui.weak(language.text("ui.translating_legacy"));
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
            .show(root, |ui| {
                if self.vm.is_none() {
                    ui.vertical_centered(|ui| {
                        ui.add_space((ui.available_height() * 0.28).max(40.0));
                        ui.heading("Glulx Player");
                        ui.add_space(8.0);
                        ui.label(language.text("ui.open_a_glulx_executable_or_blorb_story_file"));
                        ui.add_space(16.0);
                        if ui.button(language.text("ui.open_story")).clicked() {
                            self.file_browser.open = true;
                        }
                    });
                    return;
                }
                let bounds = ui.available_rect_before_wrap();
                let views = if let Some(vm) = &mut self.vm {
                    vm.resize_windows(
                        bounds.width().max(0.0) as u32,
                        bounds.height().max(0.0) as u32,
                    );
                    vm.window_views()
                } else {
                    Vec::new()
                };
                self.poll_graphics(&context);
                let mut click = None;
                let mut hyperlink = None;
                let mut grid_submitted = false;
                for view in views {
                    let rect = egui::Rect::from_min_size(
                        bounds.min + egui::vec2(view.rect[0] as f32, view.rect[1] as f32),
                        egui::vec2(view.rect[2] as f32, view.rect[3] as f32),
                    );
                    if rect.width() < 1.0 || rect.height() < 1.0 {
                        continue;
                    }
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .max_rect(rect)
                            .id_salt(("glk-window", view.id)),
                        |ui| {
                            ui.set_clip_rect(rect.intersect(bounds));
                            if self.settings.window_borders {
                                ui.painter().rect_stroke(
                                    rect,
                                    0.0,
                                    egui::Stroke::new(1.0_f32, Color32::from_gray(190)),
                                    egui::StrokeKind::Inside,
                                );
                            }
                            match view.kind {
                                3 => {
                                    egui::ScrollArea::vertical().stick_to_bottom(true).show(
                                        ui,
                                        |ui| {
                                            if let Some(value) = text_buffer::show(
                                                ui,
                                                &view,
                                                &self.settings,
                                                self.vm.as_ref().unwrap(),
                                                &mut self.buffer_images,
                                            ) {
                                                hyperlink = Some((view.id, value));
                                            }
                                        },
                                    );
                                }
                                4 => {
                                    let editor = self.vm.as_ref().and_then(|vm| {
                                        (accept_input
                                            && vm.is_grid_line_input()
                                            && vm.input_window() == view.id)
                                            .then_some(text_grid::GridEditor {
                                                text: &mut self.input,
                                                maximum_length: vm.line_input_max_len(),
                                            })
                                    });
                                    let response =
                                        text_grid::show(ui, rect, &view, &self.settings, editor);
                                    if let Some([x, y]) = response.cell {
                                        click = Some((view.id, x, y));
                                    }
                                    if let Some(value) = response.hyperlink {
                                        hyperlink = Some((view.id, value));
                                    }
                                    if response.changed
                                        && let Some(vm) = &mut self.vm
                                    {
                                        let _ = vm.update_line_input(&self.input);
                                    }
                                    grid_submitted |= response.submitted;
                                }
                                5 => {
                                    let response = ui.allocate_rect(rect, egui::Sense::click());
                                    if let Some(graphics) = self.graphics.get(&view.id) {
                                        let destination = egui::Rect::from_min_size(
                                            rect.min,
                                            egui::vec2(
                                                graphics.pixels.width() as f32,
                                                graphics.pixels.height() as f32,
                                            ),
                                        );
                                        ui.painter().image(
                                            graphics.texture.id(),
                                            destination,
                                            egui::Rect::from_min_max(
                                                egui::Pos2::ZERO,
                                                egui::pos2(1.0, 1.0),
                                            ),
                                            Color32::WHITE,
                                        );
                                    }
                                    if response.clicked()
                                        && let Some(pos) = response.interact_pointer_pos()
                                    {
                                        click = Some((
                                            view.id,
                                            (pos.x - rect.min.x).max(0.0) as u32,
                                            (pos.y - rect.min.y).max(0.0) as u32,
                                        ));
                                    }
                                }
                                _ => {}
                            }
                        },
                    );
                }
                if let Some(vm) = &mut self.vm {
                    if let Some((window, x, y)) = click {
                        let _ = vm.mouse_input(window, x, y);
                    }
                    if let Some((window, value)) = hyperlink {
                        let _ = vm.hyperlink_input(window, value);
                    }
                }
                if grid_submitted {
                    self.submit_input();
                }
            });
    }

    fn dialog_open(&self, context: &egui::Context) -> bool {
        self.show_resources
            || self.show_options
            || self.show_about
            || self.show_story_info
            || self.show_scrollback
            || self.file_browser.open
            || self.error.is_some()
            || egui::Popup::is_any_open(context)
    }

    fn character_input(&mut self, context: &egui::Context) {
        if self.dialog_open(context) {
            return;
        }
        if !matches!(
            self.vm.as_ref().and_then(Vm::input_request),
            Some(InputRequest::Character)
        ) {
            return;
        }
        let key = context.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Key {
                    key, pressed: true, ..
                } => glk_character_key(*key),
                egui::Event::Text(text) | egui::Event::Paste(text) => {
                    text.chars().next().map(|c| c as u32)
                }
                _ => None,
            })
        });
        if let Some(key) = key {
            // A game keystroke must not simultaneously navigate or activate
            // the player's menus (Tab/Return in particular).
            context.input_mut(|input| {
                input.events.retain(|event| {
                    !matches!(
                        event,
                        egui::Event::Key { pressed: true, .. }
                            | egui::Event::Text(_)
                            | egui::Event::Paste(_)
                    )
                })
            });
            self.submit_key(key);
        }
    }

    fn input_bar(&mut self, root: &mut egui::Ui) {
        let language = self.settings.language.resolve();
        let accept_input = !self.dialog_open(root.ctx());
        let request = self.vm.as_ref().and_then(Vm::input_request);
        if request.is_none() {
            return;
        }
        egui::Panel::bottom("input")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(235, 237, 238))
                    .inner_margin(10.0),
            )
            .show(root, |ui| {
                if !accept_input {
                    ui.disable();
                }
                if matches!(request, Some(InputRequest::File { .. }))
                    && let Some(vm) = &self.vm
                {
                    ui.label(language.message(&vm.file_prompt_message()));
                }
                ui.horizontal(|ui| {
                    if let Some(vm) = &mut self.vm {
                        let windows = vm.pending_input_windows();
                        if windows.len() > 1 {
                            let mut selected = vm.input_window();
                            egui::ComboBox::from_id_salt("input-window")
                                .selected_text(language.format("ui.window", &[&selected]))
                                .show_ui(ui, |ui| {
                                    for window in windows {
                                        ui.selectable_value(
                                            &mut selected,
                                            window,
                                            language.format("ui.window", &[&window]),
                                        );
                                    }
                                });
                            if selected != vm.input_window() {
                                let _ = vm.update_line_input(&self.input);
                                vm.select_input_window(selected);
                                self.input = vm.initial_input();
                            }
                        }
                    }
                    // Changing the window can also change the input kind.
                    let request = self.vm.as_ref().and_then(Vm::input_request);
                    if matches!(request, Some(InputRequest::Character)) {
                        ui.label(language.text("ui.press_a_key"));
                        if ui.button(language.text("ui.return")).clicked() {
                            self.submit_key(0xffff_fffa);
                        }
                        return;
                    }
                    let grid_line = self.vm.as_ref().is_some_and(Vm::is_grid_line_input);
                    let mut enter = false;
                    if !grid_line {
                        ui.label(match request {
                            Some(InputRequest::Line { .. }) => ">",
                            Some(InputRequest::File { writing: true }) => {
                                language.text("ui.save_file")
                            }
                            Some(InputRequest::File { writing: false }) => {
                                language.text("ui.open_file")
                            }
                            _ => "",
                        });
                        let limit = match request {
                            Some(InputRequest::Line { maximum_length }) => maximum_length as usize,
                            _ => usize::MAX,
                        };
                        let response = ui.add_sized(
                            [ui.available_width() - 72.0, 30.0],
                            egui::TextEdit::singleline(&mut self.input)
                                .char_limit(limit)
                                .font(egui::TextStyle::Monospace),
                        );
                        if response.changed()
                            && let Some(vm) = &mut self.vm
                        {
                            let _ = vm.update_line_input(&self.input);
                        }
                        enter = accept_input
                            && (response.has_focus() || response.lost_focus())
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                        // Re-requesting an existing focus interrupts IME in
                        // egui 0.36 and can create a native IME/repaint loop.
                        if accept_input && !ui.memory(|memory| memory.has_focus(response.id)) {
                            response.request_focus();
                        }
                    } else {
                        ui.label(language.text("ui.type_in_highlighted_field"));
                    }
                    let terminator =
                        if accept_input && matches!(request, Some(InputRequest::Line { .. })) {
                            self.vm.as_ref().and_then(|vm| {
                                vm.line_terminators().iter().copied().find(|code| {
                                    glk_terminator_key(*code)
                                        .is_some_and(|key| ui.input(|input| input.key_pressed(key)))
                                })
                            })
                        } else {
                            None
                        };
                    if let Some(terminator) = terminator {
                        self.submit_terminated_input(terminator);
                    } else if ui.button(language.text("ui.send")).clicked() || enter {
                        self.submit_input();
                    }
                });
            });
    }

    fn status_bar(&mut self, root: &mut egui::Ui) {
        let language = self.settings.language.resolve();
        if !self.settings.show_chrome {
            return;
        }
        egui::Panel::bottom("status").show(root, |ui| {
            ui.horizontal(|ui| {
                ui.small(language.message(&self.status));
                if let Some(path) = &self.story_path {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.small(path.display().to_string());
                    });
                }
            });
        });
    }

    fn dialogs(&mut self, context: &egui::Context) {
        let language = self.settings.language.resolve();
        let mut resource_selection = None;
        egui::Window::new(language.text("ui.story_resources"))
            .id(egui::Id::new("ui.story_resources"))
            .open(&mut self.show_resources)
            .collapsible(false)
            .default_width(540.0)
            .show(context, |ui| {
                ui.label(language.text("ui.choose_the_pictures_sounds_and_data_for_this"));
                ui.radio_value(
                    &mut self.resource_choice,
                    0,
                    language.text("ui.find_a_same_name_archive_automatically"),
                );
                ui.radio_value(
                    &mut self.resource_choice,
                    1,
                    language.text("ui.use_only_resources_embedded_in_the_story"),
                );
                ui.radio_value(
                    &mut self.resource_choice,
                    2,
                    language.text("ui.use_an_archive_or_resource_directory"),
                );
                if self.resource_choice == 2 {
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.resource_path);
                        if ui.button(language.text("ui.browse")).clicked() {
                            self.file_browser.resources = true;
                            self.file_browser.open = true;
                        }
                    });
                }
                ui.separator();
                ui.label(language.text("ui.applying_resources_restarts_the_story_save_your_game"));
                let valid = self.resource_choice != 2 || !self.resource_path.trim().is_empty();
                if ui
                    .add_enabled(
                        valid,
                        egui::Button::new(language.text("ui.restart_with_selected_resources")),
                    )
                    .clicked()
                {
                    resource_selection = Some(match self.resource_choice {
                        1 => ResourceSelection::None,
                        2 => ResourceSelection::Path(PathBuf::from(self.resource_path.trim())),
                        _ => ResourceSelection::Auto,
                    });
                }
            });
        if let Some(selection) = resource_selection
            && let Some(path) = self.story_path.clone()
        {
            self.load_story_with_resources(path, selection);
            self.show_resources = self.error.is_some();
        }
        if self.show_story_info
            && let Some(vm) = &self.vm
        {
            let metadata = vm.metadata();
            if self.cover.is_none()
                && let Some(data) = vm.cover()
                && let Ok(pixels) = crate::picture::decode(data)
            {
                self.cover = Some(context.load_texture(
                    "story-cover",
                    texture_image(context, &pixels),
                    egui::TextureOptions::LINEAR,
                ));
            }
            egui::Window::new(language.text("ui.story_information"))
                .id(egui::Id::new("ui.story_information"))
                .open(&mut self.show_story_info)
                .show(context, |ui| {
                    ui.heading(vm.story_title());
                    if let Some(cover) = &self.cover {
                        ui.add(egui::Image::new(cover).max_height(300.0));
                    }
                    if !metadata.author.is_empty() {
                        ui.label(language.format("ui.by", &[&metadata.author]));
                    }
                    if !metadata.headline.is_empty() {
                        ui.label(&metadata.headline);
                    }
                    if !metadata.description.is_empty() {
                        ui.label(&metadata.description);
                    }
                    if !metadata.ifid.is_empty() {
                        ui.small(format!("IFID: {}", metadata.ifid));
                    }
                    if let Some(path) = vm.resource_path() {
                        ui.label(language.format("ui.resources", &[&path.display()]));
                    }
                    let descriptions = vm.resource_descriptions();
                    if !descriptions.is_empty() {
                        ui.separator();
                        ui.collapsing(language.text("ui.image_and_sound_descriptions"), |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(300.0)
                                .show(ui, |ui| {
                                    for description in descriptions {
                                        let kind = if description.usage == *b"Pict" {
                                            language.text("ui.image")
                                        } else {
                                            language.text("ui.sound")
                                        };
                                        ui.label(format!("{kind}: {}", description.text));
                                    }
                                });
                        });
                    }
                });
        }
        if let Some(error) = self.error.clone() {
            let mut open = true;
            egui::Window::new(language.text("ui.glulx_error"))
                .id(egui::Id::new("ui.glulx_error"))
                .collapsible(false)
                .resizable(true)
                .open(&mut open)
                .show(context, |ui| {
                    ui.colored_label(Color32::from_rgb(170, 45, 40), language.message(&error));
                    if ui.button(language.text("ui.dismiss")).clicked() {
                        self.error = None;
                    }
                });
            if !open {
                self.error = None;
            }
        }
        egui::Window::new(language.text("ui.options_window"))
            .id(egui::Id::new("ui.options"))
            .open(&mut self.show_options)
            .resizable(true)
            .default_width(520.0)
            .show(context, |ui| {
                ui.label(language.text("ui.interface_language"));
                egui::ComboBox::from_id_salt("interface-language")
                    .selected_text(match self.settings.language {
                        LanguagePreference::System => language.text("ui.follow_system"),
                        LanguagePreference::English => language.text("language.english"),
                        LanguagePreference::Chinese => language.text("language.chinese"),
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.settings.language,
                            LanguagePreference::System,
                            language.text("ui.follow_system"),
                        );
                        ui.selectable_value(
                            &mut self.settings.language,
                            LanguagePreference::English,
                            language.text("language.english"),
                        );
                        ui.selectable_value(
                            &mut self.settings.language,
                            LanguagePreference::Chinese,
                            language.text("language.chinese"),
                        );
                    });
                ui.heading(language.text("ui.display"));
                ui.add(
                    egui::Slider::new(&mut self.settings.font_size, 12.0..=32.0)
                        .text(language.text("ui.text_size")),
                );
                ui.label(language.text("ui.extra_fallback_font"));
                ui.text_edit_singleline(&mut self.settings.fallback_font);
                if ui.button(language.text("ui.apply_font")).clicked() {
                    self.fonts = fonts::Fonts::new(context, &self.settings.fallback_font);
                }
                ui.weak(language.format("ui.fallback_fonts_loaded", &[&self.fonts.fallback_count]));
                if let Some(error) = &self.fonts.error {
                    ui.colored_label(Color32::from_rgb(170, 50, 45), language.message(error));
                }
                color_setting(ui, language.text("ui.text"), &mut self.settings.text_color);
                color_setting(
                    ui,
                    language.text("ui.background"),
                    &mut self.settings.background_color,
                );
                color_setting(
                    ui,
                    language.text("ui.hyperlinks"),
                    &mut self.settings.hyperlink_color,
                );
                ui.checkbox(
                    &mut self.settings.window_borders,
                    language.text("ui.borders_between_game_windows"),
                );
                ui.checkbox(
                    &mut self.settings.show_chrome,
                    language.text("ui.menus_toolbar_and_status_bar"),
                );
                ui.separator();
                ui.heading(language.text("ui.translation"));
                ui.checkbox(
                    &mut self.settings.translation.enabled,
                    language.text("ui.enable_turn_translation"),
                );
                ui.label(language.text("ui.openai_compatible_endpoint"));
                ui.text_edit_singleline(&mut self.settings.translation.endpoint);
                ui.label(language.text("ui.model"));
                ui.text_edit_singleline(&mut self.settings.translation.model);
                ui.label(language.text("ui.target_language"));
                ui.text_edit_singleline(&mut self.settings.translation.target_language);
                ui.label(language.text("ui.api_key_kept_in_local_app_settings"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.settings.translation.api_key)
                        .password(true),
                );
                ui.label(language.text("ui.system_prompt"));
                ui.add(
                    egui::TextEdit::multiline(&mut self.settings.translation.system_prompt)
                        .desired_rows(4),
                );
            });
        egui::Window::new(language.text("ui.scrollback"))
            .id(egui::Id::new("ui.scrollback"))
            .open(&mut self.show_scrollback)
            .default_size([720.0, 560.0])
            .show(context, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.label(&self.transcript);
                });
            });
        egui::Window::new(language.text("ui.about_glulx_player"))
            .id(egui::Id::new("ui.about_glulx_player"))
            .open(&mut self.show_about)
            .collapsible(false)
            .resizable(false)
            .show(context, |ui| {
                ui.heading("Glulx Player 0.1.0");
                ui.label(language.text("ui.a_pure_rust_glulx_vm_with_a_cross"));
                ui.label(language.text("ui.vm_behavior_is_based_on_the_glulx_specification"));
                ui.label(language.text("ui.the_interface_follows_the_windows_git_player_workflow"));
            });
    }
}

impl eframe::App for PlayerApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, STORAGE_KEY, &self.settings);
        if let Some(vm) = &self.vm {
            let _ = vm.flush_streams();
        }
        if let Some(vm) = &self.vm
            && vm.state() != RunState::Halted
        {
            let canvases = self
                .graphics
                .iter()
                .map(|(&window, canvas)| SavedCanvas {
                    window,
                    width: canvas.pixels.width(),
                    height: canvas.pixels.height(),
                    pixels: canvas.pixels.as_raw().clone(),
                })
                .collect();
            eframe::set_value(
                storage,
                "glulx-session-v1",
                &SessionRef {
                    version: 1,
                    vm,
                    transcript: &self.transcript,
                    input: &self.input,
                    timer: vm.timer_interval(),
                    canvases,
                },
            );
        } else {
            storage.set_string("glulx-session-v1", String::new());
        }
    }

    fn auto_save_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(30)
    }

    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        // Eframe also runs logic while the window is hidden. Keep timers,
        // sound notifications and translation results progressing there.
        self.run_vm();
        self.poll_translations();
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

    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let context = root.ctx().clone();
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
                .map(|file| file.path().to_owned())
                .collect::<Vec<_>>()
        }) {
            if is_story_path(&path) {
                self.load_story(path);
                break;
            }
        }
        self.character_input(&context);
        self.poll_graphics(&context);
        self.menu_bar(root);

        self.status_bar(root);
        self.input_bar(root);
        self.story_view(root);
        self.dialogs(&context);
        if let Some(path) = self
            .file_browser
            .show(&context, self.settings.language.resolve())
        {
            if self.file_browser.resources {
                self.resource_path = path.display().to_string();
                self.resource_choice = 2;
                self.show_resources = true;
            } else {
                self.load_story(path);
            }
        }
        if self
            .vm
            .as_ref()
            .is_some_and(|vm| vm.state() == RunState::Running)
        {
            context.request_repaint();
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

fn ensure_canvas<'a>(
    context: &egui::Context,
    graphics: &'a mut BTreeMap<u32, DisplayedGraphics>,
    window: u32,
    size: [u32; 2],
    background: u32,
) -> &'a mut DisplayedGraphics {
    let canvas = graphics.entry(window).or_insert_with(|| {
        let mut canvas = DisplayedGraphics::new(context, window, size);
        for pixel in canvas.pixels.pixels_mut() {
            *pixel = rgba(background);
        }
        canvas
    });
    if canvas.pixels.dimensions() != (size[0].max(1), size[1].max(1)) {
        let mut resized =
            image::RgbaImage::from_pixel(size[0].max(1), size[1].max(1), rgba(background));
        image::imageops::overlay(&mut resized, &canvas.pixels, 0, 0);
        canvas.pixels = resized;
    }
    canvas
}

fn fill_rect(canvas: &mut image::RgbaImage, rect: [i32; 4], color: u32) {
    let [left, top, width, height] = rect;
    let (left, top) = (i64::from(left), i64::from(top));
    let (width, height) = (i64::from(width as u32), i64::from(height as u32));
    if width == 0 || height == 0 {
        return;
    }
    let right = (left + width).clamp(0, i64::from(canvas.width()));
    let bottom = (top + height).clamp(0, i64::from(canvas.height()));
    let left = left.clamp(0, i64::from(canvas.width()));
    let top = top.clamp(0, i64::from(canvas.height()));
    let color = rgba(color);
    for y in top..bottom {
        for x in left..right {
            canvas.put_pixel(x as u32, y as u32, color);
        }
    }
}

fn rgba(color: u32) -> image::Rgba<u8> {
    image::Rgba([
        ((color >> 16) & 0xff) as u8,
        ((color >> 8) & 0xff) as u8,
        (color & 0xff) as u8,
        0xff,
    ])
}

fn texture_image(context: &egui::Context, pixels: &image::RgbaImage) -> egui::ColorImage {
    let maximum = context.input(|input| input.max_texture_side).max(1) as u32;
    let side = pixels.width().max(pixels.height());
    if side <= maximum {
        return color_image(pixels);
    }
    let width = (u64::from(pixels.width()) * u64::from(maximum) / u64::from(side)).max(1) as u32;
    let height = (u64::from(pixels.height()) * u64::from(maximum) / u64::from(side)).max(1) as u32;
    color_image(&image::imageops::resize(
        pixels,
        width,
        height,
        image::imageops::FilterType::Triangle,
    ))
}

fn color_image(pixels: &image::RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied(
        [pixels.width() as usize, pixels.height() as usize],
        pixels.as_raw(),
    )
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

fn color_word(value: u32) -> Color32 {
    Color32::from_rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

fn glk_character_key(key: egui::Key) -> Option<u32> {
    use egui::Key;
    Some(match key {
        Key::ArrowLeft => 0xffff_fffe,
        Key::ArrowRight => 0xffff_fffd,
        Key::ArrowUp => 0xffff_fffc,
        Key::ArrowDown => 0xffff_fffb,
        Key::Enter => 0xffff_fffa,
        Key::Delete | Key::Backspace => 0xffff_fff9,
        Key::Escape => 0xffff_fff8,
        Key::Tab => 0xffff_fff7,
        Key::PageUp => 0xffff_fff6,
        Key::PageDown => 0xffff_fff5,
        Key::Home => 0xffff_fff4,
        Key::End => 0xffff_fff3,
        Key::F1 => 0xffff_ffef,
        Key::F2 => 0xffff_ffee,
        Key::F3 => 0xffff_ffed,
        Key::F4 => 0xffff_ffec,
        Key::F5 => 0xffff_ffeb,
        Key::F6 => 0xffff_ffea,
        Key::F7 => 0xffff_ffe9,
        Key::F8 => 0xffff_ffe8,
        Key::F9 => 0xffff_ffe7,
        Key::F10 => 0xffff_ffe6,
        Key::F11 => 0xffff_ffe5,
        Key::F12 => 0xffff_ffe4,
        _ => return None,
    })
}

fn glk_terminator_key(code: u32) -> Option<egui::Key> {
    use egui::Key;
    if code == 0xffff_fff8 {
        return Some(Key::Escape);
    }
    [
        Key::F1,
        Key::F2,
        Key::F3,
        Key::F4,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::F9,
        Key::F10,
        Key::F11,
        Key::F12,
    ]
    .get(0xffff_ffefu32.wrapping_sub(code) as usize)
    .copied()
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

    #[test]
    fn graphics_fill_is_clipped_to_the_canvas() {
        let mut canvas = image::RgbaImage::from_pixel(3, 2, rgba(0xffffff));
        fill_rect(&mut canvas, [-1, 1, 3, 2], 0x123456);

        assert_eq!(*canvas.get_pixel(0, 1), rgba(0x123456));
        assert_eq!(*canvas.get_pixel(1, 1), rgba(0x123456));
        assert_eq!(*canvas.get_pixel(2, 1), rgba(0xffffff));
        assert_eq!(*canvas.get_pixel(0, 0), rgba(0xffffff));
    }

    #[test]
    fn narrow_large_images_upload_within_the_host_texture_limit() {
        let context = egui::Context::default();
        for size in [(20_000, 1), (1, 20_000), (4000, 2000)] {
            let pixels = image::RgbaImage::from_pixel(size.0, size.1, rgba(0x123456));
            let image = texture_image(&context, &pixels);
            let maximum = context.input(|input| input.max_texture_side);
            assert!(image.size.iter().all(|side| *side > 0 && *side <= maximum));
            assert!(
                image
                    .pixels
                    .iter()
                    .all(|pixel| *pixel == color_word(0x123456))
            );
            let texture =
                context.load_texture("oversize-picture", image, egui::TextureOptions::LINEAR);
            assert!(texture.size().iter().all(|side| *side <= maximum));
            assert_eq!(pixels.dimensions(), size);
        }
    }

    #[test]
    fn graphics_fill_clips_unsigned_extents_without_signed_overflow() {
        let mut canvas = image::RgbaImage::from_pixel(3, 2, rgba(0xffffff));
        fill_rect(&mut canvas, [i32::MIN, i32::MIN, -1, -1], 0x123456);
        assert!(canvas.pixels().all(|pixel| *pixel == rgba(0x123456)));
        fill_rect(&mut canvas, [0, 0, i32::MIN, i32::MIN], 0xabcdef);
        assert!(canvas.pixels().all(|pixel| *pixel == rgba(0xabcdef)));
        fill_rect(&mut canvas, [i32::MAX, 0, -1, -1], 0);
        fill_rect(&mut canvas, [0, 0, 0, -1], 0);
        assert!(canvas.pixels().all(|pixel| *pixel == rgba(0xabcdef)));
    }

    #[test]
    fn resizing_graphics_preserves_visible_pixels_and_discards_cropped_ones() {
        let context = egui::Context::default();
        let mut graphics = BTreeMap::new();
        let canvas = ensure_canvas(&context, &mut graphics, 1, [4, 3], 0x112233);
        canvas.pixels.put_pixel(0, 0, rgba(0xabcdef));
        canvas.pixels.put_pixel(3, 2, rgba(0x334455));
        // Resize twice without an intervening draw; clipped pixels must not
        // reappear, and only the newly exposed region gets the new background.
        ensure_canvas(&context, &mut graphics, 1, [2, 1], 0x556677);
        let canvas = ensure_canvas(&context, &mut graphics, 1, [4, 3], 0x778899);
        assert_eq!(*canvas.pixels.get_pixel(0, 0), rgba(0xabcdef));
        assert_eq!(*canvas.pixels.get_pixel(1, 0), rgba(0x112233));
        assert_eq!(*canvas.pixels.get_pixel(2, 0), rgba(0x778899));
        assert_eq!(*canvas.pixels.get_pixel(3, 2), rgba(0x778899));
    }
}

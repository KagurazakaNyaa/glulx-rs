use crate::memory_budget::{Budget, MemoryPolicy, Overrides, ResourceBudgets, startup_snapshot};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
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

mod canvas;
mod font_dialog;
use canvas::Canvas as DisplayedGraphics;
mod fonts;
mod media;
mod settings_file;
mod text_buffer;
mod text_grid;
mod windows;

const STORAGE_KEY: &str = "glulx-rs-settings";
// RON expands each byte into a decimal integer, then eframe copies the whole
// string. Large media packages must not enter this synchronous UI-thread path.
const MAX_DESKTOP_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerSettings {
    pub language: LanguagePreference,
    pub max_memory_mib: Budget,
    pub max_process_memory_mib: Budget,
    pub resource_limits: ResourceBudgets,
    pub font_size: f32,
    pub fallback_font: String,
    pub system_font: String,
    pub text_color: [u8; 3],
    pub background_color: [u8; 3],
    pub hyperlink_color: [u8; 3],
    pub show_chrome: bool,
    pub window_borders: bool,
    pub show_log_window: bool,
    pub show_translation_window: bool,
    pub translation: TranslationSettings,
}

impl Default for PlayerSettings {
    fn default() -> Self {
        Self {
            language: LanguagePreference::System,
            max_memory_mib: MemoryPolicy::default().max_memory_mib,
            max_process_memory_mib: Budget::Fixed(0),
            resource_limits: ResourceBudgets::default(),
            font_size: 18.0,
            fallback_font: String::new(),
            system_font: String::new(),
            text_color: [32, 34, 37],
            background_color: [248, 248, 246],
            hyperlink_color: [20, 94, 150],
            show_chrome: true,
            window_borders: true,
            show_log_window: true,
            show_translation_window: true,
            translation: TranslationSettings::default(),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
enum TurnTranslation {
    #[default]
    NotRequested,
    Pending,
    Complete(Result<String, String>),
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Turn {
    view: u64,
    original: String,
    translation: TurnTranslation,
}

#[derive(Serialize, Deserialize)]
struct SavedCanvas {
    window: u32,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    #[serde(default)]
    links: Vec<SavedLinkRegion>,
}
#[derive(Serialize, Deserialize)]
struct SavedLinkRegion {
    clip: [u32; 4],
    hyperlink: u32,
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

struct PendingImageDecode {
    id: u64,
    request: crate::vm::ImageRequest,
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
    memory_overrides: Overrides,
    settings_file: settings_file::SettingsFile,
    vm: Option<Vm>,
    story_path: Option<PathBuf>,
    story_title: String,
    transcript: String,
    transcript_lines: Vec<std::ops::Range<usize>>,
    turn_buffer: String,
    turn_windows: Vec<(u32, String)>,
    translation_view: u64,
    translation_view_start: usize,
    translation_view_settings: Option<TranslationSettings>,
    new_translation_view: bool,
    translation_capture_enabled: bool,
    turns: Vec<Turn>,
    input: String,
    pending_keys: VecDeque<u32>,
    game_status: String,
    graphics: BTreeMap<u32, DisplayedGraphics>,
    dirty_graphics: HashSet<u32>,
    presented_graphics: BTreeMap<u32, std::sync::Arc<DisplayedGraphics>>,
    presented_views: std::sync::Arc<[crate::vm::WindowView]>,
    presented_content_revisions: BTreeMap<u32, u64>,
    presented_revision: u64,
    text_layout_revisions: BTreeMap<u32, u64>,
    text_layouts: text_buffer::LayoutCache,
    grid_galleys: text_grid::GalleyCache,
    presented_state: RunState,
    image_cache: HashMap<u32, (std::sync::Arc<canvas::ImageAsset>, u64)>,
    image_cache_tick: u64,
    image_decoder: media::ImageDecodeWorker,
    image_results: HashMap<u64, Result<std::sync::Arc<image::RgbaImage>, String>>,
    pending_graphics: VecDeque<GraphicsRequest>,
    pending_image: Option<PendingImageDecode>,
    gpu_canvas: bool,
    buffer_images: text_buffer::ImageCache,
    status: String,
    error: Option<String>,
    show_options: bool,
    show_translation_settings: bool,
    show_about: bool,
    show_story_info: bool,
    cover: Option<egui::TextureHandle>,
    file_browser: FileBrowser,
    translator: Translator,
    pending_translations: HashMap<u64, usize>,
    last_state: RunState,
    fonts: fonts::Fonts,
    pending_font_metrics: bool,
    show_resources: bool,
    resource_choice: u8,
    resource_path: String,
}

impl PlayerApp {
    fn clear_presentation(&mut self) {
        self.text_layouts.clear();
        self.grid_galleys.clear();
        self.pending_keys.clear();
        self.dirty_graphics.clear();
        self.presented_graphics.clear();
        self.presented_views = Default::default();
        self.presented_content_revisions.clear();
        self.presented_revision = 0;
        self.text_layout_revisions.clear();
        self.presented_state = RunState::Running;
    }

    fn clear_media(&mut self) {
        self.image_cache.clear();
        self.image_results.clear();
        self.pending_graphics.clear();
        self.pending_image = None;
    }

    fn rebuild_transcript_index(&mut self) {
        self.transcript_lines.clear();
        if self.transcript.is_empty() {
            return;
        }
        let mut start = 0;
        for (offset, byte) in self.transcript.bytes().enumerate() {
            if byte == b'\n' {
                self.transcript_lines.push(start..offset);
                start = offset + 1;
            }
        }
        self.transcript_lines.push(start..self.transcript.len());
    }

    fn append_transcript(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let start = self.transcript.len();
        self.transcript.push_str(text);
        if self.transcript_lines.is_empty() && start != 0 {
            self.rebuild_transcript_index();
        }
        if self.transcript_lines.is_empty() {
            self.transcript_lines.push(start..start);
        }
        for (offset, byte) in text.bytes().enumerate() {
            let end = start + offset + 1;
            if byte == b'\n' {
                self.transcript_lines.last_mut().unwrap().end = end - 1;
                self.transcript_lines.push(end..end);
            } else {
                self.transcript_lines.last_mut().unwrap().end = end;
            }
        }
    }

    fn desktop_snapshot_too_large(&self) -> bool {
        let bytes = self
            .vm
            .as_ref()
            .map_or(0, Vm::snapshot_byte_len)
            .saturating_add(self.transcript.len())
            .saturating_add(self.input.len());
        self.graphics.values().fold(bytes, |bytes, canvas| {
            bytes.saturating_add(canvas.size[0] as usize * canvas.size[1] as usize * 4)
        }) > MAX_DESKTOP_SNAPSHOT_BYTES
    }

    pub fn new(creation: &eframe::CreationContext<'_>, initial_story: Option<PathBuf>) -> Self {
        Self::new_with_resources(creation, initial_story, ResourceSelection::Auto)
    }

    pub fn new_with_resources(
        creation: &eframe::CreationContext<'_>,
        initial_story: Option<PathBuf>,
        selection: ResourceSelection,
    ) -> Self {
        Self::new_with_memory_policy(creation, initial_story, selection, Overrides::default())
    }

    pub fn new_with_memory_policy(
        creation: &eframe::CreationContext<'_>,
        initial_story: Option<PathBuf>,
        selection: ResourceSelection,
        memory_overrides: Overrides,
    ) -> Self {
        let _ = startup_snapshot();
        let _stage = crate::diagnostics::stage("create-app-or-restore-session");
        let mut gpu_canvas = false;
        if let Some(gl) = &creation.gl {
            use eframe::glow::HasContext;
            // Eframe has made this context current during app creation.
            let renderer = unsafe { gl.get_parameter_string(eframe::glow::RENDERER) };
            let software = [
                "llvmpipe",
                "softpipe",
                "swiftshader",
                "software",
                "gdi generic",
            ]
            .iter()
            .any(|name| renderer.to_lowercase().contains(name));
            gpu_canvas = !software || std::env::var_os("GLULX_FORCE_GPU").is_some();
            crate::diagnostics::record(format_args!(
                "graphics_gpu={gpu_canvas} renderer={renderer}"
            ));
        }
        let settings: PlayerSettings = creation
            .storage
            .and_then(|storage| eframe::get_value(storage, STORAGE_KEY))
            .unwrap_or_default();
        let (mut settings_file, settings) = settings_file::SettingsFile::application(settings);
        settings_file.save(&settings);
        let fonts = fonts::Fonts::new(
            &creation.egui_ctx,
            &settings.fallback_font,
            &settings.system_font,
        );
        let mut app = Self {
            translation_capture_enabled: settings.translation.enabled,
            settings,
            memory_overrides,
            settings_file,
            vm: None,
            story_path: None,
            story_title: "Glulx Player".to_owned(),
            transcript: String::new(),
            transcript_lines: Vec::new(),
            turn_buffer: String::new(),
            turn_windows: Vec::new(),
            translation_view: 0,
            translation_view_start: 0,
            translation_view_settings: None,
            new_translation_view: false,
            turns: Vec::new(),
            input: String::new(),
            pending_keys: VecDeque::new(),
            game_status: String::new(),
            graphics: BTreeMap::new(),
            dirty_graphics: HashSet::new(),
            presented_graphics: BTreeMap::new(),
            presented_views: Default::default(),
            presented_content_revisions: BTreeMap::new(),
            presented_revision: 0,
            text_layout_revisions: BTreeMap::new(),
            text_layouts: Default::default(),
            grid_galleys: Default::default(),
            presented_state: RunState::Running,
            image_cache: HashMap::new(),
            image_cache_tick: 0,
            image_decoder: Default::default(),
            image_results: HashMap::new(),
            pending_graphics: VecDeque::new(),
            pending_image: None,
            gpu_canvas,
            buffer_images: text_buffer::ImageCache::default(),
            status: "ui.open_a_ulx_or_gblorb_story_to_begin".to_owned(),
            error: None,
            show_options: false,
            show_translation_settings: false,
            show_about: false,
            show_story_info: false,
            cover: None,
            file_browser: FileBrowser::new(),
            translator: Translator::new(),
            pending_translations: HashMap::new(),
            last_state: RunState::Halted,
            fonts,
            pending_font_metrics: false,
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
            match (|| {
                let mut vm = session.vm;
                let policy = app.memory_policy();
                vm.set_resource_limits(policy.resource_limits.resolve(startup_snapshot())?);
                vm.set_memory_limit(policy.max_memory_mib.vm_bytes(startup_snapshot())?)
                    .map_err(|error| error.to_string())?;
                vm.validate_session().map_err(|error| error.to_string())
            })() {
                Ok(mut vm) => {
                    vm.enable_audio();
                    vm.resume_timer(session.timer);
                    app.story_title = vm.story_title().to_owned();
                    app.story_path = vm.story_path().map(Path::to_owned);
                    app.last_state = vm.state();
                    app.vm = Some(vm);
                    app.transcript = session.transcript;
                    app.rebuild_transcript_index();
                    app.input = session.input;
                    for canvas in session.canvases {
                        if let Some(pixels) =
                            image::RgbaImage::from_raw(canvas.width, canvas.height, canvas.pixels)
                        {
                            let links = canvas
                                .links
                                .iter()
                                .map(|link| (link.clip, link.hyperlink))
                                .collect::<Vec<_>>();
                            app.graphics.insert(
                                canvas.window,
                                DisplayedGraphics::from_pixels_with_links(
                                    &creation.egui_ctx,
                                    pixels,
                                    &links,
                                ),
                            );
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

    fn memory_policy(&self) -> MemoryPolicy {
        self.memory_overrides.apply(MemoryPolicy {
            max_memory_mib: self.settings.max_memory_mib,
            max_process_memory_mib: self.settings.max_process_memory_mib,
            resource_limits: self.settings.resource_limits,
        })
    }

    fn load_story(&mut self, path: PathBuf) {
        self.load_story_with_resources(path, ResourceSelection::Auto);
    }

    fn load_story_with_resources(&mut self, path: PathBuf, selection: ResourceSelection) {
        let _stage = crate::diagnostics::stage("load-story");
        let loaded = (|| -> Result<Vm, String> {
            let policy = self.memory_policy();
            let maximum = policy.max_memory_mib.vm_bytes(startup_snapshot())?;
            let resources = policy.resource_limits.resolve(startup_snapshot())?;
            let story =
                Story::open_with_resources(&path, selection).map_err(|error| error.to_string())?;
            let mut vm =
                Vm::new_with_memory_limit(story, maximum).map_err(|error| error.to_string())?;
            vm.set_resource_limits(resources);
            Ok(vm)
        })();
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
                self.transcript_lines.clear();
                self.reset_translation_history();
                self.input.clear();
                self.game_status.clear();
                self.graphics.clear();
                self.clear_presentation();
                self.clear_media();
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
                    self.transcript_lines.clear();
                    self.reset_translation_history();
                    self.game_status.clear();
                    self.graphics.clear();
                    self.clear_presentation();
                    self.clear_media();
                    self.buffer_images.clear();
                    self.error = None;
                    self.status = "ui.story_restarted".to_owned();
                    self.last_state = RunState::Running;
                }
                Err(error) => self.fail(error.to_string()),
            }
        }
    }
    fn stop_story(&mut self) {
        if let Some(vm) = &mut self.vm {
            vm.stop();
        }
    }

    fn pending_input(&self) -> Option<InputRequest> {
        self.vm.as_ref().and_then(Vm::pending_input_request)
    }

    fn run_vm(&mut self) {
        self.sync_translation_capture();
        let _stage = crate::diagnostics::stage("vm-slice");
        let capture_translation = self.settings.translation.enabled;
        let Some((
            presentation_revision,
            output,
            game_status,
            text_events,
            state,
            initial_input,
            current_presentation_revision,
            vm_error,
        )) = self.vm.as_mut().map(|vm| {
            if capture_translation {
                vm.enable_text_buffer_events();
            } else {
                vm.disable_text_buffer_events();
            }
            let word = |c: [u8; 3]| u32::from_be_bytes([0, c[0], c[1], c[2]]);
            vm.set_light_fonts(self.fonts.light_fonts);
            vm.set_glyph_support(self.fonts.support.clone());
            if !self.pending_font_metrics {
                vm.set_text_metrics(self.fonts.metrics.clone());
            }
            vm.set_text_appearance(
                self.settings.font_size,
                word(self.settings.text_color),
                word(self.settings.background_color),
            );
            let presentation_revision = vm.presentation_revision();
            let vm_error = run_vm_slice(vm).err().map(|error| {
                let pc = vm.pc();
                vm.stop();
                format!("{error}\nProgram counter: {pc:#010x}")
            });
            let output = vm.take_output();
            crate::diagnostics::vm(vm);
            let game_status = vm.status_text();
            let text_events = vm.take_text_buffer_events();
            let state = vm.state();
            let initial_input = (state == RunState::WaitingForLine).then(|| vm.initial_input());
            let current_presentation_revision = vm.presentation_revision();
            (
                presentation_revision,
                output,
                game_status,
                text_events,
                state,
                initial_input,
                current_presentation_revision,
                vm_error,
            )
        })
        else {
            return;
        };
        self.game_status = game_status;
        if !output.is_empty() {
            self.append_transcript(&output);
        }
        if let Some(error) = vm_error {
            self.error = Some(error);
            self.status = "ui.vm_stopped_after_an_error".to_owned();
        }
        let output_boundary = current_presentation_revision != presentation_revision;
        if state != self.last_state {
            crate::diagnostics::record(format_args!("state {:?} -> {state:?}", self.last_state));
        }
        // A timer can cancel and re-request input during one VM slice, leaving
        // the same RunState but supplying a different prefilled line.
        if let Some(initial_input) = initial_input {
            self.input = initial_input;
        }
        self.capture_translation_events(text_events);
        if output_boundary || (state != RunState::Running && self.last_state == RunState::Running) {
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
        self.turn_windows.clear();
        if source.trim().is_empty() {
            return;
        }
        let starting_view = self.new_translation_view || self.turns.is_empty();
        if self.new_translation_view && !self.turns.is_empty() {
            let current = &self.turns[self.translation_view_start..];
            let same_source = current
                .iter()
                .map(|turn| turn.original.as_str())
                .collect::<String>()
                == source;
            let same_capture = current.iter().all(|turn| {
                match (&turn.translation, self.settings.translation.enabled) {
                    (TurnTranslation::NotRequested, false) => true,
                    (TurnTranslation::Pending | TurnTranslation::Complete(Ok(_)), true) => {
                        self.translation_view_settings.as_ref() == Some(&self.settings.translation)
                    }
                    _ => false,
                }
            });
            if same_source && same_capture {
                self.new_translation_view = false;
                return;
            }
            self.translation_view += 1;
            self.translation_view_start = self.turns.len();
        }
        self.new_translation_view = false;
        if starting_view {
            self.translation_view_settings = Some(self.settings.translation.clone());
        } else if self.translation_view_settings.as_ref() != Some(&self.settings.translation) {
            self.translation_view_settings = None; // This view contains mixed capture settings.
        }
        let index = self.turns.len();
        self.turns.push(Turn {
            view: self.translation_view,
            original: source.clone(),
            translation: TurnTranslation::NotRequested,
        });
        if self.settings.translation.enabled {
            match self.translator.submit(source, &self.settings.translation) {
                Submission::Cached(value) => {
                    self.turns[index].translation = TurnTranslation::Complete(Ok(value));
                    crate::diagnostics::record(format_args!("translation cached turn={index}"));
                }
                Submission::Queued(id) => {
                    self.turns[index].translation = TurnTranslation::Pending;
                    self.pending_translations.insert(id, index);
                    crate::diagnostics::record(format_args!(
                        "translation queued turn={index} id={id}"
                    ));
                }
            }
        }
    }

    fn poll_translations(&mut self) {
        self.sync_translation_capture();
        for result in self.translator.poll() {
            if let Some(index) = self.pending_translations.remove(&result.id)
                && let Some(turn) = self.turns.get_mut(index)
            {
                turn.translation = TurnTranslation::Complete(result.result);
                crate::diagnostics::record(format_args!(
                    "translation completed turn={index} id={}",
                    result.id
                ));
            }
        }
    }

    fn sync_translation_capture(&mut self) {
        if self.translation_capture_enabled != self.settings.translation.enabled {
            // Match the reference capture boundary: changing the switch drops
            // unfinished capture, but never touches recorded turns or requests.
            self.turn_buffer.clear();
            self.turn_windows.clear();
            self.translation_capture_enabled = self.settings.translation.enabled;
        }
    }

    fn capture_translation_events(&mut self, events: Vec<crate::vm::TextBufferEvent>) {
        for event in events {
            match event {
                crate::vm::TextBufferEvent::Text { window, text } => {
                    self.turn_buffer.push_str(&text);
                    if let Some((last_window, last_text)) = self.turn_windows.last_mut()
                        && *last_window == window
                    {
                        last_text.push_str(&text);
                    } else {
                        self.turn_windows.push((window, text));
                    }
                }
                crate::vm::TextBufferEvent::Clear { window } => {
                    self.turn_windows.retain(|(id, _)| *id != window);
                    self.turn_buffer = self
                        .turn_windows
                        .iter()
                        .map(|(_, text)| text.as_str())
                        .collect();
                    self.new_translation_view = true;
                }
            }
        }
    }

    fn reset_translation_history(&mut self) {
        self.turn_buffer.clear();
        self.turn_windows.clear();
        self.turns.clear();
        self.pending_translations.clear();
        self.translation_view = 0;
        self.translation_view_start = 0;
        self.translation_view_settings = None;
        self.new_translation_view = false;
    }

    fn apply_draw_request(
        &mut self,
        context: &egui::Context,
        request: crate::vm::ImageRequest,
        decoded: Option<Result<std::sync::Arc<image::RgbaImage>, String>>,
        limits: crate::memory::ResourceLimits,
    ) {
        if request.canvas_size.contains(&0) {
            return;
        }
        self.image_cache_tick += 1;
        let tick = self.image_cache_tick;
        let source = if let Some((source, used)) = self.image_cache.get_mut(&request.resource) {
            *used = tick;
            source.clone()
        } else {
            let Some(decoded) = decoded else {
                return;
            };
            match decoded {
                Ok(decoded) => {
                    let source = canvas::ImageAsset::new(context, decoded);
                    self.image_cache
                        .insert(request.resource, (source.clone(), tick));
                    while self
                        .image_cache
                        .values()
                        .map(|(asset, _)| asset.byte_len())
                        .sum::<usize>()
                        > crate::memory::ResourceLimits::bytes(limits.graphics_cache_mib)
                    {
                        let oldest = *self
                            .image_cache
                            .iter()
                            .min_by_key(|(_, (_, used))| *used)
                            .unwrap()
                            .0;
                        self.image_cache.remove(&oldest);
                    }
                    source
                }
                Err(error) => {
                    self.status = format!("Could not decode picture {}: {error}", request.resource);
                    return;
                }
            }
        };
        let size = request
            .requested_size
            .unwrap_or([source.pixels.width(), source.pixels.height()]);
        let canvas = ensure_canvas(
            context,
            &mut self.graphics,
            request.window,
            request.canvas_size,
            0xffffff,
        );
        canvas.use_cpu(!self.gpu_canvas);
        canvas.draw_hyperlinked(context, source, request.position, size, request.hyperlink);
        self.dirty_graphics.insert(request.window);
    }

    fn poll_graphics(&mut self, context: &egui::Context) {
        let _stage = crate::diagnostics::stage("graphics");
        let limits = self
            .vm
            .as_ref()
            .map(Vm::resource_limits)
            .unwrap_or_default();
        let results: Vec<_> = self.image_decoder.poll().collect();
        for result in results {
            self.image_results.insert(result.id, result.image);
        }
        self.pending_graphics
            .extend(self.vm.as_mut().map(Vm::take_graphics).unwrap_or_default());

        if let Some(pending) = self.pending_image.take() {
            let Some(decoded) = self.image_results.remove(&pending.id) else {
                self.pending_image = Some(pending);
                context.request_repaint();
                return;
            };
            self.apply_draw_request(context, pending.request, Some(decoded), limits);
        }

        while let Some(request) = self.pending_graphics.pop_front() {
            match request {
                GraphicsRequest::Resize {
                    window,
                    background,
                    canvas_size,
                } => {
                    if canvas_size.contains(&0) {
                        self.graphics.remove(&window);
                        self.dirty_graphics.insert(window);
                    } else {
                        ensure_canvas(context, &mut self.graphics, window, canvas_size, background)
                            .use_cpu(!self.gpu_canvas);
                        self.dirty_graphics.insert(window);
                    }
                }
                GraphicsRequest::Draw(mut request) => {
                    if request.canvas_size.contains(&0) {
                        continue;
                    }
                    if self.image_cache.contains_key(&request.resource) {
                        self.apply_draw_request(context, request, None, limits);
                        continue;
                    }
                    if let Some(decoded) = request.decoded.take() {
                        self.apply_draw_request(context, request, Some(Ok(decoded)), limits);
                        continue;
                    }
                    let data = std::mem::take(&mut request.data);
                    let Some(id) = self.image_decoder.submit(
                        data,
                        crate::memory::ResourceLimits::bytes(limits.decoded_image_mib) as u64,
                    ) else {
                        self.status = format!(
                            "Could not decode picture {}: worker unavailable",
                            request.resource
                        );
                        continue;
                    };
                    self.pending_image = Some(PendingImageDecode { id, request });
                    context.request_repaint();
                    break;
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
                    canvas.use_cpu(!self.gpu_canvas);
                    canvas.fill(context, rect, color);
                    self.dirty_graphics.insert(window);
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
                    canvas.use_cpu(!self.gpu_canvas);
                    canvas.clear(color);
                    self.dirty_graphics.insert(window);
                }
                GraphicsRequest::Close { window } => {
                    self.graphics.remove(&window);
                    self.dirty_graphics.insert(window);
                }
            }
        }
    }

    fn publish_story(&mut self, context: &egui::Context) {
        let Some(vm) = &self.vm else {
            return;
        };
        let revision = vm.presentation_revision();
        let state = vm.state();
        if revision == self.presented_revision
            && (state == RunState::Running || state == self.presented_state)
        {
            return;
        }
        self.presented_revision = revision;
        self.presented_state = state;
        let descriptors = vm.window_descriptors();
        let current_ids: HashSet<u32> =
            descriptors.iter().map(|descriptor| descriptor.id).collect();
        let changed_ids: Vec<u32> = descriptors
            .iter()
            .filter(|descriptor| {
                let old = self
                    .presented_views
                    .iter()
                    .find(|view| view.id == descriptor.id);
                old.is_none_or(|view| {
                    self.presented_content_revisions.get(&descriptor.id)
                        != Some(&descriptor.content_revision)
                        || !same_window_metadata(view, descriptor)
                })
            })
            .map(|descriptor| descriptor.id)
            .collect();
        let removed = self
            .presented_views
            .iter()
            .any(|view| !current_ids.contains(&view.id));
        let text_changed = !changed_ids.is_empty() || removed;
        if !text_changed && self.dirty_graphics.is_empty() {
            return;
        }
        let _stage = crate::diagnostics::stage("publish-story");
        if text_changed {
            let structural = removed
                || changed_ids
                    .iter()
                    .any(|id| !self.presented_views.iter().any(|view| view.id == *id));
            let replacements: Vec<_> = changed_ids
                .iter()
                .filter_map(|id| vm.window_view(*id).map(|view| (*id, view)))
                .collect();
            if structural {
                let mut views = self.presented_views.to_vec();
                views.retain(|view| current_ids.contains(&view.id));
                for (id, view) in replacements {
                    if let Some(existing) = views.iter_mut().find(|old| old.id == id) {
                        *existing = view;
                    } else {
                        views.push(view);
                    }
                }
                views.sort_by_key(|view| view.id);
                self.presented_views = views.into();
            } else {
                let views = std::sync::Arc::make_mut(&mut self.presented_views);
                for (id, view) in replacements {
                    if let Some(existing) = views.iter_mut().find(|old| old.id == id) {
                        *existing = view;
                    }
                }
            }
            for descriptor in descriptors
                .iter()
                .filter(|descriptor| changed_ids.contains(&descriptor.id))
            {
                crate::diagnostics::record(format_args!(
                    "window-layout id={} kind={} rect={:?} font_size={} hints={:?}",
                    descriptor.id,
                    descriptor.kind,
                    descriptor.rect,
                    descriptor.appearance.font_size,
                    descriptor.hints
                ));
            }
            self.text_layouts
                .retain_windows(self.presented_views.as_ref());
            self.text_layout_revisions
                .retain(|id, _| current_ids.contains(id));
            for id in &changed_ids {
                let layout_revision = self.text_layout_revisions.entry(*id).or_default();
                *layout_revision = layout_revision.wrapping_add(1);
            }
            self.presented_content_revisions
                .retain(|id, _| current_ids.contains(id));
            for descriptor in descriptors {
                self.presented_content_revisions
                    .insert(descriptor.id, descriptor.content_revision);
            }
        }
        self.dirty_graphics.clear();
        let _graphics = crate::diagnostics::stage("graphics-upload");
        for canvas in self.graphics.values_mut() {
            canvas.use_cpu(!self.gpu_canvas);
            canvas.prepare(context);
        }
        self.presented_graphics = self
            .graphics
            .iter()
            .map(|(&id, canvas)| (id, std::sync::Arc::new(canvas.clone())))
            .collect();
        crate::diagnostics::record(format_args!(
            "present revision={revision} state={state:?} windows={}",
            self.presented_views.len()
        ));
    }

    fn submit_input(&mut self) {
        let _ = self.submit_terminated_input(0);
    }

    fn submit_terminated_input(&mut self, terminator: u32) -> Result<(), ()> {
        let Some(request) = self.vm.as_ref().and_then(Vm::input_request) else {
            return Err(());
        };
        let is_file = matches!(request, InputRequest::File { .. });
        let input = std::mem::take(&mut self.input);
        if !is_file {
            self.append_transcript(&format!("> {input}\n"));
        }
        let Some(vm) = &mut self.vm else {
            return Err(());
        };
        let result = if terminator == 0 {
            vm.provide_input(&input)
        } else {
            vm.provide_terminated_input(&input, terminator)
        };
        if let Err(error) = result {
            self.fail(error.to_string());
            Err(())
        } else {
            self.last_state = RunState::Running;
            Ok(())
        }
    }

    fn submit_key(&mut self, key: u32) -> Result<(), ()> {
        let Some(vm) = &mut self.vm else {
            return Err(());
        };
        self.input.clear();
        if let Err(error) = vm.provide_key(key) {
            self.fail(error.to_string());
            Err(())
        } else {
            self.last_state = RunState::Running;
            Ok(())
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
                        self.stop_story();
                        ui.close();
                    }
                });
                ui.menu_button(language.text("ui.view"), |ui| {
                    if ui.button(language.text("ui.story_information")).clicked() {
                        self.show_story_info = true;
                        ui.close();
                    }
                    if ui
                        .add(
                            egui::Button::new(language.text("ui.log_and_input_window"))
                                .shortcut_text("Ctrl+Shift+L"),
                        )
                        .clicked()
                    {
                        self.settings.show_log_window = true;
                        ui.close();
                    }
                    if ui.button(language.text("ui.options")).clicked() {
                        self.show_options = true;
                        ui.close();
                    }
                    ui.checkbox(
                        &mut self.settings.show_translation_window,
                        language.text("ui.translation_window"),
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
                {
                    self.stop_story();
                }
                ui.separator();
                ui.toggle_value(&mut self.settings.show_log_window, language.text("ui.log"))
                    .on_hover_text(language.text("ui.show_or_hide_log_and_input_window"));
                ui.toggle_value(
                    &mut self.settings.show_translation_window,
                    language.text("ui.translation"),
                )
                .on_hover_text(language.text("ui.show_or_hide_translation_window"));
                if ui
                    .toggle_value(&mut self.show_options, language.text("ui.settings"))
                    .on_hover_text(language.text("ui.show_or_hide_settings_window"))
                    .clicked()
                    && !self.show_options
                {
                    self.settings_file.save(&self.settings);
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
                if let Some(vm) = &mut self.vm {
                    vm.resize_windows(
                        bounds.width().max(0.0) as u32,
                        bounds.height().max(0.0) as u32,
                    );
                }
                self.poll_graphics(&context);
                self.publish_story(&context);
                let views = self.presented_views.clone();
                if views.is_empty() {
                    ui.weak(language.text("ui.preparing_game_display"));
                }
                let mut click = None;
                let mut hyperlink = None;
                let mut graphics_hyperlink = None;
                let mut grid_submitted = false;
                for view in views.iter() {
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
                                                view,
                                                &self.settings,
                                                self.vm.as_ref(),
                                                &mut self.buffer_images,
                                                &mut self.image_decoder,
                                                &mut self.image_results,
                                                &mut self.text_layouts,
                                                self.text_layout_revisions
                                                    .get(&view.id)
                                                    .copied()
                                                    .unwrap_or_default(),
                                            ) {
                                                hyperlink = Some((view.id, value));
                                            }
                                        },
                                    );
                                }
                                4 => {
                                    let editor = self.vm.as_ref().and_then(|vm| {
                                        (accept_input
                                            && !self.settings.show_log_window
                                            && vm.is_grid_line_input()
                                            && vm.input_window() == view.id)
                                            .then_some(text_grid::GridEditor {
                                                text: &mut self.input,
                                                maximum_length: vm.line_input_max_len(),
                                            })
                                    });
                                    let response = text_grid::show(
                                        ui,
                                        rect,
                                        view,
                                        &self.settings,
                                        editor,
                                        &mut self.grid_galleys,
                                    );
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
                                    grid_submitted |= response.submitted
                                        && self.vm.as_ref().and_then(Vm::input_request).is_some();
                                }
                                5 => {
                                    let response = ui.allocate_rect(rect, egui::Sense::click());
                                    if let Some(graphics) = self.presented_graphics.get(&view.id) {
                                        graphics.paint(ui.painter(), rect.min);
                                    }
                                    if response.clicked()
                                        && let Some(pos) = response.interact_pointer_pos()
                                    {
                                        let local = [
                                            (pos.x - rect.min.x).max(0.0) as u32,
                                            (pos.y - rect.min.y).max(0.0) as u32,
                                        ];
                                        click = Some((view.id, local[0], local[1]));
                                        if let Some(value) = self
                                            .presented_graphics
                                            .get(&view.id)
                                            .and_then(|graphics| graphics.hyperlink_at(local))
                                            .filter(|value| *value != 0)
                                        {
                                            graphics_hyperlink = Some((view.id, value));
                                        }
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
                    if let Some((window, value)) = graphics_hyperlink {
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
            || self.show_about
            || self.show_story_info
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
        if let Some(key) = self.pending_keys.pop_front() {
            let _ = self.submit_key(key);
            return;
        }
        let event = context.input(|input| {
            input.events.iter().enumerate().find_map(|(index, event)| {
                let (key, remainder) = match event {
                    egui::Event::Key {
                        key, pressed: true, ..
                    } => (glk_character_key(*key)?, Vec::new()),
                    egui::Event::Text(text) | egui::Event::Paste(text) => {
                        let mut chars = text.chars();
                        let key = chars.next()? as u32;
                        (key, chars.map(|character| character as u32).collect())
                    }
                    _ => return None,
                };
                Some((index, key, remainder))
            })
        });
        if let Some((index, key, remainder)) = event {
            context.input_mut(|input| {
                if index < input.events.len() {
                    input.events.remove(index);
                }
            });
            self.pending_keys.extend(remainder);
            let _ = self.submit_key(key);
        }
    }

    fn replay_pending_paste(&mut self) -> bool {
        let Some(maximum) = self
            .vm
            .as_ref()
            .and_then(Vm::input_request)
            .and_then(|request| match request {
                InputRequest::Line { maximum_length } => Some(maximum_length as usize),
                _ => None,
            })
        else {
            return false;
        };
        let mut changed = false;
        let mut submit = false;
        while let Some(&key) = self.pending_keys.front() {
            if matches!(key, 10 | 13 | 0xffff_fffa) {
                self.pending_keys.pop_front();
                submit = true;
                break;
            }
            let Some(character) = char::from_u32(key) else {
                self.pending_keys.pop_front();
                continue;
            };
            if character.is_control() {
                break;
            }
            self.pending_keys.pop_front();
            if self.input.chars().count() < maximum {
                self.input.push(character);
                changed = true;
            }
        }
        if changed {
            let input = self.input.clone();
            if let Some(vm) = &mut self.vm {
                let _ = vm.update_line_input(&input);
            }
        }
        submit
    }

    fn input_bar(&mut self, root: &mut egui::Ui) {
        let language = self.settings.language.resolve();
        let accept_input = !self.dialog_open(root.ctx());
        let can_submit = self.vm.as_ref().and_then(Vm::input_request).is_some();
        let request = self.pending_input();
        egui::Panel::bottom("input")
            .min_size(51.0)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(235, 237, 238))
                    .inner_margin(10.0),
            )
            .show(root, |ui| {
                ui.set_min_height(30.0);
                if request.is_none() {
                    ui.weak(language.message(&self.status));
                    return;
                }
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
                    let request = self.pending_input();
                    if matches!(request, Some(InputRequest::Character)) {
                        ui.label(language.text("ui.press_a_key"));
                        if ui
                            .add_enabled(can_submit, egui::Button::new(language.text("ui.return")))
                            .clicked()
                        {
                            let _ = self.submit_key(0xffff_fffa);
                        }
                        return;
                    }
                    ui.label(match request {
                        Some(InputRequest::Line { .. }) => ">",
                        Some(InputRequest::File { writing: true }) => language.text("ui.save_file"),
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
                    let enter = accept_input
                        && can_submit
                        && (response.has_focus() || response.lost_focus())
                        && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    // Re-requesting an existing focus interrupts IME in
                    // egui 0.36 and can create a native IME/repaint loop.
                    if accept_input && !ui.memory(|memory| memory.has_focus(response.id)) {
                        response.request_focus();
                    }
                    let terminator = if accept_input
                        && can_submit
                        && matches!(request, Some(InputRequest::Line { .. }))
                    {
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
                        let _ = self.submit_terminated_input(terminator);
                    } else if ui
                        .add_enabled(can_submit, egui::Button::new(language.text("ui.send")))
                        .clicked()
                        || enter
                    {
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
                ui.add(
                    egui::Label::new(RichText::new(language.message(&self.status)).small())
                        .truncate(),
                );
                if self.desktop_snapshot_too_large() {
                    ui.small(language.text("ui.session_restore_off"))
                        .on_hover_text(
                            language.text("ui.this_story_is_too_large_for_automatic_session"),
                        );
                }
                if let Some(path) = &self.story_path {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(RichText::new(path.display().to_string()).small())
                                .truncate(),
                        );
                    });
                }
            });
        });
    }

    fn dialogs(&mut self, context: &egui::Context) {
        let language = self.settings.language.resolve();
        let mut resource_selection = None;
        egui::Window::new(language.text("ui.story_resources"))
            .id(egui::Id::new("story-resources"))
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
                && let Ok(pixels) = crate::picture::decode_with_limit(
                    data,
                    crate::memory::ResourceLimits::bytes(vm.resource_limits().decoded_image_mib)
                        as u64,
                )
            {
                self.cover = Some(context.load_texture(
                    "story-cover",
                    texture_image(context, &pixels),
                    egui::TextureOptions::LINEAR,
                ));
            }
            egui::Window::new(language.text("ui.story_information"))
                .id(egui::Id::new("story-info"))
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
                .id(egui::Id::new("glulx-error"))
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
        egui::Window::new(language.text("ui.about_glulx_player"))
            .id(egui::Id::new("about-player"))
            .open(&mut self.show_about)
            .collapsible(false)
            .resizable(false)
            .show(context, |ui| {
                ui.heading("Glulx Player 0.1.0");
                ui.label(
                    language
                        .text("A pure Rust Glulx VM with a cross-platform graphical interface."),
                );
                ui.label(language.text("ui.vm_behavior_is_based_on_the_glulx_specification"));
                ui.label(language.text("ui.the_interface_follows_the_windows_git_player_workflow"));
            });
    }
}

impl eframe::App for PlayerApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let _stage = crate::diagnostics::stage("session-save");
        crate::diagnostics::record(format_args!("session-save begin"));
        if self.settings_file.save(&self.settings) {
            storage.remove_string(STORAGE_KEY);
        } else if self.settings_file.path.is_none() {
            eframe::set_value(storage, STORAGE_KEY, &self.settings);
        }
        if let Some(vm) = &self.vm {
            let _ = vm.flush_streams();
        }
        if let Some(vm) = &self.vm
            && vm.state() != RunState::Halted
            && !self.desktop_snapshot_too_large()
        {
            let canvases = self
                .graphics
                .iter()
                .map(|(&window, canvas)| SavedCanvas {
                    window,
                    width: canvas.size[0],
                    height: canvas.size[1],
                    pixels: canvas.rasterize().into_raw(),
                    links: canvas
                        .link_regions()
                        .into_iter()
                        .map(|(clip, hyperlink)| SavedLinkRegion { clip, hyperlink })
                        .collect(),
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
            if self.desktop_snapshot_too_large() {
                crate::diagnostics::record(format_args!(
                    "session-save skipped: large snapshot; in-game saves remain available"
                ));
            }
            storage.set_string("glulx-session-v1", String::new());
        }
        crate::diagnostics::record(format_args!("session-save end"));
    }

    fn auto_save_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(30)
    }

    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        // Eframe also runs logic while the window is hidden. Keep timers,
        // sound notifications and translation results progressing there.
        self.run_vm();
        self.poll_translations();
        if context.input(|input| input.viewport().visible() == Some(false)) {
            self.poll_graphics(context);
            self.publish_story(context);
        }
        if self
            .vm
            .as_ref()
            .is_some_and(|vm| vm.state() == RunState::Running)
        {
            context.request_repaint();
        } else {
            context.request_repaint_after(idle_repaint_delay(self.vm.as_ref()));
        }
    }

    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let _stage = crate::diagnostics::stage("ui");
        let context = root.ctx().clone();
        if self.pending_font_metrics {
            // The new font definitions are active at this UI pass, after the
            // settings window queued them in the preceding pass.
            if let Some(vm) = &mut self.vm {
                vm.set_text_metrics(self.fonts.metrics.clone());
            }
            self.grid_galleys.clear();
            self.pending_font_metrics = false;
        }
        if context.input_mut(|input| {
            input.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::L)
        }) {
            self.settings.show_log_window = true;
        }
        if context.input_mut(|input| {
            input.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::T)
        }) {
            self.settings.show_translation_window = true;
        }
        if context.input(|input| {
            input.modifiers.ctrl && input.modifiers.alt && input.key_pressed(egui::Key::L)
        }) {
            self.settings.translation.enabled = !self.settings.translation.enabled;
            if self.settings.translation.enabled {
                self.settings.show_translation_window = true;
            }
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
        let paste_submit = self.replay_pending_paste();
        self.menu_bar(root);

        self.status_bar(root);
        self.story_view(root);
        if paste_submit {
            self.submit_input();
        }
        self.dialogs(&context);
        self.auxiliary_windows(&context);
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
    }
}

fn idle_repaint_delay(vm: Option<&Vm>) -> std::time::Duration {
    let idle = std::time::Duration::from_millis(100);
    let timer = vm
        .and_then(Vm::next_timer_delay)
        .map_or(idle, |delay| delay.min(idle));
    if vm.is_some_and(Vm::audio_needs_poll) {
        timer.min(std::time::Duration::from_millis(10))
    } else {
        timer
    }
}

fn same_window_metadata(
    view: &crate::vm::WindowView,
    descriptor: &crate::vm::WindowDescriptor,
) -> bool {
    view.kind == descriptor.kind
        && view.rect == descriptor.rect
        && view.grid_size == descriptor.grid_size
        && view.grid_cursor == descriptor.grid_cursor
        && view.appearance == descriptor.appearance
        && view.hints == descriptor.hints
}

/// Spend a short time budget advancing the story before presenting a frame.
fn run_vm_slice(vm: &mut Vm) -> Result<RunState, crate::VmError> {
    let started = std::time::Instant::now();
    let revision = vm.presentation_revision();
    loop {
        let state = vm.run_presentation_steps(1024)?;
        if state != RunState::Running
            || vm.presentation_revision() != revision
            || started.elapsed() >= std::time::Duration::from_millis(8)
        {
            return Ok(state);
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
    _context: &egui::Context,
    graphics: &'a mut BTreeMap<u32, DisplayedGraphics>,
    window: u32,
    size: [u32; 2],
    background: u32,
) -> &'a mut DisplayedGraphics {
    let canvas = graphics
        .entry(window)
        .or_insert_with(|| DisplayedGraphics::new(size, background));
    canvas.resize(size, background);
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
    fn gui_cli_memory_overrides_do_not_change_persisted_settings() {
        let context = egui::Context::default();
        let overrides = Overrides {
            game: Some(Budget::Fixed(2048)),
            process: Some(Budget::Percent { percent: 75 }),
            resources: [("--max-undo-memory".to_owned(), Budget::Fixed(8192))].into(),
        };
        let app = PlayerApp::new_with_memory_policy(
            &eframe::CreationContext::_new_kittest(context),
            None,
            ResourceSelection::Auto,
            overrides,
        );
        let effective = app.memory_policy();
        assert_eq!(effective.max_memory_mib, Budget::Fixed(2048));
        assert_eq!(effective.resource_limits.undo_mib, Budget::Fixed(8192));
        assert_eq!(
            app.settings.max_memory_mib,
            MemoryPolicy::default().max_memory_mib
        );
        assert_eq!(app.settings.max_process_memory_mib, Budget::Fixed(0));
        let saved: PlayerSettings =
            serde_json::from_slice(&serde_json::to_vec(&app.settings).unwrap()).unwrap();
        assert_eq!(
            saved.resource_limits.undo_mib,
            ResourceBudgets::default().undo_mib
        );
    }

    #[test]
    fn transcript_line_index_tracks_incremental_appends() {
        let context = egui::Context::default();
        let mut app = PlayerApp::new(&eframe::CreationContext::_new_kittest(context), None);
        app.append_transcript("first\nsecond");
        app.append_transcript("\nthird");
        let lines = app
            .transcript_lines
            .iter()
            .map(|range| app.transcript[range.clone()].to_owned())
            .collect::<Vec<_>>();
        assert_eq!(lines, ["first", "second", "third"]);
    }

    #[test]
    fn translation_views_append_until_clear_and_discard_unsubmitted_cleared_text() {
        use crate::vm::TextBufferEvent::{Clear, Text};
        let context = egui::Context::default();
        let mut app = PlayerApp::new(&eframe::CreationContext::_new_kittest(context), None);
        app.capture_translation_events(vec![Text {
            window: 1,
            text: "Tutorial.\n".into(),
        }]);
        app.finish_turn();
        app.capture_translation_events(vec![Text {
            window: 1,
            text: "Action result.\n".into(),
        }]);
        app.finish_turn();
        assert_eq!(
            app.turns.iter().map(|turn| turn.view).collect::<Vec<_>>(),
            [0, 0]
        );
        app.capture_translation_events(vec![
            Text {
                window: 1,
                text: "Obsolete unseen output".into(),
            },
            Text {
                window: 2,
                text: "Other window.\n".into(),
            },
            Clear { window: 1 },
            Text {
                window: 1,
                text: "Next screen.\n".into(),
            },
        ]);
        app.finish_turn();
        assert_eq!(app.turns[2].view, 1);
        assert_eq!(app.turns[2].original, "Other window.\nNext screen.\n");
        // An identical redraw is the same visible view, without extra history.
        app.capture_translation_events(vec![
            Clear { window: 1 },
            Text {
                window: 1,
                text: app.turns[2].original.clone(),
            },
        ]);
        app.finish_turn();
        assert_eq!(app.turns.len(), 3);
        app.capture_translation_events(vec![Text {
            window: 1,
            text: "More on this screen.\n".into(),
        }]);
        app.finish_turn();
        assert_eq!(app.turns[3].view, 1);
    }

    #[test]
    fn enabling_translation_does_not_backfill_history_or_buffered_old_text() {
        let context = egui::Context::default();
        let mut app = PlayerApp::new(&eframe::CreationContext::_new_kittest(context), None);
        // Invalid URL prevents any network access if the regression submits.
        app.settings.translation.endpoint.clear();
        app.turn_buffer = "An old paragraph.".into();
        app.finish_turn();
        assert_eq!(app.turns[0].translation, TurnTranslation::NotRequested);
        app.turn_buffer = "Old output before the next input wait.".into();
        app.settings.translation.enabled = true;
        app.poll_translations();
        assert!(
            app.pending_translations.is_empty(),
            "enabling must not submit historical paragraphs"
        );
        assert!(
            app.turn_buffer.is_empty(),
            "pre-enable output must not enter the next request"
        );
        assert_eq!(app.turns[0].translation, TurnTranslation::NotRequested);
        app.turn_buffer = "A new paragraph.".into();
        app.finish_turn();
        assert_eq!(app.turns[1].translation, TurnTranslation::Pending);
        assert_eq!(
            app.pending_translations
                .values()
                .copied()
                .collect::<Vec<_>>(),
            [1]
        );
    }

    #[test]
    fn toggling_capture_preserves_completed_and_pending_history() {
        let context = egui::Context::default();
        let mut app = PlayerApp::new(&eframe::CreationContext::_new_kittest(context), None);
        app.turns = vec![
            Turn {
                view: 0,
                original: "Earlier text".into(),
                translation: TurnTranslation::Complete(Ok("Earlier translation".into())),
            },
            Turn {
                view: 0,
                original: "Not requested".into(),
                translation: TurnTranslation::NotRequested,
            },
            Turn {
                view: 0,
                original: "Submitted text".into(),
                translation: TurnTranslation::Pending,
            },
        ];
        app.pending_translations.insert(7, 2);
        let history = app.turns.clone();
        for enabled in [true, false, true, false] {
            app.settings.translation.enabled = enabled;
            app.poll_translations();
            assert_eq!(app.turns, history);
            assert_eq!(app.pending_translations.get(&7), Some(&2));
        }
    }

    #[test]
    fn partial_story_output_stays_hidden_until_the_next_select_boundary() {
        fn glk(code: &mut Vec<u8>, selector: u16, args: &[u32]) {
            for arg in args.iter().rev() {
                code.extend([0x40, 0x83]);
                code.extend(arg.to_be_bytes());
            }
            code.extend([0x81, 0x30, 0x12, 0]);
            code.extend(selector.to_be_bytes());
            code.push(args.len() as u8);
        }
        let mut code = Vec::new();
        glk(&mut code, 0x23, &[0, 0, 0, 3, 0]);
        glk(&mut code, 0x2f, &[1]);
        code.extend([0x81, 0x49, 0x11, 2, 0]); // Glk output
        code.extend([0x70, 0x01, b'A']);
        glk(&mut code, 0xc0, &[0x110]);
        code.extend([0x70, 0x01, b'B']);
        glk(&mut code, 0xc0, &[0x110]);
        code.extend([0x81, 0x20]);
        let context = egui::Context::default();
        let mut app = PlayerApp::new(
            &eframe::CreationContext::_new_kittest(context.clone()),
            None,
        );
        app.vm = Some(
            Vm::new(Story::from_bytes(&crate::vm::tests::image_with_program(&code), None).unwrap())
                .unwrap(),
        );
        let text = |app: &PlayerApp| {
            app.presented_views
                .iter()
                .flat_map(|view| &view.runs)
                .map(|run| run.text.as_str())
                .collect::<String>()
        };
        app.vm
            .as_mut()
            .unwrap()
            .run_presentation_steps(200)
            .unwrap();
        app.publish_story(&context);
        assert_eq!(text(&app), "A");
        let vm = app.vm.as_mut().unwrap();
        vm.resize_windows(900, 600); // deliver Arrange, then produce part of the next frame
        assert_eq!(vm.run_steps(1).unwrap(), RunState::Running);
        assert!(
            vm.window_views()[0]
                .runs
                .iter()
                .any(|run| run.text.contains('B'))
        );
        app.publish_story(&context);
        assert_eq!(
            text(&app),
            "A",
            "an execution budget is not a display boundary"
        );
        app.vm
            .as_mut()
            .unwrap()
            .run_presentation_steps(200)
            .unwrap();
        app.publish_story(&context);
        assert_eq!(text(&app), "AB");
    }

    #[test]
    fn host_loop_executes_game_instructions() {
        let context = egui::Context::default();
        let mut app = PlayerApp::new(
            &eframe::CreationContext::_new_kittest(context.clone()),
            None,
        );
        let code = [0x81, 0x49, 0x11, 2, 0, 0x70, 1, b'Z', 0x81, 0x20];
        app.vm = Some(
            Vm::new(Story::from_bytes(&crate::vm::tests::image_with_program(&code), None).unwrap())
                .unwrap(),
        );
        let mut output = context.run_ui(Default::default(), |_root| app.run_vm());
        output.textures_delta.clear();
        assert_eq!(app.vm.as_ref().unwrap().state(), RunState::Halted);
        assert_eq!(app.transcript, "Z");
    }

    #[test]
    fn pasted_text_replays_into_a_following_line_request() {
        let program = [
            0x40, 0x80, 0x40, 0x81, 0x10, 0x40, 0x82, 0x01, 0x40, 0x40, 0x81, 0x01, 0x81, 0x30,
            0x12, 0x00, 0x00, 0xd0, 0x04, 0x40, 0x82, 0x01, 0x10, 0x81, 0x30, 0x12, 0x00, 0x00,
            0xc0, 0x01, 0x81, 0x20,
        ];
        let story =
            Story::from_bytes(&crate::vm::tests::image_with_program(&program), None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        assert_eq!(vm.open_window(&[0, 0, 0, 3, 0]), 1);
        assert_eq!(vm.run_steps(32).unwrap(), RunState::WaitingForLine);

        let context = egui::Context::default();
        let mut app = PlayerApp::new(&eframe::CreationContext::_new_kittest(context), None);
        app.vm = Some(vm);
        app.pending_keys
            .extend("next\n".chars().map(|character| character as u32));
        assert!(app.replay_pending_paste());
        assert_eq!(app.input, "next");
        app.submit_input();
        assert!(app.pending_keys.is_empty());
    }

    #[test]
    fn host_wakes_for_game_timer_without_waiting_one_hundred_ms() {
        let mut vm = Vm::new(
            Story::from_bytes(&crate::vm::tests::image_with_program(&[0x81, 0x20]), None).unwrap(),
        )
        .unwrap();
        vm.resume_timer(Some(10));
        assert!(idle_repaint_delay(Some(&vm)) <= std::time::Duration::from_millis(10));
        vm.resume_timer(Some(0));
        assert_eq!(
            idle_repaint_delay(Some(&vm)),
            std::time::Duration::from_millis(100)
        );
    }

    #[test]
    fn timed_vm_slice_yields_for_busy_stories_and_stops_at_halt() {
        let story = |program: &[u8]| {
            Story::from_bytes(&crate::vm::tests::image_with_program(program), None).unwrap()
        };
        let mut busy = Vm::new(story(&[0x81, 0x04, 0x02, 0, 0x43])).unwrap();
        assert_eq!(run_vm_slice(&mut busy).unwrap(), RunState::Running);
        let mut finished = Vm::new(story(&[0x81, 0x20])).unwrap();
        assert_eq!(run_vm_slice(&mut finished).unwrap(), RunState::Halted);
    }

    #[test]
    fn arrange_processing_keeps_input_and_story_geometry_stable() {
        for submitted in [false, true] {
            let program = [
                0x40, 0x80, 0x40, 0x81, 3, 0x40, 0x80, 0x40, 0x80, 0x40, 0x80, 0x81, 0x30, 0x11, 0,
                0x23, 5, // open text buffer
                0x40, 0x80, 0x40, 0x81, 16, 0x40, 0x82, 1, 0x40, 0x40, 0x81, 1, 0x81, 0x30, 0x12,
                0, 0, 0xd0, 4, // request line
                0x40, 0x82, 1, 0x10, 0x81, 0x30, 0x12, 0, 0, 0xc0, 1, // select
                0, 0, 0, 0, 0, // event handling spans multiple host frames
                0x81, 0x20,
            ];
            // Keep the fixture interactive when the VM handles Arrange in the
            // same host pass.
            let mut program = program.to_vec();
            let select = 0x43
                + program
                    .windows(4)
                    .position(|bytes| bytes == [0x40, 0x82, 1, 0x10])
                    .unwrap();
            program.truncate(program.len() - 2);
            program.extend([0x81, 0x04, 0x02]);
            program.extend((select as u16).to_be_bytes());
            let story =
                Story::from_bytes(&crate::vm::tests::image_with_program(&program), None).unwrap();
            let context = egui::Context::default();
            let creation = eframe::CreationContext::_new_kittest(context.clone());
            let mut app = PlayerApp::new(&creation, None);
            let mut vm = Vm::new(story).unwrap();
            // Drain the initial window-open Arrange, then stop at the line wait.
            for _ in 0..40 {
                if vm.run_steps(1).unwrap() == RunState::WaitingForLine {
                    break;
                }
            }
            assert_eq!(vm.state(), RunState::WaitingForLine);
            app.vm = Some(vm);
            let render = |app: &mut PlayerApp| {
                let mut output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1100.0, 760.0),
                        )),
                        ..Default::default()
                    },
                    |root| {
                        eframe::App::ui(app, root, &mut eframe::Frame::_new_kittest());
                    },
                );
                output.textures_delta.clear();
                app.presented_views[0].rect
            };
            let _ = render(&mut app);
            let waiting_rect = render(&mut app);
            if submitted && app.vm.as_ref().and_then(Vm::input_request).is_some() {
                app.vm.as_mut().unwrap().provide_input("look").unwrap();
            }
            for _ in 0..3 {
                assert_eq!(
                    render(&mut app),
                    waiting_rect,
                    "processing Arrange must not hide the input panel"
                );
            }
        }
    }

    #[test]
    fn large_blorb_does_not_serialize_a_blocking_desktop_snapshot() {
        use eframe::{App, Storage};
        #[derive(Default)]
        struct Store(BTreeMap<String, String>);
        impl Storage for Store {
            fn get_string(&self, key: &str) -> Option<String> {
                self.0.get(key).cloned()
            }
            fn set_string(&mut self, key: &str, value: String) {
                self.0.insert(key.into(), value);
            }
            fn remove_string(&mut self, key: &str) {
                self.0.remove(key);
            }
            fn flush(&mut self) {}
        }
        let image = crate::vm::tests::image_with_program(&[0x81, 0x20]);
        let resources = vec![0; 16 * 1024 * 1024];
        let blorb = crate::story::tests::resource_blorb(&[
            ((*b"GLUL", Some((*b"Exec", 0))), &image),
            ((*b"PNG ", Some((*b"Pict", 1))), &resources),
        ]);
        let context = egui::Context::default();
        let mut app = PlayerApp::new(&eframe::CreationContext::_new_kittest(context), None);
        app.vm = Some(Vm::new(Story::from_bytes(&blorb, None).unwrap()).unwrap());
        let mut store = Store::default();
        store.set_string("glulx-session-v1", "stale previous game".into());
        app.save(&mut store);
        assert!(
            store.get_string("glulx-session-v1").unwrap().is_empty(),
            "large snapshots must be skipped before serialization"
        );
        assert!(
            store.get_string(STORAGE_KEY).is_some(),
            "settings still persist"
        );
        assert_eq!(app.vm.as_ref().unwrap().state(), RunState::Running);
        // Small stories retain the existing self-contained desktop format.
        app.vm = Some(Vm::new(Story::from_bytes(&image, None).unwrap()).unwrap());
        app.save(&mut store);
        let session: Session = eframe::get_value(&store, "glulx-session-v1").unwrap();
        assert!(session.vm.validate_session().is_ok());
    }

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
        canvas.fill(&context, [0, 0, 1, 1], 0xabcdef);
        canvas.fill(&context, [3, 2, 1, 1], 0x334455);
        // Resize twice without an intervening draw; clipped pixels must not
        // reappear, and only the newly exposed region gets the new background.
        ensure_canvas(&context, &mut graphics, 1, [2, 1], 0x556677);
        let canvas = ensure_canvas(&context, &mut graphics, 1, [4, 3], 0x778899);
        let pixels = canvas.rasterize();
        assert_eq!(*pixels.get_pixel(0, 0), rgba(0xabcdef));
        assert_eq!(*pixels.get_pixel(1, 0), rgba(0x112233));
        assert_eq!(*pixels.get_pixel(2, 0), rgba(0x778899));
        assert_eq!(*pixels.get_pixel(3, 2), rgba(0x778899));
    }

    #[test]
    fn removed_graphics_are_dropped_from_the_presented_snapshot() {
        let context = egui::Context::default();
        let mut app = PlayerApp::new(
            &eframe::CreationContext::_new_kittest(context.clone()),
            None,
        );
        app.vm = Some(
            Vm::new(
                Story::from_bytes(&crate::vm::tests::image_with_program(&[0x81, 0x20]), None)
                    .unwrap(),
            )
            .unwrap(),
        );
        let canvas = ensure_canvas(&context, &mut app.graphics, 7, [4, 4], 0x112233).clone();
        app.presented_graphics
            .insert(7, std::sync::Arc::new(canvas));
        app.graphics.remove(&7);
        app.dirty_graphics.insert(7);
        app.presented_revision = u64::MAX;

        app.publish_story(&context);

        assert!(!app.presented_graphics.contains_key(&7));
    }
}

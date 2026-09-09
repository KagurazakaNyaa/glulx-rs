//! Native companion windows; their layout never changes the Glk viewport.
use super::*;

impl PlayerApp {
    pub(super) fn auxiliary_windows(&mut self, context: &egui::Context) {
        let language = self.settings.language.resolve();
        if self.settings.show_log_window {
            let mut open = true;
            context.show_viewport_immediate(
                egui::ViewportId::from_hash_of("player-log-input"),
                egui::ViewportBuilder::default()
                    .with_title(language.text("ui.glulx_player_log_and_input"))
                    .with_active(false)
                    .with_inner_size([680.0, 520.0])
                    .with_min_inner_size([360.0, 200.0]),
                |root, _| {
                    if root.input(|input| input.viewport().close_requested()) {
                        open = false;
                        return;
                    }
                    let context = root.ctx().clone();
                    self.character_input(&context);
                    self.input_bar(root);
                    egui::CentralPanel::default().show(root, |ui| {
                        ui.push_id("player-log-controls", |ui| {
                            egui::ScrollArea::vertical()
                                .stick_to_bottom(true)
                                .show(ui, |ui| {
                                    ui.add(egui::Label::new(&self.transcript).selectable(true));
                                });
                        });
                    });
                    if self
                        .vm
                        .as_ref()
                        .is_some_and(|vm| vm.state() == RunState::Running)
                    {
                        context.request_repaint_of(egui::ViewportId::ROOT);
                    }
                },
            );
            self.settings.show_log_window = open;
        }
        if self.settings.show_translation_window {
            let mut open = true;
            context.show_viewport_immediate(
                egui::ViewportId::from_hash_of("player-translation"),
                egui::ViewportBuilder::default()
                    .with_title(language.text("ui.glulx_player_translation"))
                    .with_active(false)
                    .with_inner_size([440.0, 520.0])
                    .with_min_inner_size([280.0, 180.0]),
                |root, _| {
                    if root.input(|input| input.viewport().close_requested()) {
                        open = false;
                        return;
                    }
                    egui::CentralPanel::default().show(root, |ui| {
                        ui.push_id("player-translation-controls", |ui| {
                            ui.checkbox(
                                &mut self.settings.translation.enabled,
                                language.text("ui.translate_new_text"),
                            );
                            if ui
                                .button(language.text("ui.translation_settings"))
                                .clicked()
                            {
                                self.show_translation_settings = true;
                                context.send_viewport_cmd_to(
                                    egui::ViewportId::from_hash_of("player-translation-settings"),
                                    egui::ViewportCommand::Focus,
                                );
                            }
                            ui.separator();
                            egui::ScrollArea::vertical()
                                .stick_to_bottom(true)
                                .show(ui, |ui| {
                                    if !self.settings.translation.enabled {
                                        ui.weak(language.text("ui.translation_is_off"));
                                    }
                                    translation_turns(
                                        ui,
                                        &self.turns,
                                        self.settings.font_size,
                                        language,
                                    );
                                });
                        });
                    });
                },
            );
            self.settings.show_translation_window = open;
        }
        if self.show_options {
            let mut open = true;
            context.show_viewport_immediate(
                egui::ViewportId::from_hash_of("player-settings"),
                egui::ViewportBuilder::default()
                    .with_title(language.text("ui.glulx_player_settings"))
                    .with_inner_size([540.0, 660.0])
                    .with_min_inner_size([380.0, 280.0]),
                |root, _| {
                    if root.input(|input| input.viewport().close_requested()) {
                        open = false;
                        return;
                    }
                    egui::CentralPanel::default().show(root, |ui| {
                        ui.push_id("player-settings-controls", |ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| self.settings_contents(ui));
                        });
                    });
                },
            );
            self.show_options = open;
            if !open {
                self.settings_file.save(&self.settings);
            }
        }
        if self.show_translation_settings {
            let mut open = true;
            context.show_viewport_immediate(
                egui::ViewportId::from_hash_of("player-translation-settings"),
                egui::ViewportBuilder::default()
                    .with_title(language.text("ui.glulx_player_translation_settings"))
                    .with_inner_size([540.0, 660.0])
                    .with_min_inner_size([380.0, 280.0]),
                |root, _| {
                    if root.input(|input| input.viewport().close_requested()) {
                        open = false;
                        return;
                    }
                    egui::CentralPanel::default().show(root, |ui| {
                        ui.push_id("player-translation-settings-controls", |ui| {
                            egui::ScrollArea::vertical()
                                .show(ui, |ui| self.translation_settings_contents(ui));
                        });
                    });
                },
            );
            self.show_translation_settings = open;
            if !open {
                self.settings_file.save(&self.settings);
            }
        }
    }
    fn settings_contents(&mut self, ui: &mut egui::Ui) {
        let before = self.settings.language;
        let language = before.resolve();
        ui.label(language.text("ui.interface_language"));
        egui::ComboBox::from_id_salt("interface-language")
            .selected_text(match before {
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
            })
            .response
            .on_hover_text(language.text("ui.interface_language"));
        if before != self.settings.language {
            self.settings_file.save(&self.settings);
            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
        }
        let language = self.settings.language.resolve();
        let context = ui.ctx().clone();
        self.settings_save_controls(ui);
        ui.separator();
        let appearance = (
            self.settings.font_size,
            self.settings.text_color,
            self.settings.background_color,
        );
        ui.heading(language.text("ui.display"));
        ui.add(
            egui::Slider::new(&mut self.settings.font_size, 12.0..=32.0)
                .text(language.text("ui.text_size")),
        );
        ui.horizontal(|ui| {
            if ui.button(language.text("ui.choose_system_font")).clicked() {
                match super::font_dialog::choose_family(&self.settings.system_font, language) {
                    Ok(Some(family)) => {
                        self.settings.system_font = family;
                        self.settings.fallback_font.clear();
                        self.apply_selected_font(&context);
                    }
                    Ok(None) => {}
                    Err(error) => self.fonts.error = Some(error),
                }
            }
            if ui.button(language.text("ui.choose_font_file")).clicked() {
                match super::font_dialog::choose_file(language) {
                    Ok(Some(path)) => {
                        self.settings.fallback_font = path.display().to_string();
                        self.settings.system_font.clear();
                        self.apply_selected_font(&context);
                    }
                    Ok(None) => {}
                    Err(error) => self.fonts.error = Some(error),
                }
            }
        });
        if !self.settings.system_font.is_empty() {
            ui.label(language.format("ui.system_font", &[&self.settings.system_font]));
        }
        ui.label(language.text("ui.font_file_ttf_otf_or_ttc"));
        ui.text_edit_singleline(&mut self.settings.fallback_font);
        if ui
            .add_enabled(
                !self.settings.fallback_font.trim().is_empty(),
                egui::Button::new(language.text("ui.apply_font_file")),
            )
            .clicked()
        {
            self.settings.system_font.clear();
            self.apply_selected_font(&context);
        }
        if ui.button(language.text("ui.use_default_fonts")).clicked() {
            self.settings.system_font.clear();
            self.settings.fallback_font.clear();
            self.apply_selected_font(&context);
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
        if appearance
            != (
                self.settings.font_size,
                self.settings.text_color,
                self.settings.background_color,
            )
        {
            // Refresh an idle presentation after the next logic pass applies
            // host colors/fonts. A running story still waits for its boundary.
            self.presented_state = RunState::Running;
        }
    }

    fn settings_save_controls(&mut self, ui: &mut egui::Ui) {
        let language = self.settings.language.resolve();
        if let Some(path) = &self.settings_file.path {
            ui.add(
                egui::Label::new(language.format("ui.settings_file", &[&path.display()]))
                    .truncate(),
            )
            .on_hover_text(path.display().to_string());
        }
        if ui.button(language.text("ui.save_settings")).clicked() {
            self.settings_file.save(&self.settings);
        }
        if let Some(error) = &self.settings_file.error {
            ui.colored_label(Color32::from_rgb(170, 50, 45), language.message(error));
        }
    }

    fn translation_settings_contents(&mut self, ui: &mut egui::Ui) {
        let language = self.settings.language.resolve();
        self.settings_save_controls(ui);
        ui.separator();
        ui.heading(language.text("ui.translation"));
        ui.checkbox(
            &mut self.settings.translation.enabled,
            language.text("ui.translate_new_text"),
        );
        ui.label(language.text("ui.openai_compatible_endpoint"));
        ui.text_edit_singleline(&mut self.settings.translation.endpoint);
        ui.label(language.text("ui.model"));
        ui.text_edit_singleline(&mut self.settings.translation.model);
        ui.label(language.text("ui.target_language"));
        ui.text_edit_singleline(&mut self.settings.translation.target_language);
        ui.label(language.text("ui.api_key_kept_in_local_app_settings"));
        ui.add(egui::TextEdit::singleline(&mut self.settings.translation.api_key).password(true));
        ui.checkbox(
            &mut self.settings.translation.use_system_prompt,
            language.text("ui.use_system_prompt"),
        );
        ui.add_enabled_ui(self.settings.translation.use_system_prompt, |ui| {
            ui.label(language.text("ui.system_prompt"));
            ui.add(
                egui::TextEdit::multiline(&mut self.settings.translation.system_prompt)
                    .desired_rows(4),
            );
        });
        ui.label(language.text("ui.user_prompt"));
        ui.add(
            egui::TextEdit::multiline(&mut self.settings.translation.user_prompt).desired_rows(4),
        );
        ui.weak(language.format("ui.prompt_template_help", &[&"{target}", &"{text}"]));
        ui.label(language.text("ui.sampling_parameters"));
        ui.weak(language.text("ui.sampling_parameters_help"));
        let settings = &mut self.settings.translation;
        optional_translation_parameter(
            ui,
            language.text("ui.temperature"),
            &mut settings.temperature,
            0.7,
            0.0..=2.0,
        );
        optional_translation_parameter(ui, "top_p", &mut settings.top_p, 0.6, 0.0..=1.0);
        optional_translation_parameter(ui, "top_k", &mut settings.top_k, 20, 0..=u32::MAX);
        optional_translation_parameter(
            ui,
            "repetition_penalty",
            &mut settings.repetition_penalty,
            1.05,
            0.01..=100.0,
        );
        optional_translation_parameter(
            ui,
            language.text("ui.max_tokens"),
            &mut settings.max_tokens,
            4096,
            1..=u32::MAX,
        );
    }

    fn apply_selected_font(&mut self, context: &egui::Context) {
        self.fonts = fonts::Fonts::new(
            context,
            &self.settings.fallback_font,
            &self.settings.system_font,
        );
        self.pending_font_metrics = true;
        self.presented_state = RunState::Running;
        self.settings_file.save(&self.settings);
    }
}

fn translation_turns(ui: &mut egui::Ui, turns: &[Turn], font_size: f32, language: Language) {
    let Some(current) = turns.last() else {
        return;
    };
    let current_start = turns
        .iter()
        .rposition(|turn| turn.view != current.view)
        .map_or(0, |index| index + 1);
    let (history, current) = turns.split_at(current_start);
    if !history.is_empty() {
        let groups = history.chunk_by(|left, right| left.view == right.view);
        let count = groups.clone().count();
        let history = egui::CollapsingHeader::new(language.format("ui.history", &[&count]))
            .id_salt("translation-history")
            .default_open(false)
            .show(ui, |ui| {
                for group in groups.rev() {
                    translation_view(ui, group, font_size, language);
                    ui.separator();
                }
            });
        if history.header_response.clicked() {
            // Expanding adds content above the current view. Release the log's
            // bottom sticking so the opened history remains in view.
            history.header_response.scroll_to_me(Some(egui::Align::Min));
        }
        ui.add_space(8.0);
    }
    translation_view(ui, current, font_size, language);
}

fn translation_view(ui: &mut egui::Ui, turns: &[Turn], font_size: f32, language: Language) {
    ui.push_id(("translation-view", turns[0].view), |ui| {
        for (index, turn) in turns.iter().enumerate() {
            ui.push_id(index, |ui| {
                ui.label(RichText::new(turn.original.trim()).weak().size(font_size));
                match &turn.translation {
                    TurnTranslation::Complete(Ok(value)) => {
                        ui.label(RichText::new(value).size(font_size));
                    }
                    TurnTranslation::Complete(Err(error)) => {
                        ui.colored_label(Color32::from_rgb(170, 50, 45), language.message(error));
                    }
                    TurnTranslation::Pending => {
                        ui.weak(language.text("ui.translating"));
                    }
                    TurnTranslation::NotRequested => {
                        ui.weak(language.text("ui.not_translated"));
                    }
                }
                ui.add_space(14.0);
            });
        }
    });
}

fn optional_translation_parameter<T: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut Option<T>,
    initial: T,
    range: std::ops::RangeInclusive<T>,
) {
    ui.horizontal(|ui| {
        let mut enabled = value.is_some();
        if ui.checkbox(&mut enabled, label).changed() {
            *value = enabled.then_some(initial);
        }
        if let Some(value) = value {
            ui.add(
                egui::DragValue::new(value)
                    .range(range)
                    .speed(if T::INTEGRAL { 1.0 } else { 0.01 }),
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_language_updates_settings_labels_without_changing_translation_target() {
        let context = egui::Context::default();
        let mut app = PlayerApp::new(
            &eframe::CreationContext::_new_kittest(context.clone()),
            None,
        );
        let target = app.settings.translation.target_language.clone();
        for (preference, expected, absent) in [
            (
                LanguagePreference::English,
                "Interface language",
                "界面语言",
            ),
            (
                LanguagePreference::Chinese,
                "界面语言",
                "Interface language",
            ),
            (
                LanguagePreference::English,
                "Interface language",
                "界面语言",
            ),
        ] {
            app.settings.language = preference;
            let output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 1600.0),
                    )),
                    ..Default::default()
                },
                |root| {
                    egui::CentralPanel::default().show(root, |ui| app.settings_contents(ui));
                },
            );
            fn collect(shape: &egui::epaint::Shape, text: &mut Vec<String>) {
                match shape {
                    egui::epaint::Shape::Vec(shapes) => {
                        shapes.iter().for_each(|s| collect(s, text))
                    }
                    egui::epaint::Shape::Text(shape) => text.push(shape.galley.job.text.clone()),
                    _ => {}
                }
            }
            let mut labels = Vec::new();
            for shape in &output.shapes {
                collect(&shape.shape, &mut labels);
            }
            output.drop_without_applying_deltas();
            assert!(labels.iter().any(|label| label == expected), "{labels:?}");
            assert!(!labels.iter().any(|label| label == absent));
            assert_eq!(app.settings.translation.target_language, target);
        }
    }

    fn frame(
        context: &egui::Context,
        turns: &[Turn],
        events: Vec<egui::Event>,
    ) -> Vec<(String, egui::Rect)> {
        frame_with_scroll(context, turns, events, false).0
    }

    fn frame_with_scroll(
        context: &egui::Context,
        turns: &[Turn],
        events: Vec<egui::Event>,
        scroll: bool,
    ) -> (Vec<(String, egui::Rect)>, f32) {
        let mut offset = 0.0;
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    if scroll {
                        egui::vec2(440.0, 300.0)
                    } else {
                        egui::vec2(800.0, 1600.0)
                    },
                )),
                events,
                ..Default::default()
            },
            |root| {
                egui::CentralPanel::default().show(root, |ui| {
                    if scroll {
                        offset = egui::ScrollArea::vertical()
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                translation_turns(ui, turns, 18.0, Language::English);
                            })
                            .state
                            .offset
                            .y;
                    } else {
                        translation_turns(ui, turns, 18.0, Language::English);
                    }
                });
            },
        );
        fn collect_text(
            shape: &egui::epaint::Shape,
            clip: egui::Rect,
            texts: &mut Vec<(String, egui::Rect)>,
        ) {
            match shape {
                egui::epaint::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect_text(shape, clip, texts);
                    }
                }
                egui::epaint::Shape::Text(text) => {
                    let rect = text.galley.rect.translate(text.pos.to_vec2());
                    if clip.intersects(rect) {
                        texts.push((text.galley.job.text.clone(), rect));
                    }
                }
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for shape in &output.shapes {
            collect_text(&shape.shape, shape.clip_rect, &mut texts);
        }
        output.drop_without_applying_deltas();
        (texts, offset)
    }

    fn click_history(
        context: &egui::Context,
        turns: &[Turn],
        texts: &[(String, egui::Rect)],
    ) -> Vec<(String, egui::Rect)> {
        let position = texts
            .iter()
            .find(|(text, _)| text.starts_with("History ("))
            .expect("history toggle")
            .1
            .center();
        let pointer = |pressed| egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            context,
            turns,
            vec![egui::Event::PointerMoved(position), pointer(true)],
        );
        frame(context, turns, vec![pointer(false)]);
        frame(context, turns, Vec::new())
    }

    fn test_context() -> egui::Context {
        let context = egui::Context::default();
        context.options_mut(|options| options.warn_on_id_clash = true);
        context.global_style_mut(|style| style.animation_time = 0.0);
        context
    }

    #[test]
    fn translation_turns_have_no_widget_id_collisions() {
        let context = test_context();
        let turns: Vec<_> = (0..12)
            .map(|index| Turn {
                view: index / 3,
                original: format!("Original turn {index}"),
                translation: TurnTranslation::Complete(Ok(format!("Translation {index}"))),
            })
            .collect();
        let collapsed = frame(&context, &turns, Vec::new());
        let expanded = click_history(&context, &turns, &collapsed);
        for texts in [&collapsed, &expanded] {
            assert!(
                texts
                    .iter()
                    .all(|(text, _)| !text.contains("use of widget")),
                "history groups and current paragraphs must have independent widget IDs"
            );
        }
        assert!(expanded.iter().any(|(text, _)| text == "Original turn 0"));
        assert!(expanded.iter().any(|(text, _)| text == "Translation 11"));
    }

    #[test]
    fn history_is_collapsed_and_current_originals_and_translations_are_visible() {
        let context = test_context();
        let turns: Vec<_> = (0..5)
            .map(|index| Turn {
                view: index / 2,
                original: format!("Original turn {index}"),
                translation: TurnTranslation::Complete(Ok(format!("Translation {index}"))),
            })
            .collect();
        let collapsed = frame(&context, &turns, Vec::new());
        assert!(collapsed.iter().any(|(text, _)| text == "History (2)"));
        for index in 0..4 {
            assert!(!collapsed.iter().any(|(text, _)| {
                text == &format!("Original turn {index}") || text == &format!("Translation {index}")
            }));
        }
        assert!(collapsed.iter().any(|(text, _)| text == "Original turn 4"));
        assert!(collapsed.iter().any(|(text, _)| text == "Translation 4"));

        let expanded = click_history(&context, &turns, &collapsed);
        for index in 0..5 {
            assert!(
                expanded
                    .iter()
                    .any(|(text, _)| text == &format!("Original turn {index}"))
            );
            assert!(
                expanded
                    .iter()
                    .any(|(text, _)| text == &format!("Translation {index}"))
            );
        }
        assert!(!expanded.iter().any(|(text, _)| text == "Original"));
        assert_eq!(
            expanded
                .iter()
                .filter(|(text, _)| text.starts_with("History ("))
                .count(),
            1,
            "one history toggle reveals every archived view without nested toggles"
        );
        let collapsed_again = click_history(&context, &turns, &expanded);
        assert!(
            !collapsed_again
                .iter()
                .any(|(text, _)| text == "Original turn 0")
        );
        assert!(
            collapsed_again
                .iter()
                .any(|(text, _)| text == "Original turn 4")
        );
        assert!(
            collapsed_again
                .iter()
                .any(|(text, _)| text == "Translation 4")
        );
    }

    #[test]
    fn opening_history_keeps_it_visible_in_the_scroll_area() {
        let context = test_context();
        let turns: Vec<_> = (0..21)
            .map(|index| Turn {
                view: index,
                original: format!("Original turn {index}"),
                translation: TurnTranslation::Complete(Ok(format!("Translation {index}"))),
            })
            .collect();
        let (collapsed, offset) = frame_with_scroll(&context, &turns, Vec::new(), true);
        assert_eq!(offset, 0.0);
        assert!(collapsed.iter().any(|(text, _)| text == "Translation 20"));
        let position = collapsed
            .iter()
            .find(|(text, _)| text == "History (20)")
            .expect("visible history toggle")
            .1
            .center();
        let pointer = |pressed| egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame_with_scroll(
            &context,
            &turns,
            vec![egui::Event::PointerMoved(position), pointer(true)],
            true,
        );
        frame_with_scroll(&context, &turns, vec![pointer(false)], true);
        // Let subsequent scroll/layout passes settle after the content grows.
        for _ in 0..12 {
            let (expanded, offset) = frame_with_scroll(&context, &turns, Vec::new(), true);
            assert!(offset < 1.0, "opening history must release bottom sticking");
            assert!(expanded.iter().any(|(text, _)| text == "History (20)"));
            assert!(expanded.iter().any(|(text, _)| text == "Original turn 19"));
            assert!(expanded.iter().any(|(text, _)| text == "Translation 19"));
            assert!(!expanded.iter().any(|(text, _)| text == "Translation 20"));
        }
    }
}

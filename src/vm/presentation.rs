use super::*;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
pub struct TextRun {
    pub text: String,
    pub style: u32,
    pub hyperlink: u32,
    #[serde(default)]
    pub image: Option<BufferImage>,
    #[serde(default)]
    pub flow_break: bool,
}

/// Retain scaling rules so text-buffer pictures reflow when the window resizes.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
pub struct BufferImage {
    pub resource: u32,
    pub original: [u32; 2],
    pub alignment: u32,
    pub size: [u32; 2],
    pub rule: u32,
    pub max_width: u32,
}

impl BufferImage {
    pub fn dimensions(&self, window_width: u32) -> [u32; 2] {
        let width = match self.rule & 3 {
            1 => self.original[0] as u128,
            2 => self.size[0] as u128,
            _ => window_width as u128 * self.size[0] as u128 / 65536,
        };
        let height = match self.rule & 12 {
            4 => self.original[1] as u128,
            8 => self.size[1] as u128,
            _ => {
                width * self.original[1] as u128 * self.size[1] as u128
                    / (self.original[0].max(1) as u128 * 65536)
            }
        };
        let limit = window_width as u128 * self.max_width as u128 / 65536;
        let (width, height) = if self.max_width != 0 && width > limit {
            (limit, height.saturating_mul(limit) / width)
        } else {
            (width, height)
        };
        [
            width.min(u32::MAX as u128) as u32,
            height.min(u32::MAX as u128) as u32,
        ]
    }
}
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct TextAppearance {
    pub font_size: f32,
    pub foreground: u32,
    pub background: u32,
    #[serde(skip)]
    pub light_fonts: [bool; 2],
}
impl Default for TextAppearance {
    fn default() -> Self {
        Self {
            font_size: 18.0,
            foreground: 0x202225,
            background: 0xf8f8f6,
            light_fonts: [false; 2],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedStyle {
    pub font_size: f32,
    pub weight: i32,
    pub oblique: bool,
    pub proportional: bool,
    pub foreground: u32,
    pub background: u32,
    pub reverse: bool,
    pub indentation: i32,
    pub paragraph_indentation: i32,
    pub justification: u32,
}

impl ResolvedStyle {
    pub(super) fn resolve(
        kind: u32,
        style: u32,
        hints: &BTreeMap<(u32, u32), u32>,
        appearance: TextAppearance,
    ) -> Self {
        let hint = |id| hints.get(&(style, id)).copied();
        let grid = kind == WINTYPE_TEXT_GRID;
        let proportional = !grid && hint(6).map_or(style != 2, |v| v != 0);
        let reverse = hint(9) == Some(1);
        let mut foreground = hint(7).unwrap_or(appearance.foreground) & 0xffffff;
        let mut background = hint(8).unwrap_or(appearance.background) & 0xffffff;
        if reverse {
            std::mem::swap(&mut foreground, &mut background);
        }
        Self {
            font_size: if grid {
                13.0
            } else {
                (appearance.font_size + hint(3).unwrap_or(0) as i32 as f32 * 2.0).clamp(8.0, 64.0)
            },
            weight: hint(4).map_or(i32::from(matches!(style, 3 | 4 | 5 | 8)), |v| {
                if v as i32 == -1 && appearance.light_fonts[usize::from(!proportional)] {
                    -1
                } else {
                    i32::from(v as i32 > 0)
                }
            }),
            // Glk defaults: Emphasized and Note are italic; Alert is the
            // bold-italic face. An explicit oblique hint still overrides the
            // style default below.
            oblique: hint(5).map_or(matches!(style, 1 | 5 | 6), |v| v != 0),
            proportional,
            foreground,
            background,
            reverse,
            indentation: if grid {
                0
            } else {
                (hint(0).unwrap_or(0) as i32).saturating_mul(8)
            },
            paragraph_indentation: if grid {
                0
            } else {
                (hint(1).unwrap_or(0) as i32).saturating_mul(8)
            },
            justification: if grid {
                0
            } else {
                hint(2).filter(|v| *v <= 3).unwrap_or(0)
            },
        }
    }

    fn measure(self, hint: u32) -> Option<u32> {
        Some(match hint {
            0 => self.indentation as u32,
            1 => self.paragraph_indentation as u32,
            2 => self.justification,
            3 => self.font_size as u32,
            4 => self.weight as u32,
            5 => u32::from(self.oblique),
            6 => u32::from(self.proportional),
            7 => self.foreground,
            8 => self.background,
            9 => u32::from(self.reverse),
            _ => return None,
        })
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
pub struct WindowView {
    pub id: u32,
    pub kind: u32,
    pub rect: [u32; 4],
    pub runs: Vec<TextRun>,
    pub grid: String,
    pub grid_cells: Vec<GridCell>,
    pub grid_size: [u32; 2],
    pub grid_cursor: [u32; 2],
    pub appearance: TextAppearance,
    pub hints: BTreeMap<(u32, u32), u32>,
}
impl WindowView {
    pub fn style(&self, style: u32) -> ResolvedStyle {
        ResolvedStyle::resolve(self.kind, style, &self.hints, self.appearance)
    }
}

impl Vm {
    pub fn set_light_fonts(&mut self, available: [bool; 2]) {
        if self.text_appearance.light_fonts != available {
            self.text_appearance.light_fonts = available;
            self.presentation_revision = self.presentation_revision.wrapping_add(1);
            self.layout_windows();
            if self.glk_root != 0 && !self.events.iter().any(|event| event[0] == 5) {
                self.events.push_back([5, 0, 0, 0]);
            }
        }
    }

    pub fn set_text_appearance(&mut self, font_size: f32, foreground: u32, background: u32) {
        let font_size = if font_size.is_finite() {
            font_size.clamp(8.0, 64.0)
        } else {
            18.0
        };
        let foreground = foreground & 0xffffff;
        let background = background & 0xffffff;
        let resized = self.text_appearance.font_size != font_size;
        let appearance_changed = resized
            || self.text_appearance.foreground != foreground
            || self.text_appearance.background != background;
        if appearance_changed {
            self.presentation_revision = self.presentation_revision.wrapping_add(1);
        }
        if resized && self.glk_root != 0 && !self.events.iter().any(|e| e[0] == 5) {
            self.events.push_back([5, 0, 0, 0]);
        }
        self.text_appearance = TextAppearance {
            font_size,
            foreground,
            background,
            ..self.text_appearance
        };
        if resized {
            self.layout_windows();
        }
    }

    pub fn image_resource(&self, resource: u32) -> Option<&[u8]> {
        self.story.resource(*b"Pict", resource)
    }

    pub fn resource_descriptions(&self) -> Vec<crate::story::ResourceDescription> {
        self.story.resource_descriptions()
    }

    pub(super) fn picture_dimensions(&mut self, resource: u32) -> Option<[u32; 2]> {
        *self.image_info.entry(resource).or_insert_with(|| {
            let data = self.story.resource(*b"Pict", resource)?;
            let decoded = std::sync::Arc::new(
                crate::picture::decode_with_limit(
                    data,
                    crate::memory::ResourceLimits::bytes(self.resource_limits.decoded_image_mib)
                        as u64,
                )
                .ok()?,
            );
            let size = [decoded.width(), decoded.height()];
            // Keep only the most recent validation result, not every image
            // queried by a game that scans its resource catalog at startup.
            self.decoded_picture = Some((resource, decoded));
            Some(size)
        })
    }

    pub(super) fn draw_image(&mut self, selector: u32, args: &[u32]) -> u32 {
        if !self.graphical_host {
            return 0;
        }
        let arg = |n: usize| args.get(n).copied().unwrap_or(0);
        let (window_id, resource) = (arg(0), arg(1));
        let Some(original) = self.picture_dimensions(resource) else {
            return 0;
        };
        let Some(window) = self
            .glk_windows
            .get(&window_id)
            .filter(|w| matches!(w.kind, WINTYPE_TEXT_BUFFER | WINTYPE_GRAPHICS))
        else {
            return 0;
        };
        let Some(data) = self.story.resource(*b"Pict", resource) else {
            return 0;
        };
        let rule = match selector {
            0xe1 => 1 | 4,
            0xe2 => 2 | 8,
            _ => arg(6),
        };
        if rule & !15 != 0 || rule & 3 == 0 || rule & 12 == 0 {
            return 0;
        }
        let buffer = window.kind == WINTYPE_TEXT_BUFFER;
        let image = BufferImage {
            resource,
            original,
            alignment: arg(2),
            size: [arg(4), arg(5)],
            rule,
            max_width: if !buffer {
                0
            } else if selector == 0xec {
                arg(7)
            } else {
                65536
            },
        };
        if buffer {
            // A zero scaling operand is permanently invisible. Do not turn it
            // into stream content (which would prevent a later margin image).
            // Sizes rounded to zero only at the current window width still
            // retain their rules, so they can reappear after a resize.
            if (rule & 3 != 1 && image.size[0] == 0) || (rule & 12 != 4 && image.size[1] == 0) {
                return 1;
            }
            if !(1..=5).contains(&image.alignment)
                || (image.alignment >= 4 && !window.at_buffer_line_start())
            {
                return 0;
            }
            let window = self.glk_windows.get_mut(&window_id).unwrap();
            window.runs.push(TextRun {
                text: String::new(),
                style: window.style,
                hyperlink: window.hyperlink,
                image: Some(image),
                flow_break: false,
            });
        } else {
            let size = image.dimensions(window.width);
            if size[0] != 0 && size[1] != 0 {
                self.graphics.push(GraphicsRequest::Draw(ImageRequest {
                    decoded: self
                        .decoded_picture
                        .as_ref()
                        .filter(|(id, _)| *id == resource)
                        .map(|(_, decoded)| decoded.clone()),
                    window: window_id,
                    resource,
                    data: data.to_vec(),
                    position: [arg(2) as i32, arg(3) as i32],
                    requested_size: Some(size),
                    canvas_size: [window.width, window.height],
                }));
            }
        }
        1
    }

    pub(super) fn flow_break(&mut self, window: u32) {
        if let Some(window) = self
            .glk_windows
            .get_mut(&window)
            .filter(|w| w.kind == WINTYPE_TEXT_BUFFER)
        {
            window.runs.push(TextRun {
                text: String::new(),
                style: window.style,
                hyperlink: window.hyperlink,
                image: None,
                flow_break: true,
            });
        }
    }

    pub fn set_graphical_host(&mut self, enabled: bool) {
        self.graphical_host = enabled;
        self.terminal_host = false;
    }

    /// Enable text windows and direct keyboard input for an interactive TTY.
    /// Graphics, mouse, hyperlinks, audio and font measurements stay disabled.
    pub fn set_terminal_host(&mut self, enabled: bool) {
        self.graphical_host = false;
        self.terminal_host = enabled;
    }

    pub fn window_views(&self) -> Vec<WindowView> {
        self.glk_windows
            .iter()
            .filter(|(_, w)| w.kind != 1)
            .map(|(&id, w)| WindowView {
                id,
                kind: w.kind,
                rect: w.rect,
                runs: w.runs.clone(),
                grid_cells: w.grid_cells(),
                grid_size: [w.width, w.height],
                grid_cursor: [w.cursor_x, w.cursor_y],
                appearance: self.text_appearance,
                grid: w
                    .grid
                    .chunks(w.width.max(1) as usize)
                    .map(|row| row.iter().collect::<String>())
                    .collect::<Vec<_>>()
                    .join("\n"),
                hints: w.hints.clone(),
            })
            .collect()
    }
    pub(super) fn style_call(&mut self, selector: u32, args: &[u32]) -> Result<u32, VmError> {
        let arg = |n: usize| args.get(n).copied().unwrap_or(0);
        Ok(match selector {
            0x86 | 0x87 | 0x100 | 0x101 => {
                let explicit = matches!(selector, 0x87 | 0x101);
                let stream = if explicit {
                    arg(0)
                } else {
                    self.glk_current_stream
                };
                let value = arg(usize::from(explicit));
                let mut stream = stream;
                let mut seen = HashSet::new();
                while seen.insert(stream) {
                    let Some(GlkStream {
                        target: GlkStreamTarget::Window(id),
                        ..
                    }) = self.glk_streams.get(&stream)
                    else {
                        break;
                    };
                    let Some(window) = self.glk_windows.get_mut(id) else {
                        break;
                    };
                    if selector >= 0x100 {
                        window.hyperlink = value;
                        break; // Hyperlink state is window-specific, not a style command.
                    }
                    window.style = if value <= 10 { value } else { 0 };
                    stream = window.echo_stream;
                    if stream == 0 {
                        break;
                    }
                }
                0
            }
            0xb0 | 0xb1 => {
                if arg(1) <= 10 && arg(2) <= 9 {
                    for kind in [3, 4] {
                        if (arg(0) == 0 || arg(0) == kind)
                            && (kind == 3 || matches!(arg(2), 4 | 5 | 7..=9))
                        {
                            if selector == 0xb0 {
                                self.style_hints.insert((kind, arg(1), arg(2)), arg(3));
                            } else {
                                self.style_hints.remove(&(kind, arg(1), arg(2)));
                            }
                        }
                    }
                }
                0
            }
            0xb2 => self.glk_windows.get(&arg(0)).map_or(0, |w| {
                if !self.graphical_host || !matches!(w.kind, 3 | 4) || arg(1) > 10 || arg(2) > 10 {
                    return 0;
                }
                let one = ResolvedStyle::resolve(w.kind, arg(1), &w.hints, self.text_appearance);
                let two = ResolvedStyle::resolve(w.kind, arg(2), &w.hints, self.text_appearance);
                u32::from(one != two)
            }),
            0xb3 => {
                let value = self
                    .glk_windows
                    .get(&arg(0))
                    .filter(|w| self.graphical_host && matches!(w.kind, 3 | 4) && arg(1) <= 10)
                    .and_then(|w| {
                        ResolvedStyle::resolve(w.kind, arg(1), &w.hints, self.text_appearance)
                            .measure(arg(2))
                    });
                if let Some(value) = value {
                    self.write_glk_reference(arg(3), value)?;
                    1
                } else {
                    0
                }
            }
            _ => 0,
        })
    }
    pub fn mouse_input(&mut self, window: u32, x: u32, y: u32) -> Result<(), VmError> {
        if self.mouse_requests.remove(&window) {
            self.events.push_back([4, window, x, y]);
            self.poll_events()?;
        }
        Ok(())
    }
    pub fn hyperlink_input(&mut self, window: u32, value: u32) -> Result<(), VmError> {
        if value != 0 && self.hyperlink_requests.remove(&window) {
            self.events.push_back([8, window, value, 0]);
            self.poll_events()?;
        }
        Ok(())
    }
}

impl GlkWindow {
    fn at_buffer_line_start(&self) -> bool {
        let mut at_start = true;
        let mut margin = false;
        for run in &self.runs {
            if run.flow_break {
                if margin {
                    at_start = true;
                    margin = false;
                }
            } else if let Some(image) = &run.image {
                if image.alignment >= 4 {
                    margin = true;
                } else {
                    at_start = false;
                }
            } else if !run.text.is_empty() {
                at_start = run.text.ends_with('\n');
            }
        }
        at_start
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pictured_vm() -> Vm {
        let image = super::super::tests::image_with_program(&[0x81, 0x20]);
        let picture = image::RgbaImage::from_pixel(80, 40, image::Rgba([20, 90, 160, 255]));
        let mut encoded = std::io::Cursor::new(Vec::new());
        picture
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let picture = encoded.into_inner();
        let mut blorb = b"FORM\0\0\0\0IFRS".to_vec();
        let offset = 12 + 24 + 8 + image.len();
        let mut index = 1u32.to_be_bytes().to_vec();
        index.extend_from_slice(b"Pict");
        index.extend_from_slice(&7u32.to_be_bytes());
        index.extend_from_slice(&(offset as u32).to_be_bytes());
        for (kind, data) in [(b"RIdx", index), (b"GLUL", image), (b"PNG ", picture)] {
            blorb.extend_from_slice(kind);
            blorb.extend_from_slice(&(data.len() as u32).to_be_bytes());
            blorb.extend_from_slice(&data);
            if data.len() % 2 == 1 {
                blorb.push(0);
            }
        }
        let len = blorb.len() as u32 - 8;
        blorb[4..8].copy_from_slice(&len.to_be_bytes());
        Vm::new(Story::from_bytes(&blorb, None).unwrap()).unwrap()
    }

    fn measured(vm: &mut Vm, window: u32, style: u32, hint: u32) -> u32 {
        assert_eq!(
            vm.style_call(0xb3, &[window, style, hint, u32::MAX])
                .unwrap(),
            1
        );
        vm.stack.pop_u32().unwrap()
    }

    #[test]
    fn first_graphics_draw_reuses_validation_decode_but_sessions_keep_source_bytes() {
        let mut vm = pictured_vm();
        assert!(vm.picture_dimensions(7).is_some());
        let validated = vm.decoded_picture.as_ref().unwrap().1.clone();
        let window = vm.open_window(&[0, 0, 0, 5, 0]);
        vm.take_graphics();
        assert_eq!(vm.draw_image(0xe1, &[window, 7, 0, 0]), 1);
        let GraphicsRequest::Draw(draw) = vm.take_graphics().pop().unwrap() else {
            panic!("expected a graphics draw");
        };
        assert!(std::sync::Arc::ptr_eq(
            draw.decoded.as_ref().unwrap(),
            &validated
        ));
        let restored: ImageRequest =
            serde_json::from_str(&serde_json::to_string(&draw).unwrap()).unwrap();
        assert!(restored.decoded.is_none());
        assert_eq!(crate::picture::decode(&restored.data).unwrap(), *validated);
    }

    #[test]
    fn corrupt_picture_reports_failure_before_enqueuing_a_draw() {
        let mut vm = pictured_vm();
        let data = vm.story.container.as_mut().unwrap();
        let idat = data.windows(4).position(|part| part == b"IDAT").unwrap();
        data[idat + 4] ^= 0xff;
        let window = vm.open_window(&[0, 0, 0, 5, 0]);
        vm.take_graphics();
        assert_eq!(vm.picture_dimensions(7), None);
        assert_eq!(vm.draw_image(0xe1, &[window, 7, 0, 0]), 0);
        assert!(vm.take_graphics().is_empty());
        let buffer = vm.open_window(&[window, 0x22, 50, 3, 0]);
        assert_eq!(vm.draw_image(0xe1, &[buffer, 7, 1, 0]), 0);
        assert!(vm.glk_windows[&buffer].runs.is_empty());
    }

    #[test]
    fn style_measure_reports_rendered_values_and_hints_apply_only_to_new_windows() {
        let mut vm = pictured_vm();
        for (hint, value) in [
            (0, 2),
            (1, (-1i32) as u32),
            (2, 3),
            (3, 4),
            (7, 0x123456),
            (8, 0xabcdef),
            (9, 1),
        ] {
            vm.style_call(0xb0, &[0, 9, hint, value]).unwrap();
        }
        let buffer = vm.open_window(&[0, 0, 0, 3, 0]);
        for (hint, expected) in [
            (0, 16),
            (1, (-8i32) as u32),
            (2, 3),
            (3, 26),
            (7, 0xabcdef),
            (8, 0x123456),
            (9, 1),
        ] {
            assert_eq!(measured(&mut vm, buffer, 9, hint), expected, "hint {hint}");
        }
        assert_eq!(measured(&mut vm, buffer, 0, 3), 18);
        vm.style_call(0xb0, &[3, 9, 3, 2]).unwrap();
        assert_eq!(measured(&mut vm, buffer, 9, 3), 26);
        let grid = vm.open_window(&[buffer, 0x12, 3, 4, 0]);
        assert_eq!(measured(&mut vm, grid, 9, 3), 13);
        assert_eq!(measured(&mut vm, grid, 9, 0), 0);
        assert_eq!(measured(&mut vm, grid, 9, 7), 0xabcdef);
        assert_eq!(measured(&mut vm, grid, 9, 6), 0);
        vm.set_text_appearance(22.0, 0x001122, 0x334455);
        assert_eq!(measured(&mut vm, buffer, 0, 3), 22);
        assert_eq!(measured(&mut vm, buffer, 9, 3), 30);
        assert_eq!(measured(&mut vm, buffer, 0, 7), 0x001122);
        assert_eq!(vm.style_call(0xb3, &[buffer, 11, 7, 0]).unwrap(), 0);
        assert_eq!(vm.style_call(0xb3, &[buffer, 0, 10, 0]).unwrap(), 0);
    }

    #[test]
    fn default_note_and_alert_styles_match_garglk_faces() {
        let vm = pictured_vm();
        let hints = BTreeMap::new();
        let appearance = vm.text_appearance;
        let note = ResolvedStyle::resolve(WINTYPE_TEXT_BUFFER, 6, &hints, appearance);
        let alert = ResolvedStyle::resolve(WINTYPE_TEXT_BUFFER, 5, &hints, appearance);
        let normal = ResolvedStyle::resolve(WINTYPE_TEXT_BUFFER, 0, &hints, appearance);

        assert!(note.oblique);
        assert!(alert.oblique);
        assert!(alert.weight > 0);
        assert!(!normal.oblique);
    }

    #[test]
    fn text_appearance_changes_publish_once_and_repeated_values_are_stable() {
        let mut vm = pictured_vm();
        let revision = vm.presentation_revision();
        vm.set_text_appearance(18.0, 0x202225, 0xf8f8f6);
        assert_eq!(vm.presentation_revision(), revision);

        vm.set_text_appearance(18.0, 0x123456, 0xf8f8f6);
        let changed = vm.presentation_revision();
        assert_ne!(changed, revision);
        vm.set_text_appearance(18.0, 0x123456, 0xf8f8f6);
        assert_eq!(vm.presentation_revision(), changed);
    }

    #[test]
    fn font_availability_changes_publish_layout_updates_once() {
        let mut vm = pictured_vm();
        let initial = vm.presentation_revision();
        vm.set_light_fonts([true, false]);
        let changed = vm.presentation_revision();
        assert_ne!(changed, initial);
        vm.set_light_fonts([true, false]);
        assert_eq!(vm.presentation_revision(), changed);
    }

    #[test]
    fn style_commands_follow_echo_chains_without_echoing_backward() {
        let mut vm = pictured_vm();
        let first = vm.open_window(&[0, 0, 0, 3, 0]);
        let second = vm.open_window(&[first, 0x21, 50, 3, 0]);
        let third = vm.open_window(&[second, 0x21, 50, 4, 0]);
        let streams = [first, second, third].map(|id| vm.glk_windows[&id].stream);
        vm.glk_windows.get_mut(&first).unwrap().echo_stream = streams[1];
        vm.glk_windows.get_mut(&second).unwrap().echo_stream = streams[2];
        vm.style_call(0x87, &[streams[0], 1]).unwrap();
        vm.glk_write_char(streams[0], 'A');
        assert_eq!(vm.glk_windows[&second].runs[0].style, 1);
        assert_eq!(vm.glk_windows[&third].grid_cells()[0].style, 1);
        vm.style_call(0x87, &[streams[1], 3]).unwrap();
        assert_eq!(vm.glk_windows[&first].style, 1);
        assert_eq!(vm.glk_windows[&second].style, 3);
        assert_eq!(vm.glk_windows[&third].style, 3);
    }

    #[test]
    fn style_distinguish_compares_actual_grid_and_buffer_appearance() {
        let mut vm = pictured_vm();
        vm.style_call(0xb0, &[0, 9, 4, u32::MAX]).unwrap(); // unsupported light weight renders regular
        let buffer = vm.open_window(&[0, 0, 0, 3, 0]);
        assert_eq!(vm.style_call(0xb2, &[buffer, 0, 9]).unwrap(), 0);
        assert_eq!(vm.style_call(0xb2, &[buffer, 0, 3]).unwrap(), 1);
        assert_eq!(vm.style_call(0xb2, &[buffer, 3, 4]).unwrap(), 0);
        vm.style_call(0xb0, &[3, 0, 9, 1]).unwrap();
        assert_eq!(vm.style_call(0xb2, &[buffer, 0, 3]).unwrap(), 1);
        let grid = vm.open_window(&[buffer, 0x12, 3, 4, 0]);
        assert_eq!(vm.style_call(0xb2, &[grid, 0, 2]).unwrap(), 0);
        assert_eq!(vm.style_call(0xb2, &[grid, 0, 1]).unwrap(), 1);
        vm.set_graphical_host(false);
        assert_eq!(vm.style_call(0xb2, &[buffer, 0, 3]).unwrap(), 0);
        assert_eq!(vm.style_call(0xb3, &[buffer, 0, 3, 0]).unwrap(), 0);
    }

    #[test]
    fn light_style_queries_follow_host_faces_and_reinstall_after_sessions() {
        let mut vm = pictured_vm();
        vm.style_call(0xb0, &[0, 9, 4, u32::MAX]).unwrap();
        let buffer = vm.open_window(&[0, 0, 0, 3, 0]);
        let grid = vm.open_window(&[buffer, 0x12, 3, 4, 0]);
        assert_eq!(measured(&mut vm, buffer, 9, 4), 0);
        vm.set_light_fonts([true, false]);
        assert_eq!(measured(&mut vm, buffer, 9, 4), u32::MAX);
        assert_eq!(measured(&mut vm, grid, 9, 4), 0);
        assert_eq!(vm.style_call(0xb2, &[buffer, 0, 9]).unwrap(), 1);
        vm.set_light_fonts([true, true]);
        vm.set_text_appearance(20.0, 0, 0xffffff);
        assert_eq!(measured(&mut vm, grid, 9, 4), u32::MAX);
        vm.text_appearance =
            serde_json::from_str(&serde_json::to_string(&vm.text_appearance).unwrap()).unwrap();
        assert_eq!(measured(&mut vm, buffer, 9, 4), 0);
        vm.set_light_fonts([true, true]);
        assert_eq!(measured(&mut vm, buffer, 9, 4), u32::MAX);
        assert_eq!(measured(&mut vm, grid, 9, 4), u32::MAX);
        vm.set_light_fonts([false, false]);
        assert_eq!(measured(&mut vm, buffer, 9, 4), 0);
    }

    #[test]
    fn text_images_preserve_order_links_and_stream_counts_across_sessions() {
        let mut vm = pictured_vm();
        let window = vm.open_window(&[0, 0, 0, 3, 0]);
        let stream = vm.glk_windows[&window].stream;
        vm.glk_current_stream = stream;
        vm.glk_write_text(stream, "before");
        vm.style_call(0x100, &[42]).unwrap();
        assert_eq!(vm.draw_image(0xe1, &[window, 7, 1, 0]), 1);
        vm.style_call(0x100, &[0]).unwrap();
        vm.glk_write_text(stream, "after");
        let runs = &vm.glk_windows[&window].runs;
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].text, "before");
        assert_eq!(runs[1].hyperlink, 42);
        assert_eq!(runs[1].image.as_ref().unwrap().original, [80, 40]);
        assert_eq!(runs[2].text, "after");
        assert_eq!(vm.glk_windows[&window].write_count, 11);
        assert_eq!(vm.output, "beforeafter");
        let restored: Vm = serde_json::from_str(&serde_json::to_string(&vm).unwrap()).unwrap();
        let restored = restored.validate_session().unwrap();
        assert!(restored.image_resource(7).is_some());
        assert_eq!(restored.window_views()[0].runs[1].hyperlink, 42);
        assert_eq!(
            restored.window_views()[0].runs[1]
                .image
                .as_ref()
                .unwrap()
                .resource,
            7
        );
    }

    #[test]
    fn margin_placement_flow_break_and_clear_obey_stream_boundaries() {
        let mut vm = pictured_vm();
        let window = vm.open_window(&[0, 0, 0, 3, 0]);
        let stream = vm.glk_windows[&window].stream;
        for size in [[0, 40], [80, 0]] {
            assert_eq!(vm.draw_image(0xe2, &[window, 7, 1, 0, size[0], size[1]]), 1);
        }
        assert!(vm.glk_windows[&window].runs.is_empty());
        assert_eq!(vm.draw_image(0xe1, &[window, 7, 4, 0]), 1);
        assert_eq!(vm.draw_image(0xe1, &[window, 7, 5, 0]), 1);
        vm.glk_write_text(stream, "beside");
        assert_eq!(vm.draw_image(0xe1, &[window, 7, 4, 0]), 0);
        vm.flow_break(window);
        assert_eq!(vm.draw_image(0xe1, &[window, 7, 4, 0]), 1);
        vm.glk_write_text(stream, "\n");
        assert_eq!(vm.draw_image(0xe1, &[window, 7, 1, 0]), 1);
        assert_eq!(vm.draw_image(0xe1, &[window, 7, 5, 0]), 0);
        super::super::tests::push_glk_arguments(&mut vm, &[window]);
        vm.glk(0x2a, 1, Destination::Discard).unwrap();
        assert!(vm.glk_windows[&window].runs.is_empty());
        vm.glk_write_text(stream, "text");
        vm.flow_break(window); // A mark without a preceding margin image is a no-op.
        assert_eq!(vm.draw_image(0xe1, &[window, 7, 5, 0]), 0);
    }

    #[test]
    fn image_scaling_is_dynamic_in_buffers_and_fixed_on_canvases() {
        let mut vm = pictured_vm();
        let buffer = vm.open_window(&[0, 0, 0, 3, 0]);
        assert_eq!(vm.draw_image(0xe1, &[buffer, 7, 1, 0]), 1);
        let picture = vm.glk_windows[&buffer].runs[0].image.as_ref().unwrap();
        assert_eq!(picture.dimensions(40), [40, 20]);
        assert_eq!(picture.dimensions(200), [80, 40]);
        assert_eq!(
            vm.draw_image(0xec, &[buffer, 7, 2, 0, 32768, 65536, 3 | 12, 0]),
            1
        );
        let ratio = vm.glk_windows[&buffer].runs[1].image.as_ref().unwrap();
        assert_eq!(ratio.dimensions(400), [200, 100]);
        assert_eq!(ratio.dimensions(200), [100, 50]);
        let mut capped = ratio.clone();
        capped.rule = 2 | 8;
        capped.size = [600, 100];
        capped.max_width = 32768;
        assert_eq!(capped.dimensions(400), [200, 33]);
        capped.max_width = 0;
        assert_eq!(capped.dimensions(400), [600, 100]);
        let extreme = BufferImage {
            original: [1, u32::MAX],
            size: [u32::MAX, u32::MAX],
            rule: 3 | 12,
            max_width: u32::MAX - 1,
            ..ratio.clone()
        };
        assert_eq!(extreme.dimensions(u32::MAX), [u32::MAX, u32::MAX]);
        let canvas = vm.open_window(&[buffer, 0x21, 50, 5, 0]);
        let width = vm.glk_windows[&canvas].width;
        assert_eq!(
            vm.draw_image(0xec, &[canvas, 7, u32::MAX, 2, 32768, 65536, 3 | 12, 1]),
            1
        );
        let GraphicsRequest::Draw(request) = vm.graphics.last().unwrap() else {
            panic!()
        };
        assert_eq!(request.requested_size, Some([width / 2, width / 4]));
        assert_eq!(request.position, [-1, 2]);
        assert_eq!(vm.draw_image(0xec, &[buffer, 7, 1, 0, 10, 10, 0, 0]), 0);
        assert_eq!(vm.draw_image(0xe1, &[buffer, 7, 0, 0]), 0);
        assert_eq!(vm.draw_image(0xe1, &[buffer, 99, 1, 0]), 0);
        vm.set_graphical_host(false);
        assert_eq!(vm.glk_gestalt(6, 0), 0);
        assert_eq!(vm.glk_gestalt(7, 3), 0);
        assert_eq!(vm.glk_gestalt(24, 3), 0);
        assert_eq!(vm.draw_image(0xe1, &[buffer, 7, 1, 0]), 0);
    }
}

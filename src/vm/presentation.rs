use super::*;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
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
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
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
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct WindowView {
    pub id: u32,
    pub kind: u32,
    pub rect: [u32; 4],
    pub runs: Vec<TextRun>,
    pub grid: String,
    pub hints: BTreeMap<(u32, u32), u32>,
}
impl Vm {
    pub fn image_resource(&self, resource: u32) -> Option<&[u8]> {
        self.story.resource(*b"Pict", resource)
    }

    pub(super) fn draw_image(&mut self, selector: u32, args: &[u32]) -> u32 {
        if !self.graphical_host {
            return 0;
        }
        let arg = |n: usize| args.get(n).copied().unwrap_or(0);
        let (window_id, resource) = (arg(0), arg(1));
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
        let Some(original) = image_dimensions(data) else {
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
                if let Some(GlkStream {
                    target: GlkStreamTarget::Window(id),
                    ..
                }) = self.glk_streams.get(&stream)
                    && let Some(window) = self.glk_windows.get_mut(id)
                {
                    if selector >= 0x100 {
                        window.hyperlink = value;
                    } else {
                        window.style = if value <= 10 { value } else { 0 };
                    }
                }
                0
            }
            0xb0 | 0xb1 => {
                if arg(1) <= 10 && (3..=9).contains(&arg(2)) {
                    for kind in [3] {
                        if arg(0) == 0 || arg(0) == kind {
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
            0xb2 => {
                if let Some(window) = self.glk_windows.get(&arg(0)) {
                    u32::from(
                        window.kind == 3
                            && arg(1) != arg(2)
                            && (arg(1) <= 10 && arg(2) <= 10)
                            && (default_style(arg(1)) != default_style(arg(2))
                                || (0..10).any(|hint| {
                                    window.hints.get(&(arg(1), hint))
                                        != window.hints.get(&(arg(2), hint))
                                })),
                    )
                } else {
                    0
                }
            }
            0xb3 => {
                let value = self
                    .glk_windows
                    .get(&arg(0))
                    .and_then(|w| w.hints.get(&(arg(1), arg(2))).copied());
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
fn default_style(style: u32) -> (bool, bool, bool) {
    (
        matches!(style, 3 | 4 | 5 | 8),
        matches!(style, 1 | 5),
        style == 2,
    )
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

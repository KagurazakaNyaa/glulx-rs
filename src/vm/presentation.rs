use super::*;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct TextRun {
    pub text: String,
    pub style: u32,
    pub hyperlink: u32,
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

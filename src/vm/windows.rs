use super::*;

impl Vm {
    pub(super) fn valid_echo(&self, id: u32, mut stream: u32) -> bool {
        let mut seen = HashSet::new();
        while stream != 0 {
            if !seen.insert(stream) {
                return false;
            }
            match self.glk_streams.get(&stream).map(|s| &s.target) {
                Some(GlkStreamTarget::Window(window)) => {
                    if *window == id {
                        return false;
                    }
                    stream = self.glk_windows.get(window).map_or(0, |w| w.echo_stream);
                }
                Some(_) => return true,
                None => return false,
            }
        }
        true
    }

    pub(super) fn open_window(&mut self, args: &[u32]) -> u32 {
        let arg = |n: usize| args.get(n).copied().unwrap_or(0);
        let (split, method, size, kind, rock) = (arg(0), arg(1), arg(2), arg(3), arg(4));
        if !matches!(kind, 2..=5)
            || (split == 0 && self.glk_root != 0)
            || (split != 0 && (!self.glk_windows.contains_key(&split) || !valid_method(method)))
        {
            return 0;
        }
        let id = self.glk_next_window;
        self.glk_next_window += 1;
        let stream = self.glk_next_stream;
        self.glk_next_stream += 1;
        let mut window = GlkWindow::new(rock, kind, stream, 80, 25);
        if kind == WINTYPE_GRAPHICS {
            window.width = 0;
            window.height = 0;
        }
        window.hints = self
            .style_hints
            .iter()
            .filter(|((k, _, _), _)| *k == kind)
            .map(|((_, style, hint), value)| ((*style, *hint), *value))
            .collect();
        self.glk_windows.insert(id, window);
        self.glk_streams.insert(
            stream,
            GlkStream {
                rock: 0,
                target: GlkStreamTarget::Window(id),
            },
        );
        if split == 0 {
            self.glk_root = id;
        } else {
            let parent = self.glk_windows[&split].parent;
            let pair_id = self.glk_next_window;
            self.glk_next_window += 1;
            let mut pair = GlkWindow::new(0, 1, 0, 0, 0);
            pair.parent = parent;
            pair.children = Some([split, id]);
            pair.children_reversed = Some(method & 1 == 0);
            pair.method = method;
            pair.split_size = size;
            pair.key = id;
            self.glk_windows.insert(pair_id, pair);
            self.glk_windows.get_mut(&split).unwrap().parent = pair_id;
            self.glk_windows.get_mut(&id).unwrap().parent = pair_id;
            if parent == 0 {
                self.glk_root = pair_id;
            } else if let Some(children) = &mut self.glk_windows.get_mut(&parent).unwrap().children
            {
                for child in children {
                    if *child == split {
                        *child = pair_id;
                    }
                }
            }
        }
        self.layout_windows();
        id
    }
    pub(super) fn close_window(&mut self, id: u32) -> (u32, u32) {
        let Some(window) = self.glk_windows.get(&id) else {
            return (0, 0);
        };
        let write_count = window.write_count;
        let parent = window.parent;
        if parent == 0 {
            self.glk_root = 0;
        } else {
            let pair = self.glk_windows.remove(&parent).unwrap();
            let sibling = pair
                .children
                .unwrap()
                .into_iter()
                .find(|child| *child != id)
                .unwrap();
            self.glk_windows.get_mut(&sibling).unwrap().parent = pair.parent;
            if pair.parent == 0 {
                self.glk_root = sibling;
            } else if let Some(children) =
                &mut self.glk_windows.get_mut(&pair.parent).unwrap().children
            {
                for child in children {
                    if *child == parent {
                        *child = sibling;
                    }
                }
            }
        }
        let mut pending = vec![id];
        while let Some(id) = pending.pop() {
            if let Some(window) = self.glk_windows.remove(&id) {
                if let Some(children) = window.children {
                    pending.extend(children);
                }
                if window.stream != 0 {
                    self.glk_streams.remove(&window.stream);
                    for other in self.glk_windows.values_mut() {
                        if other.echo_stream == window.stream {
                            other.echo_stream = 0;
                        }
                    }
                    if self.glk_current_stream == window.stream {
                        self.glk_current_stream = 0;
                    }
                }
                self.graphics.push(GraphicsRequest::Close { window: id });
                self.requests.remove(&id);
                self.mouse_requests.remove(&id);
                self.hyperlink_requests.remove(&id);
                self.events.retain(|event| event[1] != id);
            }
        }
        let existing: HashSet<u32> = self.glk_windows.keys().copied().collect();
        for window in self.glk_windows.values_mut() {
            if !existing.contains(&window.key) {
                window.key = 0;
            }
        }
        self.layout_windows();
        (0, write_count)
    }
    pub(super) fn set_arrangement(&mut self, args: &[u32]) {
        let id = args.first().copied().unwrap_or(0);
        let method = args.get(1).copied().unwrap_or(0);
        let size = args.get(2).copied().unwrap_or(0);
        let key = args.get(3).copied().unwrap_or(0);
        let Some(pair) = self.glk_windows.get(&id).filter(|w| w.kind == 1) else {
            return;
        };
        if !valid_method(method) || (method & 2) != (pair.method & 2) {
            return;
        }
        if key != 0 && (!self.is_descendant(key, id) || self.glk_windows[&key].kind == 1) {
            return;
        }
        let reversed = self.pair_children_reversed(pair);
        let pair = self.glk_windows.get_mut(&id).unwrap();
        pair.children_reversed = Some(reversed);
        pair.method = method;
        pair.split_size = size;
        if key != 0 {
            pair.key = key;
        }
        self.layout_windows();
    }
    fn pair_children_reversed(&self, pair: &GlkWindow) -> bool {
        pair.children_reversed.unwrap_or_else(|| {
            // Earlier session snapshots derived physical order from the key
            // and direction. Preserve that displayed order when loading them.
            let key_second = pair
                .children
                .is_some_and(|children| !self.is_descendant(pair.key, children[0]));
            key_second == (pair.method & 1 == 0)
        })
    }
    fn is_descendant(&self, mut child: u32, parent: u32) -> bool {
        while child != 0 {
            if child == parent {
                return true;
            }
            child = self.glk_windows.get(&child).map_or(0, |w| w.parent);
        }
        false
    }
    pub(super) fn layout_windows(&mut self) {
        let mut pending = vec![(
            self.glk_root,
            [0, 0, self.viewport_size[0], self.viewport_size[1]],
        )];
        while let Some((id, [x, y, width, height])) = pending.pop() {
            let Some(window) = self.glk_windows.get(&id) else {
                continue;
            };
            let pair = window.children.map(|children| {
                (
                    children,
                    window.method,
                    window.split_size,
                    window.key,
                    self.pair_children_reversed(window),
                )
            });
            if let Some((children, method, size, key, reversed)) = pair {
                let vertical = method & 2 != 0;
                let extent = if vertical { height } else { width };
                let unit = self
                    .glk_windows
                    .get(&key)
                    .map_or(1, |window| match window.kind {
                        3 => self.window_text_metrics(window)[usize::from(vertical)],
                        4 => [GRID_CELL_WIDTH, GRID_CELL_HEIGHT][usize::from(vertical)],
                        _ => 1,
                    });
                let requested = if method & 0x30 == 0x20 {
                    (extent as u64 * size.min(100) as u64 / 100) as u32
                } else {
                    size.saturating_mul(unit)
                };
                let constrained_extent = requested.min(extent);
                let first = usize::from(reversed);
                let constrained = if method & 1 == 0 { first } else { 1 - first };
                let mut offset = 0;
                for index in [first, 1 - first] {
                    let child_extent = if index == constrained {
                        constrained_extent
                    } else {
                        extent - constrained_extent
                    };
                    pending.push((
                        children[index],
                        if vertical {
                            [x, y + offset, width, child_extent]
                        } else {
                            [x + offset, y, child_extent, height]
                        },
                    ));
                    offset += child_extent;
                }
                self.glk_windows.get_mut(&id).unwrap().children_reversed = Some(reversed);
            }
            let text_metrics = if self.glk_windows[&id].kind == 3 {
                self.window_text_metrics(&self.glk_windows[&id])
            } else {
                [GRID_CELL_WIDTH, GRID_CELL_HEIGHT]
            };
            let window = self.glk_windows.get_mut(&id).unwrap();
            window.rect = [x, y, width, height];
            let (width, height) = if matches!(window.kind, 3 | 4) {
                (width / text_metrics[0], height / text_metrics[1])
            } else if matches!(window.kind, 1 | 2) {
                (0, 0)
            } else {
                (width, height)
            };
            if window.kind == 4 && (width != window.width || height != window.height) {
                window.resize_grid(width, height);
            }
            if window.kind == WINTYPE_GRAPHICS && (width != window.width || height != window.height)
            {
                self.graphics.push(GraphicsRequest::Resize {
                    window: id,
                    background: window.background_color,
                    canvas_size: [width, height],
                });
            }
            window.width = width;
            window.height = height;
        }
    }
}
fn valid_method(method: u32) -> bool {
    method & !0x133 == 0 && matches!(method & 0x30, 0x10 | 0x20)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::tests::image_with_program;

    fn vm() -> Vm {
        Vm::new(Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap()).unwrap()
    }

    #[test]
    fn changing_split_direction_constrains_the_other_side_without_moving_children() {
        for direction in [0, 2] {
            for legacy in [false, true] {
                let mut vm = vm();
                let old = vm.open_window(&[0, 0, 0, 5, 0]);
                let new = vm.open_window(&[old, 0x10 | direction, 20, 5, 0]);
                let pair = vm.glk_windows[&old].parent;
                let axis = usize::from(direction == 2);
                let extent = vm.viewport_size[axis];
                assert_eq!(vm.glk_windows[&new].rect[axis], 0);
                assert_eq!(vm.glk_windows[&new].rect[axis + 2], 20);
                assert_eq!(vm.glk_windows[&old].rect[axis], 20);
                if legacy {
                    vm.glk_windows.get_mut(&pair).unwrap().children_reversed = None;
                }
                vm.set_arrangement(&[pair, 0x10 | direction | 1, 20, 0]);
                assert_eq!(vm.glk_windows[&new].rect[axis], 0);
                assert_eq!(vm.glk_windows[&new].rect[axis + 2], extent - 20);
                assert_eq!(vm.glk_windows[&old].rect[axis], extent - 20);
                assert_eq!(vm.glk_windows[&old].rect[axis + 2], 20);
                let before = [vm.glk_windows[&old].rect, vm.glk_windows[&new].rect];
                // A different key supplies units but does not rearrange children.
                vm.set_arrangement(&[pair, 0x10 | direction | 1, 20, old]);
                assert_eq!(
                    [vm.glk_windows[&old].rect, vm.glk_windows[&new].rect],
                    before
                );
            }
        }
    }

    #[test]
    fn complementary_proportions_and_descendant_keys_preserve_physical_order() {
        let mut vm = vm();
        let bottom = vm.open_window(&[0, 0, 0, 5, 0]);
        let top = vm.open_window(&[bottom, 0x22, 30, 5, 0]);
        let pair = vm.glk_windows[&top].parent;
        let before = [vm.glk_windows[&top].rect, vm.glk_windows[&bottom].rect];
        vm.set_arrangement(&[pair, 0x23, 70, bottom]);
        assert_eq!(
            [vm.glk_windows[&top].rect, vm.glk_windows[&bottom].rect],
            before
        );
        let nested = vm.open_window(&[top, 0x21, 50, 4, 0]);
        let upper_pair = vm.glk_windows[&top].parent;
        vm.set_arrangement(&[pair, 0x12, 3, nested]);
        assert_eq!(
            vm.glk_windows[&upper_pair].rect,
            [0, 0, 640, 3 * GRID_CELL_HEIGHT]
        );
        assert_eq!(vm.glk_windows[&bottom].rect[1], 3 * GRID_CELL_HEIGHT);
        vm.close_window(nested);
        assert_eq!(vm.glk_windows[&top].rect[1], 0);
        assert!(vm.glk_windows[&bottom].rect[1] > 0);
    }

    #[test]
    fn blank_windows_have_no_glk_measurement_units() {
        let mut vm = vm();
        let blank = vm.open_window(&[0, 0, 0, 2, 0]);
        assert_eq!(vm.glk_windows[&blank].rect, [0, 0, 640, 480]);
        assert_eq!(
            (vm.glk_windows[&blank].width, vm.glk_windows[&blank].height),
            (0, 0)
        );
        vm.open_window(&[blank, 0x21, 25, 5, 0]);
        assert!(vm.glk_windows[&blank].rect[2] > 0);
        assert_eq!(
            (vm.glk_windows[&blank].width, vm.glk_windows[&blank].height),
            (0, 0)
        );
    }

    #[test]
    fn text_buffer_sizes_use_the_key_windows_normal_font_metrics() {
        let mut vm = vm();
        vm.set_text_metrics(std::sync::Arc::new(|style| {
            [10, style.font_size as u32 + 6]
        }));
        let graphics = vm.open_window(&[0, 0, 0, 5, 0]);
        let buffer = vm.open_window(&[graphics, 0x12, 3, 3, 0]);
        assert_eq!(vm.glk_windows[&buffer].rect, [0, 0, 640, 72]);
        assert_eq!(
            (
                vm.glk_windows[&buffer].width,
                vm.glk_windows[&buffer].height
            ),
            (64, 3)
        );
        vm.set_text_appearance(22.0, 0, 0xffffff);
        assert_eq!(vm.glk_windows[&buffer].rect, [0, 0, 640, 84]);
        let grid = vm.open_window(&[graphics, 0x12, 3, 4, 0]);
        assert_eq!(vm.glk_windows[&grid].rect[3], 3 * GRID_CELL_HEIGHT);
        assert_eq!(vm.glk_windows[&grid].width, 640 / GRID_CELL_WIDTH);
    }
}

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
        let pair = self.glk_windows.get_mut(&id).unwrap();
        pair.method = method;
        pair.split_size = size;
        if key != 0 {
            pair.key = key;
        }
        self.layout_windows();
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
            let pair = window
                .children
                .map(|children| (children, window.method, window.split_size, window.key));
            if let Some((children, method, size, key)) = pair {
                let vertical = method & 2 != 0;
                let extent = if vertical { height } else { width };
                let key_kind = self.glk_windows.get(&key).map_or(2, |w| w.kind);
                let unit = if matches!(key_kind, 3 | 4) {
                    if vertical { 16 } else { 8 }
                } else {
                    1
                };
                let requested = if method & 0x30 == 0x20 {
                    (extent as u64 * size.min(100) as u64 / 100) as u32
                } else {
                    size.saturating_mul(unit)
                };
                let key_extent = requested.min(extent);
                let key_index = usize::from(!self.is_descendant(key, children[0]));
                let first = if method & 1 == 0 {
                    key_index
                } else {
                    1 - key_index
                };
                let mut offset = 0;
                for index in [first, 1 - first] {
                    let child_extent = if index == key_index {
                        key_extent
                    } else {
                        extent - key_extent
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
            }
            let window = self.glk_windows.get_mut(&id).unwrap();
            window.rect = [x, y, width, height];
            let (width, height) = if matches!(window.kind, 3 | 4) {
                (width / 8, height / 16)
            } else if window.kind == 1 {
                (0, 0)
            } else {
                (width, height)
            };
            if window.kind == 4 && (width != window.width || height != window.height) {
                let mut grid = vec![' '; width as usize * height as usize];
                for y in 0..height.min(window.height) {
                    for x in 0..width.min(window.width) {
                        grid[(y * width + x) as usize] =
                            window.grid[(y * window.width + x) as usize];
                    }
                }
                window.grid = grid;
            }
            window.width = width;
            window.height = height;
        }
    }
}
fn valid_method(method: u32) -> bool {
    method & !0x133 == 0 && matches!(method & 0x30, 0x10 | 0x20)
}

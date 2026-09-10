use super::*;

/// Grid layout and pointer hit testing use the same logical pixel geometry.
pub const GRID_CELL_WIDTH: u32 = 8;
pub const GRID_CELL_HEIGHT: u32 = 16;
pub const GRID_FONT_SIZE: f32 = 13.0;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridCell {
    pub character: char,
    pub style: u32,
    pub hyperlink: u32,
}

impl GlkWindow {
    pub(super) fn normalize_grid_cursor(&mut self) {
        // A cursor moved arbitrarily far to the right wraps once, to the start
        // of the next row. It does not imply printing all intervening columns.
        if self.cursor_x >= self.width {
            self.cursor_x = 0;
            self.cursor_y = self.cursor_y.saturating_add(1);
        }
    }

    pub(super) fn put_grid_char(&mut self, character: char) {
        self.normalize_grid_cursor();
        if self.width == 0 || self.cursor_y >= self.height {
            return;
        }
        if character == '\n' {
            self.cursor_x = 0;
            self.cursor_y = self.cursor_y.saturating_add(1);
            return;
        }
        let index = self.cursor_y as usize * self.width as usize + self.cursor_x as usize;
        if let Some(cell) = self.grid.get_mut(index) {
            *cell = character;
            // Older desktop snapshots retain their characters but predate
            // per-cell attributes. Missing attributes mean Normal, no link.
            self.grid_styles.resize(self.grid.len(), 0);
            self.grid_hyperlinks.resize(self.grid.len(), 0);
            self.grid_styles[index] = self.style;
            self.grid_hyperlinks[index] = self.hyperlink;
        }
        self.cursor_x += 1;
    }

    pub(super) fn resize_grid(&mut self, width: u32, height: u32) {
        if width == self.width && height == self.height {
            return;
        }
        let count = width as usize * height as usize;
        let mut characters = vec![' '; count];
        let mut styles = vec![0; count];
        let mut hyperlinks = vec![0; count];
        for y in 0..height.min(self.height) {
            for x in 0..width.min(self.width) {
                let source = y as usize * self.width as usize + x as usize;
                let target = y as usize * width as usize + x as usize;
                characters[target] = self.grid[source];
                styles[target] = self.grid_styles.get(source).copied().unwrap_or(0);
                hyperlinks[target] = self.grid_hyperlinks.get(source).copied().unwrap_or(0);
            }
        }
        self.grid = characters;
        self.grid_styles = styles;
        self.grid_hyperlinks = hyperlinks;
        self.content_revision = self.content_revision.wrapping_add(1);
    }

    pub(super) fn grid_cells(&self) -> Vec<GridCell> {
        self.grid
            .iter()
            .enumerate()
            .map(|(index, &character)| GridCell {
                character,
                style: self.grid_styles.get(index).copied().unwrap_or(0),
                hyperlink: self.grid_hyperlinks.get(index).copied().unwrap_or(0),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vm() -> Vm {
        let story = Story::from_bytes(
            &super::super::tests::image_with_program(&[0x81, 0x20]),
            None,
        )
        .unwrap();
        Vm::new(story).unwrap()
    }

    #[test]
    fn grid_wraps_at_columns_and_stops_after_last_row() {
        let mut window = GlkWindow::new(0, WINTYPE_TEXT_GRID, 1, 3, 2);
        for character in "abcdefghi\nZ".chars() {
            window.put_grid_char(character);
        }
        assert_eq!(window.grid.iter().collect::<String>(), "abcdef");
        assert_eq!((window.cursor_x, window.cursor_y), (0, 2));
        window.cursor_x = 1;
        window.cursor_y = 1;
        window.put_grid_char('\n');
        window.put_grid_char('!');
        assert_eq!(window.grid.iter().collect::<String>(), "abcdef");
        window.cursor_x = 0;
        window.cursor_y = 0;
        window.put_grid_char('R');
        assert_eq!(window.grid[0], 'R');
    }

    #[test]
    fn moved_grid_cursor_wraps_once_and_never_overflows() {
        let mut window = GlkWindow::new(0, WINTYPE_TEXT_GRID, 1, 3, 3);
        window.cursor_x = u32::MAX;
        window.put_grid_char('X');
        assert_eq!(window.grid[3], 'X');
        assert_eq!((window.cursor_x, window.cursor_y), (1, 1));
        window.cursor_x = u32::MAX;
        window.cursor_y = u32::MAX;
        window.put_grid_char('\n');
        window.put_grid_char('!');
        assert_eq!(window.cursor_y, u32::MAX);
        assert_eq!(window.grid.iter().filter(|&&c| c != ' ').count(), 1);
        window.resize_grid(0, 0);
        window.width = 0;
        window.height = 0;
        window.put_grid_char('!');
        assert!(window.grid.is_empty());
    }

    #[test]
    fn grid_attributes_follow_overwrite_clear_resize_and_session_roundtrip() {
        let mut vm = vm();
        vm.resize_windows(24, 32);
        let id = vm.open_window(&[0, 0, 0, WINTYPE_TEXT_GRID, 11]);
        let stream = vm.glk_windows[&id].stream;
        vm.style_call(0x87, &[stream, 3]).unwrap();
        vm.style_call(0x101, &[stream, 42]).unwrap();
        vm.glk_write_text(stream, "ABCDEF");
        vm.style_call(0x87, &[stream, 1]).unwrap();
        vm.style_call(0x101, &[stream, 7]).unwrap();
        let window = vm.glk_windows.get_mut(&id).unwrap();
        window.cursor_x = 1;
        window.cursor_y = 0;
        vm.glk_write_char(stream, 'X');
        vm.resize_windows(16, 32);
        vm.resize_windows(32, 48);
        let expected = vec![
            ('A', 3, 42),
            ('X', 1, 7),
            (' ', 0, 0),
            (' ', 0, 0),
            ('D', 3, 42),
            ('E', 3, 42),
            (' ', 0, 0),
            (' ', 0, 0),
            (' ', 0, 0),
            (' ', 0, 0),
            (' ', 0, 0),
            (' ', 0, 0),
        ];
        let cells = vm.glk_windows[&id].grid_cells();
        assert_eq!(
            cells
                .iter()
                .map(|c| (c.character, c.style, c.hyperlink))
                .collect::<Vec<_>>(),
            expected,
        );
        let serialized = serde_json::to_vec(&vm).unwrap();
        let mut restored = serde_json::from_slice::<Vm>(&serialized)
            .unwrap()
            .validate_session()
            .unwrap();
        assert_eq!(restored.glk_windows[&id].grid_cells(), cells);
        super::super::tests::push_glk_arguments(&mut restored, &[id]);
        restored.glk(0x2a, 1, Destination::Discard).unwrap();
        let window = &restored.glk_windows[&id];
        assert!(window.grid_cells().iter().all(|cell| *cell
            == GridCell {
                character: ' ',
                style: 0,
                hyperlink: 0
            }));
        assert_eq!((window.cursor_x, window.cursor_y), (0, 0));
        // Clearing resets cells to Normal but leaves the output style current.
        assert_eq!((window.style, window.hyperlink), (1, 7));
    }

    #[test]
    fn old_grid_sessions_default_attributes_without_losing_characters() {
        let mut window = GlkWindow::new(0, WINTYPE_TEXT_GRID, 1, 2, 1);
        window.grid = vec!['O', 'K'];
        window.grid_styles.clear();
        window.grid_hyperlinks.clear();
        assert_eq!(
            window.grid_cells()[0],
            GridCell {
                character: 'O',
                style: 0,
                hyperlink: 0
            }
        );
        window.style = 8;
        window.hyperlink = 12;
        window.put_grid_char('N');
        assert_eq!(
            window.grid_cells()[0],
            GridCell {
                character: 'N',
                style: 8,
                hyperlink: 12
            }
        );
        assert_eq!(
            window.grid_cells()[1],
            GridCell {
                character: 'K',
                style: 0,
                hyperlink: 0
            }
        );
    }

    #[test]
    fn font_capability_follows_host_coverage_and_is_reconfigured_after_restore() {
        let mut vm = vm();
        assert_eq!(vm.glk_gestalt(3, 'A' as u32), 2);
        assert_eq!(vm.glk_gestalt(3, '中' as u32), 0);
        vm.set_glyph_support(std::sync::Arc::new(|character| {
            character.is_ascii() || character == '中'
        }));
        assert_eq!(vm.glk_gestalt(3, '中' as u32), 2);
        assert_eq!(vm.glk_gestalt(3, '文' as u32), 0);
        assert_eq!(vm.glk_gestalt(3, '\n' as u32), 2);
        for invalid in [0, 9, 13, 0x7f, 0x9f, 0xd800, 0x110000] {
            assert_eq!(vm.glk_gestalt(3, invalid), 0);
        }
        // Ability to enter/store Unicode does not depend on installed fonts.
        assert_eq!(vm.glk_gestalt(1, '文' as u32), 1);
        assert_eq!(vm.glk_gestalt(2, '文' as u32), 1);
        let mut restored = serde_json::from_slice::<Vm>(&serde_json::to_vec(&vm).unwrap()).unwrap();
        assert_eq!(restored.glk_gestalt(3, '中' as u32), 0);
        restored.set_graphical_host(false);
        assert_eq!(restored.glk_gestalt(3, '文' as u32), 2);
    }
}

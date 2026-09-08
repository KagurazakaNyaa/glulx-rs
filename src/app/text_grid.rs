//! A grid cell has identical geometry for text, styles, editing and hit tests.

use std::sync::Arc;

use eframe::egui::{self, RichText};

use super::{PlayerSettings, color_word, rgb};
use crate::vm::{GRID_CELL_HEIGHT, GRID_CELL_WIDTH, GridCell, WindowView};

pub(super) struct GridEditor<'a> {
    pub text: &'a mut String,
    pub maximum_length: u32,
}

#[derive(Default)]
pub(super) struct GridResponse {
    pub cell: Option<[u32; 2]>,
    pub hyperlink: Option<u32>,
    pub changed: bool,
    pub submitted: bool,
}

fn cell_at(rect: egui::Rect, size: [u32; 2], pos: egui::Pos2) -> Option<[u32; 2]> {
    if pos.x < rect.left() || pos.y < rect.top() {
        return None;
    }
    let x = ((pos.x - rect.left()) / GRID_CELL_WIDTH as f32) as u32;
    let y = ((pos.y - rect.top()) / GRID_CELL_HEIGHT as f32) as u32;
    (x < size[0] && y < size[1]).then_some([x, y])
}

fn cell_rect(rect: egui::Rect, x: u32, y: u32) -> egui::Rect {
    egui::Rect::from_min_size(
        rect.min + egui::vec2((x * GRID_CELL_WIDTH) as f32, (y * GRID_CELL_HEIGHT) as f32),
        egui::vec2(GRID_CELL_WIDTH as f32, GRID_CELL_HEIGHT as f32),
    )
}

/// Wide fallback glyphs still occupy one Glk grid cell. Compress their mesh
/// horizontally instead of clipping away the right half of the character.
fn fit_galley(mut galley: Arc<egui::Galley>, width: f32) -> Arc<egui::Galley> {
    let factor = (width / galley.size().x).min(1.0);
    if factor < 1.0 {
        let galley = Arc::make_mut(&mut galley);
        galley.rect.min.x *= factor;
        galley.rect.max.x *= factor;
        galley.mesh_bounds.min.x *= factor;
        galley.mesh_bounds.max.x *= factor;
        for placed in &mut galley.rows {
            placed.pos.x *= factor;
            let row = Arc::make_mut(&mut placed.row);
            row.size.x *= factor;
            row.visuals.mesh_bounds.min.x *= factor;
            row.visuals.mesh_bounds.max.x *= factor;
            for vertex in &mut row.visuals.mesh.vertices {
                vertex.pos.x *= factor;
            }
        }
    }
    galley
}

fn paint_cell(
    ui: &egui::Ui,
    rect: egui::Rect,
    cell: &GridCell,
    view: &WindowView,
    settings: &PlayerSettings,
) {
    let style = view.style(cell.style);
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 0.0, color_word(style.background));
    if cell.character == ' ' && cell.hyperlink == 0 {
        return;
    }
    let foreground = if cell.hyperlink != 0 {
        rgb(settings.hyperlink_color)
    } else {
        color_word(style.foreground)
    };
    let mut rich = RichText::new(cell.character)
        .monospace()
        .size(style.font_size)
        .color(foreground);
    if style.oblique {
        rich = rich.italics();
    }
    if cell.hyperlink != 0 {
        rich = rich.underline();
    }
    let mut job = egui::text::LayoutJob::default();
    rich.append_to(
        &mut job,
        ui.style(),
        egui::FontSelection::Default,
        egui::Align::BOTTOM,
    );
    let galley = fit_galley(ui.fonts(|fonts| fonts.layout_job(job)), rect.width());
    let origin = rect.min + (rect.size() - galley.size()) * 0.5;
    if style.weight > 0 {
        // Egui's `RichText::strong` only changes the text color. A second
        // shifted glyph actually increases weight while preserving cell size.
        painter.galley(origin + egui::vec2(0.45, 0.0), galley.clone(), foreground);
    }
    painter.galley(origin, galley, foreground);
}

pub(super) fn show(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    view: &WindowView,
    settings: &PlayerSettings,
    editor: Option<GridEditor<'_>>,
) -> GridResponse {
    let mut result = GridResponse::default();
    ui.painter()
        .rect_filled(rect, 0.0, color_word(view.style(0).background));
    let response = ui.allocate_rect(rect, egui::Sense::click());
    for (index, cell) in view.grid_cells.iter().enumerate() {
        if view.grid_size[0] == 0 {
            break;
        }
        let x = index as u32 % view.grid_size[0];
        let y = index as u32 / view.grid_size[0];
        let rect = cell_rect(rect, x, y);
        if rect.intersects(ui.clip_rect()) {
            paint_cell(ui, rect, cell, view, settings);
        }
    }

    if let Some(pos) = response.hover_pos()
        && let Some([x, y]) = cell_at(rect, view.grid_size, pos)
    {
        let index = y as usize * view.grid_size[0] as usize + x as usize;
        let link = view.grid_cells.get(index).map_or(0, |cell| cell.hyperlink);
        if link != 0 {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if response.clicked() {
            result.cell = Some([x, y]);
            result.hyperlink = (link != 0).then_some(link);
        }
    }

    if let Some(editor) = editor
        && view.grid_cursor[0] < view.grid_size[0]
        && view.grid_cursor[1] < view.grid_size[1]
    {
        if let Some((end, _)) = editor
            .text
            .char_indices()
            .nth(editor.maximum_length as usize)
        {
            editor.text.truncate(end);
            result.changed = true;
        }
        let input_style = view.style(8);
        let start = cell_rect(rect, view.grid_cursor[0], view.grid_cursor[1]);
        let available_cells = view.grid_size[0] - view.grid_cursor[0];
        let input_rect = egui::Rect::from_min_size(
            start.min,
            egui::vec2(
                (available_cells.min(editor.maximum_length.saturating_add(1)) * GRID_CELL_WIDTH)
                    as f32,
                GRID_CELL_HEIGHT as f32,
            ),
        );
        ui.painter()
            .rect_filled(input_rect, 0.0, color_word(input_style.background));
        let response = ui.put(
            input_rect,
            egui::TextEdit::singleline(editor.text)
                .id_source(("grid-line", view.id))
                .font(egui::FontId::monospace(input_style.font_size))
                .text_color(color_word(input_style.foreground))
                .char_limit(editor.maximum_length as usize)
                .frame(false)
                .margin(egui::Vec2::ZERO)
                .desired_width(input_rect.width()),
        );
        result.changed |= response.changed();
        result.submitted = (response.has_focus() || response.lost_focus())
            && ui.input(|input| input.key_pressed(egui::Key::Enter));
        response.request_focus();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> WindowView {
        let mut cells = vec![
            GridCell {
                character: ' ',
                style: 0,
                hyperlink: 0
            };
            30
        ];
        cells[14] = GridCell {
            character: 'X',
            style: 3,
            hyperlink: 99,
        };
        WindowView {
            id: 1,
            kind: 4,
            rect: [0, 0, 80, 48],
            runs: Vec::new(),
            grid: String::new(),
            grid_cells: cells,
            grid_size: [10, 3],
            grid_cursor: [2, 1],
            appearance: crate::vm::TextAppearance::default(),
            hints: [((3, 7), 0x123456), ((3, 8), 0xabcdef), ((3, 9), 1)]
                .into_iter()
                .collect(),
        }
    }

    fn frame(
        context: &egui::Context,
        view: &WindowView,
        events: Vec<egui::Event>,
        editor: Option<GridEditor<'_>>,
    ) -> (egui::FullOutput, GridResponse) {
        let mut editor = editor;
        let mut response = GridResponse::default();
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(320.0, 200.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let rect =
                        egui::Rect::from_min_size(egui::pos2(28.0, 22.0), egui::vec2(80.0, 48.0));
                    response = show(ui, rect, view, &PlayerSettings::default(), editor.take());
                });
            },
        );
        (output, response)
    }

    #[test]
    fn grid_renderer_preserves_style_backgrounds_and_delivers_cell_hyperlink() {
        let context = egui::Context::default();
        let view = view();
        let (output, _) = frame(&context, &view, Vec::new(), None);
        let cell = egui::Rect::from_min_size(egui::pos2(60.0, 38.0), egui::vec2(8.0, 16.0));
        assert!(output.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::epaint::Shape::Rect(rect) if rect.rect == cell && rect.fill == color_word(0x123456))
        }));
        let pointer = |pressed| egui::Event::PointerButton {
            pos: cell.center(),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            &context,
            &view,
            vec![egui::Event::PointerMoved(cell.center()), pointer(true)],
            None,
        );
        let (_, response) = frame(&context, &view, vec![pointer(false)], None);
        assert_eq!(response.cell, Some([4, 1]));
        assert_eq!(response.hyperlink, Some(99));
    }

    #[test]
    fn grid_editor_accepts_input_at_cursor_and_enforces_field_length() {
        let context = egui::Context::default();
        let view = view();
        let mut text = "xy".to_owned();
        frame(
            &context,
            &view,
            Vec::new(),
            Some(GridEditor {
                text: &mut text,
                maximum_length: 3,
            }),
        );
        let (_, response) = frame(
            &context,
            &view,
            vec![egui::Event::Text("1234".to_owned())],
            Some(GridEditor {
                text: &mut text,
                maximum_length: 3,
            }),
        );
        assert!(response.changed);
        assert_eq!(text, "xy1");
        let (_, response) = frame(
            &context,
            &view,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            Some(GridEditor {
                text: &mut text,
                maximum_length: 3,
            }),
        );
        assert!(response.submitted);
    }

    #[test]
    fn grid_hit_coordinates_match_all_rendered_cells_and_exclude_margins() {
        let rect = egui::Rect::from_min_size(egui::pos2(28.0, 22.0), egui::vec2(83.0, 51.0));
        let size = [10, 3];
        for y in 0..size[1] {
            for x in 0..size[0] {
                let cell = cell_rect(rect, x, y);
                assert_eq!(cell_at(rect, size, cell.center()), Some([x, y]));
                assert_eq!(cell_at(rect, size, cell.min), Some([x, y]));
            }
        }
        assert_eq!(cell_at(rect, size, egui::pos2(108.0, 22.0)), None);
        assert_eq!(cell_at(rect, size, egui::pos2(28.0, 70.0)), None);
        assert_eq!(cell_at(rect, size, egui::pos2(27.9, 22.0)), None);
        assert_eq!(cell_at(rect, size, egui::pos2(28.0, 21.9)), None);
        assert_eq!(cell_at(rect, [0, 0], rect.min), None);
    }

    #[test]
    fn wide_glyph_geometry_fits_exactly_inside_one_grid_cell() {
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let original = ui.fonts(|fonts| {
                    fonts.layout_no_wrap(
                        "W".to_owned(),
                        egui::FontId::monospace(32.0),
                        egui::Color32::WHITE,
                    )
                });
                assert!(original.size().x > GRID_CELL_WIDTH as f32);
                let fitted = fit_galley(original.clone(), GRID_CELL_WIDTH as f32);
                assert_eq!(fitted.size().x, GRID_CELL_WIDTH as f32);
                assert_eq!(fitted.size().y, original.size().y);
                assert!(
                    fitted.rows[0].visuals.mesh_bounds.width()
                        < original.rows[0].visuals.mesh_bounds.width()
                );
            });
        });
    }
}

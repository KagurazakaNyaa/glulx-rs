//! Flowing text and images share one layout so resizing preserves Glk's stream order.

use std::{collections::HashMap, sync::Arc};

use eframe::egui::{self, Color32, RichText};

use super::{PlayerSettings, color_word, rgb, texture_image};
use crate::{Vm, vm::WindowView};

#[derive(Default)]
pub(super) struct ImageCache(HashMap<u32, Option<egui::TextureHandle>>);

impl ImageCache {
    pub fn clear(&mut self) {
        self.0.clear();
    }

    fn get(&mut self, ui: &egui::Ui, vm: &Vm, resource: u32) -> Option<&egui::TextureHandle> {
        self.0
            .entry(resource)
            .or_insert_with(|| {
                let bytes = vm.image_resource(resource)?;
                let pixels = crate::picture::decode(bytes).ok()?;
                Some(ui.ctx().load_texture(
                    format!("glk-buffer-image-{resource}"),
                    texture_image(ui.ctx(), &pixels),
                    egui::TextureOptions::LINEAR,
                ))
            })
            .as_ref()
    }
}

enum Paint {
    Text(Arc<egui::Galley>, bool),
    Image(u32),
}

#[derive(Clone, Copy, Debug)]
struct Item {
    index: usize,
    size: egui::Vec2,
    /// Text has a baseline; images use one of Glk's five alignments.
    ascent: Option<f32>,
    alignment: u32,
    hyperlink: u32,
}

#[derive(Clone, Copy, Default)]
struct ParagraphStyle {
    indentation: f32,
    first_indent: f32,
    justification: u32,
}

impl From<crate::vm::ResolvedStyle> for ParagraphStyle {
    fn from(style: crate::vm::ResolvedStyle) -> Self {
        Self {
            indentation: style.indentation as f32,
            first_indent: style.paragraph_indentation as f32,
            justification: style.justification,
        }
    }
}

enum Token {
    Paragraph(ParagraphStyle),
    Word(Vec<Item>),
    Space(Vec<Item>),
    Image(Item),
    Newline,
    FlowBreak,
}

#[derive(Debug)]
struct Placed {
    item: Item,
    rect: egui::Rect,
}

struct Layout {
    width: f32,
    ascent: f32,
    descent: f32,
    y: f32,
    line: Vec<(f32, Item, bool)>,
    paragraph: ParagraphStyle,
    first_line: bool,
    floats: Vec<(u32, egui::Rect)>,
    placed: Vec<Placed>,
}

impl Layout {
    fn new(width: f32, ascent: f32, descent: f32) -> Self {
        Self {
            width: width.max(1.0),
            ascent,
            descent,
            y: 0.0,
            line: Vec::new(),
            paragraph: ParagraphStyle::default(),
            first_line: true,
            floats: Vec::new(),
            placed: Vec::new(),
        }
    }

    fn margin_bounds(&self) -> (f32, f32) {
        self.floats
            .iter()
            .filter(|(_, rect)| rect.bottom() > self.y)
            .fold((0.0_f32, self.width), |(left, right), (side, rect)| {
                if *side == 4 {
                    (left.max(rect.right() + 4.0), right)
                } else {
                    (left, right.min(rect.left() - 4.0))
                }
            })
    }

    fn bounds(&self) -> (f32, f32) {
        let (left, right) = self.margin_bounds();
        let first = if self.first_line {
            self.paragraph.first_indent
        } else {
            0.0
        };
        let left = left + self.paragraph.indentation + first;
        (left, (right - self.paragraph.indentation).max(left + 1.0))
    }

    fn cursor(&self) -> f32 {
        self.line
            .last()
            .map_or_else(|| self.bounds().0, |(x, item, _)| x + item.size.x)
    }

    /// If both margins leave too little room, continue beneath the first ending image.
    fn make_room(&mut self, width: f32, margin: bool) {
        loop {
            let (left, right) = if margin {
                self.margin_bounds()
            } else {
                self.bounds()
            };
            if width <= right - left || !self.floats.iter().any(|(_, rect)| rect.bottom() > self.y)
            {
                return;
            }
            let next = self
                .floats
                .iter()
                .map(|(_, rect)| rect.bottom())
                .filter(|bottom| *bottom > self.y)
                .min_by(f32::total_cmp);
            let Some(next) = next else { return };
            self.y = next;
        }
    }

    fn word(&mut self, items: &[Item], space: bool) {
        let width = items.iter().map(|item| item.size.x).sum::<f32>();
        if width > self.width && items.len() > 1 {
            for item in items {
                self.word(&[*item], space);
            }
            return;
        }
        if self.cursor() + width > self.bounds().1 && !self.line.is_empty() {
            if space {
                // A trailing space cannot create an extra line before a newline.
                return;
            }
            self.end_line(false, false);
        }
        if self.line.is_empty() {
            self.make_room(width, false);
        }
        for item in items {
            let x = self.cursor();
            self.line.push((x, *item, space));
        }
    }

    fn margin(&mut self, item: Item) {
        // A flow-break can become inactive after reflow. Its following margin image
        // must then obey the same start-of-line rule as any other margin image.
        if !self.line.is_empty() {
            return;
        }
        self.make_room(item.size.x, true);
        let (left, right) = self.margin_bounds();
        let x = if item.alignment == 4 {
            left
        } else {
            right - item.size.x
        };
        let rect = egui::Rect::from_min_size(egui::pos2(x, self.y), item.size);
        self.floats.push((item.alignment, rect));
        self.placed.push(Placed { item, rect });
    }

    fn end_line(&mut self, force: bool, last: bool) {
        if self.line.is_empty() {
            if force {
                self.y += self.ascent + self.descent;
            }
            return;
        }
        let ascent = self
            .line
            .iter()
            .filter_map(|(_, item, _)| item.ascent)
            .fold(self.ascent, f32::max);
        let mut top = -ascent;
        let mut bottom = self.descent;
        for (_, item, _) in &self.line {
            let relative_top = item_top(*item, ascent);
            top = top.min(relative_top);
            bottom = bottom.max(relative_top + item.size.y);
        }
        let baseline = self.y - top;
        let content_end = self
            .line
            .iter()
            .rposition(|(_, _, space)| !space)
            .map(|i| self.line[i].0 + self.line[i].1.size.x)
            .unwrap_or_else(|| self.bounds().0);
        let (left, right) = self.bounds();
        let remaining = (right - content_end).max(0.0);
        let shift = match self.paragraph.justification {
            2 => remaining / 2.0,
            3 => remaining,
            _ => 0.0,
        };
        let gaps = self
            .line
            .iter()
            .filter(|(x, _, space)| *space && *x > left && *x < content_end)
            .count();
        let expansion = if self.paragraph.justification == 1 && !last && gaps > 0 {
            remaining / gaps as f32
        } else {
            0.0
        };
        let mut offset = shift;
        for (x, mut item, space) in self.line.drain(..) {
            let position = x + offset;
            if space && x > left && x < content_end {
                item.size.x += expansion;
                offset += expansion;
            }
            let rect = egui::Rect::from_min_size(
                egui::pos2(position, baseline + item_top(item, ascent)),
                item.size,
            );
            self.placed.push(Placed { item, rect });
        }
        self.y += bottom - top;
        self.first_line = false;
    }

    fn flow_break(&mut self) {
        let bottom = self
            .floats
            .iter()
            .map(|(_, rect)| rect.bottom())
            .fold(self.y, f32::max);
        if bottom > self.y {
            self.end_line(false, true);
            self.y = self.y.max(bottom);
        }
    }

    fn format(mut self, tokens: &[Token]) -> (Vec<Placed>, f32) {
        let mut empty_tail = false;
        for token in tokens {
            match token {
                Token::Paragraph(style) => {
                    self.paragraph = *style;
                    self.first_line = true;
                }
                Token::Word(items) => {
                    self.word(items, false);
                    empty_tail = false;
                }
                Token::Space(items) => {
                    self.word(items, true);
                    empty_tail = false;
                }
                Token::Image(item) if item.size.x == 0.0 || item.size.y == 0.0 => {}
                Token::Image(item) if item.alignment >= 4 => self.margin(*item),
                Token::Image(item) => {
                    self.word(&[*item], false);
                    empty_tail = false;
                }
                Token::Newline => {
                    self.end_line(true, true);
                    self.first_line = true;
                    empty_tail = true;
                }
                Token::FlowBreak => {
                    let old_y = self.y;
                    self.flow_break();
                    empty_tail |= self.y != old_y;
                }
            }
        }
        self.end_line(false, true);
        if empty_tail {
            self.y += self.ascent + self.descent;
        }
        let height = self
            .floats
            .iter()
            .map(|(_, rect)| rect.bottom())
            .fold(self.y, f32::max);
        (self.placed, height)
    }
}

fn item_top(item: Item, text_ascent: f32) -> f32 {
    if let Some(ascent) = item.ascent {
        -ascent
    } else {
        match item.alignment {
            1 => -item.size.y,
            2 => -text_ascent,
            _ => -(text_ascent + item.size.y) / 2.0,
        }
    }
}

fn rich_text(
    text: &str,
    run: &crate::vm::TextRun,
    view: &WindowView,
    settings: &PlayerSettings,
) -> RichText {
    let style = view.style(run.style);
    let mut rich = RichText::new(text)
        .size(style.font_size)
        .color(color_word(style.foreground))
        .background_color(color_word(style.background));
    if style.oblique {
        rich = rich.italics();
    }
    rich = rich.family(super::fonts::family(style));
    if run.hyperlink != 0 {
        rich = rich.color(rgb(settings.hyperlink_color)).underline();
    }
    rich
}

fn measure(ui: &egui::Ui, rich: RichText) -> (Arc<egui::Galley>, f32) {
    measure_width(ui, rich, f32::INFINITY)
}

fn measure_width(ui: &egui::Ui, rich: RichText, width: f32) -> (Arc<egui::Galley>, f32) {
    let mut job = egui::text::LayoutJob::default();
    rich.append_to(
        &mut job,
        ui.style(),
        egui::FontSelection::Default,
        egui::Align::BOTTOM,
    );
    job.wrap.max_width = width;
    let galley = ui.fonts(|fonts| fonts.layout_job(job));
    let ascent = galley
        .rows
        .first()
        .and_then(|row| row.glyphs.first().map(|glyph| row.pos.y + glyph.pos.y))
        .unwrap_or(galley.size().y * 0.8);
    (galley, ascent)
}

/// CJK scripts have ordinary break opportunities between characters, without spaces.
fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x2e80..=0x9fff | 0xac00..=0xd7af | 0xf900..=0xfaff | 0x20000..=0x3134f)
}

fn prepare(
    ui: &egui::Ui,
    view: &WindowView,
    settings: &PlayerSettings,
    width: f32,
) -> (Vec<Token>, Vec<Paint>, f32, f32) {
    let (base, ascent) = measure(ui, RichText::new("M").size(settings.font_size));
    let descent = (base.size().y - ascent).max(0.0);
    let mut tokens = Vec::new();
    let mut paints = Vec::new();
    let mut paragraph_start = true;
    for run in &view.runs {
        if paragraph_start && (!run.text.is_empty() || run.image.is_some()) {
            tokens.push(Token::Paragraph(view.style(run.style).into()));
            paragraph_start = false;
        }
        if run.flow_break {
            tokens.push(Token::FlowBreak);
        }
        if let Some(image) = &run.image {
            let size = image.dimensions(width.max(1.0) as u32);
            if size[0] != 0 && size[1] != 0 {
                let item = Item {
                    index: paints.len(),
                    size: egui::vec2(size[0] as f32, size[1] as f32),
                    ascent: None,
                    alignment: image.alignment,
                    hyperlink: run.hyperlink,
                };
                paints.push(Paint::Image(image.resource));
                tokens.push(Token::Image(item));
            }
        }
        let mut start = 0;
        while start < run.text.len() {
            let first = run.text[start..].chars().next().unwrap();
            if first == '\n' {
                tokens.push(Token::Newline);
                start += 1;
                paragraph_start = true;
                continue;
            }
            if paragraph_start {
                tokens.push(Token::Paragraph(view.style(run.style).into()));
                paragraph_start = false;
            }
            let space = first.is_whitespace();
            let mut end = start + first.len_utf8();
            if !is_cjk(first) {
                for ch in run.text[end..].chars() {
                    if ch == '\n' || ch.is_whitespace() != space || is_cjk(ch) {
                        break;
                    }
                    end += ch.len_utf8();
                }
            }
            let (galley, ascent) =
                measure(ui, rich_text(&run.text[start..end], run, view, settings));
            let fragments = if galley.size().x > width {
                // Long unbroken words still have to remain readable in a narrow window.
                // Let the font layout choose emergency breaks, preserving combining glyphs.
                let (wrapped, _) = measure_width(
                    ui,
                    rich_text(&run.text[start..end], run, view, settings),
                    width,
                );
                wrapped
                    .rows
                    .iter()
                    .map(|row| {
                        let text: String = row.glyphs.iter().map(|glyph| glyph.chr).collect();
                        measure(ui, rich_text(&text, run, view, settings))
                    })
                    .collect::<Vec<_>>()
            } else {
                vec![(galley, ascent)]
            };
            // Styling or hyperlink changes inside a word must not create wrap opportunities.
            // CJK glyphs, unlike Latin word fragments, each allow a break.
            let merge = start == 0 && !is_cjk(first) && fragments.len() == 1;
            for (galley, ascent) in fragments {
                let item = Item {
                    index: paints.len(),
                    size: galley.size(),
                    ascent: Some(ascent),
                    alignment: 0,
                    hyperlink: run.hyperlink,
                };
                paints.push(Paint::Text(galley, view.style(run.style).weight > 0));
                match tokens.last_mut() {
                    Some(Token::Word(items)) if merge && !space => items.push(item),
                    Some(Token::Space(items)) if merge && space => items.push(item),
                    _ if space => tokens.push(Token::Space(vec![item])),
                    _ => tokens.push(Token::Word(vec![item])),
                }
            }
            start = end;
        }
    }
    (tokens, paints, ascent, descent)
}

pub(super) fn show(
    ui: &mut egui::Ui,
    view: &WindowView,
    settings: &PlayerSettings,
    vm: &Vm,
    images: &mut ImageCache,
) -> Option<u32> {
    let width = ui.available_width().max(1.0);
    let (tokens, paints, ascent, descent) = prepare(ui, view, settings, width);
    let (placed, height) = Layout::new(width, ascent, descent).format(&tokens);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let mut hyperlink = None;
    for (index, placed) in placed.iter().enumerate() {
        let destination = placed.rect.translate(rect.min.to_vec2());
        if !ui.is_rect_visible(destination) {
            continue;
        }
        match &paints[placed.item.index] {
            Paint::Text(galley, bold) => {
                if let Some(section) = galley.job.sections.first() {
                    ui.painter()
                        .rect_filled(destination, 0.0, section.format.background);
                }
                ui.painter()
                    .galley(destination.min, galley.clone(), Color32::WHITE);
                if *bold {
                    ui.painter().galley(
                        destination.min + egui::vec2(0.45, 0.0),
                        galley.clone(),
                        Color32::WHITE,
                    );
                }
            }
            Paint::Image(resource) => {
                if let Some(texture) = images.get(ui, vm, *resource) {
                    ui.painter().image(
                        texture.id(),
                        destination,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
            }
        }
        if placed.item.hyperlink != 0
            && ui
                .interact(
                    destination,
                    ui.id().with(("buffer-link", index)),
                    egui::Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
        {
            hyperlink = Some(placed.item.hyperlink);
        }
    }
    hyperlink
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(index: usize, width: f32) -> Token {
        Token::Word(vec![Item {
            index,
            size: egui::vec2(width, 20.0),
            ascent: Some(15.0),
            alignment: 0,
            hyperlink: 0,
        }])
    }

    fn picture(index: usize, width: f32, height: f32, alignment: u32) -> Token {
        Token::Image(Item {
            index,
            size: egui::vec2(width, height),
            ascent: None,
            alignment,
            hyperlink: 42,
        })
    }

    fn layout(tokens: &[Token], width: f32) -> (Vec<Placed>, f32) {
        Layout::new(width, 15.0, 5.0).format(tokens)
    }

    fn rect(items: &[Placed], index: usize) -> egui::Rect {
        items
            .iter()
            .find(|item| item.item.index == index)
            .unwrap()
            .rect
    }

    fn paragraph(indentation: f32, first_indent: f32, justification: u32) -> Token {
        Token::Paragraph(ParagraphStyle {
            indentation,
            first_indent,
            justification,
        })
    }
    fn space(index: usize, width: f32) -> Token {
        let Token::Word(items) = text(index, width) else {
            unreachable!()
        };
        Token::Space(items)
    }

    #[test]
    fn paragraph_alignment_and_hanging_indent_use_available_line_width() {
        for (align, expected) in [(0, 10.0), (2, 35.0), (3, 60.0)] {
            let (items, _) = layout(&[paragraph(10.0, 0.0, align), text(0, 30.0)], 100.0);
            assert_eq!(rect(&items, 0).left(), expected);
        }
        let (items, _) = layout(
            &[
                paragraph(20.0, -10.0, 0),
                text(0, 60.0),
                text(1, 50.0),
                Token::Newline,
                text(2, 10.0),
            ],
            100.0,
        );
        assert_eq!(rect(&items, 0).left(), 10.0);
        assert_eq!(rect(&items, 1).left(), 20.0);
        assert_eq!(rect(&items, 2).left(), 10.0);
    }

    #[test]
    fn full_justification_expands_only_wrapped_lines_and_respects_margins() {
        let (items, _) = layout(
            &[
                paragraph(10.0, 10.0, 1),
                text(0, 30.0),
                space(1, 5.0),
                text(2, 30.0),
                space(3, 5.0),
                text(4, 30.0),
            ],
            100.0,
        );
        assert_eq!(rect(&items, 0).left(), 20.0);
        assert_eq!(rect(&items, 2).right(), 90.0);
        assert_eq!(rect(&items, 4).left(), 10.0);
        let (items, _) = layout(
            &[
                paragraph(8.0, 0.0, 3),
                picture(0, 20.0, 80.0, 5),
                text(1, 20.0),
            ],
            100.0,
        );
        assert_eq!(rect(&items, 1).right(), 68.0);
        assert_eq!(rect(&items, 0).right(), 100.0);
    }

    #[test]
    fn inline_images_share_the_text_baseline_and_expand_the_line() {
        for alignment in 1..=3 {
            let (items, height) = layout(
                &[
                    text(0, 30.0),
                    picture(1, 25.0, 50.0, alignment),
                    text(2, 30.0),
                ],
                200.0,
            );
            let text = rect(&items, 0);
            let image = rect(&items, 1);
            let baseline = text.top() + 15.0;
            match alignment {
                1 => assert_eq!(image.bottom(), baseline),
                2 => assert_eq!(image.top(), text.top()),
                3 => assert_eq!(image.center().y, (text.top() + baseline) / 2.0),
                _ => unreachable!(),
            }
            assert_eq!(image.left(), text.right());
            assert_eq!(rect(&items, 2).top(), text.top());
            assert!(height >= image.bottom());
            assert_eq!(items[1].item.hyperlink, 42);
        }
    }

    #[test]
    fn text_wraps_beside_both_margins_and_returns_to_full_width() {
        let mut tokens = vec![picture(0, 25.0, 50.0, 4), picture(1, 25.0, 30.0, 5)];
        for index in 2..7 {
            tokens.push(text(index, 40.0));
            tokens.push(Token::Newline);
        }
        let (items, _) = layout(&tokens, 120.0);
        assert_eq!(rect(&items, 0).left(), 0.0);
        assert_eq!(rect(&items, 1).right(), 120.0);
        assert_eq!(rect(&items, 2).min, egui::pos2(29.0, 0.0));
        assert_eq!(rect(&items, 4).min, egui::pos2(29.0, 40.0));
        assert_eq!(rect(&items, 5).left(), 0.0);
        assert!(rect(&items, 2).right() < rect(&items, 1).left());
    }

    #[test]
    fn repeated_margin_images_do_not_overlap_and_flow_break_clears_all() {
        let tokens = [
            picture(0, 20.0, 80.0, 4),
            picture(1, 20.0, 60.0, 4),
            picture(2, 20.0, 40.0, 5),
            text(3, 25.0),
            Token::FlowBreak,
            text(4, 40.0),
        ];
        let (items, _) = layout(&tokens, 120.0);
        assert_eq!(rect(&items, 1).left(), 24.0);
        assert_eq!(rect(&items, 3).left(), 48.0);
        assert_eq!(rect(&items, 4).min, egui::pos2(0.0, 80.0));
        assert!(!rect(&items, 0).intersects(rect(&items, 1)));
    }

    #[test]
    fn resizing_reflows_words_and_changes_whether_a_flow_break_is_active() {
        let tokens = [
            picture(0, 30.0, 60.0, 4),
            text(1, 40.0),
            text(2, 40.0),
            text(3, 40.0),
            Token::FlowBreak,
            text(4, 40.0),
        ];
        let (wide, _) = layout(&tokens, 200.0);
        let (narrow, _) = layout(&tokens, 80.0);
        assert_eq!(rect(&wide, 1).top(), rect(&wide, 3).top());
        assert_eq!(rect(&narrow, 3).top(), 40.0);
        assert_eq!(rect(&wide, 4).min, egui::pos2(0.0, 60.0));
        assert_eq!(rect(&narrow, 4).min, egui::pos2(0.0, 60.0));
        let (without_margin, _) = layout(&[text(0, 30.0), Token::FlowBreak, text(1, 30.0)], 100.0);
        assert_eq!(
            rect(&without_margin, 0).top(),
            rect(&without_margin, 1).top()
        );
    }

    #[test]
    fn squeezed_text_moves_below_images_and_inline_images_wrap_as_units() {
        let tokens = [
            picture(0, 40.0, 60.0, 4),
            picture(1, 40.0, 35.0, 5),
            text(2, 40.0),
            picture(3, 65.0, 30.0, 1),
        ];
        let (items, _) = layout(&tokens, 100.0);
        assert_eq!(rect(&items, 2).min, egui::pos2(44.0, 35.0));
        assert!(rect(&items, 3).top() >= 60.0);
        assert_eq!(rect(&items, 3).left(), 0.0);
    }

    #[test]
    fn newlines_preserve_blank_lines_and_the_empty_final_line() {
        let (items, height) = layout(
            &[
                text(0, 30.0),
                Token::Newline,
                Token::Newline,
                text(1, 30.0),
                Token::Newline,
            ],
            100.0,
        );
        assert_eq!(rect(&items, 1).top(), 40.0);
        assert_eq!(height, 80.0);
        assert_eq!(layout(&[Token::Newline, Token::Newline], 100.0).1, 60.0);
        let Token::Word(space) = text(1, 10.0) else {
            unreachable!()
        };
        assert_eq!(
            layout(
                &[text(0, 100.0), Token::Space(space), Token::Newline],
                100.0
            )
            .1,
            40.0
        );
    }

    #[test]
    fn zero_sized_images_do_not_advance_or_indent_the_stream() {
        for alignment in 1..=5 {
            for size in [[0.0, 50.0], [50.0, 0.0]] {
                let tokens = [
                    picture(0, size[0], size[1], alignment),
                    text(1, 20.0),
                    Token::FlowBreak,
                    text(2, 20.0),
                ];
                let (items, height) = layout(&tokens, 100.0);
                assert_eq!(items.len(), 2);
                assert_eq!(rect(&items, 1).min, egui::Pos2::ZERO);
                assert_eq!(rect(&items, 2).min, egui::pos2(20.0, 0.0));
                assert_eq!(height, 20.0);
            }
        }
    }

    #[test]
    fn expired_flow_break_does_not_allow_a_margin_image_inside_text() {
        let tokens = [
            picture(0, 20.0, 10.0, 4),
            text(1, 40.0),
            Token::Newline,
            text(2, 40.0),
            Token::FlowBreak,
            picture(3, 20.0, 20.0, 4),
            text(4, 40.0),
        ];
        let (items, _) = layout(&tokens, 120.0);
        assert!(items.iter().all(|item| item.item.index != 3));
        assert_eq!(rect(&items, 2).top(), rect(&items, 4).top());
    }

    fn run(text: &str, style: u32) -> crate::vm::TextRun {
        crate::vm::TextRun {
            text: text.to_owned(),
            style,
            hyperlink: 0,
            image: None,
            flow_break: false,
        }
    }

    #[test]
    fn font_layout_keeps_style_fragments_in_one_word_and_wraps_long_words() {
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                let mut view = WindowView {
                    id: 1,
                    kind: 3,
                    rect: [0, 0, 100, 100],
                    runs: vec![run("sec", 0), run("ret", 1)],
                    grid: String::new(),
                    grid_cells: Vec::new(),
                    grid_size: [0, 0],
                    grid_cursor: [0, 0],
                    appearance: Default::default(),
                    hints: Default::default(),
                };
                let settings = PlayerSettings::default();
                let (tokens, _, _, _) = prepare(ui, &view, &settings, 200.0);
                assert!(matches!(&tokens[..], [Token::Paragraph(_), Token::Word(items)] if items.len() == 2));
                view.runs = vec![run(
                    "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz",
                    0,
                )];
                let (tokens, paints, ascent, descent) = prepare(ui, &view, &settings, 70.0);
                let (items, _) = Layout::new(70.0, ascent, descent).format(&tokens);
                assert!(items.len() > 2);
                assert!(items.iter().all(|item| item.rect.right() <= 70.1));
                let reconstructed: String = paints
                    .iter()
                    .filter_map(|paint| match paint {
                        Paint::Text(galley, _) => Some(galley.text()),
                        _ => None,
                    })
                    .collect();
                assert_eq!(reconstructed, view.runs[0].text);
            });
        });
    }

    #[test]
    fn image_rules_recompute_on_resize_and_unlimited_images_can_be_clipped() {
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                let mut image = run("", 0);
                image.image = Some(crate::vm::BufferImage {
                    resource: 1,
                    original: [300, 150],
                    alignment: 1,
                    size: [32768, 65536],
                    rule: 3 | 12,
                    max_width: 65536,
                });
                let mut view = WindowView {
                    id: 1,
                    kind: 3,
                    rect: [0, 0, 200, 200],
                    runs: vec![image],
                    grid: String::new(),
                    grid_cells: Vec::new(),
                    grid_size: [0, 0],
                    grid_cursor: [0, 0],
                    appearance: Default::default(),
                    hints: Default::default(),
                };
                let settings = PlayerSettings::default();
                for width in [200.0, 100.0] {
                    let (tokens, _, ascent, descent) = prepare(ui, &view, &settings, width);
                    let (items, _) = Layout::new(width, ascent, descent).format(&tokens);
                    assert_eq!(items[0].rect.width(), width / 2.0);
                    assert_eq!(items[0].rect.height(), width / 4.0);
                }
                let image = view.runs[0].image.as_mut().unwrap();
                image.rule = 1 | 4;
                image.max_width = 0;
                let (tokens, _, ascent, descent) = prepare(ui, &view, &settings, 100.0);
                let (items, height) = Layout::new(100.0, ascent, descent).format(&tokens);
                assert_eq!(items[0].rect.width(), 300.0);
                assert_eq!(items[0].rect.height(), 150.0);
                let clip = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, height));
                assert_eq!(items[0].rect.intersect(clip).width(), 100.0);
            });
        });
    }
}

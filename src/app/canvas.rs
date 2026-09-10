//! Retained graphics primitives: OpenGL performs normal scaling and blending.
//! CPU rasterization is reserved for snapshots and bounded-history compaction.
use super::*;
use std::sync::Arc;

type Clip = [i64; 4];
const MAX_OPERATIONS: usize = 1024;

pub(super) struct ImageAsset {
    pub pixels: Arc<image::RgbaImage>,
    texture: egui::TextureHandle,
    opaque: bool,
}
impl ImageAsset {
    pub fn byte_len(&self) -> usize {
        self.pixels.len() + self.texture.size()[0] * self.texture.size()[1] * 4
    }

    pub fn new(context: &egui::Context, pixels: Arc<image::RgbaImage>) -> Arc<Self> {
        let opaque = pixels.pixels().all(|pixel| pixel[3] == 255);
        let texture = context.load_texture(
            "glk-picture",
            texture_image(context, &pixels),
            egui::TextureOptions::LINEAR,
        );
        Arc::new(Self {
            pixels,
            texture,
            opaque,
        })
    }
}

#[derive(Clone)]
enum Operation {
    Fill {
        clip: Clip,
        color: u32,
    },
    Image {
        clip: Clip,
        source: Arc<ImageAsset>,
        position: [i32; 2],
        size: [u32; 2],
    },
}

#[derive(Clone, Copy)]
struct LinkRegion {
    clip: Clip,
    hyperlink: u32,
}
impl Operation {
    fn clip(&self) -> Clip {
        match self {
            Self::Fill { clip, .. } | Self::Image { clip, .. } => *clip,
        }
    }
    fn crop(&mut self, bounds: Clip) {
        match self {
            Self::Fill { clip, .. } | Self::Image { clip, .. } => *clip = intersect(*clip, bounds),
        }
    }
}

#[derive(Clone)]
pub(super) struct Canvas {
    pub size: [u32; 2],
    background: u32,
    operations: Vec<Operation>,
    cpu_pixels: Option<Arc<image::RgbaImage>>,
    cpu_texture: Option<egui::TextureHandle>,
    cpu_dirty: bool,
    links: Vec<LinkRegion>,
}
impl Canvas {
    pub fn new(size: [u32; 2], background: u32) -> Self {
        let size = size.map(|side| side.max(1));
        Self {
            size,
            background,
            operations: vec![Operation::Fill {
                clip: [0, 0, size[0] as i64, size[1] as i64],
                color: background,
            }],
            cpu_pixels: None,
            cpu_texture: None,
            cpu_dirty: false,
            links: vec![LinkRegion {
                clip: [0, 0, size[0] as i64, size[1] as i64],
                hyperlink: 0,
            }],
        }
    }
    pub fn use_cpu(&mut self, enabled: bool) {
        if enabled && self.cpu_pixels.is_none() {
            self.cpu_pixels = Some(Arc::new(self.rasterize()));
            self.cpu_dirty = true;
        }
    }
    pub fn prepare(&mut self, context: &egui::Context) {
        if self.cpu_dirty {
            if let Some(pixels) = &self.cpu_pixels {
                let image = texture_image(context, pixels);
                if let Some(texture) = &mut self.cpu_texture {
                    texture.set(image, egui::TextureOptions::LINEAR);
                } else {
                    self.cpu_texture = Some(context.load_texture(
                        "glk-software-canvas",
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                }
            }
            self.cpu_dirty = false;
        }
    }
    fn bounds(&self) -> Clip {
        [0, 0, self.size[0] as i64, self.size[1] as i64]
    }
    pub fn from_pixels(context: &egui::Context, pixels: image::RgbaImage) -> Self {
        let mut canvas = Self::new([pixels.width(), pixels.height()], 0xffffff);
        let source = ImageAsset::new(context, Arc::new(pixels));
        canvas.draw(context, source, [0, 0], canvas.size);
        canvas
    }
    pub fn resize(&mut self, size: [u32; 2], background: u32) {
        let size = size.map(|side| side.max(1));
        if size == self.size {
            return;
        }
        if let Some(pixels) = &mut self.cpu_pixels {
            let mut resized = image::RgbaImage::from_pixel(size[0], size[1], rgba(background));
            image::imageops::overlay(&mut resized, pixels.as_ref(), 0, 0);
            *pixels = Arc::new(resized);
            self.cpu_dirty = true;
        }
        self.size = size;
        self.background = background;
        let bounds = self.bounds();
        for operation in &mut self.operations {
            operation.crop(bounds);
        }
        self.operations
            .retain(|operation| nonempty(operation.clip()));
        for region in &mut self.links {
            region.clip = intersect(region.clip, bounds);
        }
        self.links.retain(|region| nonempty(region.clip));
    }
    pub fn clear(&mut self, color: u32) {
        if let Some(pixels) = &mut self.cpu_pixels {
            for pixel in Arc::make_mut(pixels).pixels_mut() {
                *pixel = rgba(color);
            }
            self.cpu_dirty = true;
        }
        self.background = color;
        self.operations.clear();
        self.operations.push(Operation::Fill {
            clip: self.bounds(),
            color,
        });
        self.links.clear();
        self.links.push(LinkRegion {
            clip: self.bounds(),
            hyperlink: 0,
        });
    }
    pub fn fill(&mut self, context: &egui::Context, rect: [i32; 4], color: u32) {
        if let Some(pixels) = &mut self.cpu_pixels {
            fill_rect(Arc::make_mut(pixels), rect, color);
            self.cpu_dirty = true;
        }
        let clip = intersect(
            [
                rect[0] as i64,
                rect[1] as i64,
                rect[0] as i64 + rect[2] as u32 as i64,
                rect[1] as i64 + rect[3] as u32 as i64,
            ],
            self.bounds(),
        );
        if nonempty(clip) {
            self.links.push(LinkRegion { clip, hyperlink: 0 });
            self.push(context, Operation::Fill { clip, color }, true);
        }
    }
    pub fn draw(
        &mut self,
        context: &egui::Context,
        source: Arc<ImageAsset>,
        position: [i32; 2],
        size: [u32; 2],
    ) {
        self.draw_hyperlinked(context, source, position, size, 0);
    }
    pub fn draw_hyperlinked(
        &mut self,
        context: &egui::Context,
        source: Arc<ImageAsset>,
        position: [i32; 2],
        size: [u32; 2],
        hyperlink: u32,
    ) {
        if let Some(pixels) = &mut self.cpu_pixels {
            crate::picture::draw_scaled(Arc::make_mut(pixels), &source.pixels, position, size);
            self.cpu_dirty = true;
        }
        let clip = intersect(
            [
                position[0] as i64,
                position[1] as i64,
                position[0] as i64 + size[0] as i64,
                position[1] as i64 + size[1] as i64,
            ],
            self.bounds(),
        );
        if nonempty(clip) {
            self.links.push(LinkRegion { clip, hyperlink });
            let opaque = source.opaque;
            self.push(
                context,
                Operation::Image {
                    clip,
                    source,
                    position,
                    size,
                },
                opaque,
            );
        }
    }
    pub fn hyperlink_at(&self, position: [u32; 2]) -> Option<u32> {
        let point = [position[0] as i64, position[1] as i64];
        if point[0] >= self.size[0] as i64 || point[1] >= self.size[1] as i64 {
            return None;
        }
        self.links
            .iter()
            .rev()
            .find(|region| {
                point[0] >= region.clip[0]
                    && point[0] < region.clip[2]
                    && point[1] >= region.clip[1]
                    && point[1] < region.clip[3]
            })
            .map(|region| region.hyperlink)
    }
    fn push(&mut self, context: &egui::Context, operation: Operation, opaque: bool) {
        if opaque {
            let cover = operation.clip();
            self.operations.retain(|old| !contains(cover, old.clip()));
        }
        self.operations.push(operation);
        if self.operations.len() > MAX_OPERATIONS {
            crate::diagnostics::record(format_args!("graphics: compact retained canvas"));
            let pixels = self.rasterize();
            self.operations.clear();
            let source = ImageAsset::new(context, Arc::new(pixels));
            self.operations.push(Operation::Image {
                clip: self.bounds(),
                source,
                position: [0, 0],
                size: self.size,
            });
        }
    }
    pub fn paint(&self, painter: &egui::Painter, origin: egui::Pos2) {
        let rect = |clip: Clip| {
            egui::Rect::from_min_max(
                origin + egui::vec2(clip[0] as f32, clip[1] as f32),
                origin + egui::vec2(clip[2] as f32, clip[3] as f32),
            )
        };
        if let Some(texture) = &self.cpu_texture {
            painter.image(
                texture.id(),
                rect(self.bounds()),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            return;
        }
        painter.rect_filled(rect(self.bounds()), 0.0, color_word(self.background));
        for operation in &self.operations {
            match operation {
                Operation::Fill { clip, color } => {
                    painter.rect_filled(rect(*clip), 0.0, color_word(*color));
                }
                Operation::Image {
                    clip,
                    source,
                    position,
                    size,
                } => {
                    // Clip destination coordinates first; even huge unsigned Glk
                    // extents produce small quads and bounded UV calculations.
                    let uv = |x: i64, y: i64| {
                        egui::pos2(
                            ((x - position[0] as i64) as f64 / size[0] as f64) as f32,
                            ((y - position[1] as i64) as f64 / size[1] as f64) as f32,
                        )
                    };
                    painter.image(
                        source.texture.id(),
                        rect(*clip),
                        egui::Rect::from_min_max(uv(clip[0], clip[1]), uv(clip[2], clip[3])),
                        Color32::WHITE,
                    );
                }
            }
        }
    }
    pub fn rasterize(&self) -> image::RgbaImage {
        if let Some(pixels) = &self.cpu_pixels {
            return pixels.as_ref().clone();
        }
        let mut pixels =
            image::RgbaImage::from_pixel(self.size[0], self.size[1], rgba(self.background));
        for operation in &self.operations {
            match operation {
                Operation::Fill { clip, color } => fill_rect(
                    &mut pixels,
                    [
                        clip[0] as i32,
                        clip[1] as i32,
                        (clip[2] - clip[0]) as i32,
                        (clip[3] - clip[1]) as i32,
                    ],
                    *color,
                ),
                Operation::Image {
                    clip,
                    source,
                    position,
                    size,
                } => {
                    if *clip == self.bounds() {
                        crate::picture::draw_scaled(&mut pixels, &source.pixels, *position, *size);
                    } else {
                        crate::picture::draw_scaled_clipped(
                            &mut pixels,
                            &source.pixels,
                            *position,
                            *size,
                            clip.map(|v| v as u32),
                        );
                    }
                }
            }
        }
        pixels
    }
}
fn intersect(a: Clip, b: Clip) -> Clip {
    [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ]
}
fn nonempty(clip: Clip) -> bool {
    clip[0] < clip[2] && clip[1] < clip[3]
}
fn contains(a: Clip, b: Clip) -> bool {
    a[0] <= b[0] && a[1] <= b[1] && a[2] >= b[2] && a[3] >= b[3]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_crop_is_permanent_and_snapshot_blending_matches_the_canvas() {
        for cpu in [false, true] {
            let context = egui::Context::default();
            let pixels = Arc::new(image::RgbaImage::from_pixel(
                4,
                2,
                image::Rgba([255, 0, 0, 128]),
            ));
            let source = ImageAsset::new(&context, pixels);
            let mut canvas = Canvas::new([4, 2], 0x0000ff);
            canvas.use_cpu(cpu);
            canvas.draw(&context, source, [0, 0], [4, 2]);
            canvas.resize([2, 1], 0);
            canvas.resize([4, 2], 0x00ff00);
            let result = canvas.rasterize();
            assert_eq!(result.get_pixel(0, 0).0, [128, 0, 127, 255]);
            assert_eq!(result.get_pixel(3, 0).0, [0, 255, 0, 255]);
            assert_eq!(result.get_pixel(0, 1).0, [0, 255, 0, 255]);
        }
    }

    #[test]
    fn opaque_updates_replace_covered_commands_and_history_is_bounded() {
        let context = egui::Context::default();
        let mut canvas = Canvas::new([4, 2], 0);
        for _ in 0..100 {
            canvas.fill(&context, [0, 0, 2, 1], 0xffffff);
        }
        assert_eq!(canvas.operations.len(), 2);
        let source = ImageAsset::new(
            &context,
            Arc::new(image::RgbaImage::from_pixel(
                1,
                1,
                image::Rgba([255, 0, 0, 1]),
            )),
        );
        for _ in 0..MAX_OPERATIONS {
            canvas.draw(&context, source.clone(), [0, 0], [1, 1]);
        }
        assert!(canvas.operations.len() < MAX_OPERATIONS);
        assert_eq!(canvas.rasterize().get_pixel(3, 1).0, [0, 0, 0, 255]);
    }

    #[test]
    fn hyperlink_hit_testing_tracks_images_and_opaque_fills() {
        let context = egui::Context::default();
        let source = ImageAsset::new(
            &context,
            Arc::new(image::RgbaImage::from_pixel(
                2,
                2,
                image::Rgba([255, 0, 0, 255]),
            )),
        );
        let mut canvas = Canvas::new([8, 8], 0);
        canvas.draw_hyperlinked(&context, source.clone(), [1, 1], [4, 4], 42);
        assert_eq!(canvas.hyperlink_at([2, 2]), Some(42));
        assert_eq!(canvas.hyperlink_at([0, 0]), Some(0));
        canvas.fill(&context, [2, 2, 2, 2], 0xffffff);
        assert_eq!(canvas.hyperlink_at([2, 2]), Some(0));
        assert_eq!(canvas.hyperlink_at([1, 1]), Some(42));
        assert_eq!(canvas.hyperlink_at([99, 99]), None);
    }
}

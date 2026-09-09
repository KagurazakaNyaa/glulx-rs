//! Decode pictures consistently and rasterize only visible destination pixels.
use std::io::Cursor;

use image::{ImageReader, RgbaImage};

// Glk permits unavailable resources to fail. Bound decoded resources, not the
// requested draw size: a huge scaled image may have a tiny visible portion.
const MAX_PIXELS: u64 = 16 * 1024 * 1024;

pub(crate) fn decode(data: &[u8]) -> image::ImageResult<RgbaImage> {
    let reader = || ImageReader::new(Cursor::new(data)).with_guessed_format();
    let (width, height) = reader()?.into_dimensions()?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(image::ImageError::Limits(
            image::error::LimitError::from_kind(image::error::LimitErrorKind::InsufficientMemory),
        ));
    }
    let mut reader = reader()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_PIXELS * 8);
    reader.limits(limits);
    Ok(reader.decode()?.to_rgba8())
}

pub(crate) fn draw_scaled(
    canvas: &mut RgbaImage,
    source: &RgbaImage,
    position: [i32; 2],
    size: [u32; 2],
) {
    let clip = [0, 0, canvas.width(), canvas.height()];
    draw_scaled_clipped(canvas, source, position, size, clip);
}

pub(crate) fn draw_scaled_clipped(
    canvas: &mut RgbaImage,
    source: &RgbaImage,
    position: [i32; 2],
    size: [u32; 2],
    clip: [u32; 4],
) {
    if size.contains(&0) || source.width() == 0 || source.height() == 0 {
        return;
    }
    let [left, top] = position.map(i64::from);
    let right = (left + i64::from(size[0]))
        .clamp(0, i64::from(canvas.width()))
        .min(clip[2] as i64);
    let bottom = (top + i64::from(size[1]))
        .clamp(0, i64::from(canvas.height()))
        .min(clip[3] as i64);
    // Bilinear samples use source pixel centers. Work and memory depend on the
    // clipped canvas, even for unsigned extents such as 0xFFFFFFFF.
    for y in top.max(clip[1] as i64)..bottom {
        let sy = ((y - top) as f64 + 0.5) * f64::from(source.height()) / f64::from(size[1]) - 0.5;
        let sy = sy.clamp(0.0, f64::from(source.height() - 1));
        let y0 = sy.floor() as u32;
        let fy = sy - f64::from(y0);
        for x in left.max(clip[0] as i64)..right {
            let sx =
                ((x - left) as f64 + 0.5) * f64::from(source.width()) / f64::from(size[0]) - 0.5;
            let sx = sx.clamp(0.0, f64::from(source.width() - 1));
            let x0 = sx.floor() as u32;
            let fx = sx - f64::from(x0);
            let neighbors = [
                (x0, y0, (1.0 - fx) * (1.0 - fy)),
                ((x0 + 1).min(source.width() - 1), y0, fx * (1.0 - fy)),
                (x0, (y0 + 1).min(source.height() - 1), (1.0 - fx) * fy),
                (
                    (x0 + 1).min(source.width() - 1),
                    (y0 + 1).min(source.height() - 1),
                    fx * fy,
                ),
            ];
            let mut color = [0.0; 4];
            for (px, py, weight) in neighbors {
                let pixel = source.get_pixel(px, py).0;
                let alpha = f64::from(pixel[3]);
                for channel in 0..3 {
                    color[channel] += f64::from(pixel[channel]) * alpha * weight;
                }
                color[3] += alpha * weight;
            }
            let alpha = color[3];
            if alpha > 0.0 {
                let target = canvas.get_pixel_mut(x as u32, y as u32);
                let background_alpha = f64::from(target[3]) * (1.0 - alpha / 255.0);
                let combined_alpha = alpha + background_alpha;
                for channel in 0..3 {
                    target[channel] = ((color[channel]
                        + f64::from(target[channel]) * background_alpha)
                        / combined_alpha)
                        .round() as u8;
                }
                target[3] = combined_alpha.round() as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn huge_scaled_images_clip_to_visible_pixels_and_blend_alpha() {
        let mut canvas = RgbaImage::from_pixel(3, 2, Rgba([0, 0, 255, 255]));
        let source = RgbaImage::from_pixel(2, 1, Rgba([255, 0, 0, 128]));
        draw_scaled(&mut canvas, &source, [i32::MIN, -1], [u32::MAX; 2]);
        for pixel in canvas.pixels() {
            assert_eq!(pixel.0, [128, 0, 127, 255]);
        }
        let saved = canvas.clone();
        draw_scaled(&mut canvas, &source, [i32::MAX; 2], [u32::MAX; 2]);
        draw_scaled(&mut canvas, &source, [0; 2], [0, u32::MAX]);
        assert_eq!(canvas, saved);
    }

    #[test]
    fn scaled_pixels_keep_source_coordinates_after_left_clipping() {
        let mut canvas = RgbaImage::new(2, 1);
        let source = RgbaImage::from_fn(4, 1, |x, _| Rgba([x as u8 * 60, 0, 0, 255]));
        draw_scaled(&mut canvas, &source, [-2, 0], [4, 1]);
        assert_eq!(canvas.get_pixel(0, 0).0, [120, 0, 0, 255]);
        assert_eq!(canvas.get_pixel(1, 0).0, [180, 0, 0, 255]);
    }
}

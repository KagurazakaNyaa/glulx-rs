//! Decode pictures consistently and rasterize only visible destination pixels.
use std::io::Cursor;

use image::{ImageReader, RgbaImage};

// Glk permits unavailable resources to fail. Bound decoded resources, not the
// requested draw size: a huge scaled image may have a tiny visible portion.

#[cfg(test)]
pub(crate) fn decode(data: &[u8]) -> image::ImageResult<RgbaImage> {
    decode_with_limit(data, 64 * 1024 * 1024)
}

pub(crate) fn decode_with_limit(data: &[u8], maximum: u64) -> image::ImageResult<RgbaImage> {
    dimensions_with_limit(data, maximum)?;
    let reader = || ImageReader::new(Cursor::new(data)).with_guessed_format();
    let mut reader = reader()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(maximum.saturating_mul(2));
    reader.limits(limits);
    Ok(reader.decode()?.to_rgba8())
}

pub(crate) fn dimensions_with_limit(data: &[u8], maximum: u64) -> image::ImageResult<[u32; 2]> {
    let reader = ImageReader::new(Cursor::new(data)).with_guessed_format()?;
    let (width, height) = reader.into_dimensions()?;
    if width == 0
        || height == 0
        || (u64::from(width) * u64::from(height))
            .checked_mul(4)
            .is_none_or(|bytes| bytes > maximum)
    {
        return Err(image::ImageError::Limits(
            image::error::LimitError::from_kind(image::error::LimitErrorKind::InsufficientMemory),
        ));
    }
    Ok([width, height])
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
    // clipped canvas, even for unsigned extents such as 0xFFFFFFFF. Precompute
    // the one-dimensional coordinates once per draw; the old inner loop did
    // the same division, clamp and floor for every destination pixel.
    let x_start = left.max(clip[0] as i64);
    let y_start = top.max(clip[1] as i64);
    let x_samples: Vec<_> = (x_start..right)
        .map(|x| {
            let sx =
                ((x - left) as f64 + 0.5) * f64::from(source.width()) / f64::from(size[0]) - 0.5;
            let sx = sx.clamp(0.0, f64::from(source.width() - 1));
            let x0 = sx.floor() as u32;
            (x0, (x0 + 1).min(source.width() - 1), sx - f64::from(x0))
        })
        .collect();
    let y_samples: Vec<_> = (y_start..bottom)
        .map(|y| {
            let sy =
                ((y - top) as f64 + 0.5) * f64::from(source.height()) / f64::from(size[1]) - 0.5;
            let sy = sy.clamp(0.0, f64::from(source.height() - 1));
            let y0 = sy.floor() as u32;
            (y0, (y0 + 1).min(source.height() - 1), sy - f64::from(y0))
        })
        .collect();
    for (y, &(y0, y1, fy)) in (y_start..bottom).zip(&y_samples) {
        let top_weight = 1.0 - fy;
        for (x, &(x0, x1, fx)) in (x_start..right).zip(&x_samples) {
            let left_weight = 1.0 - fx;
            let weights = [
                left_weight * top_weight,
                fx * top_weight,
                left_weight * fy,
                fx * fy,
            ];
            let pixels = [
                source.get_pixel(x0, y0).0,
                source.get_pixel(x1, y0).0,
                source.get_pixel(x0, y1).0,
                source.get_pixel(x1, y1).0,
            ];
            let alpha = weights
                .iter()
                .zip(pixels.iter())
                .map(|(weight, pixel)| weight * f64::from(pixel[3]))
                .sum::<f64>();
            let mut color = [0.0; 4];
            for (pixel, weight) in pixels.iter().zip(weights) {
                let weighted_alpha = weight * f64::from(pixel[3]);
                for channel in 0..3 {
                    color[channel] += f64::from(pixel[channel]) * weighted_alpha;
                }
            }
            color[3] = alpha;
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
    fn decoded_image_limit_checks_rgba_size_before_decoding() {
        let pixels = RgbaImage::new(4, 4);
        let mut data = Cursor::new(Vec::new());
        pixels.write_to(&mut data, image::ImageFormat::Png).unwrap();
        assert!(decode_with_limit(data.get_ref(), 63).is_err());
        assert_eq!(decode_with_limit(data.get_ref(), 64).unwrap().len(), 64);
        assert!(decode_with_limit(data.get_ref(), 0).is_err());
    }

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

//! Preparing images for the API, and drawing/cropping the boxes Jev returns.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageReader, Rgb, RgbImage};

use crate::jev::BoxAnswer;
use crate::video::Frame;

/// Size limits for images sent to the API (see docs.typesafe.ai/images#limits).
const MIN_SIDE: u32 = 256;
const MAX_SIDE: u32 = 1568;
const MIN_PIXELS: u32 = 78_400;

/// The color of the outline drawn around the product.
const OUTLINE: Rgb<u8> = Rgb([229, 81, 186]);

/// Picks a size that keeps the aspect ratio and fits the API's limits:
/// shrink if a side is over MAX_SIDE, enlarge if a side or the total area is too small.
pub fn fit_size(width: u32, height: u32) -> (u32, u32) {
    let (w, h) = (width as f64, height as f64);
    let mut scale = 1.0_f64;
    if w.max(h) > MAX_SIDE as f64 {
        scale = MAX_SIDE as f64 / w.max(h);
    }
    if w.min(h) * scale < MIN_SIDE as f64 {
        scale = MIN_SIDE as f64 / w.min(h);
    }
    if w * h * scale * scale < MIN_PIXELS as f64 {
        scale = (MIN_PIXELS as f64 / (w * h)).sqrt();
    }
    // Round up to even numbers: ffmpeg needs them, and rounding up never drops below a minimum.
    let even = |x: f64| ((x / 2.0).ceil() as u32 * 2).max(2);
    (even(w * scale), even(h * scale))
}

/// Loads the reference image, turns it upright, sizes it within the API limits,
/// and returns it as JPEG bytes ready to upload.
pub fn prepare_reference(path: &Path) -> Result<Vec<u8>> {
    let context = || format!("reading reference image {}", path.display());
    let mut decoder = ImageReader::open(path)
        .with_context(context)?
        .with_guessed_format()
        .with_context(context)?
        .into_decoder()
        .with_context(context)?;
    // The API ignores EXIF orientation, so rotate the pixels themselves.
    let orientation = decoder.orientation().with_context(context)?;
    let mut img = DynamicImage::from_decoder(decoder).with_context(context)?;
    img.apply_orientation(orientation);

    let (width, height) = fit_size(img.width(), img.height());
    if (width, height) != (img.width(), img.height()) {
        img = img.resize_exact(width, height, FilterType::Lanczos3);
    }
    encode_jpeg(&img.to_rgb8())
}

/// Saves two images for a matching frame: the whole frame with the box outlined,
/// and the box cropped out. Returns their paths.
pub fn save_box_images(
    frame_path: &Path,
    product_box: &BoxAnswer,
    out_dir: &Path,
    frame: &Frame,
) -> Result<(PathBuf, PathBuf)> {
    let img = image::open(frame_path)
        .with_context(|| format!("opening frame {}", frame_path.display()))?
        .to_rgb8();
    let (x, y, w, h) = pixel_rect(product_box, img.width(), img.height());

    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let stem = format!("frame-{:04}_{:.1}s", frame.index, frame.seconds);

    let crop_path = out_dir.join(format!("{stem}_crop.jpg"));
    let crop = image::imageops::crop_imm(&img, x, y, w, h).to_image();
    std::fs::write(&crop_path, encode_jpeg(&crop)?)?;

    let outlined_path = out_dir.join(format!("{stem}_outlined.jpg"));
    let mut outlined = img;
    let thickness = (outlined.width().max(outlined.height()) / 200).max(3);
    draw_rect(&mut outlined, x, y, w, h, thickness);
    std::fs::write(&outlined_path, encode_jpeg(&outlined)?)?;

    Ok((outlined_path, crop_path))
}

/// Converts a box's 0-to-1 fractions into a pixel rectangle (x, y, width, height)
/// that is at least 1 pixel in size and stays inside the image.
fn pixel_rect(b: &BoxAnswer, width: u32, height: u32) -> (u32, u32, u32, u32) {
    let e = &b.edges;
    let to_px = |fraction: f64, size: u32| (fraction.clamp(0.0, 1.0) * size as f64).round() as u32;
    let (left, top) = (
        to_px(e.left, width).min(width - 1),
        to_px(e.top, height).min(height - 1),
    );
    let (right, bottom) = (
        to_px(e.right, width).max(left + 1),
        to_px(e.bottom, height).max(top + 1),
    );
    (left, top, right - left, bottom - top)
}

/// Draws a hollow rectangle `thickness` pixels wide, inside the given area.
fn draw_rect(img: &mut RgbImage, x: u32, y: u32, w: u32, h: u32, thickness: u32) {
    let (x_end, y_end) = ((x + w).min(img.width()), (y + h).min(img.height()));
    for py in y..y_end {
        for px in x..x_end {
            let near_edge = px < x + thickness
                || px + thickness >= x_end
                || py < y + thickness
                || py + thickness >= y_end;
            if near_edge {
                img.put_pixel(px, py, OUTLINE);
            }
        }
    }
}

fn encode_jpeg(img: &RgbImage) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    // `Cursor` lets the encoder write into a Vec in memory as if it were a file.
    JpegEncoder::new_with_quality(Cursor::new(&mut bytes), 90).encode_image(img)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jev::Edges;

    #[test]
    fn keeps_sizes_that_already_fit() {
        assert_eq!(fit_size(608, 1080), (608, 1080));
    }

    #[test]
    fn shrinks_4k() {
        assert_eq!(fit_size(3840, 2160), (1568, 882));
    }

    #[test]
    fn enlarges_narrow_video() {
        assert_eq!(fit_size(320, 180), (456, 256));
    }

    #[test]
    fn enlarges_small_square_to_minimum_area() {
        let (w, h) = fit_size(256, 256);
        assert!(w * h >= MIN_PIXELS);
    }

    #[test]
    fn box_converts_to_pixels() {
        let edges = Edges {
            top: 0.25,
            bottom: 0.75,
            left: 0.5,
            right: 1.0,
        };
        let b = BoxAnswer {
            edges,
            confidence: 0.9,
        };
        assert_eq!(pixel_rect(&b, 200, 100), (100, 25, 100, 50));
    }
}

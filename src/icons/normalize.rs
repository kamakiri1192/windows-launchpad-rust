//! Normalize raw decoded icons into a fixed-size RGBA bitmap.
//!
//! Win32 / `image` crate sources hand us bitmaps of arbitrary size and aspect
//! ratio. Launchpad draws every icon inside a square tile, so we:
//!   1. Scale the source to fit a `TARGET` x `TARGET` square, keeping aspect
//!      ratio (longest side = TARGET).
//!   2. Center it on a transparent canvas so the result is exactly TARGET².
//!
//! The output is tightly packed RGBA8, ready for an `Rgba8Unorm` texture.

use image::{imageops, Rgba, RgbaImage};

/// Edge length (px) of a single normalized icon cell.
pub const TARGET: u32 = 128;

/// A decoded icon: tightly-packed RGBA8, row-major, `w * h * 4` bytes.
///
/// Alpha is straight (not premultiplied). Premultiplication happens in the
/// icon shader at sample time, so the atlas stores source alpha verbatim.
#[derive(Debug, Clone)]
pub struct DecodedIcon {
    pub rgba: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

impl DecodedIcon {
    /// Build from any `image` decoder output. Reinterprets BGRA / paletted
    /// forms into straight RGBA8.
    #[allow(dead_code)] // used by a future "load icon from image file" path
    pub fn from_dynamic(img: image::DynamicImage) -> Self {
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());
        Self {
            rgba: rgba.into_raw(),
            w,
            h,
        }
    }
}

/// Resize `src` into a TARGET×TARGET square with aspect ratio preserved and
/// transparent padding around it.
///
/// Empty input (0×0) yields a fully transparent cell, which renders as nothing
/// on top of the fallback color tile.
pub fn normalize(src: &DecodedIcon) -> DecodedIcon {
    normalize_to(src, TARGET)
}

/// Same as [`normalize`] but with an explicit target size (used by tests).
pub fn normalize_to(src: &DecodedIcon, target: u32) -> DecodedIcon {
    if target == 0 {
        return DecodedIcon {
            rgba: Vec::new(),
            w: 0,
            h: 0,
        };
    }
    // Zero-size input → blank transparent cell.
    if src.w == 0 || src.h == 0 || src.rgba.len() < (src.w as usize * src.h as usize * 4) {
        return blank(target);
    }

    let src_img =
        RgbaImage::from_raw(src.w, src.h, src.rgba.clone()).unwrap_or_else(|| blank_image(target));
    let src_img = crop_to_opaque_bounds(&src_img).unwrap_or(src_img);
    let src_w = src_img.width();
    let src_h = src_img.height();

    // Fit-inside: the longest side maps to `target`.
    let (new_w, new_h) = fit_dimensions(src_w, src_h, target);
    let mut scaled = imageops::resize(&src_img, new_w, new_h, imageops::FilterType::Lanczos3);

    // Center on a transparent canvas.
    let mut canvas = RgbaImage::from_pixel(target, target, Rgba([0, 0, 0, 0]));
    let dx = (target - new_w) / 2;
    let dy = (target - new_h) / 2;
    imageops::overlay(&mut canvas, &scaled, dx.into(), dy.into());
    // Drop the borrow before moving out of `scaled` (kept for clarity).
    let _ = &mut scaled;

    let (w, h) = (canvas.width(), canvas.height());
    DecodedIcon {
        rgba: canvas.into_raw(),
        w,
        h,
    }
}

/// Scale (w, h) so the longest side equals `target`, preserving aspect ratio.
fn fit_dimensions(w: u32, h: u32, target: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (1, 1);
    }
    let max = w.max(h);
    let scale = target as f64 / max as f64;
    // Round; clamp to at least 1 to avoid zero-area images.
    let nw = ((w as f64 * scale).round() as u32).max(1);
    let nh = ((h as f64 * scale).round() as u32).max(1);
    (nw, nh)
}

fn crop_to_opaque_bounds(src: &RgbaImage) -> Option<RgbaImage> {
    let (w, h) = src.dimensions();
    let mut min_x = w;
    let mut min_y = h;
    let mut max_x = 0u32;
    let mut max_y = 0u32;

    for y in 0..h {
        for x in 0..w {
            if src.get_pixel(x, y)[3] > 10 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    if min_x > max_x || min_y > max_y {
        return None;
    }

    let crop_w = max_x - min_x + 1;
    let crop_h = max_y - min_y + 1;
    if min_x == 0 && min_y == 0 && crop_w == w && crop_h == h {
        return None;
    }

    Some(imageops::crop_imm(src, min_x, min_y, crop_w, crop_h).to_image())
}

fn blank(target: u32) -> DecodedIcon {
    let n = (target as usize).pow(2) * 4;
    DecodedIcon {
        rgba: vec![0; n],
        w: target,
        h: target,
    }
}

fn blank_image(target: u32) -> RgbaImage {
    RgbaImage::from_pixel(target, target, Rgba([0, 0, 0, 0]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> DecodedIcon {
        DecodedIcon {
            rgba: c.repeat((w * h) as usize),
            w,
            h,
        }
    }

    #[test]
    fn fit_preserves_aspect_and_clamps_longest_side() {
        // Landscape: width is the long side.
        let (w, h) = fit_dimensions(256, 128, 128);
        assert_eq!((w, h), (128, 64));
        // Portrait: height is the long side.
        let (w, h) = fit_dimensions(64, 256, 128);
        assert_eq!((w, h), (32, 128));
        // Square.
        let (w, h) = fit_dimensions(200, 200, 100);
        assert_eq!((w, h), (100, 100));
    }

    #[test]
    fn fit_upscales_small_sources_to_target() {
        let (w, h) = fit_dimensions(48, 48, 128);
        assert_eq!((w, h), (128, 128));
    }

    #[test]
    fn normalize_produces_target_square() {
        let src = solid(256, 128, [255, 0, 0, 255]);
        let out = normalize(&src);
        assert_eq!(out.w, TARGET);
        assert_eq!(out.h, TARGET);
        assert_eq!(out.rgba.len(), (TARGET as usize).pow(2) * 4);
    }

    #[test]
    fn normalize_centers_content_and_pads_transparent() {
        // 128×128 red source → fills exactly, no padding.
        let src = solid(128, 128, [255, 0, 0, 255]);
        let out = normalize_to(&src, 128);
        // Corner should be red (no padding needed).
        assert_eq!(&out.rgba[0..4], &[255, 0, 0, 255]);

        // 64×64 green source into 128 canvas → upscales to fill.
        let src = solid(64, 64, [0, 255, 0, 255]);
        let out = normalize_to(&src, 128);
        // Top-left corner is green because the source is upscaled.
        assert_eq!(&out.rgba[0..4], &[0, 255, 0, 255]);
        // Center pixel is also green.
        let cx = 64;
        let cy = 64;
        let idx = ((cy * 128 + cx) * 4) as usize;
        assert_eq!(&out.rgba[idx..idx + 4], &[0, 255, 0, 255]);
    }

    #[test]
    fn normalize_crops_transparent_padding_before_scaling() {
        let mut rgba = vec![0u8; 128 * 128 * 4];
        for y in 0..32usize {
            for x in 0..32usize {
                let idx = (y * 128 + x) * 4;
                rgba[idx..idx + 4].copy_from_slice(&[0, 0, 255, 255]);
            }
        }
        let src = DecodedIcon {
            rgba,
            w: 128,
            h: 128,
        };
        let out = normalize_to(&src, 128);

        assert_eq!(&out.rgba[0..4], &[0, 0, 255, 255]);
        let bottom_right = (127 * 128 + 127) * 4;
        assert_eq!(&out.rgba[bottom_right..bottom_right + 4], &[0, 0, 255, 255]);
    }

    #[test]
    fn normalize_handles_zero_size_input() {
        let src = DecodedIcon {
            rgba: vec![],
            w: 0,
            h: 0,
        };
        let out = normalize(&src);
        assert_eq!(out.w, TARGET);
        assert_eq!(out.h, TARGET);
        // All transparent.
        assert!(out.rgba.iter().all(|&b| b == 0));
    }

    #[test]
    fn normalize_to_zero_target_is_empty() {
        let src = solid(32, 32, [1, 2, 3, 4]);
        let out = normalize_to(&src, 0);
        assert_eq!((out.w, out.h), (0, 0));
        assert!(out.rgba.is_empty());
    }
}

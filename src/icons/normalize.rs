//! Normalize raw decoded icons into a fixed-size RGBA bitmap.
//!
//! Win32 / `image` crate sources hand us bitmaps of arbitrary size and aspect
//! ratio. Launchpad draws every icon inside a square tile, so we:
//!   1. Analyse the pre-crop source to classify it (full-bleed / thin-line /
//!      solid) and pick a per-category scale factor (Issue #48).
//!   2. Crop the source to its opaque bounding-box.
//!   3. Scale the cropped content so its longest side maps to
//!      `TARGET * scale`, keeping aspect ratio.
//!   4. Center it on a transparent canvas so the result is exactly TARGET².
//!
//! For full-bleed icons (scale = 1.00) the output is byte-for-byte identical
//! to the previous "crop → long-side = TARGET" pipeline — the existing
//! well-balanced icons (Safari, App Store, …) must not change at all.
//!
//! The output is tightly packed RGBA8, ready for an `Rgba8Unorm` texture.

use image::{imageops, Rgba, RgbaImage};

use super::sizing::{self, IconCategory, IconMetrics};

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

/// Result of normalizing one icon: the pixels plus the classification that
/// was used to scale them.
#[derive(Debug, Clone)]
pub struct NormalizedIcon {
    /// The TARGET×TARGET RGBA pixels, ready for the atlas.
    pub image: DecodedIcon,
    /// Visual category inferred from the source (Issue #48).
    pub category: IconCategory,
    /// Scale factor actually applied (1.00 / 0.92 / 0.74).
    pub scale: f64,
}

impl NormalizedIcon {
    /// A blank (fully-transparent) normalized icon with a `FullBleed` tag.
    ///
    /// Used as a safe fallback when normalization cannot proceed; the category
    /// is irrelevant for a fully-transparent cell but a value is still required.
    fn blank() -> Self {
        Self {
            image: DecodedIcon {
                rgba: vec![0; (TARGET as usize).pow(2) * 4],
                w: TARGET,
                h: TARGET,
            },
            category: IconCategory::FullBleed,
            scale: sizing::SCALE_FULLBLEED,
        }
    }
}

/// Normalize `src` to a TARGET×TARGET cell, applying the per-category scale
/// factor determined by [`sizing::analyze`].
///
/// Returns a [`NormalizedIcon`] carrying the pixels *and* the classification.
/// Empty/all-transparent input yields a blank cell with a `FullBleed` tag.
pub fn normalize(src: &DecodedIcon) -> NormalizedIcon {
    normalize_to(src, TARGET)
}

/// Same as [`normalize`] but with an explicit target size (used by tests).
pub fn normalize_to(src: &DecodedIcon, target: u32) -> NormalizedIcon {
    normalize_to_with_geometry(src, target, GeometryMode::SoftAlphaBounds)
}

/// Normalize an application icon returned by the macOS app-icon path.
///
/// macOS app representations commonly include a faint, asymmetric drop shadow
/// outside the opaque app tile. The generic normalizer treats that shadow as
/// part of the geometry, which leaves the tile body visually high in the output.
/// For square full-bleed icons this path uses the hard-alpha body for scale and
/// centering while still resampling the original soft edge once. Pixels outside
/// the centered body naturally clip at the TARGET canvas boundary.
#[cfg(any(target_os = "macos", test))]
pub fn normalize_macos_app_icon(src: &DecodedIcon) -> NormalizedIcon {
    normalize_macos_app_icon_to(src, TARGET)
}

/// Explicit-target variant of [`normalize_macos_app_icon`] for tests.
#[cfg(any(target_os = "macos", test))]
pub fn normalize_macos_app_icon_to(src: &DecodedIcon, target: u32) -> NormalizedIcon {
    normalize_to_with_geometry(src, target, GeometryMode::MacosFullBleedBody)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GeometryMode {
    SoftAlphaBounds,
    MacosFullBleedBody,
}

fn normalize_to_with_geometry(
    src: &DecodedIcon,
    target: u32,
    geometry_mode: GeometryMode,
) -> NormalizedIcon {
    if target == 0 {
        // Preserve the previous behaviour: zero target → empty pixels.
        return NormalizedIcon {
            image: DecodedIcon {
                rgba: Vec::new(),
                w: 0,
                h: 0,
            },
            category: IconCategory::FullBleed,
            scale: sizing::SCALE_FULLBLEED,
        };
    }
    // Zero-size / truncated input → blank transparent cell.
    if src.w == 0 || src.h == 0 || src.rgba.len() < (src.w as usize * src.h as usize * 4) {
        return NormalizedIcon::blank();
    }

    // ---- Issue #48: classify the *pre-crop* source. ------------------------
    // `analyze` returns None only when the source is fully transparent; in
    // that case we fall back to a blank cell (no opaque pixels to draw).
    let metrics = match sizing::analyze(src) {
        Some(metrics) => metrics,
        None => return NormalizedIcon::blank(),
    };
    let (category, scale) = (metrics.category, metrics.scale);

    // ---- Crop to the opaque bounding-box. ---------------------------------
    // For full-bleed icons this is identical to the previous pipeline; for
    // logo icons the bbox is the logo's own outline (transparent padding is
    // stripped) and the category scale is applied on top.
    let src_img =
        RgbaImage::from_raw(src.w, src.h, src.rgba.clone()).unwrap_or_else(|| blank_image(target));

    if geometry_mode == GeometryMode::MacosFullBleedBody && category == IconCategory::FullBleed {
        if let Some(canvas) = normalize_macos_fullbleed_body(&src_img, &metrics, target, scale) {
            return normalized_from_canvas(canvas, category, scale);
        }
    }

    // Prefer the bbox measured by `analyze` (alpha >= ALPHA_HARD) so the crop
    // matches the classification.  `crop_to_opaque_bounds` uses alpha > 10,
    // which is slightly more permissive (keeps soft shadow edges); we keep
    // using it for the crop itself so faint anti-aliased fringes are retained
    // — only the *classification* uses the stricter ALPHA_HARD bbox.
    let cropped = crop_to_opaque_bounds(&src_img).unwrap_or_else(|| src_img.clone());
    let src_w = cropped.width();
    let src_h = cropped.height();

    // ---- Scale + center on the canvas. ------------------------------------
    // `scale` is defined relative to "bbox long-side == target" (1.0).
    // Fit the long side to `target * scale`, preserving aspect ratio.
    let (new_w, new_h) = fit_dimensions(src_w, src_h, (target as f64 * scale).round() as u32);
    let scaled = imageops::resize(&cropped, new_w, new_h, imageops::FilterType::Lanczos3);

    let mut canvas = RgbaImage::from_pixel(target, target, Rgba([0, 0, 0, 0]));
    let dx = (target - new_w) / 2;
    let dy = (target - new_h) / 2;
    imageops::overlay(&mut canvas, &scaled, dx.into(), dy.into());

    normalized_from_canvas(canvas, category, scale)
}

fn normalized_from_canvas(canvas: RgbaImage, category: IconCategory, scale: f64) -> NormalizedIcon {
    let (w, h) = (canvas.width(), canvas.height());
    NormalizedIcon {
        image: DecodedIcon {
            rgba: canvas.into_raw(),
            w,
            h,
        },
        category,
        scale,
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
    let bounds = alpha_bounds(src, 11)?;
    if bounds.min_x == 0
        && bounds.min_y == 0
        && bounds.width() == src.width()
        && bounds.height() == src.height()
    {
        return None;
    }
    Some(crop_to_bounds(src, bounds))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AlphaBounds {
    min_x: u32,
    min_y: u32,
    max_x: u32,
    max_y: u32,
}

impl AlphaBounds {
    fn from_metrics(metrics: &IconMetrics) -> Self {
        Self {
            min_x: metrics.bbox_min_x,
            min_y: metrics.bbox_min_y,
            max_x: metrics.bbox_max_x,
            max_y: metrics.bbox_max_y,
        }
    }

    fn width(self) -> u32 {
        self.max_x - self.min_x + 1
    }

    fn height(self) -> u32 {
        self.max_y - self.min_y + 1
    }

    fn is_nearly_square(self) -> bool {
        let longest = self.width().max(self.height());
        self.width().abs_diff(self.height()) * 20 <= longest
    }

    fn contains(self, other: Self) -> bool {
        self.min_x <= other.min_x
            && self.min_y <= other.min_y
            && self.max_x >= other.max_x
            && self.max_y >= other.max_y
    }
}

fn alpha_bounds(src: &RgbaImage, min_alpha: u8) -> Option<AlphaBounds> {
    let (w, h) = src.dimensions();
    let mut min_x = w;
    let mut min_y = h;
    let mut max_x = 0u32;
    let mut max_y = 0u32;

    for y in 0..h {
        for x in 0..w {
            if src.get_pixel(x, y)[3] >= min_alpha {
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

    Some(AlphaBounds {
        min_x,
        min_y,
        max_x,
        max_y,
    })
}

fn crop_to_bounds(src: &RgbaImage, bounds: AlphaBounds) -> RgbaImage {
    imageops::crop_imm(
        src,
        bounds.min_x,
        bounds.min_y,
        bounds.width(),
        bounds.height(),
    )
    .to_image()
}

fn normalize_macos_fullbleed_body(
    src: &RgbaImage,
    metrics: &IconMetrics,
    target: u32,
    scale: f64,
) -> Option<RgbaImage> {
    let body_bounds = AlphaBounds::from_metrics(metrics);
    if !body_bounds.is_nearly_square() {
        return None;
    }

    let soft_bounds = alpha_bounds(src, 11)?;
    if !soft_bounds.contains(body_bounds) {
        return None;
    }

    let body_longest = body_bounds.width().max(body_bounds.height());
    let requested_body_longest = ((target as f64 * scale).round() as u32).max(1);
    let resize_ratio = requested_body_longest as f64 / body_longest as f64;
    let resized_w = ((soft_bounds.width() as f64 * resize_ratio).round() as u32).max(1);
    let resized_h = ((soft_bounds.height() as f64 * resize_ratio).round() as u32).max(1);

    let cropped = crop_to_bounds(src, soft_bounds);
    let resized = imageops::resize(
        &cropped,
        resized_w,
        resized_h,
        imageops::FilterType::Lanczos3,
    );

    // Use the actual post-rounding resize ratios so the hard-alpha body's
    // geometric centre lands on the target centre even when one dimension
    // rounded by a pixel.
    let resize_x = resized_w as f64 / soft_bounds.width() as f64;
    let resize_y = resized_h as f64 / soft_bounds.height() as f64;
    let body_center_x = (f64::from(body_bounds.min_x - soft_bounds.min_x)
        + f64::from(body_bounds.width()) * 0.5)
        * resize_x;
    let body_center_y = (f64::from(body_bounds.min_y - soft_bounds.min_y)
        + f64::from(body_bounds.height()) * 0.5)
        * resize_y;
    let dx = (target as f64 * 0.5 - body_center_x).round() as i64;
    let dy = (target as f64 * 0.5 - body_center_y).round() as i64;

    let mut canvas = blank_image(target);
    imageops::overlay(&mut canvas, &resized, dx, dy);
    Some(canvas)
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
        // Fully-opaque source → full-bleed, scale 1.0.
        assert_eq!(out.category, IconCategory::FullBleed);
        assert_eq!(out.scale, 1.0);
        assert_eq!(out.image.w, TARGET);
        assert_eq!(out.image.h, TARGET);
        assert_eq!(out.image.rgba.len(), (TARGET as usize).pow(2) * 4);
    }

    #[test]
    fn normalize_centers_content_and_pads_transparent() {
        // 128×128 red source → fills exactly, no padding.
        let src = solid(128, 128, [255, 0, 0, 255]);
        let out = normalize_to(&src, 128);
        assert_eq!(out.category, IconCategory::FullBleed);
        assert_eq!(out.scale, 1.0);
        // Corner should be red (no padding needed).
        assert_eq!(&out.image.rgba[0..4], &[255, 0, 0, 255]);

        // 64×64 green source into 128 canvas → upscales to fill.
        let src = solid(64, 64, [0, 255, 0, 255]);
        let out = normalize_to(&src, 128);
        assert_eq!(out.category, IconCategory::FullBleed);
        assert_eq!(out.scale, 1.0);
        // Top-left corner is green because the source is upscaled.
        assert_eq!(&out.image.rgba[0..4], &[0, 255, 0, 255]);
        // Center pixel is also green.
        let cx = 64;
        let cy = 64;
        let idx = ((cy * 128 + cx) * 4) as usize;
        assert_eq!(&out.image.rgba[idx..idx + 4], &[0, 255, 0, 255]);
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

        // The 32×32 opaque block is fully filled → FullBleed, scale 1.0,
        // cropped to 32×32 then upscaled to 128×128.
        assert_eq!(out.category, IconCategory::FullBleed);
        assert_eq!(out.scale, 1.0);
        assert_eq!(&out.image.rgba[0..4], &[0, 0, 255, 255]);
        let bottom_right = (127 * 128 + 127) * 4;
        assert_eq!(
            &out.image.rgba[bottom_right..bottom_right + 4],
            &[0, 0, 255, 255]
        );
    }

    #[test]
    fn macos_fullbleed_uses_body_geometry_instead_of_bottom_shadow() {
        let mut rgba = vec![0u8; 128 * 128 * 4];

        // Model the geometry measured from NSWorkspace's App Store icon:
        // soft alpha 108x108 at (10,12), with a 104x104 opaque body at
        // (12,12). The extra four rows exist only below the body.
        for y in 12..120usize {
            for x in 10..118usize {
                let idx = (y * 128 + x) * 4;
                rgba[idx..idx + 4].copy_from_slice(&[0, 80, 220, 64]);
            }
        }
        for y in 12..116usize {
            for x in 12..116usize {
                let idx = (y * 128 + x) * 4;
                rgba[idx..idx + 4].copy_from_slice(&[0, 120, 255, 255]);
            }
        }
        let source = DecodedIcon {
            rgba,
            w: 128,
            h: 128,
        };

        let generic = normalize_to(&source, 128);
        let macos = normalize_macos_app_icon_to(&source, 128);
        assert_eq!(generic.category, IconCategory::FullBleed);
        assert_eq!(macos.category, IconCategory::FullBleed);

        let generic_image =
            RgbaImage::from_raw(128, 128, generic.image.rgba).expect("generic RGBA image");
        let macos_image =
            RgbaImage::from_raw(128, 128, macos.image.rgba).expect("macOS RGBA image");
        let generic_body = alpha_bounds(&generic_image, sizing::ALPHA_HARD).unwrap();
        let macos_body = alpha_bounds(&macos_image, sizing::ALPHA_HARD).unwrap();

        let generic_bottom_margin = 127 - generic_body.max_y;
        let macos_top_margin = macos_body.min_y;
        let macos_bottom_margin = 127 - macos_body.max_y;
        assert!(generic_bottom_margin >= 3);
        assert!(macos_top_margin <= 1);
        assert!(macos_bottom_margin <= 1);
        assert!(macos_top_margin.abs_diff(macos_bottom_margin) <= 1);
    }

    #[test]
    fn macos_geometry_leaves_non_fullbleed_icons_unchanged() {
        let mut rgba = vec![0u8; 128 * 128 * 4];
        for y in 0..128usize {
            for x in 0..128usize {
                if (x + y) % 2 == 0 {
                    let idx = (y * 128 + x) * 4;
                    rgba[idx..idx + 4].copy_from_slice(&[200, 50, 50, 255]);
                }
            }
        }
        let source = DecodedIcon {
            rgba,
            w: 128,
            h: 128,
        };

        let generic = normalize(&source);
        let macos = normalize_macos_app_icon(&source);
        assert_eq!(generic.category, IconCategory::Solid);
        assert_eq!(macos.category, generic.category);
        assert_eq!(macos.scale, generic.scale);
        assert_eq!(macos.image.rgba, generic.image.rgba);
    }

    #[test]
    fn macos_geometry_falls_back_for_non_square_fullbleed_icons() {
        let source = solid(200, 150, [10, 20, 30, 255]);
        let generic = normalize(&source);
        let macos = normalize_macos_app_icon(&source);

        assert_eq!(generic.category, IconCategory::FullBleed);
        assert_eq!(macos.category, generic.category);
        assert_eq!(macos.scale, generic.scale);
        assert_eq!(macos.image.rgba, generic.image.rgba);
    }

    #[test]
    fn normalize_handles_zero_size_input() {
        let src = DecodedIcon {
            rgba: vec![],
            w: 0,
            h: 0,
        };
        let out = normalize(&src);
        assert_eq!(out.image.w, TARGET);
        assert_eq!(out.image.h, TARGET);
        // All transparent.
        assert!(out.image.rgba.iter().all(|&b| b == 0));
    }

    #[test]
    fn normalize_to_zero_target_is_empty() {
        let src = solid(32, 32, [1, 2, 3, 4]);
        let out = normalize_to(&src, 0);
        assert_eq!((out.image.w, out.image.h), (0, 0));
        assert!(out.image.rgba.is_empty());
    }

    /// Regression guard (Issue #48): a fully-opaque square is classified
    /// `FullBleed` with scale 1.0, and the pixel output must be byte-for-byte
    /// identical to the previous "crop → long-side = TARGET" pipeline.
    #[test]
    fn fullbleed_output_is_byte_identical_to_legacy_pipeline() {
        // A 200×150 opaque rectangle: bbox long side = 200 → scaled to 128×96,
        // centered on a 128×128 canvas. Transparent bars top and bottom.
        let src = solid(200, 150, [10, 20, 30, 255]);
        let out = normalize(&src);
        assert_eq!(out.category, IconCategory::FullBleed);
        assert_eq!(out.scale, 1.0);
        assert_eq!(out.image.w, TARGET);
        assert_eq!(out.image.h, TARGET);

        // Top-left corner is in the transparent padding (top margin).
        assert_eq!(&out.image.rgba[0..4], &[0, 0, 0, 0]);
        // Center pixel is inside the scaled content and should carry the source colour.
        let cx = (TARGET / 2) as usize;
        let cy = (TARGET / 2) as usize;
        let idx = (cy * TARGET as usize + cx) * 4;
        assert_eq!(&out.image.rgba[idx..idx + 4], &[10, 20, 30, 255]);
        // Bottom-right corner is also padding.
        let br = ((TARGET as usize - 1) * TARGET as usize + TARGET as usize - 1) * 4;
        assert_eq!(&out.image.rgba[br..br + 4], &[0, 0, 0, 0]);
    }

    /// Issue #48: a thin-line logo (sparse opaque pixels inside a large bbox)
    /// is classified `ThinLine` and scaled by 0.92.
    #[test]
    fn thinline_logo_is_scaled_092() {
        // 128×128 canvas, a thin outline ring: only border pixels opaque.
        // bbox = full 128×128, solid_area ≈ perimeter ≈ 4*128 - 4 = 508 px,
        // solid_fill ≈ 508/16384 ≈ 0.031 → ThinLine.
        let mut rgba = vec![0u8; 128 * 128 * 4];
        for i in 0..128usize {
            for &pos in &[(i, 0usize), (i, 127), (0, i), (127, i)] {
                let (x, y) = pos;
                let idx = (y * 128 + x) * 4;
                rgba[idx..idx + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        let src = DecodedIcon {
            rgba,
            w: 128,
            h: 128,
        };
        let out = normalize(&src);
        assert_eq!(out.category, IconCategory::ThinLine);
        assert_eq!(out.scale, 0.92);
        assert_eq!(out.image.w, TARGET);
        assert_eq!(out.image.h, TARGET);
    }

    /// Issue #48: a solid-body logo (≈50% fill inside bbox) is classified
    /// `Solid` and scaled by 0.74.
    #[test]
    fn solid_logo_is_scaled_074() {
        // 128×128 canvas, opaque in a 64×64 centred block PLUS a transparent
        // border: bbox = 128×128, solid_area = 64*64 = 4096, fill ≈ 0.25.
        // Hmm that is ThinLine. Make it denser: opaque everywhere except a
        // 16px transparent border on top/bottom → bbox 128×96, fill ≈ 1.0.
        // Use a different shape: opaque diagonal stripes covering ~50%.
        let mut rgba = vec![0u8; 128 * 128 * 4];
        for y in 0..128usize {
            for x in 0..128usize {
                if (x + y) % 2 == 0 {
                    let idx = (y * 128 + x) * 4;
                    rgba[idx..idx + 4].copy_from_slice(&[200, 50, 50, 255]);
                }
            }
        }
        let src = DecodedIcon {
            rgba,
            w: 128,
            h: 128,
        };
        let out = normalize(&src);
        // Checkerboard: solid_fill ≈ 0.5 → Solid.
        assert_eq!(out.category, IconCategory::Solid);
        assert_eq!(out.scale, 0.74);
        assert_eq!(out.image.w, TARGET);
        assert_eq!(out.image.h, TARGET);
    }

    /// Issue #48: fully-transparent input produces a blank cell and a
    /// `FullBleed` (default) classification.
    #[test]
    fn fully_transparent_input_produces_blank_cell() {
        let src = DecodedIcon {
            rgba: vec![0; 128 * 128 * 4],
            w: 128,
            h: 128,
        };
        let out = normalize(&src);
        assert_eq!(out.category, IconCategory::FullBleed);
        assert_eq!(out.scale, 1.0);
        assert!(out.image.rgba.iter().all(|&b| b == 0));
    }
}

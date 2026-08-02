//! One-pass icon normalization using the optional learned scale policy.
//!
//! The historical [`crate::icons::normalize`] module remains the stable
//! rule-only implementation and test reference. The live worker calls this
//! module so a manual override or learned correction can replace the rule scale
//! before pixels are resized, avoiding a second lossy resample.

use image::{imageops, Rgba, RgbaImage};

use super::features;
use super::normalize::{DecodedIcon, NormalizedIcon, TARGET};
use super::scale_model::ScalePolicy;
use super::sizing::{self, IconCategory};

pub fn normalize_for_app(
    src: &DecodedIcon,
    app_id: &str,
    source_path: &str,
    policy: &ScalePolicy,
) -> NormalizedIcon {
    normalize_to_for_app(src, TARGET, app_id, source_path, policy)
}

pub fn normalize_to_for_app(
    src: &DecodedIcon,
    target: u32,
    app_id: &str,
    source_path: &str,
    policy: &ScalePolicy,
) -> NormalizedIcon {
    if target == 0 {
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

    let Some(required_len) = (src.w as usize)
        .checked_mul(src.h as usize)
        .and_then(|pixels| pixels.checked_mul(4))
    else {
        return blank(target);
    };
    if src.w == 0 || src.h == 0 || src.rgba.len() < required_len {
        return blank(target);
    }

    let Some(metrics) = sizing::analyze(src) else {
        return blank(target);
    };
    let visual_features = features::extract(src);
    let decision = policy.decide(
        app_id,
        source_path,
        visual_features.as_ref(),
        metrics.scale as f32,
    );
    let scale = f64::from(decision.final_scale);

    let source =
        RgbaImage::from_raw(src.w, src.h, src.rgba.clone()).unwrap_or_else(|| blank_image(target));
    let cropped = crop_to_opaque_bounds(&source).unwrap_or_else(|| source.clone());
    let requested_long_side = ((target as f64 * scale).round() as u32).max(1);
    let (new_w, new_h) = fit_dimensions(cropped.width(), cropped.height(), requested_long_side);
    let scaled = imageops::resize(&cropped, new_w, new_h, imageops::FilterType::Lanczos3);

    let mut canvas = blank_image(target);
    let dx = (i64::from(target) - i64::from(new_w)) / 2;
    let dy = (i64::from(target) - i64::from(new_h)) / 2;
    imageops::overlay(&mut canvas, &scaled, dx, dy);

    NormalizedIcon {
        image: DecodedIcon {
            rgba: canvas.into_raw(),
            w: target,
            h: target,
        },
        category: metrics.category,
        scale,
    }
}

fn blank(target: u32) -> NormalizedIcon {
    NormalizedIcon {
        image: DecodedIcon {
            rgba: vec![0; target as usize * target as usize * 4],
            w: target,
            h: target,
        },
        category: IconCategory::FullBleed,
        scale: sizing::SCALE_FULLBLEED,
    }
}

fn blank_image(target: u32) -> RgbaImage {
    RgbaImage::from_pixel(target, target, Rgba([0, 0, 0, 0]))
}

fn fit_dimensions(width: u32, height: u32, target: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (1, 1);
    }
    let longest = width.max(height);
    let ratio = target as f64 / longest as f64;
    let new_width = ((width as f64 * ratio).round() as u32).max(1);
    let new_height = ((height as f64 * ratio).round() as u32).max(1);
    (new_width, new_height)
}

fn crop_to_opaque_bounds(source: &RgbaImage) -> Option<RgbaImage> {
    let (width, height) = source.dimensions();
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0u32;
    let mut max_y = 0u32;

    for y in 0..height {
        for x in 0..width {
            if source.get_pixel(x, y)[3] > 10 {
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
    let crop_width = max_x - min_x + 1;
    let crop_height = max_y - min_y + 1;
    if min_x == 0 && min_y == 0 && crop_width == width && crop_height == height {
        return None;
    }
    Some(imageops::crop_imm(source, min_x, min_y, crop_width, crop_height).to_image())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icons::normalize;
    use crate::icons::scale_model::{
        IconScaleOverride, IconScaleOverrides, OVERRIDES_FORMAT_VERSION,
    };

    fn checkerboard() -> DecodedIcon {
        let mut rgba = vec![0u8; 128 * 128 * 4];
        for y in 0..128usize {
            for x in 0..128usize {
                if (x + y) % 2 == 0 {
                    let index = (y * 128 + x) * 4;
                    rgba[index..index + 4].copy_from_slice(&[220, 80, 40, 255]);
                }
            }
        }
        DecodedIcon {
            rgba,
            w: 128,
            h: 128,
        }
    }

    fn opaque_extent(image: &DecodedIcon) -> (u32, u32) {
        let mut min_x = image.w;
        let mut min_y = image.h;
        let mut max_x = 0;
        let mut max_y = 0;
        for y in 0..image.h {
            for x in 0..image.w {
                let alpha = image.rgba[((y * image.w + x) * 4 + 3) as usize];
                if alpha > 10 {
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }
        (max_x - min_x + 1, max_y - min_y + 1)
    }

    #[test]
    fn empty_policy_matches_rule_only_normalizer() {
        let source = checkerboard();
        let legacy = normalize::normalize(&source);
        let adaptive = normalize_for_app(&source, "app", "path", &ScalePolicy::default());
        assert_eq!(adaptive.category, legacy.category);
        assert_eq!(adaptive.scale, legacy.scale);
        assert_eq!(adaptive.image.rgba, legacy.image.rgba);
    }

    #[test]
    fn manual_override_changes_the_one_pass_output_extent() {
        let source = checkerboard();
        let policy = ScalePolicy::from_parts(
            None,
            IconScaleOverrides {
                format_version: OVERRIDES_FORMAT_VERSION,
                entries: vec![IconScaleOverride {
                    key: "app".into(),
                    name: "App".into(),
                    scale: 0.90,
                }],
            },
        )
        .unwrap();
        let rule = normalize::normalize(&source);
        let adaptive = normalize_for_app(&source, "app", "path", &policy);
        assert!((adaptive.scale - 0.90).abs() < 1.0e-6);
        assert!(opaque_extent(&adaptive.image).0 > opaque_extent(&rule.image).0);
    }
}

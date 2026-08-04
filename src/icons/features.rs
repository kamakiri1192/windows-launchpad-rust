//! Machine-learning-ready visual features extracted from raw application icons.
//!
//! The current sizing heuristic uses only `solid_fill`. This module keeps that
//! metric and adds geometry, topology, alpha, placement, and luminance signals
//! that can be exported as training data without changing runtime sizing yet.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use super::normalize::DecodedIcon;
use super::sizing::{self, ALPHA_HARD};

/// Stable number and ordering of scalar values returned by
/// [`IconVisualFeatures::as_array`].
pub const FEATURE_COUNT: usize = 19;

/// Stable names matching the order returned by [`IconVisualFeatures::as_array`].
pub const FEATURE_NAMES: [&str; FEATURE_COUNT] = [
    "source_coverage_10",
    "source_coverage_64",
    "source_coverage_128",
    "source_coverage_224",
    "alpha_mass",
    "bbox_width_ratio",
    "bbox_height_ratio",
    "bbox_area_ratio",
    "aspect_ratio_log",
    "solid_fill",
    "centroid_x",
    "centroid_y",
    "perimeter_ratio",
    "circularity",
    "hole_ratio",
    "connected_components",
    "dominant_component_ratio",
    "mean_luminance",
    "luminance_stddev",
];

/// Deterministic, resolution-independent measurements for one decoded icon.
///
/// All ratios are normalized to the source image or the alpha-hard bounding box,
/// making records comparable across Windows ICO sizes and macOS Retina assets.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IconVisualFeatures {
    /// Fraction of source pixels with alpha greater than 10.
    pub source_coverage_10: f32,
    /// Fraction of source pixels with alpha at least 64.
    pub source_coverage_64: f32,
    /// Fraction of source pixels with alpha at least [`ALPHA_HARD`].
    pub source_coverage_128: f32,
    /// Fraction of source pixels with alpha at least 224.
    pub source_coverage_224: f32,
    /// Sum of alpha divided by `255 * source_pixel_count`.
    pub alpha_mass: f32,

    /// Alpha-hard bounding-box width divided by source width.
    pub bbox_width_ratio: f32,
    /// Alpha-hard bounding-box height divided by source height.
    pub bbox_height_ratio: f32,
    /// Alpha-hard bounding-box area divided by source area.
    pub bbox_area_ratio: f32,
    /// Natural log of `bbox_width / bbox_height`; zero means square.
    pub aspect_ratio_log: f32,
    /// Alpha-hard pixels divided by bounding-box area.
    pub solid_fill: f32,

    /// Alpha-weighted horizontal centroid within the bounding box, in `[0, 1]`.
    pub centroid_x: f32,
    /// Alpha-weighted vertical centroid within the bounding box, in `[0, 1]`.
    pub centroid_y: f32,

    /// Four-connected hard-alpha perimeter normalized so a filled square is 1.
    pub perimeter_ratio: f32,
    /// `4πA / P²`, clamped to `[0, 1]`; a disk approaches 1.
    pub circularity: f32,
    /// Enclosed transparent pixels divided by bounding-box area.
    pub hole_ratio: f32,
    /// Number of four-connected hard-alpha components.
    pub connected_components: f32,
    /// Largest hard-alpha component divided by total hard-alpha pixels.
    pub dominant_component_ratio: f32,

    /// Alpha-weighted Rec. 709 luminance mean, in `[0, 1]`.
    pub mean_luminance: f32,
    /// Alpha-weighted luminance standard deviation, in `[0, 1]`.
    pub luminance_stddev: f32,
}

impl IconVisualFeatures {
    /// Return features in a stable order suitable for model training/inference.
    pub fn as_array(self) -> [f32; FEATURE_COUNT] {
        [
            self.source_coverage_10,
            self.source_coverage_64,
            self.source_coverage_128,
            self.source_coverage_224,
            self.alpha_mass,
            self.bbox_width_ratio,
            self.bbox_height_ratio,
            self.bbox_area_ratio,
            self.aspect_ratio_log,
            self.solid_fill,
            self.centroid_x,
            self.centroid_y,
            self.perimeter_ratio,
            self.circularity,
            self.hole_ratio,
            self.connected_components,
            self.dominant_component_ratio,
            self.mean_luminance,
            self.luminance_stddev,
        ]
    }
}

/// Extract ML-ready features from the original, pre-crop RGBA image.
///
/// Returns `None` for zero-sized, truncated, or fully transparent inputs. The
/// function is pure and performs no image I/O.
pub fn extract(src: &DecodedIcon) -> Option<IconVisualFeatures> {
    let width = src.w as usize;
    let height = src.h as usize;
    let pixel_count = width.checked_mul(height)?;
    let required_len = pixel_count.checked_mul(4)?;
    if width == 0 || height == 0 || src.rgba.len() < required_len {
        return None;
    }

    let metrics = sizing::analyze(src)?;
    let bbox_w = metrics.bbox_w as usize;
    let bbox_h = metrics.bbox_h as usize;
    let bbox_area = bbox_w.checked_mul(bbox_h)?;

    let mut coverage_10 = 0usize;
    let mut coverage_64 = 0usize;
    let mut coverage_128 = 0usize;
    let mut coverage_224 = 0usize;
    let mut alpha_sum = 0.0f64;
    let mut centroid_weight = 0.0f64;
    let mut weighted_x = 0.0f64;
    let mut weighted_y = 0.0f64;
    let mut luminance_sum = 0.0f64;
    let mut luminance_sq_sum = 0.0f64;

    for y in 0..height {
        let row_start = y * width * 4;
        for x in 0..width {
            let i = row_start + x * 4;
            let alpha = src.rgba[i + 3];
            coverage_10 += usize::from(alpha > 10);
            coverage_64 += usize::from(alpha >= 64);
            coverage_128 += usize::from(alpha >= ALPHA_HARD);
            coverage_224 += usize::from(alpha >= 224);

            if alpha == 0 {
                continue;
            }

            let weight = alpha as f64 / 255.0;
            let local_x = x.saturating_sub(metrics.bbox_min_x as usize) as f64;
            let local_y = y.saturating_sub(metrics.bbox_min_y as usize) as f64;
            let r = src.rgba[i] as f64 / 255.0;
            let g = src.rgba[i + 1] as f64 / 255.0;
            let b = src.rgba[i + 2] as f64 / 255.0;
            let luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;

            alpha_sum += weight;
            if alpha >= ALPHA_HARD {
                centroid_weight += weight;
                weighted_x += local_x * weight;
                weighted_y += local_y * weight;
            }
            luminance_sum += luminance * weight;
            luminance_sq_sum += luminance * luminance * weight;
        }
    }

    let mut hard_mask = vec![0u8; bbox_area];
    for local_y in 0..bbox_h {
        let source_y = metrics.bbox_min_y as usize + local_y;
        let row_start = source_y * width * 4;
        for local_x in 0..bbox_w {
            let source_x = metrics.bbox_min_x as usize + local_x;
            let alpha = src.rgba[row_start + source_x * 4 + 3];
            hard_mask[local_y * bbox_w + local_x] = u8::from(alpha >= ALPHA_HARD);
        }
    }

    let hard_count = hard_mask.iter().map(|&value| value as usize).sum::<usize>();
    let perimeter = hard_perimeter(&hard_mask, bbox_w, bbox_h);
    let component_sizes = component_sizes(&hard_mask, bbox_w, bbox_h);
    let largest_component = component_sizes.iter().copied().max().unwrap_or(0);
    let hole_pixels = enclosed_background_count(&hard_mask, bbox_w, bbox_h);

    let source_area = pixel_count as f32;
    let bbox_area_f = bbox_area as f32;
    let hard_count_f = hard_count as f32;
    let centroid_x = normalized_centroid(weighted_x, centroid_weight, bbox_w);
    let centroid_y = normalized_centroid(weighted_y, centroid_weight, bbox_h);
    let mean_luminance = if alpha_sum > 0.0 {
        (luminance_sum / alpha_sum) as f32
    } else {
        0.0
    };
    let luminance_variance = if alpha_sum > 0.0 {
        (luminance_sq_sum / alpha_sum - (luminance_sum / alpha_sum).powi(2)).max(0.0)
    } else {
        0.0
    };

    let perimeter_f = perimeter as f32;
    let perimeter_ratio = if hard_count > 0 {
        perimeter_f / (4.0 * hard_count_f.sqrt())
    } else {
        0.0
    };
    let circularity = if perimeter > 0 {
        (4.0 * std::f32::consts::PI * hard_count_f / perimeter_f.powi(2)).clamp(0.0, 1.0)
    } else {
        0.0
    };

    Some(IconVisualFeatures {
        source_coverage_10: coverage_10 as f32 / source_area,
        source_coverage_64: coverage_64 as f32 / source_area,
        source_coverage_128: coverage_128 as f32 / source_area,
        source_coverage_224: coverage_224 as f32 / source_area,
        alpha_mass: (alpha_sum / pixel_count as f64) as f32,
        bbox_width_ratio: metrics.bbox_w as f32 / src.w as f32,
        bbox_height_ratio: metrics.bbox_h as f32 / src.h as f32,
        bbox_area_ratio: bbox_area_f / source_area,
        aspect_ratio_log: (metrics.bbox_w as f32 / metrics.bbox_h as f32).ln(),
        solid_fill: metrics.solid_fill as f32,
        centroid_x,
        centroid_y,
        perimeter_ratio,
        circularity,
        hole_ratio: hole_pixels as f32 / bbox_area_f,
        connected_components: component_sizes.len() as f32,
        dominant_component_ratio: if hard_count > 0 {
            largest_component as f32 / hard_count_f
        } else {
            0.0
        },
        mean_luminance,
        luminance_stddev: luminance_variance.sqrt() as f32,
    })
}

fn normalized_centroid(weighted_sum: f64, weight: f64, extent: usize) -> f32 {
    if weight <= 0.0 || extent <= 1 {
        return 0.5;
    }
    ((weighted_sum / weight) / (extent - 1) as f64).clamp(0.0, 1.0) as f32
}

fn hard_perimeter(mask: &[u8], width: usize, height: usize) -> usize {
    let mut perimeter = 0usize;
    for y in 0..height {
        for x in 0..width {
            if mask[y * width + x] == 0 {
                continue;
            }
            perimeter += usize::from(x == 0 || mask[y * width + x - 1] == 0);
            perimeter += usize::from(x + 1 == width || mask[y * width + x + 1] == 0);
            perimeter += usize::from(y == 0 || mask[(y - 1) * width + x] == 0);
            perimeter += usize::from(y + 1 == height || mask[(y + 1) * width + x] == 0);
        }
    }
    perimeter
}

fn component_sizes(mask: &[u8], width: usize, height: usize) -> Vec<usize> {
    let mut visited = vec![false; mask.len()];
    let mut sizes = Vec::new();

    for start in 0..mask.len() {
        if visited[start] || mask[start] == 0 {
            continue;
        }

        let mut queue = VecDeque::from([start]);
        visited[start] = true;
        let mut size = 0usize;

        while let Some(index) = queue.pop_front() {
            size += 1;
            let x = index % width;
            let y = index / width;
            for neighbor in neighbors4(x, y, width, height) {
                if !visited[neighbor] && mask[neighbor] != 0 {
                    visited[neighbor] = true;
                    queue.push_back(neighbor);
                }
            }
        }
        sizes.push(size);
    }

    sizes
}

fn enclosed_background_count(mask: &[u8], width: usize, height: usize) -> usize {
    let mut outside = vec![false; mask.len()];
    let mut queue = VecDeque::new();

    let enqueue = |index: usize, outside: &mut [bool], queue: &mut VecDeque<usize>| {
        if mask[index] == 0 && !outside[index] {
            outside[index] = true;
            queue.push_back(index);
        }
    };

    for x in 0..width {
        enqueue(x, &mut outside, &mut queue);
        enqueue((height - 1) * width + x, &mut outside, &mut queue);
    }
    for y in 0..height {
        enqueue(y * width, &mut outside, &mut queue);
        enqueue(y * width + width - 1, &mut outside, &mut queue);
    }

    while let Some(index) = queue.pop_front() {
        let x = index % width;
        let y = index / width;
        for neighbor in neighbors4(x, y, width, height) {
            if mask[neighbor] == 0 && !outside[neighbor] {
                outside[neighbor] = true;
                queue.push_back(neighbor);
            }
        }
    }

    mask.iter()
        .zip(outside)
        .filter(|(value, is_outside)| **value == 0 && !*is_outside)
        .count()
}

fn neighbors4(x: usize, y: usize, width: usize, height: usize) -> impl Iterator<Item = usize> {
    let left = (x > 0).then(|| y * width + x - 1);
    let right = (x + 1 < width).then(|| y * width + x + 1);
    let up = (y > 0).then(|| (y - 1) * width + x);
    let down = (y + 1 < height).then(|| (y + 1) * width + x);
    [left, right, up, down].into_iter().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_icon(w: u32, h: u32, fill: impl Fn(u32, u32) -> [u8; 4]) -> DecodedIcon {
        let mut rgba = vec![0; w as usize * h as usize * 4];
        for y in 0..h {
            for x in 0..w {
                let index = (y as usize * w as usize + x as usize) * 4;
                rgba[index..index + 4].copy_from_slice(&fill(x, y));
            }
        }
        DecodedIcon { rgba, w, h }
    }

    fn approx(actual: f32, expected: f32, epsilon: f32) {
        assert!(
            (actual - expected).abs() <= epsilon,
            "actual={actual}, expected={expected}, epsilon={epsilon}"
        );
    }

    #[test]
    fn rejects_empty_truncated_and_transparent_inputs() {
        assert!(extract(&DecodedIcon {
            rgba: vec![],
            w: 0,
            h: 0,
        })
        .is_none());
        assert!(extract(&DecodedIcon {
            rgba: vec![0; 7],
            w: 2,
            h: 2,
        })
        .is_none());
        assert!(extract(&make_icon(4, 4, |_, _| [0, 0, 0, 0])).is_none());
    }

    #[test]
    fn filled_square_has_expected_normalized_features() {
        let features = extract(&make_icon(16, 16, |_, _| [255, 255, 255, 255])).unwrap();

        approx(features.source_coverage_10, 1.0, 1e-6);
        approx(features.alpha_mass, 1.0, 1e-6);
        approx(features.bbox_area_ratio, 1.0, 1e-6);
        approx(features.aspect_ratio_log, 0.0, 1e-6);
        approx(features.solid_fill, 1.0, 1e-6);
        approx(features.centroid_x, 0.5, 1e-6);
        approx(features.centroid_y, 0.5, 1e-6);
        approx(features.perimeter_ratio, 1.0, 1e-6);
        approx(features.circularity, std::f32::consts::PI / 4.0, 1e-6);
        approx(features.hole_ratio, 0.0, 1e-6);
        approx(features.connected_components, 1.0, 1e-6);
        approx(features.dominant_component_ratio, 1.0, 1e-6);
        approx(features.mean_luminance, 1.0, 1e-6);
        approx(features.luminance_stddev, 0.0, 1e-6);
        assert_eq!(features.as_array().len(), FEATURE_COUNT);
        assert_eq!(FEATURE_NAMES.len(), FEATURE_COUNT);
        assert!(features.as_array().iter().all(|value| value.is_finite()));
    }

    #[test]
    fn transparent_padding_changes_source_coverage_but_not_solid_fill() {
        let features = extract(&make_icon(10, 10, |x, y| {
            if (3..7).contains(&x) && (3..7).contains(&y) {
                [255, 0, 0, 255]
            } else {
                [0, 0, 0, 0]
            }
        }))
        .unwrap();

        approx(features.source_coverage_128, 0.16, 1e-6);
        approx(features.bbox_area_ratio, 0.16, 1e-6);
        approx(features.solid_fill, 1.0, 1e-6);
    }

    #[test]
    fn ring_reports_an_enclosed_hole() {
        let features = extract(&make_icon(9, 9, |x, y| {
            let on_outer = (1..8).contains(&x)
                && (1..8).contains(&y)
                && (x == 1 || x == 7 || y == 1 || y == 7);
            if on_outer {
                [0, 0, 0, 255]
            } else {
                [0, 0, 0, 0]
            }
        }))
        .unwrap();

        approx(features.hole_ratio, 25.0 / 49.0, 1e-6);
        approx(features.connected_components, 1.0, 1e-6);
    }

    #[test]
    fn disconnected_shapes_report_component_count_and_dominance() {
        let features = extract(&make_icon(10, 4, |x, y| {
            let left = x < 2 && y < 2;
            let right = x >= 7 && y >= 1;
            if left || right {
                [0, 0, 0, 255]
            } else {
                [0, 0, 0, 0]
            }
        }))
        .unwrap();

        approx(features.connected_components, 2.0, 1e-6);
        approx(features.dominant_component_ratio, 9.0 / 13.0, 1e-6);
    }
}

//! Disjoint scissors for the existing full-scene SDF shader. No shape or
//! sampling coordinate changes; only pixels guaranteed to be empty are skipped.
use super::GlassShape;

/// Preserve the sequential smooth union exactly over an entire scissor.
/// Bounds enclose every sample (and its derivative helper) in the rectangle.
/// We can skip b only when h=1 everywhere, or discard an outside-only prefix
/// when h=0 everywhere. Never reorder a union or drop a frame/clip mask.
pub(super) fn contributing_shapes(
    shapes: &[GlassShape],
    scroll_x: f32,
    blend: f32,
    region: (u32, u32, u32, u32),
) -> Vec<GlassShape> {
    if !blend.is_finite() || shapes.iter().any(|s| !matches!(s.shape_type, 0 | 3 | 5)) {
        return shapes.to_vec();
    }
    let k = blend.max(0.0);
    let rect = [
        region.0 as f32 - 2.0,
        region.1 as f32 - 2.0,
        (region.0 + region.2) as f32 + 2.0,
        (region.1 + region.3) as f32 + 2.0,
    ];
    let mut result: Vec<GlassShape> = Vec::new();
    let (mut lower, mut upper) = (1.0e6_f32, 1.0e6_f32);
    for &shape in shapes {
        if shape.is_clip_only() {
            result.push(shape);
            continue;
        }
        let bounds = shape.screen_bounds(scroll_x);
        // Signed AABB distance is a lower bound on the rounded/rotated SDF.
        let shape_lower = (bounds[0] - rect[2])
            .max(rect[0] - bounds[2])
            .max(bounds[1] - rect[3])
            .max(rect[1] - bounds[3]);
        let center = [
            shape.center[0] + if shape.is_scrolling() { scroll_x } else { 0.0 },
            shape.center[1],
        ];
        let dx = (rect[0] - center[0]).abs().max((rect[2] - center[0]).abs());
        let dy = (rect[1] - center[1]).abs().max((rect[3] - center[1]).abs());
        // Distance to any point inside the shape bounds its SDF above. Allow
        // all possible center motion, including a badge's offset parent pivot.
        let shift = if matches!(shape.shape_type, 4..=6) {
            let x = shape.center[0] - shape.motion[0];
            let y = shape.center[1] - shape.motion[1];
            2.0 * (x * x + y * y).sqrt() + 2.0
        } else {
            0.0
        };
        let shape_upper = (dx * dx + dy * dy).sqrt() + shift;
        if !shape_lower.is_finite() || !shape_upper.is_finite() {
            return shapes.to_vec();
        }
        if shape_lower > upper + k + 2.0 && !shape.is_frame() {
            continue;
        }
        // Clipped shapes may be absent at some samples. Keep the old upper
        // bound and prefix unless the shape is unconditionally present.
        let unclipped = shape.clip_rect[2] <= 0.0;
        if unclipped && lower > shape_upper + k + 2.0 {
            // lower > 0 also proves no discarded shape could set inside_tint.
            result.retain(|s| s.is_clip_only() || s.is_frame());
            lower = shape_lower - 1.0;
            upper = shape_upper + 1.0;
        } else {
            lower = lower.min(shape_lower) - k * 0.25 - 1.0;
            if unclipped {
                upper = upper.min(shape_upper) + 1.0;
            }
        }
        result.push(shape);
    }
    result
}

pub(super) fn regions(
    shapes: &[GlassShape],
    scroll_x: f32,
    blend: f32,
    size: (u32, u32),
) -> Vec<(u32, u32, u32, u32)> {
    let full = || vec![(0, 0, size.0, size.1)];
    if !blend.is_finite() {
        return full();
    }
    // Repeated polynomial smooth-min stays >= min(input distances) - blend,
    // regardless of shape count. Add two pixels for derivative helper quads.
    let padding = blend.max(0.0) + 2.0;
    let mut rectangles = Vec::new();
    let mut edges = Vec::new();
    for shape in shapes {
        if shape.is_clip_only() {
            continue;
        }
        let mut half = [shape.size[0] * 0.5, shape.size[1] * 0.5];
        let mut center = shape.center;
        if shape.is_scrolling() {
            center[0] += scroll_x;
        }
        if matches!(shape.shape_type, 4..=6) {
            let rel = [
                (shape.center[0] - shape.motion[0]).abs(),
                (shape.center[1] - shape.motion[1]).abs(),
            ];
            let shift = [
                rel[0] * 0.002 + rel[1] * 0.061,
                rel[0] * 0.061 + rel[1] * 0.002 + 2.0,
            ];
            // |sin(angle)| < .061 and 1-cos(angle) < .002 for the shader's
            // +/- .06 rad wiggle. Bound the entire motion, not a sampled time.
            if matches!(shape.shape_type, 5 | 6) {
                half = [half[0] + half[1] * 0.061, half[1] + half[0] * 0.061];
            }
            half[0] += shift[0];
            half[1] += shift[1];
        }
        let bounds = [
            center[0] - half[0] - padding,
            center[1] - half[1] - padding,
            center[0] + half[0] + padding,
            center[1] + half[1] + padding,
        ];
        if !bounds.iter().all(|v| v.is_finite()) {
            return full();
        }
        let x = bounds[0].clamp(0.0, size.0 as f32).floor() as u32 / 2 * 2;
        let y = bounds[1].clamp(0.0, size.1 as f32).floor() as u32 / 2 * 2;
        let right =
            ((bounds[2].clamp(0.0, size.0 as f32).ceil() as u32).div_ceil(2) * 2).min(size.0);
        let bottom =
            ((bounds[3].clamp(0.0, size.1 as f32).ceil() as u32).div_ceil(2) * 2).min(size.1);
        if x < right && y < bottom {
            rectangles.push((x, y, right, bottom));
            edges.extend([y, bottom]);
        }
    }
    edges.sort_unstable();
    edges.dedup();
    let mut result = Vec::new();
    let mut intervals = Vec::new();
    for band in edges.windows(2) {
        intervals.clear();
        intervals.extend(
            rectangles
                .iter()
                .filter(|r| r.1 <= band[0] && r.3 >= band[1])
                .map(|r| (r.0, r.2)),
        );
        intervals.sort_unstable();
        let mut merged: Option<(u32, u32)> = None;
        for &(x, right) in &intervals {
            match merged {
                Some((left, end)) if x <= end => merged = Some((left, end.max(right))),
                Some((left, end)) => {
                    result.push((left, band[0], end - left, band[1] - band[0]));
                    merged = Some((x, right));
                }
                None => merged = Some((x, right)),
            }
        }
        if let Some((left, end)) = merged {
            result.push((left, band[0], end - left, band[1] - band[0]));
        }
    }
    // A dense panel can cover the viewport instead of preserving small holes.
    // The SDF still clears empty samples. Subdivide wide unions as well: a
    // whole row gives contributing_shapes such loose bounds that distant
    // folders survive in every fragment's loop during scrolling and wiggle.
    let area: u64 = result.iter().map(|r| u64::from(r.2) * u64::from(r.3)).sum();
    if area * 4 >= u64::from(size.0) * u64::from(size.1) * 3 {
        result = full();
    }
    // Windows retains the complete union for GPU rounding fidelity. Smaller
    // cells cannot reduce its shader loop, so keep the merged coverage and
    // avoid extra draws/shape uploads there. Empty pixels are still skipped.
    if cfg!(target_os = "windows") {
        return result;
    }
    let mut bounded = Vec::new();
    for (x, y, width, height) in result {
        // Even boundaries keep derivative helper quads inside one draw.
        // This changes neither pixel density nor the set of covered pixels.
        for top in (y..y + height).step_by(256) {
            for left in (x..x + width).step_by(256) {
                bounded.push((
                    left,
                    top,
                    (x + width - left).min(256),
                    (y + height - top).min(256),
                ));
            }
        }
    }
    bounded
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn regional_union_keeps_mask_and_nearby_shapes_in_original_order() {
        let mut shapes = vec![GlassShape::clip_rounded_rect(
            [400.0, 60.0],
            [800.0, 120.0],
            8.0,
        )];
        shapes.extend((0..20).map(|i| {
            GlassShape::rounded_rect([50.0 + i as f32 * 200.0, 60.0], [60.0, 60.0], 20.0)
        }));
        let subset = contributing_shapes(&shapes, 0.0, 16.0, (428, 38, 44, 44));
        assert!(subset.len() < shapes.len() / 2);
        assert_eq!(subset[0], shapes[0]);
        assert!(subset.contains(&shapes[3]));
        let indices: Vec<_> = subset
            .iter()
            .map(|s| shapes.iter().position(|x| x == s).unwrap())
            .collect();
        assert!(indices.windows(2).all(|p| p[0] < p[1]));
    }
    #[test]
    fn regions_are_disjoint_and_keep_smooth_union_bridges() {
        let shapes = [
            GlassShape::rounded_rect([30.0, 30.0], [20.0, 20.0], 5.0),
            GlassShape::rounded_rect([58.0, 30.0], [20.0, 20.0], 5.0),
        ];
        let result = regions(&shapes, 0.0, 16.0, (160, 96));
        let mut covered = vec![false; 160 * 96];
        for (x, y, w, h) in result {
            for yy in y..y + h {
                for xx in x..x + w {
                    let cell = &mut covered[(yy * 160 + xx) as usize];
                    assert!(!*cell, "overlapping draws");
                    *cell = true;
                }
            }
        }
        assert!(covered[30 * 160 + 44], "smooth-union gap omitted");
        assert!(!covered[90 * 160 + 150]);
    }
}

//! Generate signed-distance-field textures for context-menu brand logos.
//!
//! Implements the Jump Flooding Algorithm (JFA) with the 1+JFA refinement
//! described in <https://jobtalle.com/distance_field_transparency.html>.
//! Two Voronoi diagrams (seeds = opaque pixels, and seeds = transparent
//! pixels) are computed; each texel's signed distance to the silhouette edge
//! is derived from them, normalized to an alpha field where 0.5 is the edge,
//! values above 0.5 are inside the shape, below 0.5 are outside. The renderer
//! reconstructs a crisp, resolution-independent edge from this field (see
//! `shader_control.wgsl`
//! kind 19/20), so the logo stays sharp at any DPI without a large texture.
//!
//! The color-texture / mipmap-bleed handling from the article is unnecessary
//! here: menu brand logos are monochrome, tinted by the instance color, so we
//! only need the single-channel distance field.
//!
//! Usage:
//!   cargo run --release --example generate_logo_sdf -- \
//!       <input.png|input.svg> <output.png> [output_size]
//!
//! SVG input is rasterized with `rsvg-convert` (librsvg) at 256×256 first, so
//! install librsvg (`brew install librsvg`) when generating from an SVG.
//! PNG input is decoded directly with the `image` crate.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Default edge length (texels) of the generated SDF texture. The SDF stores a
/// higher-resolution shape in a small texture, so 128×128 keeps the atlas
/// compact while preserving thin strokes (Steam's pipes) that 64×64 crushed.
const DEFAULT_OUTPUT_SIZE: u32 = 128;
/// Rasterization resolution for SVG sources (and the resolution JFA runs at
/// before downscaling). Must be well above the output size so the silhouette
/// is sampled densely enough for an accurate distance field.
const SOURCE_RASTER_SIZE: u32 = 256;

struct RgbaImage {
    width: u32,
    height: u32,
    /// RGBA8, row-major, top row first.
    pixels: Vec<u8>,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: generate_logo_sdf <input.png|input.svg> <output.png> [output_size]");
        std::process::exit(1);
    }
    let input = PathBuf::from(&args[1]);
    let output = PathBuf::from(&args[2]);
    let output_size: u32 = args
        .get(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_OUTPUT_SIZE);

    let rgba = load_input_rgba(&input);
    let (w, h) = (rgba.width, rgba.height);

    // Alpha mask: >127 counts as inside the silhouette.
    let alpha: Vec<u8> = rgba.pixels.chunks(4).map(|px| px[3]).collect();

    // Two Voronoi diagrams via JFA (with the 1+JFA refinement).
    let opaque_voronoi = jfa(&alpha, w, h, /* want_opaque */ true);
    let transparent_voronoi = jfa(&alpha, w, h, /* want_opaque */ false);

    // Signed distance field: inside = negative, outside = positive.
    let sdf = compute_signed_distance(&alpha, &opaque_voronoi, &transparent_voronoi, w, h);

    // Normalize to [0,1] with 0.5 at the edge, values above 0.5 inside. The
    // spread (distance over which the field ramps from fully outside to fully
    // inside) is a *small* fraction of the source size: using the whole-field
    // max distance made 1px edges nearly flat in alpha and crushed thin
    // strokes. A spread of ~source/8 keeps the gradient steep at the edge so
    // thin lines survive, and makes strokes read slightly heavier (inside
    // saturates to 1 quickly), matching the "icons look a bit bigger" intent.
    let normalized = normalize_to_alpha(&sdf, w.max(h));

    // Downscale the high-resolution field to the compact output size.
    let downscaled = downsample_box(&normalized, w, h, output_size, output_size);

    save_sdf_png(&output, &downscaled, output_size, output_size);

    println!(
        "generated SDF: {} ({}x{} -> {}x{})",
        output.display(),
        w,
        h,
        output_size,
        output_size
    );
}

fn load_input_rgba(path: &Path) -> RgbaImage {
    let is_svg = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("svg"))
        .unwrap_or(false);

    let png_path = if is_svg {
        // SVG: rasterize via rsvg-convert into a temporary PNG. The `image`
        // crate does not decode SVG, so this keeps the generator usable for
        // both source kinds without pulling in a heavy resvg/usvg dependency.
        let tmp =
            std::env::temp_dir().join(format!("launchpad-sdf-input-{}.png", std::process::id()));
        let status = Command::new("rsvg-convert")
            .args([
                "-w",
                &SOURCE_RASTER_SIZE.to_string(),
                "-h",
                &SOURCE_RASTER_SIZE.to_string(),
            ])
            .arg(path)
            .args(["-o", &tmp.to_string_lossy()])
            .status()
            .expect(
                "rsvg-convert の実行に失敗。SVG 入力には librsvg が必要です \
                 (macOS: `brew install librsvg`)",
            );
        assert!(status.success(), "rsvg-convert が失敗しました: {status}");
        tmp
    } else {
        path.to_path_buf()
    };

    let img = image::open(&png_path)
        .unwrap_or_else(|err| panic!("画像の読み込みに失敗 ({:?}): {err}", png_path))
        .to_rgba8();
    let (w, h) = img.dimensions();
    RgbaImage {
        width: w,
        height: h,
        pixels: img.into_raw(),
    }
}

/// Sentinel marking "no seed reached this texel yet".
const NONE: (i32, i32) = (i32::MIN, i32::MIN);

/// Jump Flooding Algorithm (JFA) with the 1+JFA refinement.
///
/// Returns, for every texel, the coordinates of the nearest seed pixel. When
/// `want_opaque` is true the seeds are the opaque texels (alpha > 127); when
/// false the seeds are the transparent texels. `NONE` marks texels whose seed
/// set is empty (e.g. an all-opaque image has no transparent seeds).
///
/// Plain JFA leaves a small error rate (texels snapped to a slightly wrong
/// seed). 1+JFA runs one extra pass at step = 1 to correct the bulk of these.
fn jfa(alpha: &[u8], w: u32, h: u32, want_opaque: bool) -> Vec<(i32, i32)> {
    let (w, h) = (w as i32, h as i32);
    let mut grid = vec![NONE; (w * h) as usize];

    // Initialize: seed texels carry their own coordinates.
    for y in 0..h {
        for x in 0..w {
            let is_opaque = alpha[(y * w + x) as usize] > 127;
            let is_seed = if want_opaque { is_opaque } else { !is_opaque };
            if is_seed {
                grid[(y * w + x) as usize] = (x, y);
            }
        }
    }

    // JFA passes: largest power of two < max_dim, halving down to 1.
    let max_dim = w.max(h);
    let mut step = 1;
    while step * 2 < max_dim {
        step *= 2;
    }
    while step >= 1 {
        jfa_pass(&mut grid, w, h, step);
        step /= 2;
    }
    // 1+JFA: one more unit pass to reduce the residual JFA error.
    jfa_pass(&mut grid, w, h, 1);

    grid
}

/// One JFA pass: each texel looks at its 8 neighbours at ±`step` and keeps the
/// closest seed. Uses a double buffer so the pass reads the previous state.
fn jfa_pass(grid: &mut Vec<(i32, i32)>, w: i32, h: i32, step: i32) {
    let mut next = grid.clone();
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;
            let mut best = grid[idx];
            let mut best_d = dist2(x, y, best);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let nx = x + dx * step;
                    let ny = y + dy * step;
                    if !(0..w).contains(&nx) || !(0..h).contains(&ny) {
                        continue;
                    }
                    let cand = grid[(ny * w + nx) as usize];
                    if cand == NONE {
                        continue;
                    }
                    let d = dist2(x, y, cand);
                    if d < best_d {
                        best_d = d;
                        best = cand;
                    }
                }
            }
            next[idx] = best;
        }
    }
    *grid = next;
}

/// Squared euclidean distance from texel `(x, y)` to seed `q` (`i64::MAX` if
/// `q` is `NONE`). Squared distance is sufficient for comparisons.
fn dist2(x: i32, y: i32, q: (i32, i32)) -> i64 {
    if q == NONE {
        return i64::MAX;
    }
    let dx = (x - q.0) as i64;
    let dy = (y - q.1) as i64;
    dx * dx + dy * dy
}

/// Signed distance from every texel to the silhouette edge, in source pixels.
///
/// Inside the shape the distance is the distance to the nearest transparent
/// texel, negated; outside it is the distance to the nearest opaque texel. The
/// edge itself is 0. Texels whose opposite seed set is empty (no transparent /
/// no opaque seeds at all) get a large magnitude so normalization clamps them
/// to fully inside / fully outside.
fn compute_signed_distance(
    alpha: &[u8],
    opaque: &[(i32, i32)],
    transparent: &[(i32, i32)],
    w: u32,
    h: u32,
) -> Vec<f32> {
    let (w, h) = (w as usize, h as usize);
    let mut sdf = vec![0.0f32; w * h];
    let large = (w.max(h) as f32) * 2.0;
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let is_opaque = alpha[i] > 127;
            let signed = if is_opaque {
                // Inside: distance to nearest transparent texel, negated.
                match transparent[i] {
                    NONE => -large,
                    t => -((t.0 as f32 - x as f32).hypot(t.1 as f32 - y as f32)),
                }
            } else {
                // Outside: distance to nearest opaque texel, positive.
                match opaque[i] {
                    NONE => large,
                    o => (o.0 as f32 - x as f32).hypot(o.1 as f32 - y as f32),
                }
            };
            sdf[i] = signed;
        }
    }
    sdf
}

/// Map signed distances (px) to [0,1] alpha where 0.5 is the edge, values
/// above 0.5 inside.
///
/// `spread` is the distance over which the field ramps from fully inside to
/// fully outside. It must be small (a fraction of the source size), not the
/// whole-field max distance: a huge spread flattens 1px edges in alpha and
/// crushes thin strokes. With `source_size/8` the gradient stays steep at the
/// edge so thin lines survive and strokes read slightly heavier (inside
/// saturates to 1 quickly). The shader divides by `fwidth` of this value for
/// 1 px AA.
fn normalize_to_alpha(sdf: &[f32], source_size: u32) -> Vec<f32> {
    let spread = (source_size as f32 / 8.0).max(4.0);
    sdf.iter()
        .map(|&d| (0.5 - d / (2.0 * spread)).clamp(0.0, 1.0))
        .collect()
}

/// Box-filter downscale of the high-resolution field to the output size.
fn downsample_box(src: &[f32], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<f32> {
    let (sw, sh, dw, dh) = (sw as usize, sh as usize, dw as usize, dh as usize);
    let mut dst = vec![0.0f32; dw * dh];
    let sx = sw as f32 / dw as f32;
    let sy = sh as f32 / dh as f32;
    for dy in 0..dh {
        for dx in 0..dw {
            let x0 = (dx as f32 * sx).floor() as usize;
            let x1 = (((dx as f32 + 1.0) * sx).ceil() as usize).min(sw);
            let y0 = (dy as f32 * sy).floor() as usize;
            let y1 = (((dy as f32 + 1.0) * sy).ceil() as usize).min(sh);
            let mut sum = 0.0f32;
            let mut cnt = 0usize;
            for yy in y0..y1.max(y0 + 1) {
                for xx in x0..x1.max(x0 + 1) {
                    sum += src[yy * sw + xx];
                    cnt += 1;
                }
            }
            dst[dy * dw + dx] = sum / cnt.max(1) as f32;
        }
    }
    dst
}

/// Write the field as an 8-bit single-channel (L8) PNG.
fn save_sdf_png(path: &Path, sdf: &[f32], w: u32, h: u32) {
    let pixels: Vec<u8> = sdf
        .iter()
        .map(|&v| (v * 255.0).round().clamp(0.0, 255.0) as u8)
        .collect();
    image::save_buffer(path, &pixels, w, h, image::ExtendedColorType::L8)
        .unwrap_or_else(|err| panic!("SDF PNG の保存に失敗 ({:?}): {err}", path));
}

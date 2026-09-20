//! Auto-darkness QA harness for the liquid-glass backdrop classifier
//! (issue #186).
//!
//! What this binary does, in order:
//!
//! 1. Generates a synthetic backdrop corpus (12 cases, 512x320 PNG) into
//!    `docs/qa/auto-darkness/corpus/`. Each case carries a "golden bucket"
//!    (none/weak/medium/strong) encoding the expected human judgment.
//! 2. Mirrors the backdrop-statistics shader math on the CPU for the three
//!    `AutoDarknessStrategy` variants (Current, CoverageTuned,
//!    ContrastPerceptual), sampling the sRGB-encoded texels exactly like the
//!    WGSL `sample_grid` (16x16 cells, texel-center UV, no pre-linearization).
//! 3. Scores every image x strategy: white-label readability (contrast
//!    against the darkened background), over-dark penalty on dark images,
//!    and golden-bucket agreement.
//! 4. Scans the CoverageTuned constant space (luma_lo x knee_lo x knee_hi)
//!    and the ContrastPerceptual space (L_target x quantile), ranks by
//!    bucket agreement -> violations -> over-dark penalty.
//! 5. Dispatches the real shader headlessly (wgpu) per corpus image and
//!    compares `target_darkness` against the CPU mirror (parity, tolerance
//!    0.01). Strategy 1/2 branches may still be unimplemented in the shader;
//!    in that case those columns are recorded as "pending re-test" instead
//!    of failing the run.
//! 6. Renders a contact sheet and writes `docs/qa/auto-darkness/results.md`.
//!
//! Run: `cargo run --bin darkness_qa` (add `-- --skip-gpu` to skip the wgpu
//! parity section). Tests: `cargo test --bin darkness_qa`.

use std::fs;
use std::path::{Path, PathBuf};

use image::{Rgba, RgbaImage};

// ---------------------------------------------------------------------------
// Constants shared by the CPU mirror and the harness configuration
// ---------------------------------------------------------------------------

/// Corpus image size (the app captures backdrop at arbitrary resolutions; the
/// corpus picks 512x320 so the 16x16 grid hits exact texel centers: 32x20
/// texels per cell).
const IMAGE_W: u32 = 512;
const IMAGE_H: u32 = 320;
const GRID: u32 = 16;
const GRID_SAMPLES: usize = (GRID * GRID) as usize; // 256

/// Mirrors the app default `auto_darkness_max`.
const MAX_DARKNESS: f32 = 0.16;

/// Readability model: a white label of luminance 0.95 sits on the darkened
/// background (sample luminance L darkened to L*(1-d)); contrast uses the
/// standard +0.05 floor. A sample is a violation below 3:1.
const WHITE_LABEL_LUMINANCE: f32 = 0.95;
const CONTRAST_FLOOR: f32 = 0.05;
const MIN_CONTRAST: f32 = 3.0;

/// Over-dark rule: on images whose p50 linear luminance is below this, any
/// darkness above the threshold counts as a penalty equal to the darkness.
const OVER_DARK_P50_LINEAR: f32 = 0.18;
const OVER_DARK_DARKNESS_THRESHOLD: f32 = 0.02;

/// GPU parity tolerance on target_darkness.
const PARITY_TOLERANCE: f32 = 0.01;

/// Golden bucket thresholds (tentative).
const BUCKET_WEAK: f32 = 0.015;
const BUCKET_MEDIUM: f32 = 0.06;
const BUCKET_STRONG: f32 = 0.115;

// ---------------------------------------------------------------------------
// Bucket model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Bucket {
    None,
    Weak,
    Medium,
    Strong,
}

impl Bucket {
    fn from_darkness(d: f32) -> Self {
        if d < BUCKET_WEAK {
            Bucket::None
        } else if d < BUCKET_MEDIUM {
            Bucket::Weak
        } else if d < BUCKET_STRONG {
            Bucket::Medium
        } else {
            Bucket::Strong
        }
    }

    fn label(self) -> &'static str {
        match self {
            Bucket::None => "none",
            Bucket::Weak => "weak",
            Bucket::Medium => "medium",
            Bucket::Strong => "strong",
        }
    }
}

// ---------------------------------------------------------------------------
// CPU mirror of the shader math (src/liquid_glass/params.rs +
// assets/shaders/liquid_glass_backdrop_stats.wgsl)
// ---------------------------------------------------------------------------

/// WGSL `smoothstep(edge0, edge1, x)`: t*t*(3-2t) with t clamped to [0,1].
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// BT.601 luma over sRGB-encoded values, as in the shader.
fn luma_bt601(rgb: [f32; 3]) -> f32 {
    0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2]
}

/// max(rgb) - min(rgb), as in the shader.
fn chroma(rgb: [f32; 3]) -> f32 {
    rgb[0].max(rgb[1]).max(rgb[2]) - rgb[0].min(rgb[1]).min(rgb[2])
}

/// Shader `white_score` with tunable constants (strategy 0 uses
/// TunedParams::CURRENT, strategy 1 scans the tunables).
fn white_score(rgb: [f32; 3], p: &TunedParams) -> f32 {
    let bright = smoothstep(p.luma_lo, p.luma_hi, luma_bt601(rgb));
    let neutral = 1.0 - smoothstep(p.chroma_lo, p.chroma_hi, chroma(rgb));
    bright * neutral
}

/// sRGB-encoded [0,1] -> linear, piecewise as in the sRGB spec.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Relative luminance from linear RGB (Rec. 709 primaries).
fn rel_luminance(rgb: [f32; 3]) -> f32 {
    0.2126 * srgb_to_linear(rgb[0])
        + 0.7152 * srgb_to_linear(rgb[1])
        + 0.0722 * srgb_to_linear(rgb[2])
}

/// Exact mirror of the shader strategy-2 quantile: 8-bin histogram over
/// relative luminance [0, 1], binary descent for the first bin whose
/// cumulative count reaches `q * 256`, representative = bin center. The WGSL
/// quantizes to bin centers (step 0.125), so this replaces a sorted
/// nearest-rank value to keep GPU parity exact.
fn histogram_quantile_luma(samples: &[[f32; 3]], q: f32) -> f32 {
    const BINS: usize = 8;
    let mut bins = [0u32; BINS];
    for rgb in samples {
        let bin = ((rel_luminance(*rgb) * BINS as f32) as usize).min(BINS - 1);
        bins[bin] += 1;
    }
    let target = q * GRID_SAMPLES as f32;
    let mut p90_bin = 0usize;
    let mut cumulative = 0.0f32;
    let lower_half = (bins[0] + bins[1] + bins[2] + bins[3]) as f32;
    if cumulative + lower_half < target {
        cumulative += lower_half;
        p90_bin = 4;
    }
    let group = (bins[p90_bin] + bins[p90_bin + 1]) as f32;
    if cumulative + group < target {
        cumulative += group;
        p90_bin += 2;
    }
    if cumulative + (bins[p90_bin] as f32) < target {
        p90_bin += 1;
    }
    (p90_bin as f32 + 0.5) / BINS as f32
}

/// Exact mirror of the shader `sample_grid`: uv = ((x+0.5)/GRID, (y+0.5)/GRID),
/// sampled with a nearest sampler at mip 0 -> the texel at the cell center,
/// read as raw sRGB-encoded values (no pre-linearization).
fn sample_texel(img: &RgbaImage, x: u32, y: u32) -> [f32; 3] {
    let tx = (((x as f32 + 0.5) / GRID as f32) * img.width() as f32).floor() as u32;
    let ty = (((y as f32 + 0.5) / GRID as f32) * img.height() as f32).floor() as u32;
    let p = img.get_pixel(tx.min(img.width() - 1), ty.min(img.height() - 1));
    [
        p[0] as f32 / 255.0,
        p[1] as f32 / 255.0,
        p[2] as f32 / 255.0,
    ]
}

/// The 256 grid samples of an image, row-major (y, then x).
fn grid_samples(img: &RgbaImage) -> Vec<[f32; 3]> {
    (0..GRID)
        .flat_map(|y| (0..GRID).map(move |x| sample_texel(img, x, y)))
        .collect()
}

fn coverage_from_samples(samples: &[[f32; 3]], p: &TunedParams) -> f32 {
    samples.iter().map(|rgb| white_score(*rgb, p)).sum::<f32>() / GRID_SAMPLES as f32
}

/// Median linear luminance of the whole image (used by the over-dark rule).
fn image_p50_linear(img: &RgbaImage) -> f32 {
    let mut lums: Vec<f32> = img
        .pixels()
        .map(|p| {
            rel_luminance([
                p[0] as f32 / 255.0,
                p[1] as f32 / 255.0,
                p[2] as f32 / 255.0,
            ])
        })
        .collect();
    lums.sort_by(f32::total_cmp);
    lums[lums.len() / 2]
}

// ---------------------------------------------------------------------------
// Strategy parameters
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct TunedParams {
    luma_lo: f32,
    luma_hi: f32,
    chroma_lo: f32,
    chroma_hi: f32,
    knee_lo: f32,
    knee_hi: f32,
}

impl TunedParams {
    /// PR #184 baseline constants.
    const CURRENT: Self = Self {
        luma_lo: 0.68,
        luma_hi: 0.90,
        chroma_lo: 0.10,
        chroma_hi: 0.26,
        knee_lo: 0.55,
        knee_hi: 0.82,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StrategyKind {
    Current,
    CoverageTuned,
    ContrastPerceptual,
}

#[derive(Debug, Clone, Copy)]
struct StrategyParams {
    kind: StrategyKind,
    tuned: TunedParams,
    l_target: f32,
    quantile: f32,
}

impl StrategyParams {
    fn current() -> Self {
        Self {
            kind: StrategyKind::Current,
            tuned: TunedParams::CURRENT,
            l_target: 0.25,
            quantile: 0.90,
        }
    }

    fn tuned(p: TunedParams) -> Self {
        Self {
            kind: StrategyKind::CoverageTuned,
            tuned: p,
            l_target: 0.25,
            quantile: 0.90,
        }
    }

    fn perceptual(l_target: f32, quantile: f32) -> Self {
        Self {
            kind: StrategyKind::ContrastPerceptual,
            tuned: TunedParams::CURRENT,
            l_target,
            quantile,
        }
    }

    fn key(self) -> &'static str {
        match self.kind {
            StrategyKind::Current => "current",
            StrategyKind::CoverageTuned => "coverage-tuned",
            StrategyKind::ContrastPerceptual => "contrast-perceptual",
        }
    }

    /// Compact human-readable label used in the scan tables.
    fn label(self) -> String {
        match self.kind {
            StrategyKind::Current => "current".to_string(),
            StrategyKind::CoverageTuned => format!(
                "luma {:.2}/{:.2} chroma {:.2}/{:.2} knee {:.2}/{:.2}",
                self.tuned.luma_lo,
                self.tuned.luma_hi,
                self.tuned.chroma_lo,
                self.tuned.chroma_hi,
                self.tuned.knee_lo,
                self.tuned.knee_hi
            ),
            StrategyKind::ContrastPerceptual => {
                format!("L_target {:.2} q {:.2}", self.l_target, self.quantile)
            }
        }
    }
}

/// Strategy 0/1/2: mean white-coverage mapped through a smoothstep knee.
fn coverage_darkness(samples: &[[f32; 3]], p: &TunedParams, knee_lo: f32, knee_hi: f32) -> f32 {
    let c = coverage_from_samples(samples, p);
    smoothstep(knee_lo, knee_hi, c) * MAX_DARKNESS
}

/// Strategy 2: sRGB -> linear -> relative luminance per sample, histogram
/// quantile (bin-center convention, mirroring the WGSL), then
/// clamp(1 - L_target / max(p, 1e-4), 0, MAX_DARKNESS).
fn perceptual_darkness(samples: &[[f32; 3]], l_target: f32, q: f32) -> f32 {
    let p = histogram_quantile_luma(samples, q);
    (1.0 - l_target / p.max(1e-4)).clamp(0.0, MAX_DARKNESS)
}

fn strategy_darkness(samples: &[[f32; 3]], sp: &StrategyParams) -> f32 {
    match sp.kind {
        StrategyKind::Current => coverage_darkness(samples, &TunedParams::CURRENT, 0.55, 0.82),
        StrategyKind::CoverageTuned => {
            coverage_darkness(samples, &sp.tuned, sp.tuned.knee_lo, sp.tuned.knee_hi)
        }
        StrategyKind::ContrastPerceptual => perceptual_darkness(samples, sp.l_target, sp.quantile),
    }
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct Score {
    darkness: f32,
    violations: u32,
    worst_contrast: f32,
    over_dark: f32,
    bucket: Bucket,
}

/// Contrast of a white label (0.95) against the sample background darkened by
/// `darkness`: (0.95 + 0.05) / (L*(1-d) + 0.05).
fn label_contrast(sample_linear_luminance: f32, darkness: f32) -> f32 {
    (WHITE_LABEL_LUMINANCE + CONTRAST_FLOOR)
        / (sample_linear_luminance * (1.0 - darkness) + CONTRAST_FLOOR)
}

fn score_image(data: &ImageData, sp: &StrategyParams) -> Score {
    let darkness = strategy_darkness(&data.samples, sp);
    let mut violations = 0u32;
    let mut worst = f32::INFINITY;
    for rgb in &data.samples {
        let contrast = label_contrast(rel_luminance(*rgb), darkness);
        if contrast < MIN_CONTRAST {
            violations += 1;
        }
        worst = worst.min(contrast);
    }
    let over_dark =
        if data.p50_linear < OVER_DARK_P50_LINEAR && darkness > OVER_DARK_DARKNESS_THRESHOLD {
            darkness
        } else {
            0.0
        };
    Score {
        darkness,
        violations,
        worst_contrast: worst,
        over_dark,
        bucket: Bucket::from_darkness(darkness),
    }
}

// ---------------------------------------------------------------------------
// Corpus
// ---------------------------------------------------------------------------

/// Per-image analysis inputs (strategy-independent, computed once).
struct ImageData {
    id: CorpusId,
    img: RgbaImage,
    samples: Vec<[f32; 3]>,
    p50_linear: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CorpusId {
    PureWhite,
    WhiteWeb,
    BrightSaturatedYellow,
    BrightSaturatedBlue,
    LightGray,
    MidGray,
    Dark,
    DarkPhoto,
    HalfSplitWhiteDark,
    SmallWhiteWindow,
    BrightPhoto,
    DesktopLike,
}

impl CorpusId {
    const ALL: [Self; 12] = [
        Self::PureWhite,
        Self::WhiteWeb,
        Self::BrightSaturatedYellow,
        Self::BrightSaturatedBlue,
        Self::LightGray,
        Self::MidGray,
        Self::Dark,
        Self::DarkPhoto,
        Self::HalfSplitWhiteDark,
        Self::SmallWhiteWindow,
        Self::BrightPhoto,
        Self::DesktopLike,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::PureWhite => "pure_white",
            Self::WhiteWeb => "white_web",
            Self::BrightSaturatedYellow => "bright_saturated_yellow",
            Self::BrightSaturatedBlue => "bright_saturated_blue",
            Self::LightGray => "light_gray",
            Self::MidGray => "mid_gray",
            Self::Dark => "dark",
            Self::DarkPhoto => "dark_photo",
            Self::HalfSplitWhiteDark => "half_split_white_dark",
            Self::SmallWhiteWindow => "small_white_window",
            Self::BrightPhoto => "bright_photo",
            Self::DesktopLike => "desktop_like",
        }
    }

    /// Tentative human-judgment bucket. Edit this table to adjust the target.
    fn golden(self) -> Bucket {
        match self {
            Self::PureWhite => Bucket::Strong,
            Self::WhiteWeb => Bucket::Strong,
            Self::BrightSaturatedYellow => Bucket::Medium,
            Self::BrightSaturatedBlue => Bucket::Medium,
            Self::LightGray => Bucket::Weak,
            Self::MidGray => Bucket::None,
            Self::Dark => Bucket::None,
            Self::DarkPhoto => Bucket::None,
            Self::HalfSplitWhiteDark => Bucket::Medium,
            Self::SmallWhiteWindow => Bucket::Weak,
            Self::BrightPhoto => Bucket::Medium,
            Self::DesktopLike => Bucket::Weak,
        }
    }

    /// Deterministic procedural painter (no RNG, so results are reproducible).
    fn paint(self, img: &mut RgbaImage) {
        match self {
            Self::PureWhite => fill(img, [255, 255, 255]),
            Self::WhiteWeb => {
                fill(img, [255, 255, 255]);
                // Light-gray content blocks.
                rect(img, 60, 40, 180, 120, [205, 205, 205]);
                rect(img, 320, 70, 140, 100, [210, 210, 210]);
                rect(img, 40, 230, 220, 50, [200, 200, 200]);
                // Text-like thin dark lines (the "web").
                for y in (150..190).step_by(10) {
                    for x in (20..490).step_by(60) {
                        rect(img, x, y, 40, 3, [70, 70, 70]);
                    }
                }
                rect(img, 260, 200, 3, 80, [70, 70, 70]);
            }
            Self::BrightSaturatedYellow => {
                fill(img, [255, 230, 0]);
                rect(img, 80, 60, 160, 100, [235, 210, 0]);
                rect(img, 300, 180, 150, 90, [240, 216, 0]);
            }
            Self::BrightSaturatedBlue => {
                fill(img, [0, 110, 255]);
                // Darker navy "text area".
                rect(img, 40, 220, 220, 70, [0, 45, 115]);
            }
            Self::LightGray => {
                fill(img, [192, 192, 192]);
                rect(img, 60, 50, 120, 80, [200, 200, 200]);
                rect(img, 340, 120, 120, 70, [200, 200, 200]);
            }
            Self::MidGray => fill(img, [128, 128, 128]),
            Self::Dark => {
                fill(img, [32, 32, 32]);
                rect(img, 180, 90, 160, 110, [46, 46, 46]);
            }
            Self::DarkPhoto => {
                vgrad(img, [26, 28, 32], [62, 68, 80], 0, IMAGE_H);
                // Dark blob and a dim highlight, photo-like.
                blob(img, 140, 240, 110, [18, 20, 24]);
                blob(img, 380, 60, 90, [70, 76, 88]);
            }
            Self::HalfSplitWhiteDark => {
                fill(img, [255, 255, 255]);
                rect(img, IMAGE_W / 2, 0, IMAGE_W / 2, IMAGE_H, [24, 24, 24]);
            }
            Self::SmallWhiteWindow => {
                fill(img, [40, 42, 48]);
                // ~15% of the area: 200x120 = 24000 / 163840.
                rect(img, 156, 100, 200, 120, [255, 255, 255]);
            }
            Self::BrightPhoto => {
                // Diagonal pastel gradient + light patches + small saturated
                // accent blocks, mimicking a bright colorful photo.
                for y in 0..IMAGE_H {
                    for x in 0..IMAGE_W {
                        let t = (x as f32 / IMAGE_W as f32 + y as f32 / IMAGE_H as f32) / 2.0;
                        let c = lerp3([255, 232, 205], [175, 215, 255], t);
                        img.put_pixel(x, y, Rgba([c[0], c[1], c[2], 255]));
                    }
                }
                rect(img, 60, 40, 140, 90, [240, 246, 250]);
                rect(img, 320, 150, 150, 100, [225, 235, 245]);
                rect(img, 420, 60, 60, 50, [255, 95, 95]);
                rect(img, 40, 230, 80, 60, [90, 255, 140]);
                rect(img, 280, 30, 80, 60, [110, 130, 255]);
            }
            Self::DesktopLike => {
                // Sky gradient, sun, and a dark taskbar.
                for y in 0..IMAGE_H {
                    let t = y as f32 / IMAGE_H as f32;
                    let c = lerp3([218, 228, 242], [152, 162, 186], t);
                    for x in 0..IMAGE_W {
                        img.put_pixel(x, y, Rgba([c[0], c[1], c[2], 255]));
                    }
                }
                blob(img, 256, 80, 60, [250, 246, 232]);
                rect(img, 0, IMAGE_H - 60, IMAGE_W, 60, [26, 28, 34]);
            }
        }
    }

    fn generate(self, dir: &Path) -> ImageData {
        let mut img = RgbaImage::new(IMAGE_W, IMAGE_H);
        self.paint(&mut img);
        let path = dir.join(format!("{}.png", self.name()));
        img.save(&path)
            .unwrap_or_else(|e| panic!("failed to save corpus image {}: {e}", path.display()));
        let samples = grid_samples(&img);
        let p50_linear = image_p50_linear(&img);
        ImageData {
            id: self,
            img,
            samples,
            p50_linear,
        }
    }
}

// -- small deterministic painting helpers ----------------------------------

fn fill(img: &mut RgbaImage, color: [u8; 3]) {
    for p in img.pixels_mut() {
        *p = Rgba([color[0], color[1], color[2], 255]);
    }
}

fn rect(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, color: [u8; 3]) {
    for py in y..y.saturating_add(h).min(img.height()) {
        for px in x..x.saturating_add(w).min(img.width()) {
            img.put_pixel(px, py, Rgba([color[0], color[1], color[2], 255]));
        }
    }
}

fn vgrad(img: &mut RgbaImage, top: [u8; 3], bottom: [u8; 3], y0: u32, y1: u32) {
    let span = (y1 - y0).max(1);
    for y in y0..y1.min(img.height()) {
        let t = (y - y0) as f32 / span as f32;
        let c = lerp3(top, bottom, t);
        for x in 0..img.width() {
            img.put_pixel(x, y, Rgba([c[0], c[1], c[2], 255]));
        }
    }
}

fn blob(img: &mut RgbaImage, cx: u32, cy: u32, radius: u32, color: [u8; 3]) {
    let r2 = (radius * radius) as i64;
    let (cx, cy) = (cx as i64, cy as i64);
    for y in 0..img.height() {
        for x in 0..img.width() {
            let dx = x as i64 - cx;
            let dy = y as i64 - cy;
            if dx * dx + dy * dy <= r2 {
                img.put_pixel(x, y, Rgba([color[0], color[1], color[2], 255]));
            }
        }
    }
}

fn lerp3(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        (a[0] as f32 + (b[0] as f32 - a[0] as f32) * t).round() as u8,
        (a[1] as f32 + (b[1] as f32 - a[1] as f32) * t).round() as u8,
        (a[2] as f32 + (b[2] as f32 - a[2] as f32) * t).round() as u8,
    ]
}

// ---------------------------------------------------------------------------
// Parameter scans
// ---------------------------------------------------------------------------

struct ScanEntry {
    label: String,
    params: StrategyParams,
    agreement: usize,
    violations: u64,
    over_dark: f32,
}

fn scan_coverage_tuned(data: &[ImageData]) -> Vec<ScanEntry> {
    const LUMA_LOS: [f32; 5] = [0.55, 0.60, 0.65, 0.68, 0.72];
    const KNEE_LOS: [f32; 4] = [0.40, 0.47, 0.55, 0.62];
    const KNEE_HIS: [f32; 4] = [0.70, 0.76, 0.82, 0.88];
    // chroma thresholds fixed at the current values; luma_hi = luma_lo + 0.22.

    let mut entries = Vec::with_capacity(LUMA_LOS.len() * KNEE_LOS.len() * KNEE_HIS.len());
    for luma_lo in LUMA_LOS {
        for knee_lo in KNEE_LOS {
            for knee_hi in KNEE_HIS {
                let p = TunedParams {
                    luma_lo,
                    luma_hi: luma_lo + 0.22,
                    chroma_lo: 0.10,
                    chroma_hi: 0.26,
                    knee_lo,
                    knee_hi,
                };
                let params = StrategyParams::tuned(p);
                entries.push(evaluate_scan(data, params));
            }
        }
    }
    rank_scan(entries)
}

fn scan_perceptual(data: &[ImageData]) -> Vec<ScanEntry> {
    const L_TARGETS: [f32; 6] = [0.15, 0.20, 0.25, 0.28, 0.30, 0.35];
    const QUANTILES: [f32; 4] = [0.75, 0.85, 0.90, 0.95];

    let mut entries = Vec::with_capacity(L_TARGETS.len() * QUANTILES.len());
    for l_target in L_TARGETS {
        for q in QUANTILES {
            entries.push(evaluate_scan(data, StrategyParams::perceptual(l_target, q)));
        }
    }
    rank_scan(entries)
}

fn evaluate_scan(data: &[ImageData], params: StrategyParams) -> ScanEntry {
    let mut agreement = 0usize;
    let mut violations = 0u64;
    let mut over_dark = 0.0f32;
    for d in data {
        let s = score_image(d, &params);
        if s.bucket == d.id.golden() {
            agreement += 1;
        }
        violations += u64::from(s.violations);
        over_dark += s.over_dark;
    }
    ScanEntry {
        label: params.label(),
        params,
        agreement,
        violations,
        over_dark,
    }
}

fn rank_scan(mut entries: Vec<ScanEntry>) -> Vec<ScanEntry> {
    entries.sort_by(|a, b| {
        b.agreement
            .cmp(&a.agreement)
            .then(a.violations.cmp(&b.violations))
            .then(a.over_dark.total_cmp(&b.over_dark))
            .then(a.label.cmp(&b.label))
    });
    entries
}

// ---------------------------------------------------------------------------
// GPU parity (best effort; never blocks the other deliverables)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParityStatus {
    Pass,
    Pending,
    Fail,
}

impl ParityStatus {
    fn label(self) -> &'static str {
        match self {
            ParityStatus::Pass => "pass",
            ParityStatus::Pending => "pending (shader branch not implemented)",
            ParityStatus::Fail => "FAIL",
        }
    }
}

struct ParityRow {
    image: &'static str,
    gpu: [f32; 3],
    cpu: [f32; 3],
    status: [ParityStatus; 3],
}

struct GpuParity {
    adapter: String,
    rows: Vec<ParityRow>,
    current_failed: bool,
}

const AUTO_DARKNESS_UNIFORMS_SIZE: u64 = 40;
const AUTO_DARKNESS_STATE_SIZE: u64 = 96;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuUniforms {
    now: f32,
    mode: u32,
    flags: u32,
    max_darkness: f32,
    strategy: u32,
    // 戦略D local-region fields. The parity harness always dispatches the
    // global path (use_region = 0) so the CPU mirror comparison is
    // byte-for-byte unchanged.
    use_region: u32,
    region_origin: [f32; 2],
    region_extent: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuState {
    from_darkness: f32,
    target_darkness: f32,
    transition_started_at: f32,
    filtered_coverage: f32,
    last_full_update_at: f32,
    initialized: u32,
    classification_count: u32,
    displayed_darkness: f32,
    probe_samples: [f32; 16],
}

fn run_gpu_parity(
    data: &[ImageData],
    rec_tuned: &StrategyParams,
    rec_perceptual: &StrategyParams,
) -> Result<GpuParity, String> {
    pollster::block_on(gpu_parity_async(data, rec_tuned, rec_perceptual))
}

async fn gpu_parity_async(
    data: &[ImageData],
    rec_tuned: &StrategyParams,
    rec_perceptual: &StrategyParams,
) -> Result<GpuParity, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = match request_adapter(&instance).await {
        Ok(a) => a,
        Err(e) => return Err(format!("no wgpu adapter: {e}")),
    };
    let adapter_name = adapter.get_info().name;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("darkness qa device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        })
        .await
        .map_err(|e| format!("failed to create wgpu device: {e}"))?;

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("darkness qa shader"),
        source: wgpu::ShaderSource::Wgsl(
            include_str!("../../assets/shaders/liquid_glass_backdrop_stats.wgsl").into(),
        ),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("darkness qa bgl"),
        entries: &[
            texture_entry_for(0, true, wgpu::ShaderStages::COMPUTE),
            sampler_entry_for(1, wgpu::ShaderStages::COMPUTE),
            uniform_entry(2, wgpu::ShaderStages::COMPUTE, AUTO_DARKNESS_UNIFORMS_SIZE),
            storage_entry(
                3,
                wgpu::ShaderStages::COMPUTE,
                false,
                AUTO_DARKNESS_STATE_SIZE,
            ),
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("darkness qa pipeline layout"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("darkness qa pipeline"),
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("cs_main"),
        compilation_options: Default::default(),
        cache: None,
    });

    let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("darkness qa uniforms"),
        size: AUTO_DARKNESS_UNIFORMS_SIZE,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let state = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("darkness qa state"),
        size: AUTO_DARKNESS_STATE_SIZE,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    let current_sp = StrategyParams::current();
    let mut rows = Vec::with_capacity(data.len());
    let mut current_failed = false;

    for d in data {
        // Backdrop texture format matches the app (Rgba8Unorm, raw sRGB
        // bytes) so the shader sees exactly the values the CPU mirror reads.
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("darkness qa backdrop"),
            size: wgpu::Extent3d {
                width: IMAGE_W,
                height: IMAGE_H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            d.img.as_raw(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(IMAGE_W * 4),
                rows_per_image: Some(IMAGE_H),
            },
            wgpu::Extent3d {
                width: IMAGE_W,
                height: IMAGE_H,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("darkness qa bind group"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: state.as_entire_binding(),
                },
            ],
        });

        let mut gpu = [0.0f32; 3];
        for strategy in 0..3u32 {
            queue.write_buffer(&state, 0, bytemuck::bytes_of(&GpuState::default()));
            queue.write_buffer(
                &uniforms,
                0,
                bytemuck::bytes_of(&GpuUniforms {
                    now: 0.0,
                    mode: 0,  // full rate
                    flags: 1, // force classification
                    max_darkness: MAX_DARKNESS,
                    strategy,
                    use_region: 0,
                    region_origin: [0.0; 2],
                    region_extent: [1.0; 2],
                }),
            );
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("darkness qa encoder"),
            });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("darkness qa pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.dispatch_workgroups(1, 1, 1);
            }
            queue.submit(std::iter::once(encoder.finish()));
            gpu[strategy as usize] = read_state_target_darkness(&device, &queue, &state);
        }

        let cpu = [
            strategy_darkness(&d.samples, &current_sp),
            strategy_darkness(&d.samples, rec_tuned),
            strategy_darkness(&d.samples, rec_perceptual),
        ];

        rows.push(ParityRow {
            image: d.id.name(),
            gpu,
            cpu,
            status: [ParityStatus::Pending; 3],
        });
    }

    // Decide implementation status and parity per column once all rows exist.
    let s1_implemented = rows.iter().any(|r| (r.gpu[1] - r.gpu[0]).abs() > 1e-5);
    let s2_implemented = rows.iter().any(|r| (r.gpu[2] - r.gpu[0]).abs() > 1e-5);

    for row in &mut rows {
        let s0_pass = (row.gpu[0] - row.cpu[0]).abs() <= PARITY_TOLERANCE;
        if !s0_pass {
            current_failed = true;
        }
        row.status[0] = if s0_pass {
            ParityStatus::Pass
        } else {
            ParityStatus::Fail
        };
        row.status[1] = if s1_implemented {
            if (row.gpu[1] - row.cpu[1]).abs() <= PARITY_TOLERANCE {
                ParityStatus::Pass
            } else {
                ParityStatus::Fail
            }
        } else {
            ParityStatus::Pending
        };
        row.status[2] = if s2_implemented {
            if (row.gpu[2] - row.cpu[2]).abs() <= PARITY_TOLERANCE {
                ParityStatus::Pass
            } else {
                ParityStatus::Fail
            }
        } else {
            ParityStatus::Pending
        };
    }

    Ok(GpuParity {
        adapter: adapter_name,
        rows,
        current_failed,
    })
}

fn read_state_target_darkness(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    state: &wgpu::Buffer,
) -> f32 {
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("darkness qa readback"),
        size: AUTO_DARKNESS_STATE_SIZE,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("darkness qa readback encoder"),
    });
    encoder.copy_buffer_to_buffer(state, 0, &readback, 0, AUTO_DARKNESS_STATE_SIZE);
    queue.submit(std::iter::once(encoder.finish()));
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("poll darkness qa readback");
    let mapped = slice.get_mapped_range();
    let result: GpuState = bytemuck::pod_read_unaligned(&mapped);
    drop(mapped);
    readback.unmap();
    result.target_darkness
}

async fn request_adapter(instance: &wgpu::Instance) -> Result<wgpu::Adapter, String> {
    if let Ok(adapter) = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        })
        .await
    {
        return Ok(adapter);
    }
    instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        })
        .await
        .map_err(|e| format!("no fallback adapter either: {e}"))
}

fn texture_entry_for(
    binding: u32,
    filterable: bool,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry_for(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn uniform_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    min_binding_size: u64,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: std::num::NonZeroU64::new(min_binding_size),
        },
        count: None,
    }
}

fn storage_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    read_only: bool,
    min_binding_size: u64,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: std::num::NonZeroU64::new(min_binding_size),
        },
        count: None,
    }
}

// ---------------------------------------------------------------------------
// Contact sheet
// ---------------------------------------------------------------------------

/// Rows = corpus images, columns = [Current, CoverageTuned(rec),
/// ContrastPerceptual(rec)]. Each cell: darkness bar, original thumbnail
/// (top 2/3), simulated glass strip with two white label rectangles
/// (bottom 1/3).
fn draw_contact_sheet(
    path: &Path,
    data: &[ImageData],
    scores: &[[Score; 3]],
) -> Result<(), String> {
    const CELL_W: u32 = 224;
    const BAR_H: u32 = 12;
    const THUMB_H: u32 = 136;
    const STRIP_H: u32 = 68;
    const MARGIN: u32 = 10;
    const GAP: u32 = 6;

    let cell_h = BAR_H + THUMB_H + STRIP_H;
    let width = MARGIN + CELL_W * 3 + GAP * 2 + MARGIN;
    let height = MARGIN + cell_h * data.len() as u32 + GAP * (data.len() as u32 - 1) + MARGIN;
    let mut sheet = RgbaImage::new(width, height);
    fill(&mut sheet, [235, 235, 235]);

    for (row, d) in data.iter().enumerate() {
        let y0 = MARGIN + row as u32 * (cell_h + GAP);
        for (col, score) in scores[row].iter().enumerate() {
            let x0 = MARGIN + col as u32 * (CELL_W + GAP);
            // Cell border.
            rect(&mut sheet, x0, y0, CELL_W, cell_h, [120, 120, 120]);
            rect(
                &mut sheet,
                x0 + 1,
                y0 + 1,
                CELL_W - 2,
                cell_h - 2,
                [250, 250, 250],
            );

            // Darkness bar: track + black fill of width proportional to d.
            let bar_y = y0 + 3;
            rect(&mut sheet, x0 + 4, bar_y, CELL_W - 8, 6, [205, 205, 205]);
            let fill_w = ((score.darkness / MAX_DARKNESS) * (CELL_W - 8) as f32).round() as u32;
            if fill_w > 0 {
                rect(&mut sheet, x0 + 4, bar_y, fill_w, 6, [20, 20, 20]);
            }

            // Thumbnail (nearest-neighbor scale of the original).
            let thumb = image::imageops::resize(
                &d.img,
                CELL_W,
                THUMB_H,
                image::imageops::FilterType::Nearest,
            );
            let thumb_y = y0 + BAR_H;
            image::imageops::overlay(&mut sheet, &thumb, i64::from(x0), i64::from(thumb_y));

            // Glass strip: background * (1 - d), then two white label rects.
            let strip_y = thumb_y + THUMB_H;
            for sy in 0..STRIP_H {
                let src_y = (sy * IMAGE_H / STRIP_H).min(IMAGE_H - 1);
                for sx in 0..CELL_W {
                    let src_x = (sx * IMAGE_W / CELL_W).min(IMAGE_W - 1);
                    let src = d.img.get_pixel(src_x, src_y);
                    let dark = [
                        (src[0] as f32 * (1.0 - score.darkness)).round() as u8,
                        (src[1] as f32 * (1.0 - score.darkness)).round() as u8,
                        (src[2] as f32 * (1.0 - score.darkness)).round() as u8,
                    ];
                    sheet.put_pixel(
                        x0 + sx,
                        strip_y + sy,
                        Rgba([dark[0], dark[1], dark[2], 255]),
                    );
                }
            }
            rect(&mut sheet, x0 + 16, strip_y + 30, 90, 10, [255, 255, 255]);
            rect(&mut sheet, x0 + 130, strip_y + 30, 70, 10, [255, 255, 255]);
        }
    }

    sheet
        .save(path)
        .map_err(|e| format!("failed to save contact sheet {}: {e}", path.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// results.md
// ---------------------------------------------------------------------------

fn f3(x: f32) -> String {
    format!("{x:.3}")
}

fn fmt_table_row(cells: &[String]) -> String {
    format!("| {} |", cells.join(" | "))
}

struct ReportData<'a> {
    data: &'a [ImageData],
    strategies: &'a [StrategyParams; 3],
    scores: &'a [[Score; 3]],
    tuned_scan: &'a [ScanEntry],
    perceptual_scan: &'a [ScanEntry],
    parity: &'a Result<GpuParity, String>,
    corpus_dir: &'a Path,
}

fn write_results_md(path: &Path, r: &ReportData<'_>) {
    let data = r.data;
    let scores = r.scores;
    let strategies = r.strategies;

    let agreement_total = |col: usize| -> usize {
        scores
            .iter()
            .enumerate()
            .filter(|(i, row)| row[col].bucket == data[*i].id.golden())
            .count()
    };
    let violations_total = |col: usize| -> u64 {
        scores
            .iter()
            .map(|row| u64::from(row[col].violations))
            .sum()
    };
    let over_dark_total = |col: usize| -> f32 { scores.iter().map(|row| row[col].over_dark).sum() };

    let mut lines = Vec::new();
    lines.push("# Auto-Darkness QA Results".to_string());
    lines.push(String::new());
    lines.push(format!(
        "Generated by `cargo run --bin darkness_qa` on {}. Branch `wip/darkness-harness`.",
        chrono_like_date()
    ));
    lines.push(String::new());
    lines.push(format!(
        "- Corpus: {} synthetic backdrops, {}x{} PNG, in `corpus/` ({}).",
        CorpusId::ALL.len(),
        IMAGE_W,
        IMAGE_H,
        r.corpus_dir
            .strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")))
            .unwrap_or(r.corpus_dir)
            .display()
    ));
    lines.push("- Golden buckets are encoded in `src/bin/darkness_qa.rs` (`CorpusId::golden`) and are tentative; edit that table to re-target the harness.".to_string());
    lines.push(format!(
        "- Settings: max_darkness = {MAX_DARKNESS}, bucket thresholds none [0,{BUCKET_WEAK}) weak [{BUCKET_WEAK},{BUCKET_MEDIUM}) medium [{BUCKET_MEDIUM},{BUCKET_STRONG}) strong [{BUCKET_STRONG},+]."
    ));
    lines.push(format!(
        "- Readability model: white label luminance {WHITE_LABEL_LUMINANCE} over darkened sample luminance L*(1-d); contrast = ({WHITE_LABEL_LUMINANCE}+{CONTRAST_FLOOR})/(L*(1-d)+{CONTRAST_FLOOR}); violation below {MIN_CONTRAST}:1."
    ));
    lines.push(format!(
        "- Over-dark rule: p50 linear luminance < {OVER_DARK_P50_LINEAR} and darkness > {OVER_DARK_DARKNESS_THRESHOLD} -> penalty = darkness."
    ));
    lines.push(String::new());

    // --- corpus table -------------------------------------------------------
    lines.push("## 1. Corpus overview".to_string());
    lines.push(String::new());
    lines.push(fmt_table_row(&[
        "image".into(),
        "golden".into(),
        "strategy".into(),
        "darkness".into(),
        "bucket".into(),
        "violations".into(),
        "worst contrast".into(),
    ]));
    lines.push("| --- | --- | --- | --- | --- | --- | --- |".to_string());
    for (i, d) in data.iter().enumerate() {
        for (col, sp) in strategies.iter().enumerate() {
            let s = scores[i][col];
            lines.push(fmt_table_row(&[
                d.id.name().into(),
                d.id.golden().label().into(),
                sp.key().into(),
                f3(s.darkness),
                s.bucket.label().into(),
                s.violations.to_string(),
                f3(s.worst_contrast),
            ]));
        }
    }
    lines.push(String::new());
    for (col, sp) in strategies.iter().enumerate() {
        lines.push(format!(
            "- {}: golden agreement {}/{} images, {} violation samples, over-dark penalty {:.3}.",
            sp.key(),
            agreement_total(col),
            data.len(),
            violations_total(col),
            over_dark_total(col)
        ));
    }
    lines.push(String::new());

    // --- scans --------------------------------------------------------------
    lines.push("## 2. Parameter scan".to_string());
    lines.push(String::new());
    lines.push(
        "Ranking: golden-bucket agreement (primary) -> violation samples -> over-dark penalty."
            .to_string(),
    );
    lines.push(String::new());

    lines.push("### CoverageTuned (luma_lo x knee_lo x knee_hi; luma_hi = luma_lo + 0.22; chroma 0.10/0.26 fixed)".to_string());
    lines.push(String::new());
    lines.push(fmt_table_row(&[
        "rank".into(),
        "params".into(),
        "agreement".into(),
        "violations".into(),
        "over-dark".into(),
    ]));
    lines.push("| --- | --- | --- | --- | --- |".to_string());
    for (rank, e) in r.tuned_scan.iter().take(5).enumerate() {
        lines.push(fmt_table_row(&[
            (rank + 1).to_string(),
            e.label.clone(),
            format!("{}/{}", e.agreement, data.len()),
            e.violations.to_string(),
            f3(e.over_dark),
        ]));
    }
    lines.push(String::new());

    lines.push("### ContrastPerceptual (L_target x quantile)".to_string());
    lines.push(String::new());
    lines.push(fmt_table_row(&[
        "rank".into(),
        "params".into(),
        "agreement".into(),
        "violations".into(),
        "over-dark".into(),
    ]));
    lines.push("| --- | --- | --- | --- | --- |".to_string());
    for (rank, e) in r.perceptual_scan.iter().take(5).enumerate() {
        lines.push(fmt_table_row(&[
            (rank + 1).to_string(),
            e.label.clone(),
            format!("{}/{}", e.agreement, data.len()),
            e.violations.to_string(),
            f3(e.over_dark),
        ]));
    }
    lines.push(String::new());

    // --- recommended --------------------------------------------------------
    lines.push("## 3. Recommended parameters".to_string());
    lines.push(String::new());
    lines.push(format!("- CoverageTuned: `{}`", r.tuned_scan[0].label));
    lines.push(format!(
        "  (agreement {}/{}, violations {}, over-dark {:.3})",
        r.tuned_scan[0].agreement,
        data.len(),
        r.tuned_scan[0].violations,
        r.tuned_scan[0].over_dark
    ));
    lines.push(format!(
        "- ContrastPerceptual: `{}`",
        r.perceptual_scan[0].label
    ));
    lines.push(format!(
        "  (agreement {}/{}, violations {}, over-dark {:.3})",
        r.perceptual_scan[0].agreement,
        data.len(),
        r.perceptual_scan[0].violations,
        r.perceptual_scan[0].over_dark
    ));
    lines.push(String::new());

    // --- GPU parity ---------------------------------------------------------
    lines.push("## 4. GPU parity".to_string());
    lines.push(String::new());
    lines.push(format!(
        "Headless wgpu dispatch of `assets/shaders/liquid_glass_backdrop_stats.wgsl` (mode=0 full rate, flags=1 force, max_darkness={MAX_DARKNESS}); CPU mirror tolerance {PARITY_TOLERANCE}."
    ));
    lines.push("CoverageTuned column dispatched/compared with the recommended constants; ContrastPerceptual with the recommended L_target/quantile. Quantile convention: 8-bin histogram over relative luminance, first bin whose cumulative count reaches q*256, bin center as the representative (mirrors the WGSL exactly).".to_string());
    lines.push(String::new());
    match r.parity {
        Ok(p) => {
            lines.push(format!("Adapter: {}", p.adapter));
            lines.push(String::new());
            lines.push(fmt_table_row(&[
                "image".into(),
                "gpu cur".into(),
                "cpu cur".into(),
                "cur".into(),
                "gpu tuned".into(),
                "cpu tuned".into(),
                "tuned".into(),
                "gpu perc".into(),
                "cpu perc".into(),
                "perc".into(),
            ]));
            lines.push("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |".to_string());
            for r in &p.rows {
                lines.push(fmt_table_row(&[
                    r.image.into(),
                    f3(r.gpu[0]),
                    f3(r.cpu[0]),
                    r.status[0].label().into(),
                    f3(r.gpu[1]),
                    f3(r.cpu[1]),
                    r.status[1].label().into(),
                    f3(r.gpu[2]),
                    f3(r.cpu[2]),
                    r.status[2].label().into(),
                ]));
            }
            lines.push(String::new());
            if p.current_failed {
                lines.push("**Current parity FAILED** - the CPU mirror no longer matches the shader; investigate before trusting the CPU numbers.".to_string());
            }
        }
        Err(e) => {
            lines.push(format!("GPU section skipped (best effort): {e}"));
        }
    }
    lines.push(String::new());

    // --- conclusions --------------------------------------------------------
    lines.push("## 5. Conclusions".to_string());
    lines.push(String::new());
    lines.push(write_conclusions(data, strategies, scores));
    lines.push(String::new());

    let text = lines.join("\n");
    fs::write(path, text).unwrap_or_else(|e| panic!("failed to write {}: {e}", path.display()));
}

/// Data-driven narrative conclusions.
fn write_conclusions(
    data: &[ImageData],
    strategies: &[StrategyParams; 3],
    scores: &[[Score; 3]],
) -> String {
    let mut out = Vec::new();
    let find = |name: &'static str| -> ([f32; 3], [Bucket; 3]) {
        let idx = data
            .iter()
            .position(|d| d.id.name() == name)
            .expect("corpus case");
        let d = [
            scores[idx][0].darkness,
            scores[idx][1].darkness,
            scores[idx][2].darkness,
        ];
        let b = [
            scores[idx][0].bucket,
            scores[idx][1].bucket,
            scores[idx][2].bucket,
        ];
        (d, b)
    };

    let (split_d, split_b) = find("half_split_white_dark");
    out.push(format!(
        "- Mean dilution (half_split_white_dark, golden medium): Current -> d={:.3} ({}), CoverageTuned -> d={:.3} ({}), ContrastPerceptual -> d={:.3} ({}). The percentile strategy resolves the 50/50 dilution (p90 sees the bright half); the coverage strategies still average it away.",
        split_d[0], split_b[0].label(), split_d[1], split_b[1].label(), split_d[2], split_b[2].label()
    ));

    let (win_d, win_b) = find("small_white_window");
    out.push(format!(
        "- Small bright window (small_white_window, golden weak): Current -> d={:.3} ({}), CoverageTuned -> d={:.3} ({}), ContrastPerceptual -> d={:.3} ({}). Coverage-based strategies dilute the ~15% white patch to zero; the percentile is all-or-nothing (it saturates to strong whenever the quantile rank lands inside the patch), so \"weak\" is unreachable for this case with either family.",
        win_d[0], win_b[0].label(), win_d[1], win_b[1].label(), win_d[2], win_b[2].label()
    ));

    let (yel_d, yel_b) = find("bright_saturated_yellow");
    out.push(format!(
        "- Saturated bright colors (bright_saturated_yellow, golden medium): Current -> d={:.3} ({}), CoverageTuned -> d={:.3} ({}), ContrastPerceptual -> d={:.3} ({}). The white-score chroma gate zeroes every saturated pixel, so coverage strategies are blind to this case; the percentile over-responds (strong) because saturated yellow is bright in luminance.",
        yel_d[0], yel_b[0].label(), yel_d[1], yel_b[1].label(), yel_d[2], yel_b[2].label()
    ));

    let (blue_d, blue_b) = find("bright_saturated_blue");
    out.push(format!(
        "- Saturated dark colors (bright_saturated_blue, golden medium): all strategies -> d={:.3}/{:.3}/{:.3} ({}). Perceptual luminance says the image is dark, so no darkness is warranted; the golden \"medium\" for this case is likely wrong.",
        blue_d[0], blue_d[1], blue_d[2], blue_b[0].label()
    ));

    for (col, sp) in strategies.iter().enumerate() {
        let mismatches: Vec<String> = data
            .iter()
            .enumerate()
            .filter(|(i, d)| scores[*i][col].bucket != d.id.golden())
            .map(|(i, d)| {
                format!(
                    "{} ({} vs golden {})",
                    d.id.name(),
                    scores[i][col].bucket.label(),
                    d.id.golden().label()
                )
            })
            .collect();
        out.push(format!(
            "- {} mismatches {}: {}",
            sp.key(),
            mismatches.len(),
            if mismatches.is_empty() {
                "none".to_string()
            } else {
                mismatches.join(", ")
            }
        ));
    }

    out.join("\n")
}

/// Current date without pulling in a chrono dependency.
fn chrono_like_date() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let days = now.as_secs() / 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    // Howard Hinnant's civil_from_days algorithm.
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    let skip_gpu = std::env::args().any(|a| a == "--skip-gpu");
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/qa/auto-darkness");
    let corpus_dir = out_dir.join("corpus");
    fs::create_dir_all(&corpus_dir)
        .unwrap_or_else(|e| panic!("failed to create {}: {e}", corpus_dir.display()));

    println!("== darkness_qa: generating corpus ==");
    let data: Vec<ImageData> = CorpusId::ALL
        .iter()
        .map(|id| id.generate(&corpus_dir))
        .collect();
    println!(
        "wrote {} corpus images to {}",
        data.len(),
        corpus_dir.display()
    );

    println!("== scanning CoverageTuned ==");
    let tuned_scan = scan_coverage_tuned(&data);
    println!("== scanning ContrastPerceptual ==");
    let perceptual_scan = scan_perceptual(&data);

    for (label, top) in [
        ("CoverageTuned", tuned_scan[0].params),
        ("ContrastPerceptual", perceptual_scan[0].params),
    ] {
        let (agreement, violations, over_dark) = if label == "CoverageTuned" {
            (
                tuned_scan[0].agreement,
                tuned_scan[0].violations,
                tuned_scan[0].over_dark,
            )
        } else {
            (
                perceptual_scan[0].agreement,
                perceptual_scan[0].violations,
                perceptual_scan[0].over_dark,
            )
        };
        println!(
            "best {label}: {} (agreement {agreement}/{}, violations {violations}, over-dark {over_dark:.3})",
            top.label(),
            data.len()
        );
    }

    let strategies: [StrategyParams; 3] = [
        StrategyParams::current(),
        tuned_scan[0].params,
        perceptual_scan[0].params,
    ];
    let scores: Vec<[Score; 3]> = data
        .iter()
        .map(|d| {
            [
                score_image(d, &strategies[0]),
                score_image(d, &strategies[1]),
                score_image(d, &strategies[2]),
            ]
        })
        .collect();

    println!("== GPU parity (headless wgpu) ==");
    let parity = if skip_gpu {
        Err("skipped via --skip-gpu".to_string())
    } else {
        match run_gpu_parity(&data, &strategies[1], &strategies[2]) {
            Ok(p) => {
                let passed = p
                    .rows
                    .iter()
                    .filter(|r| r.status[0] == ParityStatus::Pass)
                    .count();
                println!(
                    "adapter: {} | current parity pass {}/{} | tuned/perc: {}",
                    p.adapter,
                    passed,
                    p.rows.len(),
                    if p.rows.iter().any(|r| r.status[1] != ParityStatus::Pending) {
                        "implemented"
                    } else {
                        "pending (shader strategy branches not implemented)"
                    }
                );
                Ok(p)
            }
            Err(e) => {
                eprintln!("WARNING: GPU parity unavailable (best effort, continuing): {e}");
                Err(e)
            }
        }
    };

    println!("== contact sheet ==");
    let sheet_path = out_dir.join("contact_sheet.png");
    if let Err(e) = draw_contact_sheet(&sheet_path, &data, &scores) {
        eprintln!("WARNING: contact sheet failed (best effort, continuing): {e}");
    } else {
        println!("wrote {}", sheet_path.display());
    }

    println!("== results.md ==");
    let results_path = out_dir.join("results.md");
    write_results_md(
        &results_path,
        &ReportData {
            data: &data,
            strategies: &strategies,
            scores: &scores,
            tuned_scan: &tuned_scan,
            perceptual_scan: &perceptual_scan,
            parity: &parity,
            corpus_dir: &corpus_dir,
        },
    );
    println!("wrote {}", results_path.display());

    // A genuine Current-mirror mismatch is a real bug: surface it loudly.
    match &parity {
        Ok(p) if p.current_failed => {
            eprintln!("ERROR: Current GPU parity failed - CPU mirror diverges from the shader.");
            std::process::exit(1);
        }
        _ => {}
    }
    println!("done.");
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus_image(id: CorpusId) -> RgbaImage {
        let mut img = RgbaImage::new(IMAGE_W, IMAGE_H);
        id.paint(&mut img);
        img
    }

    #[test]
    fn current_strategy_hand_computed_values() {
        let pure = grid_samples(&corpus_image(CorpusId::PureWhite));
        let mid = grid_samples(&corpus_image(CorpusId::MidGray));
        let split = grid_samples(&corpus_image(CorpusId::HalfSplitWhiteDark));
        let sp = StrategyParams::current();
        // All-white: coverage 1.0 -> smoothstep(0.55, 0.82, 1.0) = 1.0 -> max.
        assert!((strategy_darkness(&pure, &sp) - 0.16).abs() < 1e-6);
        // Mid gray: luma 0.502 < 0.68 -> white_score 0 -> darkness 0.
        assert!(strategy_darkness(&mid, &sp).abs() < 1e-6);
        // Half white/half black: coverage 0.5 -> smoothstep = 0 -> darkness 0
        // (the mean-dilution failure this corpus is designed to expose).
        assert!(strategy_darkness(&split, &sp).abs() < 1e-6);
    }

    #[test]
    fn perceptual_strategy_hand_computed_values() {
        let pure = grid_samples(&corpus_image(CorpusId::PureWhite));
        let mid = grid_samples(&corpus_image(CorpusId::MidGray));
        let split = grid_samples(&corpus_image(CorpusId::HalfSplitWhiteDark));
        let sp = StrategyParams::perceptual(0.25, 0.90);
        // All-white: bin 7 center 0.9375 -> clamp(1 - 0.25/0.9375) = 0.733 -> max 0.16.
        assert!((strategy_darkness(&pure, &sp) - 0.16).abs() < 1e-6);
        // Mid gray: luma ~0.216 -> bin 1 center 0.1875 -> negative -> 0.
        assert!(strategy_darkness(&mid, &sp).abs() < 1e-6);
        // Half split: bright half fills bin 7 -> center 0.9375 -> 0.16; dilution resolved.
        assert!((strategy_darkness(&split, &sp) - 0.16).abs() < 1e-6);
    }

    #[test]
    fn white_score_hand_computed_values() {
        let p = TunedParams::CURRENT;
        // White: bright=1, chroma=0 -> neutral=1 -> 1.0.
        assert!((white_score([1.0, 1.0, 1.0], &p) - 1.0).abs() < 1e-6);
        // Mid gray: luma 0.502 below luma_lo -> 0.
        assert!(white_score([128.0 / 255.0; 3], &p).abs() < 1e-6);
        // Saturated yellow: chroma 1.0 -> neutral 0 -> 0.
        assert!(white_score([1.0, 1.0, 0.0], &p).abs() < 1e-6);
        // Saturated blue: luma 0.114 below luma_lo -> 0.
        assert!(white_score([0.0, 0.0, 1.0], &p).abs() < 1e-6);
    }

    #[test]
    fn bucket_thresholds() {
        assert_eq!(Bucket::from_darkness(0.0), Bucket::None);
        assert_eq!(Bucket::from_darkness(0.0149), Bucket::None);
        assert_eq!(Bucket::from_darkness(BUCKET_WEAK), Bucket::Weak);
        assert_eq!(Bucket::from_darkness(0.059), Bucket::Weak);
        assert_eq!(Bucket::from_darkness(BUCKET_MEDIUM), Bucket::Medium);
        assert_eq!(Bucket::from_darkness(0.114), Bucket::Medium);
        assert_eq!(Bucket::from_darkness(BUCKET_STRONG), Bucket::Strong);
        assert_eq!(Bucket::from_darkness(MAX_DARKNESS), Bucket::Strong);
    }

    #[test]
    fn sample_grid_reads_texel_centers() {
        // 512x320 with grid 16: cell (0,0) center texel is (16,10).
        let mut img = RgbaImage::new(IMAGE_W, IMAGE_H);
        fill(&mut img, [0, 0, 0]);
        img.put_pixel(16, 10, Rgba([255, 255, 255, 255]));
        img.put_pixel(16, 9, Rgba([255, 0, 0, 255]));
        img.put_pixel(15, 10, Rgba([0, 255, 0, 255]));
        assert_eq!(sample_texel(&img, 0, 0), [1.0, 1.0, 1.0]);
    }

    #[test]
    fn tuned_equals_current_at_baseline_constants() {
        let samples = grid_samples(&corpus_image(CorpusId::WhiteWeb));
        let cur = StrategyParams::current();
        let tuned = StrategyParams::tuned(TunedParams::CURRENT);
        assert!(
            (strategy_darkness(&samples, &cur) - strategy_darkness(&samples, &tuned)).abs() < 1e-6
        );
    }
}

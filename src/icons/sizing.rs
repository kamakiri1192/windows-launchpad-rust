//! Visual-size analysis for freeform icons (Issue #48).
//!
//! This module computes the *solid-fill ratio*—how much of an icon's opaque
//! bounding-box is actually opaque—and classifies the icon into one of three
//! categories.  Each category receives a fixed scale factor that the normalize
//! step later bakes into the pixel data so that all icons feel visually
//! balanced on the Launchpad grid.
//!
//! The algorithms are **pure** (no side effects, no image I/O) and operate on
//! raw RGBA buffers via [`DecodedIcon`].

use std::fmt;

use serde::{Deserialize, Serialize};

use super::normalize::DecodedIcon;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Alpha threshold for "hard" / inherently-opaque pixels.
///
/// Semi-transparent edge pixels (alpha < 128) are treated as transparent so
/// they do not inflate the bounding-box nor the solid-area count.
pub const ALPHA_HARD: u8 = 128;

/// Solid-fill threshold above which an icon is considered full-bleed.
pub const FULLBLEED_FILL: f64 = 0.80;

/// Solid-fill threshold *below* which an icon is considered thin-line art.
pub const THINLINE_FILL: f64 = 0.40;

/// Scale factor applied to full-bleed icons (= no change from current behaviour).
pub const SCALE_FULLBLEED: f64 = 1.00;

/// Scale factor applied to thin-line icons.
pub const SCALE_THINLINE: f64 = 0.92;

/// Scale factor applied to solid-body logos.
pub const SCALE_SOLID: f64 = 0.74;

// ---------------------------------------------------------------------------
// IconCategory
// ---------------------------------------------------------------------------

/// Broad visual category of an icon, determined by how densely it fills its
/// opaque bounding-box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IconCategory {
    /// Fully-filled icons (including macOS squircles).  Scale = 1.00.
    FullBleed,

    /// Thin line-art logos, e.g. wireframe or outline drawings.  Scale = 0.92.
    ThinLine,

    /// Solid-body logos with substantial transparent gaps inside the bbox.
    /// Scale = 0.74.
    Solid,
}

impl IconCategory {
    /// Return the category as a lower-case string (matches serde representation).
    pub fn as_str(self) -> &'static str {
        match self {
            IconCategory::FullBleed => "fullbleed",
            IconCategory::ThinLine => "thinline",
            IconCategory::Solid => "solid",
        }
    }
}

impl fmt::Display for IconCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for IconCategory {
    type Err = ();

    /// Parse from a lower-case string (same representation as serde / Display).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "fullbleed" => Ok(IconCategory::FullBleed),
            "thinline" => Ok(IconCategory::ThinLine),
            "solid" => Ok(IconCategory::Solid),
            _ => Err(()),
        }
    }
}

impl IconCategory {
    /// Parse from a lower-case string, falling back to [`IconCategory::FullBleed`]
    /// when the value is unrecognised (e.g. a corrupt or future-versioned cache
    /// entry).  This keeps the launcher rendering even on bad data.
    pub fn from_str_lossy(s: &str) -> Self {
        s.parse().unwrap_or(IconCategory::FullBleed)
    }
}

// ---------------------------------------------------------------------------
// IconMetrics
// ---------------------------------------------------------------------------

/// Results of analysing one icon's opaque bounding-box and fill ratio.
#[derive(Debug, Clone, PartialEq)]
pub struct IconMetrics {
    /// Left edge of the opaque bounding-box (inclusive).
    pub bbox_min_x: u32,
    /// Top edge of the opaque bounding-box (inclusive).
    pub bbox_min_y: u32,
    /// Right edge of the opaque bounding-box (inclusive).
    pub bbox_max_x: u32,
    /// Bottom edge of the opaque bounding-box (inclusive).
    pub bbox_max_y: u32,
    /// Width of the bounding-box (`max_x - min_x + 1`).
    pub bbox_w: u32,
    /// Height of the bounding-box (`max_y - min_y + 1`).
    pub bbox_h: u32,
    /// solid_area / (bbox_w * bbox_h) — how densely the bbox is filled.
    pub solid_fill: f64,
    /// Inferred visual category.
    pub category: IconCategory,
    /// The scale factor that the normalize step should apply.
    pub scale: f64,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Analyse `src` (pre-crop, original-resolution) and return bounding-box +
/// classification metrics.
///
/// Returns `None` when every pixel is transparent (alpha < [`ALPHA_HARD`]).
pub fn analyze(src: &DecodedIcon) -> Option<IconMetrics> {
    let (w, h) = (src.w as usize, src.h as usize);
    let rgba = &src.rgba;

    // First pass: find bounding-box of alpha-hard pixels.
    let mut min_x = u32::MAX;
    let mut min_y = u32::MAX;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut solid_count = 0u64;

    // Process row by row to limit the inner-loop work.
    for y in 0..h {
        let row_start = y * w * 4;
        for x in 0..w {
            let alpha = rgba[row_start + x * 4 + 3];
            if alpha >= ALPHA_HARD {
                let xu = x as u32;
                let yu = y as u32;
                min_x = min_x.min(xu);
                min_y = min_y.min(yu);
                max_x = max_x.max(xu);
                max_y = max_y.max(yu);
                solid_count += 1;
            }
        }
    }

    // No opaque pixel at all.
    if solid_count == 0 {
        return None;
    }

    let bbox_w = max_x - min_x + 1;
    let bbox_h = max_y - min_y + 1;
    let bbox_area = bbox_w as u64 * bbox_h as u64;
    let solid_fill = solid_count as f64 / bbox_area as f64;
    let category = classify(solid_fill);
    let scale = scale_for(category);

    Some(IconMetrics {
        bbox_min_x: min_x,
        bbox_min_y: min_y,
        bbox_max_x: max_x,
        bbox_max_y: max_y,
        bbox_w,
        bbox_h,
        solid_fill,
        category,
        scale,
    })
}

/// Classify an icon by its solid-fill ratio alone.
///
/// ```text
/// solid_fill >= FULLBLEED_FILL (0.80) → FullBleed
/// solid_fill <  THINLINE_FILL (0.40)  → ThinLine
/// else                                 → Solid
/// ```
pub fn classify(solid_fill: f64) -> IconCategory {
    if solid_fill >= FULLBLEED_FILL {
        IconCategory::FullBleed
    } else if solid_fill < THINLINE_FILL {
        IconCategory::ThinLine
    } else {
        IconCategory::Solid
    }
}

/// Return the fixed scale factor for a category.
pub fn scale_for(category: IconCategory) -> f64 {
    match category {
        IconCategory::FullBleed => SCALE_FULLBLEED,
        IconCategory::ThinLine => SCALE_THINLINE,
        IconCategory::Solid => SCALE_SOLID,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a synthetic `DecodedIcon` pixel by pixel.
    fn make_icon(w: u32, h: u32, fill: impl Fn(u32, u32) -> [u8; 4]) -> DecodedIcon {
        let size = w as usize * h as usize * 4;
        let mut rgba = vec![0u8; size];
        for y in 0..h {
            for x in 0..w {
                let p = fill(x, y);
                let i = (y as usize * w as usize + x as usize) * 4;
                rgba[i..i + 4].copy_from_slice(&p);
            }
        }
        DecodedIcon { rgba, w, h }
    }

    // -- classify -----------------------------------------------------------------

    #[test]
    fn classify_fullbleed_above_threshold() {
        assert_eq!(classify(0.85), IconCategory::FullBleed);
    }

    #[test]
    fn classify_thinline_below_threshold() {
        assert_eq!(classify(0.30), IconCategory::ThinLine);
    }

    #[test]
    fn classify_solid_in_between() {
        assert_eq!(classify(0.50), IconCategory::Solid);
    }

    #[test]
    fn classify_boundary_exact_080() {
        // Boundary: >= 0.80 is FullBleed.
        assert_eq!(classify(0.80), IconCategory::FullBleed);
    }

    #[test]
    fn classify_boundary_exact_040() {
        // Boundary: 0.40 is NOT ThinLine (need strictly < 0.40).
        assert_eq!(classify(0.40), IconCategory::Solid);
    }

    // -- scale_for ----------------------------------------------------------------

    #[test]
    fn scale_for_each_category() {
        assert_eq!(scale_for(IconCategory::FullBleed), 1.00);
        assert_eq!(scale_for(IconCategory::ThinLine), 0.92);
        assert_eq!(scale_for(IconCategory::Solid), 0.74);
    }

    // -- analyze ------------------------------------------------------------------

    #[test]
    fn analyze_solid_square() {
        // Fully-opaque 64x64 red square → solid_fill ≈ 1.0, FullBleed.
        let icon = make_icon(64, 64, |_x, _y| [255, 0, 0, 255]);
        let m = analyze(&icon).expect("should not be None for a solid image");
        assert_eq!(m.bbox_w, 64);
        assert_eq!(m.bbox_h, 64);
        assert!((m.solid_fill - 1.0).abs() < 0.001);
        assert_eq!(m.category, IconCategory::FullBleed);
        assert_eq!(m.scale, 1.0);
    }

    #[test]
    fn analyze_transparent_logo() {
        // 128x128 canvas, small 20x20 opaque logo in the centre → ThinLine.
        let icon = make_icon(128, 128, |x, y| {
            if (54..74).contains(&x) && (54..74).contains(&y) {
                [0, 0, 0, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        let m = analyze(&icon).expect("small-opaque area should exist");
        assert_eq!(m.bbox_w, 20);
        assert_eq!(m.bbox_h, 20);
        // 20x20 block is fully opaque → solid_fill = 1.0, so it becomes FullBleed in
        // this synthetic case.  That is correct: the logo has no internal gaps.
        assert_eq!(m.category, IconCategory::FullBleed);
    }

    #[test]
    fn analyze_partial_fill() {
        // 64x64 bbox with a checkerboard: every-other pixel is opaque → ~0.5 fill.
        let icon = make_icon(128, 128, |x, y| {
            let in_bbox = (32..96).contains(&x) && (32..96).contains(&y);
            if in_bbox && ((x + y) % 2 == 0) {
                [0, 0, 0, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        let m = analyze(&icon).expect("should not be all-transparent");
        assert_eq!(m.bbox_w, 64);
        assert_eq!(m.bbox_h, 64);
        assert!((m.solid_fill - 0.5).abs() < 0.01);
        assert_eq!(m.category, IconCategory::Solid);
        assert_eq!(m.scale, 0.74);
    }

    #[test]
    fn analyze_all_transparent_returns_none() {
        let icon = make_icon(64, 64, |_x, _y| [0, 0, 0, 0]);
        assert!(analyze(&icon).is_none());
    }

    #[test]
    fn analyze_finds_bbox_correctly() {
        // A 100x100 canvas where only the region x=20..80, y=30..70 is opaque.
        let icon = make_icon(100, 100, |x, y| {
            if (20..80).contains(&x) && (30..70).contains(&y) {
                [0, 255, 0, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        let m = analyze(&icon).expect("should have opaque pixels");
        assert_eq!(m.bbox_min_x, 20);
        assert_eq!(m.bbox_min_y, 30);
        assert_eq!(m.bbox_max_x, 79); // exclusive upper bound was 80, so max is 79
        assert_eq!(m.bbox_max_y, 69);
        assert_eq!(m.bbox_w, 60);
        assert_eq!(m.bbox_h, 40);
    }

    #[test]
    fn analyze_squircle_like() {
        // 128x128 canvas, 16px transparent margin on all four sides.
        // Inside the "plate" (96x96), everything is opaque → solid_fill ~ 1.0,
        // FullBleed.  This mimics a macOS squircle where the corners are
        // transparent but the interior is a solid plate.
        let icon = make_icon(128, 128, |x, y| {
            if (16..112).contains(&x) && (16..112).contains(&y) {
                [100, 150, 200, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        let m = analyze(&icon).expect("should have opaque interior");
        assert_eq!(m.bbox_min_x, 16);
        assert_eq!(m.bbox_min_y, 16);
        assert_eq!(m.bbox_max_x, 111);
        assert_eq!(m.bbox_max_y, 111);
        assert_eq!(m.bbox_w, 96);
        assert_eq!(m.bbox_h, 96);
        assert!((m.solid_fill - 1.0).abs() < 0.001);
        assert_eq!(m.category, IconCategory::FullBleed);
        assert_eq!(m.scale, 1.0);
    }

    // -- Display / as_str / FromStr ----------------------------------------------

    #[test]
    fn display_and_fromstr_roundtrip() {
        for cat in [
            IconCategory::FullBleed,
            IconCategory::ThinLine,
            IconCategory::Solid,
        ] {
            let s = cat.as_str();
            assert_eq!(cat.to_string(), s);
            let parsed: IconCategory = s.parse().expect("valid category parses");
            assert_eq!(parsed, cat);
            // Lossy variant returns the same for known strings.
            assert_eq!(IconCategory::from_str_lossy(s), cat);
        }
        // Unknown string: FromStr errors, lossy falls back to FullBleed.
        let bogus: Result<IconCategory, _> = "bogus".parse();
        assert!(bogus.is_err());
        assert_eq!(
            IconCategory::from_str_lossy("bogus"),
            IconCategory::FullBleed
        );
    }

    // -- serde roundtrip ----------------------------------------------------------

    #[test]
    fn serde_roundtrip() {
        let cases = [
            (IconCategory::FullBleed, "\"fullbleed\""),
            (IconCategory::ThinLine, "\"thinline\""),
            (IconCategory::Solid, "\"solid\""),
        ];
        for (cat, expected_json) in cases {
            let json = serde_json::to_string(&cat).expect("serialize");
            assert_eq!(json, expected_json);
            let back: IconCategory = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, cat);
        }
    }
}

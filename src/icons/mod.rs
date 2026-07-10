//! Icon loading, normalization, and atlas packing for the launcher grid.
//!
//! **Historical synchronous pipeline** (kept for the `IconAtlas::pack` tests and
//! as a reference for the extraction strategy):
//!   1. [`extract`] enumerates Start Menu `.lnk` files and pulls an `HICON`
//!      from each, converting it to a `DecodedIcon` (raw RGBA).
//!   2. [`normalize`] resizes every icon to a fixed `TARGET`×`TARGET` square.
//!   3. [`IconAtlas`] packs all squares into one 2D texture and records the
//!      UV rect of each entry, which the GPU icon pipeline samples.
//!   4. [`loader`] orchestrates the above and returns the app list + atlas.
//!
//! The **live** launcher no longer uses [`IconAtlas`] / [`loader`]; it uses the
//! async, fixed-slot [`crate::icon_atlas::IconAtlas`] plus the icon worker,
//! SQLite cache, and app registry. Those older pieces are kept here with
//! `#[allow(dead_code)]` so the extraction logic and its tests stay available.

#![allow(dead_code)]

pub mod extract;
pub mod loader;
pub mod normalize;

#[allow(unused_imports)] // legacy pipeline surface; kept for reference/tests
pub use loader::{load_all_icons, AppEntry, LoadedIcons};
#[allow(unused_imports)] // public API surface for icon decoding callers
pub use normalize::{normalize, DecodedIcon, TARGET};

/// Re-exported from [`crate::ui_model::geometry`]. `UvRect` is renderer-neutral
/// data (texture coordinates carry no feature semantics), so the canonical
/// definition lives in `ui_model`. This re-export keeps historical
/// `crate::icons::UvRect` references working during the Phase 6.5 migration.
pub use crate::ui_model::geometry::UvRect;

/// A packed icon atlas: one wide RGBA8 bitmap plus the UV rect of each icon.
#[derive(Debug)]
pub struct IconAtlas {
    /// Tightly-packed RGBA8 pixels, row-major, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Per-icon placement. Index `i` corresponds to the `i`-th input icon.
    pub entries: Vec<UvRect>,
}

/// 1px padding around each cell to prevent UV bleeding at tile edges when
/// the sampler uses linear filtering.
const CELL_PAD: u32 = 1;

impl IconAtlas {
    /// Pack `icons` (already normalized to `TARGET`×`TARGET`) into a single
    /// texture laid out as a grid of square cells.
    ///
    /// The atlas width is chosen to fit a reasonable number of icons per row
    /// (capped at 2048 to stay within WebGPU's default 2D texture limit on
    /// low-end GPUs). Rows grow as needed. If the icons wouldn't fit even at
    /// the max height, excess entries are dropped and logged.
    pub fn pack(icons: &[DecodedIcon]) -> Self {
        // Cell = TARGET icon + 2*CELL_PAD padding.
        let cell = TARGET + CELL_PAD * 2;

        // Pick a square-ish atlas width that keeps things on the GPU's happy
        // path: aim for `cols` such that the texture is ≤ 2048px wide.
        let max_dim = 2048u32;
        let cols = (max_dim / cell).max(1);
        let width = cols * cell;
        // Round height up to the next full row; cap at max_dim. Always keep
        // at least one row so the GPU has a valid (non-zero) texture even when
        // no icons were packed.
        let rows_needed = (icons.len() as u32).div_ceil(cols).max(1);
        let height = (rows_needed * cell).min(max_dim);
        let capacity_rows = height / cell;
        let capacity = (cols * capacity_rows) as usize;

        let mut rgba = vec![0u8; (width * height * 4) as usize];
        let mut entries = Vec::with_capacity(icons.len());

        for (i, icon) in icons.iter().enumerate() {
            if i >= capacity {
                eprintln!("icon atlas full at {} entries; dropping the rest", capacity);
                break;
            }
            let col = (i as u32) % cols;
            let row = (i as u32) / cols;
            // Cell top-left (icon bitmap starts CELL_PAD inside).
            let icon_x = col * cell + CELL_PAD;
            let icon_y = row * cell + CELL_PAD;

            blit_icon(&mut rgba, width, icon, icon_x, icon_y);

            // UV rect over the icon bitmap itself (not the padding).
            entries.push(UvRect {
                u0: icon_x as f32 / width as f32,
                v0: icon_y as f32 / height as f32,
                u1: (icon_x + icon.w) as f32 / width as f32,
                v1: (icon_y + icon.h) as f32 / height as f32,
            });
        }

        IconAtlas {
            rgba,
            width,
            height,
            entries,
        }
    }
}

/// Copy a normalized icon's pixels into the atlas buffer at `(dst_x, dst_y)`.
///
/// Assumes the icon is fully inside the atlas bounds; out-of-range blits are
/// silently clipped per-row so a single bad icon can't corrupt neighbours.
fn blit_icon(atlas: &mut [u8], atlas_w: u32, icon: &DecodedIcon, dst_x: u32, dst_y: u32) {
    if icon.w == 0 || icon.h == 0 {
        return;
    }
    let stride = atlas_w as usize * 4;
    for y in 0..icon.h {
        let ay = dst_y as usize + y as usize;
        let src_row = y as usize * icon.w as usize * 4;
        let dst_row = ay * stride + dst_x as usize * 4;
        let row_len = icon.w as usize * 4;
        if dst_row + row_len > atlas.len() {
            break;
        }
        let src = &icon.rgba[src_row..src_row + row_len];
        let dst = &mut atlas[dst_row..dst_row + row_len];
        dst.copy_from_slice(src);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(target: u32, c: [u8; 4]) -> DecodedIcon {
        DecodedIcon {
            rgba: c.repeat((target * target) as usize),
            w: target,
            h: target,
        }
    }

    #[test]
    fn pack_empty_returns_valid_empty_atlas() {
        let a = IconAtlas::pack(&[]);
        // Still allocates a 1-row texture so the GPU has a valid resource.
        assert!(a.entries.is_empty());
        assert!(a.width > 0);
        assert!(a.height > 0);
    }

    #[test]
    fn pack_records_uv_rects_inside_unit_square() {
        let icons = vec![solid(TARGET, [255, 0, 0, 255]); 3];
        let a = IconAtlas::pack(&icons);
        assert_eq!(a.entries.len(), 3);
        for uv in &a.entries {
            assert!((0.0..=1.0).contains(&uv.u0));
            assert!((0.0..=1.0).contains(&uv.v0));
            assert!((0.0..=1.0).contains(&uv.u1));
            assert!((0.0..=1.0).contains(&uv.v1));
            assert!(uv.u1 > uv.u0);
            assert!(uv.v1 > uv.v0);
        }
    }

    #[test]
    fn pack_blits_pixels_into_atlas() {
        let icons = vec![solid(TARGET, [10, 20, 30, 40])];
        let a = IconAtlas::pack(&icons);
        // The first icon's top-left pixel lives at (CELL_PAD, CELL_PAD).
        let px = ((CELL_PAD * a.width + CELL_PAD) * 4) as usize;
        assert_eq!(&a.rgba[px..px + 4], &[10, 20, 30, 40]);
    }

    #[test]
    fn pack_many_does_not_overlap() {
        // Two icons must occupy distinct cells; the second's origin must be
        // at least one full cell away from the first's.
        let icons = vec![
            solid(TARGET, [255, 0, 0, 255]),
            solid(TARGET, [0, 255, 0, 255]),
        ];
        let a = IconAtlas::pack(&icons);
        let first_px = a.entries[0].u0 * a.width as f32;
        let second_px = a.entries[1].u0 * a.width as f32;
        assert!(second_px - first_px >= TARGET as f32);
    }
}

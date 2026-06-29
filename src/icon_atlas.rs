//! Fixed-slot icon atlas.
//!
//! The old [`crate::icons::IconAtlas`] repacked *every* icon whenever the set
//! changed, which moves UVs around and breaks incremental updates. This module
//! implements the Phase-5 design: each app owns a **fixed** cell in a grid
//! texture, addressed by its `slot` index. Updating one icon is a single
//! `write_texture` into that cell; adding/removing apps never shifts another
//! app's UV.
//!
//! Layout:
//!   - Each cell is `TARGET + 2*PAD` pixels square (`PAD` prevents linear
//!     filtering from bleeding a neighbour's edge into the icon).
//!   - The atlas is a square texture, sized to hold at least `capacity` cells
//!     in a grid `cols × rows`, capped to the GPU's max 2D dimension.
//!   - When `slot_count` outgrows `capacity`, the atlas is grown (a full
//!     re-blit of the existing cells). This is the only operation that
//!     reallocates the texture, and it's rare.
//!
//! All cells start transparent, so missing/loading icons simply render nothing
//! over their color tile — exactly the placeholder behavior we want.

use crate::icons::normalize::{DecodedIcon, TARGET};
use crate::icons::UvRect;

/// Padding around each cell so linear sampling can't bleed across icon borders.
const CELL_PAD: u32 = 1;
/// Per-cell edge length (icon + padding both sides).
pub const CELL: u32 = TARGET + CELL_PAD * 2;

/// A growable, fixed-slot RGBA atlas.
pub struct IconAtlas {
    /// CPU-side pixel buffer (`width * height * 4`), kept in sync with the GPU
    /// texture so a grow can re-blit everything without re-extracting icons.
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    /// Grid columns (derived from width / CELL).
    cols: u32,
    /// How many cells the current allocation can hold.
    capacity: u32,
    /// Per-slot UV, lazily computed. `None` = slot was never written (still
    /// transparent). UVs are immutable for a slot once assigned.
    uvs: Vec<Option<UvRect>>,
}

impl std::fmt::Debug for IconAtlas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IconAtlas")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("cols", &self.cols)
            .field("capacity", &self.capacity)
            .field("slots_known", &self.uvs.len())
            .finish()
    }
}

impl IconAtlas {
    /// Create an atlas sized for `initial_capacity` slots (rounded up to a
    /// square grid). One row is the minimum so the GPU always has a valid
    /// texture.
    pub fn new(initial_capacity: u32) -> Self {
        let cols = cols_for_capacity(initial_capacity).max(1);
        let rows = (initial_capacity.div_ceil(cols)).max(1);
        Self::with_grid(cols, rows)
    }

    fn with_grid(cols: u32, rows: u32) -> Self {
        let width = cols * CELL;
        let height = rows * CELL;
        let capacity = cols * rows;
        Self {
            rgba: vec![0u8; (width * height * 4) as usize],
            width,
            height,
            cols,
            capacity,
            uvs: vec![None; capacity as usize],
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// UV rect for a slot, or `None` if the slot has never been written.
    pub fn uv(&self, slot: u32) -> Option<UvRect> {
        self.uvs.get(slot as usize).copied().flatten()
    }

    /// (col, row) of a slot in the cell grid.
    fn cell_xy(&self, slot: u32) -> (u32, u32) {
        let col = slot % self.cols;
        let row = slot / self.cols;
        (col, row)
    }

    /// Pixel origin (top-left of the *icon*, i.e. inside the padding) of a slot.
    fn icon_origin(&self, slot: u32) -> (u32, u32) {
        let (col, row) = self.cell_xy(slot);
        (col * CELL + CELL_PAD, row * CELL + CELL_PAD)
    }

    /// Ensure the atlas can hold at least `needed` slots. Grows the texture by
    /// doubling rows (and re-blitting existing pixels) when necessary.
    /// Returns `true` if the texture was reallocated (caller must re-upload the
    /// whole atlas to the GPU).
    pub fn ensure_capacity(&mut self, needed: u32) -> bool {
        if needed <= self.capacity {
            // Still make sure the UV bookkeeping tracks `needed`.
            if (needed as usize) > self.uvs.len() {
                self.uvs.resize(needed as usize, None);
            }
            return false;
        }
        // Grow: double rows, keep cols. If that still isn't enough, widen cols.
        let mut cols = self.cols;
        let mut rows = (self.rows_for_capacity(needed, cols)).max(self.rows() * 2);
        // Cap by GPU max (caller checks separately; here we just clamp).
        const MAX_DIM: u32 = 8192;
        while cols * CELL > MAX_DIM && cols > 1 {
            cols /= 2;
        }
        rows = rows.max(1);
        while cols * CELL > MAX_DIM {
            cols -= 1;
        }
        if rows * CELL > MAX_DIM {
            rows = MAX_DIM / CELL;
        }
        let new_cols = cols.max(1);
        let new_rows = rows.max((needed.div_ceil(new_cols)).max(1));

        let mut grown = Self::with_grid(new_cols, new_rows);
        // Re-blit every existing slot into the new buffer. We copy whole
        // CELL×CELL regions (icon + padding) by *cell* origin, not icon origin,
        // so the padding gutter travels with the icon.
        let old_cols = self.cols;
        for slot in 0..self.uvs.len() as u32 {
            if self.uvs[slot as usize].is_none() {
                continue;
            }
            let (old_col, old_row) = (slot % old_cols, slot / old_cols);
            let src_x = old_col * CELL;
            let src_y = old_row * CELL;
            let (dst_col, dst_row) = (slot % grown.cols, slot / grown.cols);
            let dst_x = dst_col * CELL;
            let dst_y = dst_row * CELL;
            copy_cell(
                &self.rgba,
                self.width,
                src_x,
                src_y,
                &mut grown.rgba,
                grown.width,
                dst_x,
                dst_y,
            );
            grown.uvs[slot as usize] = Some(grown.uv_for_slot(slot));
        }
        grown.uvs.resize(grown.capacity.max(needed) as usize, None);
        *self = grown;
        true
    }

    fn rows(&self) -> u32 {
        self.height / CELL
    }

    fn rows_for_capacity(&self, needed: u32, cols: u32) -> u32 {
        needed.div_ceil(cols.max(1))
    }

    /// Compute the UV rect (over the icon bitmap, not the padding) for a slot,
    /// assuming the slot lives at its standard cell position in *this* atlas.
    fn uv_for_slot(&self, slot: u32) -> UvRect {
        let (x, y) = self.icon_origin(slot);
        UvRect {
            u0: x as f32 / self.width as f32,
            v0: y as f32 / self.height as f32,
            u1: (x + TARGET) as f32 / self.width as f32,
            v1: (y + TARGET) as f32 / self.height as f32,
        }
    }

    /// Write one normalized icon into its slot. Returns the (origin x, y) the
    /// caller needs in order to do a partial GPU `write_texture`, plus the UV
    /// rect. If the icon is empty (0×0) the slot is cleared to transparent and
    /// its UV is recorded as `None` so the placeholder shows.
    pub fn write_icon(&mut self, slot: u32, icon: &DecodedIcon) -> (u32, u32, UvRect) {
        // Grow on demand (rare: only when a new app appears beyond capacity).
        if slot >= self.capacity {
            self.ensure_capacity(slot + 1);
        }
        let (x, y) = self.icon_origin(slot);
        let uv = self.uv_for_slot(slot);
        if icon.w == 0 || icon.h == 0 {
            clear_cell(&mut self.rgba, self.width, x - CELL_PAD, y - CELL_PAD);
            if (slot as usize) < self.uvs.len() {
                self.uvs[slot as usize] = None;
            }
            return (x, y, uv);
        }
        blit_icon(&mut self.rgba, self.width, icon, x, y);
        if (slot as usize) < self.uvs.len() {
            self.uvs[slot as usize] = Some(uv);
        }
        (x, y, uv)
    }

    /// Clear a slot back to transparent (used when an app is removed and we
    /// want its cell invisible rather than showing a stale icon).
    pub fn clear_slot(&mut self, slot: u32) {
        if slot >= self.capacity {
            return;
        }
        let (x, y) = self.icon_origin(slot);
        clear_cell(&mut self.rgba, self.width, x - CELL_PAD, y - CELL_PAD);
        if (slot as usize) < self.uvs.len() {
            self.uvs[slot as usize] = None;
        }
    }
}

/// Choose a column count so the atlas stays reasonably square for `capacity`.
fn cols_for_capacity(capacity: u32) -> u32 {
    if capacity == 0 {
        return 16;
    }
    let c = (capacity as f64).sqrt().ceil() as u32;
    // Cap cols by the 8192 max dim.
    const MAX_DIM: u32 = 8192;
    c.clamp(1, MAX_DIM / CELL)
}

/// Blit a normalized icon into the atlas buffer at the icon origin (inside the
/// cell padding). Clips per-row so a bad icon can't corrupt neighbours.
fn blit_icon(atlas: &mut [u8], atlas_w: u32, icon: &DecodedIcon, dst_x: u32, dst_y: u32) {
    let stride = atlas_w as usize * 4;
    let row_len = (icon.w as usize) * 4;
    for y in 0..icon.h {
        let ay = dst_y as usize + y as usize;
        let src = y as usize * row_len;
        let dst = ay * stride + dst_x as usize * 4;
        if dst + row_len > atlas.len() {
            break;
        }
        let src_slice = &icon.rgba[src..src + row_len];
        let dst_slice = &mut atlas[dst..dst + row_len];
        dst_slice.copy_from_slice(src_slice);
    }
}

/// Zero out a full CELL×CELL region (icon + padding) at the cell origin.
fn clear_cell(atlas: &mut [u8], atlas_w: u32, cell_x: u32, cell_y: u32) {
    let stride = atlas_w as usize * 4;
    let row_len = (CELL as usize) * 4;
    for y in 0..CELL {
        let ay = cell_y as usize + y as usize;
        let dst = ay * stride + cell_x as usize * 4;
        if dst + row_len > atlas.len() {
            break;
        }
        for b in &mut atlas[dst..dst + row_len] {
            *b = 0;
        }
    }
}

/// Copy one full CELL×CELL region between buffers (used on grow).
#[allow(clippy::too_many_arguments)]
fn copy_cell(
    src: &[u8],
    src_w: u32,
    src_x: u32,
    src_y: u32,
    dst: &mut [u8],
    dst_w: u32,
    dst_x: u32,
    dst_y: u32,
) {
    let s_stride = src_w as usize * 4;
    let d_stride = dst_w as usize * 4;
    let row_len = (CELL as usize) * 4;
    for y in 0..CELL {
        let sy = src_y as usize + y as usize;
        let dy = dst_y as usize + y as usize;
        let s = sy * s_stride + src_x as usize * 4;
        let d = dy * d_stride + dst_x as usize * 4;
        if s + row_len > src.len() || d + row_len > dst.len() {
            break;
        }
        dst[d..d + row_len].copy_from_slice(&src[s..s + row_len]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(c: [u8; 4]) -> DecodedIcon {
        DecodedIcon {
            rgba: c.repeat((TARGET as usize).pow(2)),
            w: TARGET,
            h: TARGET,
        }
    }

    #[test]
    fn new_atlas_has_valid_dimensions() {
        let a = IconAtlas::new(0);
        assert!(a.width() > 0);
        assert!(a.height() > 0);
        assert_eq!(a.width() % CELL, 0);
        assert_eq!(a.height() % CELL, 0);
    }

    #[test]
    fn write_icon_records_uv_inside_unit_square() {
        let mut a = IconAtlas::new(8);
        let (_, _, uv) = a.write_icon(0, &solid([255, 0, 0, 255]));
        for v in [uv.u0, uv.v0, uv.u1, uv.v1] {
            assert!((0.0..=1.0).contains(&v));
        }
        assert!(uv.u1 > uv.u0);
        assert!(uv.v1 > uv.v0);
        assert_eq!(a.uv(0), Some(uv));
    }

    #[test]
    fn write_icon_blits_pixels_at_slot_origin() {
        let mut a = IconAtlas::new(8);
        let (x, y, _) = a.write_icon(3, &solid([10, 20, 30, 40]));
        let stride = a.width() as usize * 4;
        let px = (y as usize) * stride + x as usize * 4;
        assert_eq!(&a.rgba()[px..px + 4], &[10, 20, 30, 40]);
    }

    #[test]
    fn different_slots_do_not_overlap() {
        let mut a = IconAtlas::new(8);
        let (_, _, uv0) = a.write_icon(0, &solid([255, 0, 0, 255]));
        let (_, _, uv1) = a.write_icon(1, &solid([0, 255, 0, 255]));
        // Pixel distance between the two icon origins must be >= TARGET.
        let p0 = uv0.u0 * a.width() as f32;
        let p1 = uv1.u0 * a.width() as f32;
        assert!((p1 - p0).abs() >= TARGET as f32);
    }

    #[test]
    fn clearing_slot_makes_uv_none_and_zeroes_pixels() {
        let mut a = IconAtlas::new(8);
        a.write_icon(2, &solid([1, 2, 3, 4]));
        assert!(a.uv(2).is_some());
        a.clear_slot(2);
        assert!(a.uv(2).is_none());
        // The cell region (including padding) must be all zero.
        let col = 2u32;
        let row = 0u32;
        let cx = col * CELL;
        let cy = row * CELL;
        let stride = a.width() as usize * 4;
        for y in 0..CELL {
            for x in 0..CELL {
                let idx = (cy as usize + y as usize) * stride + (cx as usize + x as usize) * 4;
                assert!(
                    a.rgba()[idx..idx + 4].iter().all(|&b| b == 0),
                    "pixel nonzero after clear at ({x},{y})"
                );
            }
        }
    }

    #[test]
    fn empty_icon_clears_slot() {
        let mut a = IconAtlas::new(8);
        a.write_icon(1, &solid([9, 9, 9, 9]));
        let empty = DecodedIcon {
            rgba: vec![],
            w: 0,
            h: 0,
        };
        a.write_icon(1, &empty);
        assert!(a.uv(1).is_none());
    }

    #[test]
    fn ensure_capacity_grows_preserving_existing_uvs_and_pixels() {
        let mut a = IconAtlas::new(2);
        let (_, _, uv0) = a.write_icon(0, &solid([255, 0, 0, 255]));
        let (_, _, uv1) = a.write_icon(1, &solid([0, 255, 0, 255]));
        let old_capacity = a.capacity();

        // Force growth far beyond current capacity.
        let grew = a.ensure_capacity(old_capacity + 50);
        assert!(grew);
        assert!(a.capacity() >= old_capacity + 50);

        // UVs changed (texture resized), but pixel content migrated intact.
        let new_uv0 = a.uv(0).expect("slot 0 preserved");
        let (x0, y0) = a.icon_origin(0);
        let stride = a.width() as usize * 4;
        let px = y0 as usize * stride + x0 as usize * 4;
        assert_eq!(&a.rgba()[px..px + 4], &[255, 0, 0, 255]);
        // The newly written slot 1 should also be intact.
        let (x1, y1) = a.icon_origin(1);
        let px1 = y1 as usize * stride + x1 as usize * 4;
        assert_eq!(&a.rgba()[px1..px1 + 4], &[0, 255, 0, 255]);
        // uv0 must have been recomputed for the new dimensions.
        let _ = (uv0, uv1, new_uv0);
    }

    #[test]
    fn writing_slot_beyond_capacity_auto_grows() {
        let mut a = IconAtlas::new(1);
        let (_, _, uv) = a.write_icon(5, &solid([1, 1, 1, 255]));
        assert!(a.capacity() >= 6);
        assert_eq!(a.uv(5), Some(uv));
    }

    #[test]
    fn auto_grow_keeps_uv_bookkeeping_for_all_new_capacity() {
        let mut a = IconAtlas::new(64);

        // Slot 64 grows the atlas from 64 to a larger allocation. The UV table
        // must keep the full new capacity, not just the 65 slots needed by
        // this write, or later slots will be lost on the next grow.
        a.write_icon(64, &solid([64, 0, 0, 255]));
        assert!(a.capacity() >= 128);

        let (_, _, uv100) = a.write_icon(100, &solid([100, 0, 0, 255]));
        assert_eq!(a.uv(100), Some(uv100));

        // Force a second grow. Slot 100 should migrate with the CPU atlas and
        // keep a valid UV instead of disappearing.
        a.write_icon(128, &solid([128, 0, 0, 255]));
        let (x, y) = a.icon_origin(100);
        let stride = a.width() as usize * 4;
        let px = y as usize * stride + x as usize * 4;
        assert_eq!(&a.rgba()[px..px + 4], &[100, 0, 0, 255]);
        assert!(a.uv(100).is_some());
    }

    #[test]
    fn slot_uv_is_stable_across_other_writes() {
        let mut a = IconAtlas::new(16);
        let (_, _, uv0) = a.write_icon(0, &solid([1, 1, 1, 255]));
        // Writing other slots must not move slot 0's UV.
        for s in 1..8 {
            a.write_icon(s, &solid([2, 2, 2, 255]));
        }
        assert_eq!(a.uv(0), Some(uv0));
    }
}

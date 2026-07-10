//! Static tile instance buffer and the per-frame viewport/scroll uniform.
//!
//! The uniform struct mirrors the WGSL uniform block declared in
//! `src/shader.wgsl`. Tile instance data is static: it is only rebuilt after a
//! relayout or a tile-data change (reorder / icon load / spring animation), and
//! never on an animation-only frame.

use super::badges::edit_badge_sources;
use super::counters::Category;
use crate::grid::{GridLayout, TileInstance};

use super::Renderer;

/// Uniform block mirrored in WGSL.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct Uniforms {
    pub(super) viewport: [f32; 2],
    pub(super) scroll_x: f32,
    /// Global animation clock (seconds). Drives the edit-mode wiggle.
    pub(super) time: f32,
    /// Fixed page-frame center in physical px.
    pub(super) frame_center: [f32; 2],
    /// Fixed page-frame half-size in physical px.
    pub(super) frame_half_size: [f32; 2],
    /// Fixed page-frame corner radius in physical px.
    pub(super) frame_radius: f32,
    /// 1.0 while an edit-mode drag is in flight, else 0.0. Tells the dragged
    /// instance's vertex shader to follow `drag_pos` instead of its home cell.
    pub(super) drag_active: f32,
    /// Pointer position (screen px) the dragged icon follows. Only meaningful
    /// while `drag_active` is 1.0.
    pub(super) drag_pos: [f32; 2],
}

impl Renderer {
    /// Rebuild the static instance buffer from a fresh layout.
    ///
    /// Call after a resize (or any change to tile data) so the GPU sees the
    /// new tile positions. The buffer grows only if the new list exceeds the
    /// current capacity; otherwise the existing buffer is reused via
    /// `queue.write_buffer`.
    pub fn rebuild_instances(
        &mut self,
        layout: &GridLayout,
        apps: &[crate::grid::GridApp<'_>],
        anim: &[crate::grid::TileAnim],
    ) {
        let instances = layout.build_instances(self.config.width as f32, apps, anim);
        let outcome = self
            .instance_buffer
            .set(&self.device, &self.queue, &instances);
        if outcome.allocated {
            self.counters.record_creation(Category::Tile);
        }
        self.counters.record_full_scene_rebuild();
        self.liquid_glass
            .rebuild_shapes(&self.device, layout, self.config.width as f32, apps);
        self.frame_clip = super::frame_clip(layout, self.config.width);
    }

    /// Push a caller-built tile instance list to the GPU, reallocating the
    /// buffer only on capacity overflow. Used by the reorder animation, which
    /// overrides the tile positions with per-tile spring offsets before
    /// uploading.
    pub fn set_tile_instances(&mut self, instances: &[TileInstance]) {
        let outcome = self
            .instance_buffer
            .set(&self.device, &self.queue, instances);
        if outcome.allocated {
            self.counters.record_growth(Category::Tile);
        }
        self.badge_sources = edit_badge_sources(instances);
        self.update_edit_badges(0.0);
    }
}

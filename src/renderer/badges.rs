//! Edit-mode delete badges: glass shape sources + foreground ✕ marks.
//!
//! Badge geometry is derived when the neutral tile scene changes: every
//! wiggling non-dragged tile contributes one base center, pivot, radius, and
//! phase. Those static values are uploaded once; Liquid Glass and foreground
//! WGSL evaluate the per-frame wobble from the shared animation clock.

use crate::layout::grid::edit_badge_radius_for_tile_size;
use crate::liquid_glass::geometry::GlassShape;
use crate::renderer::controls::ControlInstance;
use crate::renderer::tiles::TileInstance;
use crate::ui_model::grid::TileAnim;

use super::counters::Category;
use super::Renderer;

#[derive(Debug, Clone, Copy)]
pub(crate) struct EditBadgeSource {
    base_center: [f32; 2],
    tile_center: [f32; 2],
    radius: f32,
    phase: f32,
}

impl Renderer {
    /// Recompute the badge glass shapes + foreground ✕ marks from the current
    /// `badge_sources`. Per-frame motion is evaluated by the shaders.
    pub(super) fn prepare_edit_badges(&mut self) {
        const KIND_BADGE_CLOSE: f32 = 4.0;

        let mut shapes = std::mem::take(&mut self.badge_shape_scratch);
        let mut marks = std::mem::take(&mut self.badge_mark_scratch);
        shapes.clear();
        marks.clear();
        shapes.reserve(self.badge_sources.len() + 1);
        marks.reserve(self.badge_sources.len());
        let frame = self.frame_clip;
        let clip_shape = GlassShape::clip_rounded_rect(
            [frame.0, frame.1],
            [frame.2 * 2.0, frame.3 * 2.0],
            frame.4,
        );
        for source in &self.badge_sources {
            shapes.push(GlassShape::animated_badge(
                source.base_center,
                [source.radius * 2.15, source.radius * 2.15],
                source.radius,
                source.tile_center,
                source.phase,
            ));
            marks.push(ControlInstance {
                center: source.base_center,
                params: [source.radius, 0.92, (source.radius * 0.13).max(1.4), 0.0],
                color: [1.0, 1.0, 1.0, 0.92],
                kind: [
                    KIND_BADGE_CLOSE,
                    source.tile_center[0],
                    source.tile_center[1],
                    source.phase,
                ],
            });
        }

        if !marks.is_empty() {
            shapes.insert(0, clip_shape);
        }

        self.liquid_glass
            .set_badge_shapes(&self.device, &self.queue, &shapes);
        let outcome = self
            .badge_instance_buffer
            .set(&self.device, &self.queue, &marks);
        if outcome.allocated {
            self.counters.record_growth(Category::BadgeForeground);
        }
        self.badge_shape_scratch = shapes;
        self.badge_mark_scratch = marks;
    }
}

pub(super) fn edit_badge_sources(instances: &[TileInstance]) -> Vec<EditBadgeSource> {
    const FLAG_WIGGLE: u32 = TileAnim::FLAG_WIGGLE;
    const FLAG_DRAG: u32 = TileAnim::FLAG_DRAG;

    let mut sources = Vec::new();
    for tile in instances {
        let flags = tile.extra[3] as u32;
        if flags & FLAG_WIGGLE == 0 || flags & FLAG_DRAG != 0 {
            continue;
        }

        let radius = edit_badge_radius_for_tile_size(tile.size);
        let inset = radius * 0.45;
        let center = [tile.x + inset, tile.y + inset];
        sources.push(EditBadgeSource {
            base_center: center,
            tile_center: [tile.x + tile.size * 0.5, tile.y + tile.size * 0.5],
            radius,
            phase: tile.extra[0],
        });
    }

    sources
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::grid::BASE_TILE_SIZE;

    fn tile(size: f32) -> TileInstance {
        TileInstance {
            x: 100.0,
            y: 50.0,
            size,
            radius: 19.0,
            r: 0.0,
            g: 0.0,
            b: 0.0,
            icon_index: -1.0,
            extra: [0.25, 0.0, 1.0, TileAnim::FLAG_WIGGLE as f32],
        }
    }

    #[test]
    fn edit_badge_sources_use_scaled_radius() {
        let normal = edit_badge_sources(&[tile(BASE_TILE_SIZE)]);
        let scaled = edit_badge_sources(&[tile(BASE_TILE_SIZE * 1.5)]);

        assert!((scaled[0].radius - normal[0].radius * 1.5).abs() < 1e-2);
    }

    #[test]
    fn edit_badge_center_starts_on_tile_top_left() {
        let source = edit_badge_sources(&[tile(BASE_TILE_SIZE)])[0];
        let inset = source.radius * 0.45;

        assert!((source.base_center[0] - (100.0 + inset)).abs() < 1e-4);
        assert!((source.base_center[1] - (50.0 + inset)).abs() < 1e-4);
    }
}

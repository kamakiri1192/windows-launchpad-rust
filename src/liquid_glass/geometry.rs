use crate::layout::grid::GridLayout;
use crate::ui_model::grid::GridItem;
use crate::ui_model::render_model::TileView;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GlassShape {
    /// Center position in content pixels.
    pub center: [f32; 2],
    /// Full size in pixels.
    pub size: [f32; 2],
    pub radius: f32,
    /// 0 = scrolling rounded rect (moves with `scroll_x`, e.g. tile halos),
    /// 1 = fixed rounded rect (ignores `scroll_x`, e.g. the page frame).
    pub shape_type: u32,
    /// Explicit padding so `motion` starts at the WGSL-required 16-byte
    /// boundary (offset 32).
    pub _pad: [u32; 2],
    /// Optional GPU animation payload: `(pivot_x, pivot_y, phase, flags)`.
    pub motion: [f32; 4],
}

/// Shape kind encoded into `shape_type`:
/// - 0 = scrolling rounded rectangle (moves with `scroll_x`, e.g. tile halos).
/// - 1 = fixed rounded rectangle that is the page-frame clip region (tiles are
///   clipped to this; the frame itself renders as glass).
/// - 2 = fixed rounded rectangle that lives *outside* the frame clip (the
///   bottom-center control capsule). Rendered as glass but never clipped to
///   the frame.
/// - 3 = fixed rounded rectangle used only as a clip mask. It is not rendered
///   as glass.
const SHAPE_SCROLLING: u32 = 0;
const SHAPE_FIXED: u32 = 1;
const SHAPE_CONTROL: u32 = 2;
const SHAPE_CLIP_ONLY: u32 = 3;
const SHAPE_ANIMATED_BADGE: u32 = 4;
const SHAPE_ANIMATED_SCROLLING: u32 = 5;

impl GlassShape {
    pub fn rounded_rect(center: [f32; 2], size: [f32; 2], radius: f32) -> Self {
        Self::with_kind(center, size, radius, SHAPE_SCROLLING)
    }

    /// A rounded rectangle that stays put on screen regardless of `scroll_x`.
    /// Used for the fixed page frame behind the scrolling tiles.
    pub fn fixed_rounded_rect(center: [f32; 2], size: [f32; 2], radius: f32) -> Self {
        Self::with_kind(center, size, radius, SHAPE_FIXED)
    }

    /// A fixed rounded rectangle that is rendered as glass but is *not* clipped
    /// to the page frame. Used for the bottom-center control capsule, which
    /// sits below the frame.
    pub fn control_rounded_rect(center: [f32; 2], size: [f32; 2], radius: f32) -> Self {
        Self::with_kind(center, size, radius, SHAPE_CONTROL)
    }

    /// A fixed rounded rectangle that clips scrolling shapes but is not part
    /// of the rendered glass union. Used by overlay passes that need the page
    /// frame as a mask without redrawing the frame itself.
    pub fn clip_rounded_rect(center: [f32; 2], size: [f32; 2], radius: f32) -> Self {
        Self::with_kind(center, size, radius, SHAPE_CLIP_ONLY)
    }

    pub fn animated_badge(
        center: [f32; 2],
        size: [f32; 2],
        radius: f32,
        pivot: [f32; 2],
        phase: f32,
    ) -> Self {
        let mut shape = Self::with_kind(center, size, radius, SHAPE_ANIMATED_BADGE);
        shape.motion = [pivot[0], pivot[1], phase, 1.0];
        shape
    }

    pub fn animated_scrolling_rounded_rect(
        center: [f32; 2],
        size: [f32; 2],
        radius: f32,
        phase: f32,
    ) -> Self {
        let mut shape = Self::with_kind(center, size, radius, SHAPE_ANIMATED_SCROLLING);
        shape.motion = [center[0], center[1], phase, 1.0];
        shape
    }

    fn with_kind(center: [f32; 2], size: [f32; 2], radius: f32, shape_type: u32) -> Self {
        Self {
            center,
            size,
            radius,
            shape_type,
            _pad: [0; 2],
            motion: [0.0; 4],
        }
    }
}

/// Build the base glass shapes (fixed page frame + scrolling tile halos).
/// The bottom-control capsule, if any, is appended separately via
/// [`with_control`] so the geometry buffer can be updated independently.
pub fn shapes_from_layout(
    layout: &GridLayout,
    viewport_w: f32,
    apps: &[GridItem<'_>],
) -> Vec<GlassShape> {
    let mut shapes = Vec::with_capacity(1 + apps.len().min(layout.total_tiles()));
    let (center_x, center_y, panel_w, panel_h) = layout.frame_panel_rect(viewport_w);

    // One fixed page frame centered on screen (page 0 slot). It ignores
    // `scroll_x`, so the frame stays put while the tiles/halos slide beneath.
    shapes.push(GlassShape::fixed_rounded_rect(
        [center_x, center_y],
        [panel_w, panel_h],
        layout.scaled(crate::layout::grid::FRAME_CORNER_RADIUS),
    ));

    shapes.extend(
        layout
            .build_instances(viewport_w, apps, &[])
            .iter()
            .map(|tile| shape_from_tile(layout, tile)),
    );
    shapes
}

/// Append the bottom-control capsule (if `Some`) to a base shape list.
pub fn with_control(mut shapes: Vec<GlassShape>, control: Option<GlassShape>) -> Vec<GlassShape> {
    if let Some(c) = control {
        shapes.push(c);
    }
    shapes
}

fn shape_from_tile(layout: &GridLayout, tile: &TileView) -> GlassShape {
    let center = [tile.rect.center().x, tile.rect.center().y];
    let halo_size = tile.rect.width + layout.scaled(18.0);
    let size = [halo_size, halo_size];
    GlassShape::rounded_rect(center, size, tile.radius + layout.scaled(9.0))
}

#[cfg(test)]
mod tests {
    use super::GlassShape;

    #[test]
    fn glass_shape_layout_matches_wgsl_storage_struct() {
        assert_eq!(std::mem::size_of::<GlassShape>(), 48);
        assert_eq!(std::mem::align_of::<GlassShape>(), 4);
    }
}

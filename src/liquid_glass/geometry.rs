use crate::grid::{GridApp, GridLayout, TileInstance};

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
    pub _pad: [u32; 2],
}

/// Shape kind encoded into `shape_type`:
/// - 0 = scrolling rounded rectangle (moves with `scroll_x`, e.g. tile halos).
/// - 1 = fixed rounded rectangle that is the page-frame clip region (tiles are
///   clipped to this; the frame itself renders as glass).
/// - 2 = fixed rounded rectangle that lives *outside* the frame clip (the
///   bottom-center control capsule). Rendered as glass but never clipped to
///   the frame.
const SHAPE_SCROLLING: u32 = 0;
const SHAPE_FIXED: u32 = 1;
const SHAPE_CONTROL: u32 = 2;

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

    fn with_kind(center: [f32; 2], size: [f32; 2], radius: f32, shape_type: u32) -> Self {
        Self {
            center,
            size,
            radius,
            shape_type,
            _pad: [0; 2],
        }
    }
}

/// Build the base glass shapes (fixed page frame + scrolling tile halos).
/// The bottom-control capsule, if any, is appended separately via
/// [`with_control`] so the geometry buffer can be updated independently.
pub fn shapes_from_layout(
    layout: &GridLayout,
    viewport_w: f32,
    apps: &[GridApp<'_>],
) -> Vec<GlassShape> {
    use crate::grid::FRAME_CORNER_RADIUS;

    let mut shapes = Vec::with_capacity(1 + apps.len().min(layout.total_tiles()));
    let (center_x, center_y, panel_w, panel_h) = layout.frame_panel_rect(viewport_w);

    // One fixed page frame centered on screen (page 0 slot). It ignores
    // `scroll_x`, so the frame stays put while the tiles/halos slide beneath.
    shapes.push(GlassShape::fixed_rounded_rect(
        [center_x, center_y],
        [panel_w, panel_h],
        FRAME_CORNER_RADIUS,
    ));

    shapes.extend(
        layout
            .build_instances(viewport_w, apps)
            .iter()
            .map(shape_from_tile),
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

fn shape_from_tile(tile: &TileInstance) -> GlassShape {
    let center = [tile.x + tile.size * 0.5, tile.y + tile.size * 0.5];
    let halo_size = tile.size + 18.0;
    let size = [halo_size, halo_size];
    GlassShape::rounded_rect(center, size, tile.radius + 9.0)
}

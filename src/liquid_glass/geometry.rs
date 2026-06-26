use crate::grid::{GridApp, GridLayout, TileInstance};

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
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
/// - 1 = fixed rounded rectangle (ignores `scroll_x`, e.g. the single page frame).
const SHAPE_SCROLLING: u32 = 0;
const SHAPE_FIXED: u32 = 1;

impl GlassShape {
    pub fn rounded_rect(center: [f32; 2], size: [f32; 2], radius: f32) -> Self {
        Self::with_kind(center, size, radius, SHAPE_SCROLLING)
    }

    /// A rounded rectangle that stays put on screen regardless of `scroll_x`.
    /// Used for the fixed page frame behind the scrolling tiles.
    pub fn fixed_rounded_rect(center: [f32; 2], size: [f32; 2], radius: f32) -> Self {
        Self::with_kind(center, size, radius, SHAPE_FIXED)
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

fn shape_from_tile(tile: &TileInstance) -> GlassShape {
    let center = [tile.x + tile.size * 0.5, tile.y + tile.size * 0.5];
    let halo_size = tile.size + 18.0;
    let size = [halo_size, halo_size];
    GlassShape::rounded_rect(center, size, tile.radius + 9.0)
}

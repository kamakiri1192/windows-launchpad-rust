use crate::grid::{GridApp, GridLayout, TileInstance};

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GlassShape {
    /// Center position in content pixels.
    pub center: [f32; 2],
    /// Full size in pixels.
    pub size: [f32; 2],
    pub radius: f32,
    /// 0 = rounded rectangle. Reserved for ellipse/squircle.
    pub shape_type: u32,
    pub _pad: [u32; 2],
}

impl GlassShape {
    pub fn rounded_rect(center: [f32; 2], size: [f32; 2], radius: f32) -> Self {
        Self {
            center,
            size,
            radius,
            shape_type: 0,
            _pad: [0; 2],
        }
    }
}

pub fn shapes_from_layout(
    layout: &GridLayout,
    viewport_w: f32,
    apps: &[GridApp<'_>],
) -> Vec<GlassShape> {
    let mut shapes = Vec::with_capacity(layout.page_count + apps.len().min(layout.total_tiles()));
    let grid_w =
        layout.cols as f32 * layout.tile_size + (layout.cols.saturating_sub(1)) as f32 * layout.gap;
    let grid_h = layout.rows as f32 * layout.tile_size
        + (layout.rows.saturating_sub(1)) as f32 * layout.row_gap
        + 52.0;

    for page in 0..layout.page_count {
        let page_origin_x = page as f32 * viewport_w;
        let panel_w = (grid_w + 112.0).min(viewport_w - 48.0).max(grid_w);
        let panel_h = grid_h + 72.0;
        let panel_center = [
            page_origin_x + layout.margin_left + grid_w * 0.5,
            layout.margin_top - 34.0 + panel_h * 0.5,
        ];
        shapes.push(GlassShape::rounded_rect(
            panel_center,
            [panel_w, panel_h],
            54.0,
        ));
    }

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

pub mod bottom_control;
pub mod context_menu;
pub mod control_geometry;
pub mod edit_mode;
pub mod folder_panel;
pub mod grid;
pub mod hit_map;
pub mod settings_panel;

/// Cool-neutral wash used outside a focused Liquid Glass surface. The scene
/// blur itself carries the separation; this value only lowers residual
/// contrast without tinting the transparent window backdrop.
pub const GLASS_FOCUS_VEIL_OPACITY: f32 = 0.14;

use crate::layout::hit_map::HitMap;
use crate::ui_model::render_model::RenderModel;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LayoutResult {
    pub render: RenderModel,
    pub hits: HitMap,
}

impl LayoutResult {
    pub fn new(render: RenderModel, hits: HitMap) -> Self {
        Self { render, hits }
    }
}

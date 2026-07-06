pub mod hit_map;

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

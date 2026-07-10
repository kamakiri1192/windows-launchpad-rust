//! `Renderer::prepare(&RenderModel)` — reflect renderer-neutral scene data
//! into persistent GPU resources.
//!
//! `prepare` is not a CPU renderer. It selects the render lanes described by
//! the model and converts their geometry into compact shader-facing values.
//! The Liquid Glass resource owner performs exact dirty checks and updates its
//! persistent storage buffers only when a shape actually changes.

use crate::liquid_glass::geometry::GlassShape;
use crate::ui_model::render_model::{GlassLayer, GlassMaterial, GlassSurface, RenderModel};

use super::Renderer;

/// Convert a renderer-neutral surface into the shader-facing rounded rect.
/// Layout already expresses the rect and radius in physical pixels.
fn shape_for(surface: &GlassSurface) -> GlassShape {
    let center = [surface.rect.center().x, surface.rect.center().y];
    let size = [surface.rect.width, surface.rect.height];
    match surface.material {
        GlassMaterial::Regular | GlassMaterial::Prominent => {
            GlassShape::control_rounded_rect(center, size, surface.radius)
        }
    }
}

/// The current Liquid Glass modal pass accepts one surface. Select the
/// highest-z modal surface, using later model order as the same-z tie-breaker.
/// The classification comes from renderer-neutral model data rather than a
/// feature-specific `UiId` check inside the renderer.
fn modal_shape_for(model: &RenderModel) -> Option<GlassShape> {
    model
        .glass
        .iter()
        .enumerate()
        .filter(|(_, surface)| surface.layer == GlassLayer::Modal)
        .max_by_key(|(index, surface)| (surface.z, *index))
        .map(|(_, surface)| shape_for(surface))
}

impl Renderer {
    /// Reflect the proven portions of a renderer-neutral model into persistent
    /// GPU resources. Phase 6 connects the modal glass lane; ink/text and the
    /// animation-heavy grid/control adapters remain at the app boundary.
    pub fn prepare(&mut self, model: &RenderModel) {
        self.counters.record_prepare();
        self.liquid_glass
            .set_settings_panel_shape(&self.queue, modal_shape_for(model));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_model::geometry::Rect;
    use crate::ui_model::ids::UiId;

    fn surface(id: &str, layer: GlassLayer, z: i16, cx: f32) -> GlassSurface {
        GlassSurface {
            id: UiId::launcher_item(id),
            rect: Rect::new(cx - 50.0, 20.0, 100.0, 80.0),
            radius: 18.0,
            material: GlassMaterial::Regular,
            layer,
            z,
        }
    }

    #[test]
    fn modal_selection_uses_layer_not_feature_id() {
        let mut model = RenderModel::new();
        model
            .glass
            .push(surface("arbitrary-modal", GlassLayer::Modal, 10, 100.0));

        let shape = modal_shape_for(&model).expect("modal surface");
        assert_eq!(shape.center, [100.0, 60.0]);
    }

    #[test]
    fn non_modal_surfaces_are_not_submitted_to_modal_lane() {
        let mut model = RenderModel::new();
        model
            .glass
            .push(surface("overlay", GlassLayer::Overlay, 100, 100.0));
        assert!(modal_shape_for(&model).is_none());
    }

    #[test]
    fn highest_z_modal_wins() {
        let mut model = RenderModel::new();
        model
            .glass
            .push(surface("low", GlassLayer::Modal, 10, 100.0));
        model
            .glass
            .push(surface("high", GlassLayer::Modal, 20, 200.0));

        assert_eq!(modal_shape_for(&model).unwrap().center[0], 200.0);
    }

    #[test]
    fn later_same_z_modal_wins() {
        let mut model = RenderModel::new();
        model
            .glass
            .push(surface("first", GlassLayer::Modal, 10, 100.0));
        model
            .glass
            .push(surface("later", GlassLayer::Modal, 10, 200.0));

        assert_eq!(modal_shape_for(&model).unwrap().center[0], 200.0);
    }

    #[test]
    fn shape_mapping_preserves_subpixel_geometry_exactly() {
        let source = surface("subpixel", GlassLayer::Modal, 10, 100.1);
        let shape = shape_for(&source);
        assert_eq!(shape.center[0], source.rect.center().x);
        assert_eq!(shape.center[1], source.rect.center().y);
        assert_eq!(shape.size, [source.rect.width, source.rect.height]);
        assert_eq!(shape.radius, source.radius);
    }

    #[test]
    fn empty_model_clears_modal_lane() {
        assert!(modal_shape_for(&RenderModel::new()).is_none());
    }
}

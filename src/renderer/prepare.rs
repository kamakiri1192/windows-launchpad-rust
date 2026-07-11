//! `Renderer::prepare(&RenderModel)` — reflect renderer-neutral scene data
//! into persistent GPU resources.
//!
//! `prepare` is not a CPU renderer. It selects the render lanes described by
//! the model and converts their geometry into compact shader-facing values.
//! The Liquid Glass resource owner performs exact dirty checks and updates its
//! persistent storage buffers only when a shape actually changes.

use crate::liquid_glass::geometry::GlassShape;
use crate::ui_model::render_model::{
    ControlKind, GlassBehavior, GlassLayer, GlassSurface, GlyphLane, GlyphView, IconSource,
    IconView, InkLane, InkView, RenderModel, TileView,
};

use super::controls::{
    ControlInstance, KIND_CARET, KIND_CHECK, KIND_CHEVRON, KIND_CLOSE, KIND_DOT, KIND_GEAR,
    KIND_MAGNIFIER, KIND_ROUND_RECT,
};
use super::counters::Category;
use super::icon_pipeline::IconInstance;
use super::text_engine::GlyphQuad;
use super::tiles::TileInstance;
use super::Renderer;

/// Convert a renderer-neutral surface into the shader-facing rounded rect.
/// Layout already expresses the rect and radius in physical pixels.
fn shape_for(surface: &GlassSurface) -> GlassShape {
    let center = [surface.rect.center().x, surface.rect.center().y];
    let size = [surface.rect.width, surface.rect.height];
    match surface.behavior {
        GlassBehavior::Scrolling => GlassShape::rounded_rect(center, size, surface.radius),
        GlassBehavior::FixedFrame => GlassShape::fixed_rounded_rect(center, size, surface.radius),
        GlassBehavior::Control => GlassShape::control_rounded_rect(center, size, surface.radius),
        GlassBehavior::ClipOnly => GlassShape::clip_rounded_rect(center, size, surface.radius),
    }
}

/// The current Liquid Glass modal pass accepts one surface. Select the
/// highest-z modal surface, using later model order as the same-z tie-breaker.
/// The classification comes from renderer-neutral model data rather than a
/// feature-specific `UiId` check inside the renderer.
fn highest_shape(surfaces: &[GlassSurface]) -> Option<GlassShape> {
    surfaces
        .iter()
        .enumerate()
        .max_by_key(|(index, surface)| (surface.z, *index))
        .map(|(_, surface)| shape_for(surface))
}

fn control_kind(kind: &ControlKind) -> f32 {
    match kind {
        ControlKind::Magnifier => KIND_MAGNIFIER,
        ControlKind::Dot => KIND_DOT,
        ControlKind::Caret => KIND_CARET,
        ControlKind::CloseButton => KIND_CLOSE,
        ControlKind::SettingsGear => KIND_GEAR,
        ControlKind::RowBackground | ControlKind::Toggle | ControlKind::Divider => KIND_ROUND_RECT,
        ControlKind::Checkmark => KIND_CHECK,
        ControlKind::Chevron => KIND_CHEVRON,
        // These are container/semantic views rather than foreground ink.
        ControlKind::SearchPill
        | ControlKind::PageIndicator
        | ControlKind::SearchField
        | ControlKind::EditBadge => -1.0,
    }
}

fn ink_instance(view: &InkView) -> Option<ControlInstance> {
    let kind = control_kind(&view.kind);
    (kind >= 0.0).then_some(ControlInstance {
        center: [view.center.x, view.center.y],
        params: [view.extent, view.opacity, view.stroke, view.corner_radius],
        color: [view.color.r, view.color.g, view.color.b, view.color.a],
        kind: [kind, 0.0, 0.0, 0.0],
    })
}

fn glyph_quad(view: &GlyphView) -> GlyphQuad {
    GlyphQuad {
        x: view.rect.x,
        y: view.rect.y,
        w: view.rect.width,
        h: view.rect.height,
        u0: view.uv.u0,
        v0: view.uv.v0,
        u1: view.uv.u1,
        v1: view.uv.v1,
        color: [view.color.r, view.color.g, view.color.b, view.color.a],
    }
}

fn tile_instance(view: &TileView, index: usize) -> TileInstance {
    TileInstance {
        x: view.rect.x,
        y: view.rect.y,
        size: view.rect.width,
        radius: view.radius,
        r: view.color.r,
        g: view.color.g,
        b: view.color.b,
        icon_index: if view.has_icon { index as f32 } else { -1.0 },
        extra: view.motion.shader_payload(),
    }
}

fn icon_instance(view: &IconView) -> Option<IconInstance> {
    let IconSource::AtlasUv(uv) = view.source else {
        return None;
    };
    Some(IconInstance {
        x: view.rect.x,
        y: view.rect.y,
        size: view.rect.width,
        radius: view.rect.width * (19.0 / 84.0),
        u0: uv.u0,
        v0: uv.v0,
        u1: uv.u1,
        v1: uv.v1,
        extra: view.motion.shader_payload(),
    })
}

impl Renderer {
    /// Reflect the proven portions of a renderer-neutral model into persistent
    /// GPU resources. Phase 6 connects the modal glass lane; ink/text and the
    /// animation-heavy grid/control adapters remain at the app boundary.
    pub fn prepare(&mut self, model: &RenderModel) {
        self.counters.record_prepare();
        for batch in &model.glass {
            match batch.layer {
                GlassLayer::Overlay => {
                    let shapes: Vec<_> = batch.surfaces.iter().map(shape_for).collect();
                    self.liquid_glass
                        .set_overlay_shapes(&self.device, &self.queue, &shapes);
                }
                GlassLayer::Modal => {
                    let shapes: Vec<_> = batch.surfaces.iter().map(shape_for).collect();
                    self.liquid_glass
                        .set_modal_shapes(&self.device, &self.queue, &shapes);
                }
                GlassLayer::Base => {
                    let shapes: Vec<_> = batch.surfaces.iter().map(shape_for).collect();
                    self.liquid_glass
                        .set_base_shapes(&self.device, &self.queue, &shapes);
                    if let Some(frame) = batch
                        .surfaces
                        .iter()
                        .find(|surface| surface.behavior == GlassBehavior::FixedFrame)
                    {
                        let center = frame.rect.center();
                        self.frame_clip = (
                            center.x,
                            center.y,
                            frame.rect.width * 0.5,
                            frame.rect.height * 0.5,
                            frame.radius,
                        );
                    }
                    self.counters.record_full_scene_rebuild();
                }
            }
        }

        if let Some(tiles) = &model.tiles {
            let instances: Vec<_> = tiles
                .iter()
                .enumerate()
                .map(|(index, view)| tile_instance(view, index))
                .collect();
            set_instances(
                &self.device,
                &self.queue,
                &mut self.instance_buffer,
                &instances,
                &mut self.counters,
                Category::Tile,
            );
            self.badge_sources = super::badges::edit_badge_sources(&instances);
            self.prepare_edit_badges();
        }
        if let Some(icons) = &model.icons {
            let instances: Vec<_> = icons.iter().filter_map(icon_instance).collect();
            self.dragged_icon_instance = instances
                .last()
                .map(|instance| (instance.extra[3] as u32 & 2) != 0)
                .unwrap_or(false);
            set_instances(
                &self.device,
                &self.queue,
                &mut self.icon_instance_buffer,
                &instances,
                &mut self.counters,
                Category::Icon,
            );
        }

        for batch in &model.ink {
            let instances: Vec<_> = batch.views.iter().filter_map(ink_instance).collect();
            match batch.lane {
                InkLane::BottomControl => set_instances(
                    &self.device,
                    &self.queue,
                    &mut self.control_instance_buffer,
                    &instances,
                    &mut self.counters,
                    Category::Control,
                ),
                InkLane::Gear => set_instances(
                    &self.device,
                    &self.queue,
                    &mut self.gear_instance_buffer,
                    &instances,
                    &mut self.counters,
                    Category::Gear,
                ),
                InkLane::Settings => set_instances(
                    &self.device,
                    &self.queue,
                    &mut self.settings_instance_buffer,
                    &instances,
                    &mut self.counters,
                    Category::Settings,
                ),
                InkLane::EditBadge => {}
            }
        }

        for batch in &model.glyphs {
            let quads: Vec<_> = batch.views.iter().map(glyph_quad).collect();
            match batch.lane {
                GlyphLane::Grid => set_instances(
                    &self.device,
                    &self.queue,
                    &mut self.text_instance_buffer,
                    &quads,
                    &mut self.counters,
                    Category::TextLabel,
                ),
                GlyphLane::BottomControl => set_instances(
                    &self.device,
                    &self.queue,
                    &mut self.control_text_instance_buffer,
                    &quads,
                    &mut self.counters,
                    Category::ControlText,
                ),
                GlyphLane::Settings => set_instances(
                    &self.device,
                    &self.queue,
                    &mut self.settings_text_instance_buffer,
                    &quads,
                    &mut self.counters,
                    Category::SettingsText,
                ),
            }
        }
    }
}

fn set_instances<T: bytemuck::Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &mut super::resources::InstanceBuffer<T>,
    instances: &[T],
    counters: &mut super::counters::BufferCounters,
    category: Category,
) {
    if buffer.set(device, queue, instances).allocated {
        counters.record_growth(category);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_model::geometry::{Point, Rect, UvRect};
    use crate::ui_model::ids::UiId;
    use crate::ui_model::render_model::{Color, GlassMaterial, GlyphView, InkView};

    fn surface(id: &str, z: i16, cx: f32) -> GlassSurface {
        GlassSurface {
            id: UiId::launcher_item(id),
            rect: Rect::new(cx - 50.0, 20.0, 100.0, 80.0),
            radius: 18.0,
            material: GlassMaterial::Regular,
            behavior: GlassBehavior::Control,
            z,
        }
    }

    #[test]
    fn modal_selection_uses_layer_not_feature_id() {
        let surfaces = [surface("arbitrary-modal", 10, 100.0)];
        let shape = highest_shape(&surfaces).expect("modal surface");
        assert_eq!(shape.center, [100.0, 60.0]);
    }

    #[test]
    fn non_modal_surfaces_are_not_submitted_to_modal_lane() {
        assert!(highest_shape(&[]).is_none());
    }

    #[test]
    fn highest_z_modal_wins() {
        let surfaces = [surface("low", 10, 100.0), surface("high", 20, 200.0)];
        assert_eq!(highest_shape(&surfaces).unwrap().center[0], 200.0);
    }

    #[test]
    fn later_same_z_modal_wins() {
        let surfaces = [surface("first", 10, 100.0), surface("later", 10, 200.0)];
        assert_eq!(highest_shape(&surfaces).unwrap().center[0], 200.0);
    }

    #[test]
    fn shape_mapping_preserves_subpixel_geometry_exactly() {
        let source = surface("subpixel", 10, 100.1);
        let shape = shape_for(&source);
        assert_eq!(shape.center[0], source.rect.center().x);
        assert_eq!(shape.center[1], source.rect.center().y);
        assert_eq!(shape.size, [source.rect.width, source.rect.height]);
        assert_eq!(shape.radius, source.radius);
    }

    #[test]
    fn empty_model_clears_modal_lane() {
        assert!(highest_shape(&[]).is_none());
    }

    #[test]
    fn neutral_ink_is_packed_only_inside_renderer() {
        let view = InkView {
            id: UiId::bottom_control_close(),
            center: Point::new(12.25, 34.5),
            extent: 7.0,
            opacity: 0.8,
            stroke: 1.4,
            corner_radius: 0.0,
            color: Color::rgba(1.0, 0.9, 0.8, 0.7),
            kind: ControlKind::CloseButton,
            z: 3,
        };
        let packed = ink_instance(&view).unwrap();
        assert_eq!(packed.center, [12.25, 34.5]);
        assert_eq!(packed.params, [7.0, 0.8, 1.4, 0.0]);
        assert_eq!(packed.color, [1.0, 0.9, 0.8, 0.7]);
        assert_eq!(packed.kind[0], KIND_CLOSE);
    }

    #[test]
    fn neutral_glyph_preserves_geometry_uv_and_color() {
        let view = GlyphView {
            id: UiId::settings_panel(),
            rect: Rect::new(1.0, 2.0, 3.0, 4.0),
            uv: UvRect {
                u0: 0.1,
                v0: 0.2,
                u1: 0.3,
                v1: 0.4,
            },
            color: Color::rgba(0.5, 0.6, 0.7, 0.8),
            z: 2,
        };
        let packed = glyph_quad(&view);
        assert_eq!(
            [packed.x, packed.y, packed.w, packed.h],
            [1.0, 2.0, 3.0, 4.0]
        );
        assert_eq!(
            [packed.u0, packed.v0, packed.u1, packed.v1],
            [0.1, 0.2, 0.3, 0.4]
        );
        assert_eq!(packed.color, [0.5, 0.6, 0.7, 0.8]);
    }
}

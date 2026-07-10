//! `Renderer::prepare(&RenderModel)` — the boundary that reflects a
//! renderer-neutral [`RenderModel`] into persistent GPU resources.
//!
//! `prepare` is **not** a CPU renderer. It takes a model whose production path
//! is proven (currently the settings overlay, which already emits a full
//! `RenderModel` via `layout::settings_panel`) and mirrors the parts it can
//! express safely — right now that is the glass surfaces — into the
//! corresponding Liquid Glass lane. Ink / text quads that need shader-specific
//! `ControlInstance` / `GlyphQuad` bytes, per-frame `cosmic-text` shaping, or
//! stateful animation stay in the narrow `app/render.rs` adapters; those are
//! recorded in `docs/DF_REARCHITECTURE_LOG.md` as deliberate Phase 6 limits.
//!
//! Dirty tracking: the glass section is summarized into a compact signature
//! (count + each surface's id + rect + radius + z + material). An unchanged
//! signature short-circuits the submission so a settings-animation frame that
//! doesn't move the panel glass does not re-upload or re-bind anything.

use crate::liquid_glass::geometry::GlassShape;
use crate::ui_model::render_model::{GlassMaterial, GlassSurface, RenderModel};

use super::counters::Category;
use super::Renderer;

/// A renderer-neutral glass surface categorized by how the renderer must draw
/// it. This is intentionally *not* a feature name: the renderer does not know
/// whether a surface came from settings, a folder, or the bottom control. The
/// modal lane is the only one `prepare` submits today; fixed-overlay (control
/// / gear) and base/scrolling lanes are wired in the Phase 6D glass
/// generalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GlassLane {
    /// A screen-fixed glass surface drawn above the grid, control, and gear
    /// (e.g. the settings overlay panel). Maps to the settings-panel Liquid
    /// Glass pass.
    Modal,
}

/// Render-lane classification for a `GlassSurface`. Kept here (renderer-side)
/// so the model never carries shader-specific integers or feature names.
fn lane_for(surface: &GlassSurface) -> Option<GlassLane> {
    // The settings panel is the only modal glass the production path emits
    // today. It is identified by its stable id. As more modal surfaces appear
    // (folders in Phase 8), this classifier grows here, not in the model.
    if surface.id == crate::ui_model::ids::UiId::settings_panel() {
        return Some(GlassLane::Modal);
    }
    None
}

/// Convert a renderer-neutral `GlassSurface` into the shader-facing
/// `GlassShape`. The model carries center/radius/size in physical px already
/// (layout applies scale_factor), so this is a pure mapping — no math.
fn shape_for(surface: &GlassSurface) -> GlassShape {
    let center = [surface.rect.center().x, surface.rect.center().y];
    let size = [surface.rect.width, surface.rect.height];
    // Material does not currently change the shape encoding; the appearance is
    // driven by the Liquid Glass params uniform. Kept in the signature so a
    // material switch still counts as a dirty change.
    match surface.material {
        GlassMaterial::Regular | GlassMaterial::Prominent => {
            GlassShape::control_rounded_rect(center, size, surface.radius)
        }
    }
}

impl Renderer {
    /// Reflect a renderer-neutral [`RenderModel`] into persistent GPU
    /// resources. Called from the production frame path (currently the
    /// settings overlay). Does **not** rebuild the whole scene on every call:
    /// the glass section is dirty-tracked via [`GlassSignature`], and an
    /// unchanged model short-circuits.
    ///
    /// What `prepare` owns after Phase 6C:
    /// - the settings modal glass surface (submitted to the settings-panel
    ///   Liquid Glass lane).
    ///
    /// What stays in `app/render.rs` (deliberate adapters, see the log):
    /// - settings ink (`ControlInstance` list) and title text (`GlyphQuad`
    ///   list) — these are shader-specific bytes produced by
    ///   `build_settings_panel_instances` / `build_settings_panel_text_views`,
    ///   which depend on the GPU-facing overlay builder and `cosmic-text`.
    /// - bottom-control / gear / grid / edit-badge paths, which need per-frame
    ///   shaping, spring positions, or time-based geometry the current
    ///   `RenderModel` cannot express safely.
    pub fn prepare(&mut self, model: &RenderModel) {
        self.counters.record_prepare();

        let signature = GlassSignature::from_model(model);
        if signature == self.last_glass_signature {
            // Unchanged glass: skip re-submission entirely. This is the
            // settings-animation case (alpha/pop scale changes do move the
            // glass rect when progress != 0, but a settled settings panel
            // emits an identical surface every frame).
            return;
        }

        // Collect modal glass surfaces in model order (deterministic z within
        // the lane; the Liquid Glass settings-panel pass draws one shape).
        let mut modal: Vec<GlassShape> = Vec::new();
        for surface in &model.glass {
            if let Some(GlassLane::Modal) = lane_for(surface) {
                modal.push(shape_for(surface));
            }
        }

        // Submit the modal lane. The Liquid Glass settings-panel pass expects
        // at most one shape today; submit the first and rely on the empty-list
        // semantics to hide the surface when settings closes.
        let modal_shape = modal.first().copied();
        self.set_settings_panel_glass_shape(modal_shape);
        if modal_shape.is_some() {
            self.counters.record_creation(Category::Settings);
        }

        self.last_glass_signature = signature;
    }
}

/// A compact, comparable summary of the glass section of a `RenderModel`.
/// Equality means "the renderer's glass output would be identical", so a
/// matching signature is a safe signal to skip re-submission.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct GlassSignature {
    entries: Vec<GlassSigEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct GlassSigEntry {
    // Identity + geometry + material, quantized to avoid float noise. The
    // quantization step (0.25 px) is fine enough that visible motion still
    // registers as dirty.
    id_hash: u64,
    cx_q: i32,
    cy_q: i32,
    w_q: i32,
    h_q: i32,
    r_q: i32,
    material: u8,
    z: i16,
}

impl GlassSignature {
    fn from_model(model: &RenderModel) -> Self {
        use std::hash::{Hash, Hasher};
        let mut entries = Vec::with_capacity(model.glass.len());
        for s in &model.glass {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            s.id.as_str().hash(&mut h);
            entries.push(GlassSigEntry {
                id_hash: h.finish(),
                cx_q: (s.rect.center().x * 4.0).round() as i32,
                cy_q: (s.rect.center().y * 4.0).round() as i32,
                w_q: (s.rect.width * 4.0).round() as i32,
                h_q: (s.rect.height * 4.0).round() as i32,
                r_q: (s.radius * 4.0).round() as i32,
                material: match s.material {
                    GlassMaterial::Regular => 0,
                    GlassMaterial::Prominent => 1,
                },
                z: s.z,
            });
        }
        Self { entries }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_model::geometry::Rect;
    use crate::ui_model::ids::UiId;

    fn panel_surface(cx: f32, cy: f32, w: f32, h: f32, r: f32) -> GlassSurface {
        GlassSurface {
            id: UiId::settings_panel(),
            rect: Rect::new(cx - w * 0.5, cy - h * 0.5, w, h),
            radius: r,
            material: GlassMaterial::Regular,
            z: 100,
        }
    }

    #[test]
    fn identical_models_produce_equal_signatures() {
        let a = GlassSignature::from_model(&{
            let mut m = RenderModel::new();
            m.glass
                .push(panel_surface(100.0, 200.0, 400.0, 300.0, 18.0));
            m
        });
        let b = GlassSignature::from_model(&{
            let mut m = RenderModel::new();
            m.glass
                .push(panel_surface(100.0, 200.0, 400.0, 300.0, 18.0));
            m
        });
        assert_eq!(a, b);
    }

    #[test]
    fn moved_surface_is_dirty() {
        let a = GlassSignature::from_model(&{
            let mut m = RenderModel::new();
            m.glass
                .push(panel_surface(100.0, 200.0, 400.0, 300.0, 18.0));
            m
        });
        let b = GlassSignature::from_model(&{
            let mut m = RenderModel::new();
            m.glass
                .push(panel_surface(101.0, 200.0, 400.0, 300.0, 18.0));
            m
        });
        assert_ne!(a, b);
    }

    #[test]
    fn radius_change_is_dirty() {
        let a = GlassSignature::from_model(&{
            let mut m = RenderModel::new();
            m.glass
                .push(panel_surface(100.0, 200.0, 400.0, 300.0, 18.0));
            m
        });
        let b = GlassSignature::from_model(&{
            let mut m = RenderModel::new();
            m.glass
                .push(panel_surface(100.0, 200.0, 400.0, 300.0, 19.0));
            m
        });
        assert_ne!(a, b);
    }

    #[test]
    fn material_change_is_dirty() {
        let a = GlassSignature::from_model(&{
            let mut m = RenderModel::new();
            m.glass
                .push(panel_surface(100.0, 200.0, 400.0, 300.0, 18.0));
            m
        });
        let b = GlassSignature::from_model(&{
            let mut m = RenderModel::new();
            let mut s = panel_surface(100.0, 200.0, 400.0, 300.0, 18.0);
            s.material = GlassMaterial::Prominent;
            m.glass.push(s);
            m
        });
        assert_ne!(a, b);
    }

    #[test]
    fn empty_vs_nonempty_is_dirty() {
        let empty = GlassSignature::from_model(&RenderModel::new());
        let full = GlassSignature::from_model(&{
            let mut m = RenderModel::new();
            m.glass
                .push(panel_surface(100.0, 200.0, 400.0, 300.0, 18.0));
            m
        });
        assert_ne!(empty, full);
    }

    #[test]
    fn sub_pixel_motion_below_quantization_is_not_dirty() {
        // 0.1px motion rounds to the same 0.25px quantization bin.
        let a = GlassSignature::from_model(&{
            let mut m = RenderModel::new();
            m.glass
                .push(panel_surface(100.0, 200.0, 400.0, 300.0, 18.0));
            m
        });
        let b = GlassSignature::from_model(&{
            let mut m = RenderModel::new();
            m.glass
                .push(panel_surface(100.1, 200.0, 400.0, 300.0, 18.0));
            m
        });
        assert_eq!(a, b);
    }

    #[test]
    fn lane_classifier_maps_settings_panel_to_modal() {
        let s = panel_surface(0.0, 0.0, 1.0, 1.0, 1.0);
        assert_eq!(lane_for(&s), Some(GlassLane::Modal));
    }

    #[test]
    fn lane_classifier_leaves_unknown_surfaces_unrouted() {
        let s = GlassSurface {
            id: UiId::launcher_item("some-app"),
            rect: Rect::new(0.0, 0.0, 1.0, 1.0),
            radius: 1.0,
            material: GlassMaterial::Regular,
            z: 0,
        };
        assert_eq!(lane_for(&s), None);
    }

    #[test]
    fn shape_for_maps_center_size_radius() {
        let s = panel_surface(960.0, 600.0, 400.0, 300.0, 18.0);
        let shape = shape_for(&s);
        assert!((shape.center[0] - 960.0).abs() < 1e-2);
        assert!((shape.center[1] - 600.0).abs() < 1e-2);
        assert!((shape.size[0] - 400.0).abs() < 1e-2);
        assert!((shape.size[1] - 300.0).abs() < 1e-2);
        assert!((shape.radius - 18.0).abs() < 1e-2);
    }
}

//! Layout boundary for the morphing bottom-center control (search pill ↔
//! page indicator ↔ search field) and its edit-mode satellites (Done capsule,
//! settings gear).
//!
//! This module produces, in a single layout pass, both the render-side
//! geometry (capsule shape, content layers, gear geometry, close-button X)
//! that `main.rs` adapts back into the existing renderer upload path, and the
//! hit regions that pointer routing consumes. It is the Phase 2 counterpart of
//! [`crate::layout::settings_panel`].
//!
//! Behavior preservation: the state machine, IME handling, caret blink, page
//! indicator timing, search matching, and `ControlInstance` generation remain
//! in [`crate::features::bottom_control`] / `main.rs`. This module only owns the hit map
//! and the shared geometry snapshot so render geometry and hit regions come
//! from the same calculation.

use crate::layout::control_geometry::{
    self, close_button_x_scaled, contains_capsule, edit_gear_geometry,
    resolve_scaled_with_edit_width, ControlGeometry, ControlLayer, ControlState, EditGearGeometry,
    EditWidth, Mode,
};
use crate::layout::hit_map::{HitMap, HitRegion};
use crate::layout::LayoutResult;
use crate::ui_model::geometry::{Point, Rect};
use crate::ui_model::hit::HitTarget;
use crate::ui_model::ids::UiId;

/// Half-size (logical px) of the square close-button hit region around the
/// close glyph. Matches the previous inline hit test in `main.rs`.
const CLOSE_HIT_HALF: f32 = 12.0;

/// Z-order for bottom-control hit regions. Higher z wins inside the hit map.
/// The gear sits on top of the capsule pair, and the close button sits above
/// the capsule so an open field's × is reachable even though it overlaps the
/// capsule shape.
const Z_CAPSULE: i16 = 10;
const Z_CLOSE: i16 = 20;
const Z_GEAR: i16 = 30;

/// Read-only snapshot of the inputs the layout needs to resolve the bottom
/// control's geometry and hit regions for one frame.
///
/// `main.rs` builds this from `App` state every time it needs a fresh hit map
/// (currently once per pointer press/release). The values mirror what the
/// existing adapter code passed into the individual `bottom_control` helpers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BottomControlInput {
    pub viewport: (u32, u32),
    /// Y of the bottom edge of the fixed page frame (physical px).
    pub frame_bottom: f32,
    pub scale_factor: f32,
    pub page: usize,
    pub page_count: usize,
    pub mode: Mode,
    /// 0 = pill size, 1 = full field size (animated).
    pub expand: f32,
    /// 0 = search pill content, 1 = page indicator content (animated).
    pub indicator: f32,
    pub editing: bool,
    /// 1.0 while editing, else tracks the animated collapse. Drives gear
    /// visibility and alpha.
    pub edit_visual_progress: f32,
    /// 0..1 animated Done-width morph, fed into the capsule resolve.
    pub edit_control_progress: f32,
    /// Measured laid-out width of the "完了" Done label, when available.
    pub cached_done_width: Option<f32>,
    /// True while the settings overlay is open or animating; the gear is hidden
    /// in that case.
    pub settings_open: bool,
}

impl BottomControlInput {
    fn state(&self) -> ControlState {
        ControlState::new(self.mode, self.expand, self.indicator)
    }
}

/// Resolved render-side geometry for the bottom control, produced from one
/// layout pass. `main.rs` consumes this to drive the existing renderer upload
/// path (`build_overlay_instances`, glass-shape upload, edit-gear upload).
#[derive(Debug, Clone, PartialEq)]
pub struct BottomControlLayout {
    pub capsule: (ControlGeometry, Vec<ControlLayer>),
    pub gear: Option<(EditGearGeometry, f32)>,
    /// Physical-px X of the close glyph, when the close button is visible.
    pub close_button_x: Option<f32>,
    pub input: BottomControlInput,
}

/// The result of a bottom-control layout pass: the shared geometry snapshot
/// plus the [`LayoutResult`] carrying the hit map.
#[derive(Debug, Clone, PartialEq)]
pub struct BottomControlModel {
    pub layout: BottomControlLayout,
    pub result: LayoutResult,
}

/// Narrow pointer intent for the bottom control, mirroring the settings-panel
/// intent enum. The app shell translates a point into one of these before
/// dispatching the side effect, instead of duplicating geometry inline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BottomControlPointerIntent {
    /// The pointer missed every bottom-control region.
    None,
    /// The search pill / field capsule body. A click toggles the field open or
    /// closed (or, in edit mode, hits the Done capsule).
    Capsule,
    /// The × close button inside an open search field.
    CloseButton,
    /// The edit-mode settings gear capsule.
    EditGear,
}

impl BottomControlInput {
    /// Build the geometry snapshot and hit map for this frame.
    ///
    /// The capsule geometry is resolved exactly as the previous adapter code
    /// did. The hit capsule uses the non-edit-width `resolve_scaled` shape
    /// (matching the previous `hit_test_scaled`), so the hit region is resolved
    /// without the edit-width override even though the *rendered* capsule uses
    /// the morphed width.
    pub fn build(self) -> BottomControlModel {
        let gear = self.gear_geometry();
        let capsule = self.capsule_geometry_for_render();
        let capsule_hit = self.capsule_geometry_for_hit();
        let close_button_x = self.close_button_x();

        let mut hits = HitMap::new();
        self.push_capsule_hit(&mut hits, &capsule_hit);
        self.push_close_hit(&mut hits, capsule.0.center.1, close_button_x);
        self.push_gear_hit(&mut hits, gear);

        let layout = BottomControlLayout {
            capsule,
            gear,
            close_button_x,
            input: self,
        };
        BottomControlModel {
            layout,
            result: LayoutResult::new(Default::default(), hits),
        }
    }

    /// Resolve the *rendered* capsule geometry, including the edit-width morph.
    /// This is what `render_bottom_control` and the IME caret anchor consume.
    fn capsule_geometry_for_render(&self) -> (ControlGeometry, Vec<ControlLayer>) {
        let edit_width = self.cached_done_width.map(|w| EditWidth {
            half_width: control_geometry::done_half_width(w, self.scale_factor),
            progress: self.edit_control_progress,
        });
        resolve_scaled_with_edit_width(
            self.state(),
            self.viewport,
            self.frame_bottom,
            self.page,
            self.page_count,
            self.scale_factor,
            edit_width,
        )
    }

    /// Resolve the *hit* capsule geometry, without the edit-width morph. This
    /// matches the previous `hit_test_scaled`, which used `resolve_scaled`
    /// (no edit width) for the hit shape even while the rendered capsule was
    /// morphed.
    fn capsule_geometry_for_hit(&self) -> ControlGeometry {
        let (geom, _) = resolve_scaled_with_edit_width(
            self.state(),
            self.viewport,
            self.frame_bottom,
            0,
            0,
            self.scale_factor,
            None,
        );
        geom
    }

    /// Resolve the edit-mode gear geometry, or `None` when it should not be
    /// visible. Mirrors `render_gear`'s visibility rule.
    fn gear_geometry(&self) -> Option<(EditGearGeometry, f32)> {
        if !self.editing || self.settings_open || self.edit_visual_progress <= 0.0 {
            return None;
        }
        let done_half_width = self
            .cached_done_width
            .map(|w| control_geometry::done_half_width(w, self.scale_factor))
            .unwrap_or_else(|| control_geometry::done_half_width(0.0, self.scale_factor));
        edit_gear_geometry(
            self.viewport,
            self.frame_bottom,
            self.scale_factor,
            done_half_width,
            self.edit_visual_progress,
        )
    }

    /// Resolve the close-button X, or `None` when the close button is hidden.
    /// The close button is suppressed in edit mode: while editing, the
    /// rendered search-field layers are hidden whenever
    /// `edit_visual_progress > 0`, so the × would be an invisible hotspot.
    /// The previous pointer code never evaluated the close button while
    /// editing (the edit-mode branch returned first), so suppressing the
    /// region here keeps the hit map honest without changing dispatch.
    fn close_button_x(&self) -> Option<f32> {
        if self.editing {
            return None;
        }
        close_button_x_scaled(
            self.state(),
            self.viewport,
            self.frame_bottom,
            self.scale_factor,
        )
    }

    fn push_capsule_hit(&self, hits: &mut HitMap, geom: &ControlGeometry) {
        // The capsule hit shape is the non-edit-width resolve, matching the
        // previous `hit_test_scaled` behavior. The hit map stores the bounding
        // rect for z-ordering; the precise capsule test (inner rect + endcap
        // circles) is preserved by `contains_capsule`, which callers and tests
        // use to classify points against the rounded shape.
        let hw = geom.half_size.0;
        let hh = geom.half_size.1;
        let rect = Rect::new(geom.center.0 - hw, geom.center.1 - hh, hw * 2.0, hh * 2.0);
        // `rect_inclusive` matches the previous `hit_test_scaled` behavior,
        // which used `<=` for the inner-rect and endcap tests.
        hits.push(HitRegion::rect_inclusive(
            UiId::bottom_control(),
            rect,
            self.capsule_target(),
            Z_CAPSULE,
        ));
    }

    fn push_close_hit(&self, hits: &mut HitMap, capsule_cy: f32, close_x: Option<f32>) {
        let Some(cx) = close_x else { return };
        // Only emit a close region when the field is open enough for the close
        // button to be visible. `close_button_x` already encodes that gate, so
        // reaching here means the × is shown.
        let half = CLOSE_HIT_HALF * self.scale_factor.max(1.0);
        let rect = Rect::new(cx - half, capsule_cy - half, half * 2.0, half * 2.0);
        // `rect_inclusive` matches the previous `<=` square hit test.
        hits.push(HitRegion::rect_inclusive(
            UiId::bottom_control_close(),
            rect,
            HitTarget::BottomControlClose,
            Z_CLOSE,
        ));
    }

    fn push_gear_hit(&self, hits: &mut HitMap, gear: Option<(EditGearGeometry, f32)>) {
        let Some((geom, _)) = gear else { return };
        hits.push(HitRegion::circle(
            UiId::edit_settings_gear(),
            Point::new(geom.center.0, geom.center.1),
            geom.radius,
            HitTarget::EditSettingsGear,
            Z_GEAR,
        ));
    }

    fn capsule_target(&self) -> HitTarget {
        // The capsule body is the search field while it can take input, else
        // the generic bottom-control target. `wants_keyboard` is preserved by
        // re-checking the mode the same way the state machine does.
        if self.mode.wants_keyboard() {
            HitTarget::SearchField
        } else {
            HitTarget::BottomControl
        }
    }
}

/// Test a physical-pixel point against the bottom-control hit regions and
/// return the narrow pointer intent. This is the single entry point the app
/// shell uses for press/release/click dispatch.
///
/// The hit map stores capsule/gear bounding shapes; the capsule region uses
/// its bounding rect for z-ordering, so callers that need the precise rounded
/// capsule test should defer corner cases to [`contains_capsule`].
pub fn hit_test(model: &BottomControlModel, point: Point) -> BottomControlPointerIntent {
    // The capsule region's bounding rect can contain corner points that the
    // rounded capsule does not. Preserve the previous `hit_test_scaled` shape
    // by applying the precise capsule test before consulting the bounding-rect
    // hit map for the capsule region.
    let capsule_geom = model.capsule_hit_geometry();
    if contains_capsule(&capsule_geom, point) {
        // Check higher-z regions first so the close button and gear win over
        // the capsule body where they overlap.
        if let Some(region) = model.result.hits.hit_test(point) {
            return match region.target {
                HitTarget::EditSettingsGear => BottomControlPointerIntent::EditGear,
                HitTarget::BottomControlClose => BottomControlPointerIntent::CloseButton,
                HitTarget::SearchField | HitTarget::BottomControl => {
                    BottomControlPointerIntent::Capsule
                }
                _ => BottomControlPointerIntent::Capsule,
            };
        }
        return BottomControlPointerIntent::Capsule;
    }
    // Off the capsule: the gear circle may still be hit (the gear sits beside
    // the capsule in edit mode and is not covered by the capsule shape).
    if let Some(region) = model.result.hits.hit_test(point) {
        return match region.target {
            HitTarget::EditSettingsGear => BottomControlPointerIntent::EditGear,
            HitTarget::BottomControlClose => BottomControlPointerIntent::CloseButton,
            _ => BottomControlPointerIntent::None,
        };
    }
    BottomControlPointerIntent::None
}

impl BottomControlModel {
    /// The capsule geometry used for hit-testing (non-edit-width resolve).
    fn capsule_hit_geometry(&self) -> ControlGeometry {
        let (geom, _) = resolve_scaled_with_edit_width(
            self.layout.input.state(),
            self.layout.input.viewport,
            self.layout.input.frame_bottom,
            0,
            0,
            self.layout.input.scale_factor,
            None,
        );
        geom
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::control_geometry::{edit_gear_hit, CAPSULE_HEIGHT, FIELD_HALF_WIDTH};

    fn input(mode: Mode) -> BottomControlInput {
        BottomControlInput {
            viewport: (1280, 800),
            frame_bottom: 600.0,
            scale_factor: 1.0,
            page: 0,
            page_count: 3,
            mode,
            expand: match mode {
                Mode::Field => 1.0,
                _ => 0.0,
            },
            indicator: 0.0,
            editing: false,
            edit_visual_progress: 0.0,
            edit_control_progress: 0.0,
            cached_done_width: None,
            settings_open: false,
        }
    }

    fn edit_input() -> BottomControlInput {
        BottomControlInput {
            editing: true,
            edit_visual_progress: 1.0,
            edit_control_progress: 1.0,
            cached_done_width: Some(28.0),
            ..input(Mode::Pill)
        }
    }

    #[test]
    fn pill_capsule_is_centered() {
        let model = input(Mode::Pill).build();
        let (geom, layers) = &model.layout.capsule;
        assert!((geom.center.0 - 640.0).abs() < 1e-3);
        assert!((geom.half_size.1 - CAPSULE_HEIGHT * 0.5).abs() < 1e-3);
        assert!(layers
            .iter()
            .any(|l| l.visual == control_geometry::Visual::SearchPill));
    }

    #[test]
    fn field_capsule_is_wide() {
        let model = input(Mode::Field).build();
        let (geom, _) = &model.layout.capsule;
        assert!((geom.half_size.0 - FIELD_HALF_WIDTH).abs() < 1e-2);
    }

    #[test]
    fn pill_center_hits_capsule_intent() {
        let model = input(Mode::Pill).build();
        let (geom, _) = &model.layout.capsule;
        let intent = hit_test(&model, Point::new(geom.center.0, geom.center.1));
        assert_eq!(intent, BottomControlPointerIntent::Capsule);
    }

    #[test]
    fn far_point_misses() {
        let model = input(Mode::Pill).build();
        assert_eq!(
            hit_test(&model, Point::new(10.0, 10.0)),
            BottomControlPointerIntent::None
        );
    }

    #[test]
    fn field_emits_close_region() {
        let model = input(Mode::Field).build();
        let cx = model.layout.close_button_x.expect("close x");
        let cy = model.layout.capsule.0.center.1;
        // Direct hit on the ×.
        assert_eq!(
            hit_test(&model, Point::new(cx, cy)),
            BottomControlPointerIntent::CloseButton
        );
    }

    #[test]
    fn pill_does_not_emit_close_region() {
        let model = input(Mode::Pill).build();
        assert!(model.layout.close_button_x.is_none());
        assert!(model
            .result
            .hits
            .regions()
            .iter()
            .all(|r| r.target != HitTarget::BottomControlClose));
    }

    #[test]
    fn close_region_uses_square_shape_at_dpi_floor() {
        // At scale 1.0 the close hit half-size is 12.0 (the .max(1.0) floor).
        let mut input = input(Mode::Field);
        input.scale_factor = 1.0;
        let model = input.build();
        let cx = model.layout.close_button_x.unwrap();
        let cy = model.layout.capsule.0.center.1;
        // Corner of the square (12 px on each axis) still hits.
        assert_eq!(
            hit_test(&model, Point::new(cx + 12.0, cy + 12.0)),
            BottomControlPointerIntent::CloseButton
        );
        // Just outside the square misses the close button.
        let region = model
            .result
            .hits
            .regions()
            .iter()
            .find(|r| r.target == HitTarget::BottomControlClose)
            .unwrap();
        assert!(!region.contains(Point::new(cx + 13.0, cy + 13.0)));
    }

    #[test]
    fn close_region_scales_with_dpi() {
        let mut input = input(Mode::Field);
        input.scale_factor = 2.0;
        let model = input.build();
        let cx = model.layout.close_button_x.unwrap();
        let cy = model.layout.capsule.0.center.1;
        // At scale 2.0 the half-size is 24.0.
        assert_eq!(
            hit_test(&model, Point::new(cx + 20.0, cy + 20.0)),
            BottomControlPointerIntent::CloseButton
        );
    }

    #[test]
    fn close_region_suppressed_in_edit_mode() {
        // While editing, the rendered search-field layers are hidden, so the
        // close button must not emit a hit region — otherwise it would be an
        // invisible hotspot. This mirrors the previous code, which never
        // evaluated the close button while editing.
        let mut input = input(Mode::Field);
        input.editing = true;
        input.edit_visual_progress = 1.0;
        let model = input.build();
        assert!(model.layout.close_button_x.is_none());
        assert!(model
            .result
            .hits
            .regions()
            .iter()
            .all(|r| r.target != HitTarget::BottomControlClose));
    }

    #[test]
    fn edit_mode_emits_gear_region() {
        let model = edit_input().build();
        let (gear, _) = model.layout.gear.expect("gear geometry");
        assert_eq!(
            hit_test(&model, Point::new(gear.center.0, gear.center.1)),
            BottomControlPointerIntent::EditGear
        );
    }

    #[test]
    fn gear_hidden_outside_edit_mode() {
        let model = input(Mode::Pill).build();
        assert!(model.layout.gear.is_none());
        assert!(model
            .result
            .hits
            .regions()
            .iter()
            .all(|r| r.target != HitTarget::EditSettingsGear));
    }

    #[test]
    fn gear_hidden_while_settings_open() {
        let mut input = edit_input();
        input.settings_open = true;
        let model = input.build();
        assert!(model.layout.gear.is_none());
    }

    #[test]
    fn gear_hidden_when_edit_progress_zero() {
        let mut input = edit_input();
        input.edit_visual_progress = 0.0;
        let model = input.build();
        assert!(model.layout.gear.is_none());
    }

    #[test]
    fn capsule_target_reflects_keyboard_focus() {
        let pill = input(Mode::Pill).build();
        let field = input(Mode::Field).build();
        let pill_target = pill
            .result
            .hits
            .regions()
            .iter()
            .find(|r| r.id.as_str() == "bottom-control")
            .unwrap()
            .target
            .clone();
        let field_target = field
            .result
            .hits
            .regions()
            .iter()
            .find(|r| r.id.as_str() == "bottom-control")
            .unwrap()
            .target
            .clone();
        assert_eq!(pill_target, HitTarget::BottomControl);
        assert_eq!(field_target, HitTarget::SearchField);
    }

    #[test]
    fn hit_test_preserves_capsule_corner_gap() {
        // The previous `hit_test_scaled` rejected points in the corner gap
        // between the bounding rect and the rounded capsule. The layout hit
        // test must preserve that.
        let model = input(Mode::Pill).build();
        let (geom, _) = &model.layout.capsule;
        let hw = geom.half_size.0;
        let hh = geom.half_size.1;
        let corner = Point::new(geom.center.0 + hw, geom.center.1 + hh);
        assert_eq!(hit_test(&model, corner), BottomControlPointerIntent::None);
    }

    #[test]
    fn geometry_scales_with_dpi() {
        let mut input = input(Mode::Field);
        input.scale_factor = 2.0;
        let model = input.build();
        let (geom, _) = &model.layout.capsule;
        // Height doubles with DPI.
        assert!((geom.half_size.1 - CAPSULE_HEIGHT).abs() < 1e-3);
    }

    #[test]
    fn edit_gear_hit_circle_preserved() {
        let model = edit_input().build();
        let (gear, _) = model.layout.gear.expect("gear geometry");
        // edit_gear_hit is a true circle; a point just inside the radius hits.
        let on_circle = Point::new(gear.center.0 + gear.radius * 0.9, gear.center.1);
        assert!(edit_gear_hit(&gear, on_circle.x, on_circle.y));
        assert_eq!(
            hit_test(&model, on_circle),
            BottomControlPointerIntent::EditGear
        );
        // A point just outside the radius misses.
        let off_circle = Point::new(gear.center.0 + gear.radius * 1.1, gear.center.1);
        assert!(!edit_gear_hit(&gear, off_circle.x, off_circle.y));
    }

    #[test]
    fn edit_mode_capsule_hit_uses_full_pill_width() {
        // The previous `hit_test_scaled` resolved the capsule hit shape
        // *without* the edit-width morph, so even in edit mode (where the
        // rendered Done capsule is narrower than the pill and shifted left)
        // the hit shape keeps the full pill half-width centered on the
        // viewport. The layout hit map must preserve this.
        let model = edit_input().build();
        let (rendered_geom, _) = &model.layout.capsule;
        // The rendered Done capsule is narrower than the pill...
        assert!(rendered_geom.half_size.0 < control_geometry::pill_half_width());
        // ...and it is shifted left by the edit offset, so its center is not
        // the viewport center.
        assert!(rendered_geom.center.0 < 640.0);
        // The hit capsule, however, is resolved without the edit-width morph,
        // so it is centered on the viewport with the full pill half-width.
        // A point at the viewport center must hit the capsule (not the gear,
        // which sits to the right of the shifted Done capsule).
        let point = Point::new(640.0, rendered_geom.center.1);
        assert_eq!(
            hit_test(&model, point),
            BottomControlPointerIntent::Capsule,
            "edit-mode hit shape must keep the full pill width"
        );
    }
}

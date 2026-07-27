//! Immediate-mode `Ui` context.
//!
//! The `Ui` struct is the central hub for one frame: it accepts layout
//! container calls, buffers draw data (`InkView`, `GlassSurface`,
//! `GlyphView`, `TextView`, `HitRegion`), tracks transient per-element
//! state, and finally yields a [`RenderModel`] + [`HitMap`] + [`Registry`]
//! via [`Ui::take`].

use std::collections::HashMap;

use crate::layout::hit_map::{HitMap, HitRegion};
use crate::layout::LayoutResult;
use crate::ui_model::geometry::{ClipRegion, Point, Rect};
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{
    GlassLayer, GlassSurface, GlyphLane, GlyphView, InkLane, InkView, RenderModel,
};
use crate::ui_model::text::TextView;

use super::interaction::ElementState;
use super::registry::Registry;
use super::theme::Theme;

// Used by tests; will be referenced by widget code in Phase 2.
#[allow(unused_imports)]
use crate::ui_model::render_model::{Color, ControlKind};

// ---------------------------------------------------------------------------
// Layout direction
// ---------------------------------------------------------------------------

/// Primary layout axis for the current container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutDirection {
    Vertical,
    Horizontal,
}

impl Default for LayoutDirection {
    fn default() -> Self {
        Self::Vertical
    }
}

// ---------------------------------------------------------------------------
// Ui
// ---------------------------------------------------------------------------

/// Immediate-mode UI context for one frame.
///
/// # Lifecycle
///
/// 1. Create with [`Ui::new`].
/// 2. Set input state via `set_pointer*` / `set_focused`.
/// 3. Call layout containers (`column`, `row`, …) to build the frame.
/// 4. Call [`Ui::take`] to extract [`RenderModel`], [`HitMap`], and
///    [`Registry`].
pub struct Ui {
    // ---- layout cursor --------------------------------------------------
    pub(crate) cursor_x: f32,
    pub(crate) cursor_y: f32,
    pub(crate) available_width: f32,
    pub(crate) available_height: Option<f32>,
    pub(crate) direction: LayoutDirection,
    pub(crate) spacing: f32,
    pub(crate) clip_stack: Vec<ClipRegion>,
    /// `true` until the first widget is placed in the current container.
    /// When `false`, the next widget will have `spacing` inserted before it.
    pub(crate) first_in_container: bool,

    // ---- theme ----------------------------------------------------------
    theme: Theme,

    // ---- draw-data buffers (per-lane so multiple widgets can contribute) --
    ink_by_lane: HashMap<InkLane, Vec<InkView>>,
    glass_by_layer: HashMap<GlassLayer, Vec<GlassSurface>>,
    glyphs_by_lane: HashMap<GlyphLane, Vec<GlyphView>>,
    text_views: Vec<TextView>,
    hits: HitMap,
    registry: Registry,

    // ---- transient per-element state ------------------------------------
    element_states: HashMap<UiId, ElementState>,
    focused_id: Option<UiId>,

    // ---- input ----------------------------------------------------------
    pointer_pos: Option<Point>,
    pointer_pressed: bool,
    #[allow(dead_code)]
    last_click_consumed: bool,
    /// If `Some(id)`, the widget with this id reports `clicked = true` this
    /// frame. Set by the app after performing external hit-test on pointer
    /// release. Phase 4 (settings panel migration) will wire this into the
    /// existing SettingsPressTarget routing.
    active_click_id: Option<UiId>,

    // ---- anonymous id counter -------------------------------------------
    anon_counter: u64,
}

impl Ui {
    // ------------------------------------------------------------------
    // Construction
    // ------------------------------------------------------------------

    /// Create a new `Ui` for a viewport of the given size.
    ///
    /// The root container is a column with no height limit.
    pub fn new(theme: Theme, viewport_width: f32, _viewport_height: f32) -> Self {
        Self {
            cursor_x: 0.0,
            cursor_y: 0.0,
            available_width: viewport_width,
            available_height: None,
            direction: LayoutDirection::default(),
            spacing: 0.0,
            clip_stack: Vec::new(),
            first_in_container: true,
            theme,
            ink_by_lane: HashMap::new(),
            glass_by_layer: HashMap::new(),
            glyphs_by_lane: HashMap::new(),
            text_views: Vec::new(),
            hits: HitMap::new(),
            registry: Registry::new(),
            element_states: HashMap::new(),
            focused_id: None,
            pointer_pos: None,
            pointer_pressed: false,
            last_click_consumed: false,
            active_click_id: None,
            anon_counter: 0,
        }
    }

    // ------------------------------------------------------------------
    // Input
    // ------------------------------------------------------------------

    /// Set the current pointer position in UI coordinates.
    pub fn set_pointer(&mut self, pos: Option<Point>) {
        self.pointer_pos = pos;
    }

    /// Set whether the primary pointer button is currently pressed.
    pub fn set_pointer_pressed(&mut self, pressed: bool) {
        self.pointer_pressed = pressed;
    }

    /// Set the currently focused element id.
    pub fn set_focused(&mut self, id: Option<UiId>) {
        self.focused_id = id;
    }

    /// Current pointer position, if any.
    pub fn pointer_pos(&self) -> Option<Point> {
        self.pointer_pos
    }

    /// Is the primary pointer button pressed?
    pub fn pointer_pressed(&self) -> bool {
        self.pointer_pressed
    }

    /// Currently focused element id.
    pub fn focused_id(&self) -> Option<&UiId> {
        self.focused_id.as_ref()
    }

    /// Set which widget id, if any, should be treated as "clicked" this frame.
    /// The app performs external hit-testing on pointer release and passes the
    /// winning id here. Widgets whose id matches will report `clicked = true`.
    pub fn set_active_click(&mut self, id: Option<UiId>) {
        self.active_click_id = id;
    }

    /// Returns `true` when `id` matches the active-click id for this frame.
    pub fn is_active_click(&self, id: &UiId) -> bool {
        self.active_click_id.as_ref() == Some(id)
    }

    // ------------------------------------------------------------------
    // Theme
    // ------------------------------------------------------------------

    /// Immutable reference to the current theme.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Mutable reference to the theme.
    pub fn theme_mut(&mut self) -> &mut Theme {
        &mut self.theme
    }

    /// DPI scale factor from the theme.
    pub fn scale_factor(&self) -> f32 {
        self.theme.scale_factor
    }

    // ------------------------------------------------------------------
    // Output extraction
    // ------------------------------------------------------------------

    /// Consume `self` and return the built [`RenderModel`], [`HitMap`], and
    /// [`Registry`].
    pub fn take(self) -> (RenderModel, HitMap, Registry) {
        let mut render = RenderModel::new();

        // Flush per-lane ink buffers.
        for (lane, views) in self.ink_by_lane {
            render.set_ink_batch(lane, views);
        }
        // Flush per-layer glass buffers.
        for (layer, surfaces) in self.glass_by_layer {
            render.set_glass_batch(layer, surfaces);
        }
        // Flush per-lane glyph buffers.
        for (lane, views) in self.glyphs_by_lane {
            render.set_glyph_batch(lane, views);
        }
        // Text views accumulate directly.
        render.text = self.text_views;

        (render, self.hits, self.registry)
    }

    /// Consume `self` and return a [`LayoutResult`] (render model + hit map
    /// only; the registry is discarded).
    pub fn into_layout_result(self) -> LayoutResult {
        let (render, hits, _registry) = self.take();
        LayoutResult::new(render, hits)
    }

    // ------------------------------------------------------------------
    // Draw-data push helpers
    // ------------------------------------------------------------------

    /// Push an `InkView` onto the default ink lane ([`InkLane::Settings`]).
    ///
    /// If the view's `clip` is `None` and the clip stack is non-empty, the
    /// topmost clip region is automatically applied.
    pub fn push_ink(&mut self, view: InkView) {
        self.push_ink_with_lane(InkLane::Settings, view);
    }

    /// Push an `InkView` onto a specific ink lane, with automatic clip
    /// propagation from the clip stack.
    pub fn push_ink_with_lane(&mut self, lane: InkLane, mut view: InkView) {
        self.apply_clip(&mut view.clip);
        self.ink_by_lane.entry(lane).or_default().push(view);
    }

    /// Push `GlyphView`s onto a glyph lane, with automatic clip propagation.
    pub fn push_glyphs(&mut self, lane: GlyphLane, mut views: Vec<GlyphView>) {
        for view in &mut views {
            self.apply_clip(&mut view.clip);
        }
        self.glyphs_by_lane.entry(lane).or_default().extend(views);
    }

    /// Push a `TextView`, with automatic clip propagation.
    pub fn push_text(&mut self, mut view: TextView) {
        self.apply_clip(&mut view.clip);
        self.text_views.push(view);
    }

    /// Push a `GlassSurface` onto a glass layer, with automatic clip
    /// propagation.
    pub fn push_glass(&mut self, layer: GlassLayer, mut surface: GlassSurface) {
        self.apply_clip(&mut surface.clip);
        self.glass_by_layer.entry(layer).or_default().push(surface);
    }

    /// Push a `HitRegion` onto the hit-test map.
    pub fn push_hit(&mut self, region: HitRegion) {
        self.hits.push(region);
    }

    // ------------------------------------------------------------------
    // Registry
    // ------------------------------------------------------------------

    /// Register a layout rectangle + hit rectangle for `id`.
    pub fn register(&mut self, id: UiId, rect: Rect, hit_rect: Rect) {
        self.registry.register(id, rect, hit_rect);
    }

    /// Look up the visual rectangle for `id`.
    pub fn rect(&self, id: &UiId) -> Option<Rect> {
        self.registry.rect(id)
    }

    /// Look up the hit-test rectangle for `id`.
    pub fn hit_rect(&self, id: &UiId) -> Option<Rect> {
        self.registry.hit_rect(id)
    }

    // ------------------------------------------------------------------
    // Element state
    // ------------------------------------------------------------------

    /// Read the current transient state for `id` (default if never seen).
    pub fn element_state(&self, id: &UiId) -> ElementState {
        self.element_states.get(id).copied().unwrap_or_default()
    }

    /// Mutable access to transient state for `id` (inserts default if
    /// missing).
    pub fn element_state_mut(&mut self, id: &UiId) -> &mut ElementState {
        self.element_states.entry(id.clone()).or_default()
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    /// Insert inter-element spacing (unless this is the first widget in the
    /// current container) and mark the container as no-longer-first.
    pub(crate) fn begin_widget(&mut self) {
        if !self.first_in_container {
            match self.direction {
                LayoutDirection::Vertical => self.cursor_y += self.spacing,
                LayoutDirection::Horizontal => self.cursor_x += self.spacing,
            }
        }
        self.first_in_container = false;
    }

    /// If `target` is `None` and the clip stack is non-empty, copy the
    /// topmost clip region into `target`.
    fn apply_clip(&self, target: &mut Option<ClipRegion>) {
        if target.is_none() {
            if let Some(top) = self.clip_stack.last() {
                *target = Some(*top);
            }
        }
    }

    /// Generate the next anonymous `UiId`.
    pub(crate) fn next_anon_id(&mut self) -> UiId {
        let id = UiId::named(format!("_ui_{}", self.anon_counter));
        self.anon_counter += 1;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_model::hit::HitTarget;

    fn new_ui() -> Ui {
        Ui::new(Theme::default(), 800.0, 600.0)
    }

    // ------------------------------------------------------------------
    // Clip propagation
    // ------------------------------------------------------------------

    #[test]
    fn push_ink_inherits_clip_from_stack() {
        let mut ui = new_ui();
        let clip = ClipRegion::new(Rect::new(10.0, 10.0, 100.0, 100.0), 8.0);
        ui.clip_stack.push(clip);

        let view = InkView {
            id: UiId::named("test"),
            center: Point::new(50.0, 50.0),
            extent: 20.0,
            opacity: 1.0,
            scene_blur: 0.0,
            stroke: 0.0,
            corner_radius: 0.0,
            color: Color::rgba(1.0, 1.0, 1.0, 1.0),
            kind: ControlKind::Dot,
            z: 0,
            clip: None,
        };
        ui.push_ink(view);

        let (_render, _hits, _reg) = ui.take();
        // After take, the InkView should have clip set.
        // We verify by checking the render model output.
        assert!(!_render.ink.is_empty());
        let pushed = &_render.ink[0].views[0];
        assert_eq!(pushed.clip, Some(clip));
    }

    #[test]
    fn push_ink_keeps_explicit_clip() {
        let mut ui = new_ui();
        let stack_clip = ClipRegion::new(Rect::new(0.0, 0.0, 50.0, 50.0), 4.0);
        let explicit_clip = ClipRegion::new(Rect::new(10.0, 10.0, 30.0, 30.0), 2.0);
        ui.clip_stack.push(stack_clip);

        let view = InkView {
            id: UiId::named("test"),
            center: Point::new(25.0, 25.0),
            extent: 10.0,
            opacity: 1.0,
            scene_blur: 0.0,
            stroke: 0.0,
            corner_radius: 0.0,
            color: Color::rgba(1.0, 1.0, 1.0, 1.0),
            kind: ControlKind::Dot,
            z: 0,
            clip: Some(explicit_clip),
        };
        ui.push_ink(view);

        let (_render, _hits, _reg) = ui.take();
        let pushed = &_render.ink[0].views[0];
        // Explicit clip is preserved, not overwritten by stack.
        assert_eq!(pushed.clip, Some(explicit_clip));
    }

    // ------------------------------------------------------------------
    // Hit pushing
    // ------------------------------------------------------------------

    #[test]
    fn push_hit_adds_to_hit_map() {
        let mut ui = new_ui();
        let region = HitRegion::new(
            UiId::named("btn"),
            Rect::new(10.0, 20.0, 100.0, 40.0),
            HitTarget::launcher_item("btn"),
            0,
        );
        ui.push_hit(region);

        let (_render, hits, _reg) = ui.take();
        assert_eq!(hits.len(), 1);
    }

    // ------------------------------------------------------------------
    // Element state
    // ------------------------------------------------------------------

    #[test]
    fn element_state_returns_default_for_unknown_id() {
        let ui = new_ui();
        let state = ui.element_state(&UiId::named("no-such"));
        assert_eq!(state, ElementState::default());
    }

    #[test]
    fn element_state_mut_inserts_and_returns() {
        let mut ui = new_ui();
        let id = UiId::named("widget");
        {
            let s = ui.element_state_mut(&id);
            s.hovered = true;
            s.phase = super::super::interaction::InteractionPhase::Hovered;
        }
        let s = ui.element_state(&id);
        assert!(s.hovered);
        assert_eq!(
            s.phase,
            super::super::interaction::InteractionPhase::Hovered
        );
    }

    // ------------------------------------------------------------------
    // Layout determinism
    // ------------------------------------------------------------------

    #[test]
    fn same_inputs_produce_same_registry() {
        let run = || {
            let mut ui = new_ui();
            let id = UiId::named("a");
            ui.column(5.0, |ui| {
                ui.spacer(10.0);
                let r = Rect::new(0.0, 0.0, 100.0, 30.0);
                ui.register(id.clone(), r, r);
            });
            ui.take()
        };

        let (_, _, reg1) = run();
        let (_, _, reg2) = run();
        assert_eq!(reg1.rect(&UiId::named("a")), reg2.rect(&UiId::named("a")));
    }
}

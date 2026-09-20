//! Bottom control / gear / IME / caret render adapter methods.

use crate::features::bottom_control;
use crate::renderer::text_engine as text;
use crate::ui_model::geometry::{Point, Rect, UvRect};
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{
    Color, ControlKind, GlassBehavior, GlassLayer, GlassMaterial, GlassSurface, GlyphLane,
    GlyphView, InkLane, InkView,
};

use super::helpers::{advance_unit_toward, mul_alpha};
use crate::app::state::App;

impl App {
    pub(crate) fn render_bottom_control(&mut self) -> Option<GlassSurface> {
        // Gather all the immutable data first (avoid overlapping borrows with
        // the mutable renderer/text borrows below).
        let scale = self.scale_factor;
        // Measure the query width exactly via cosmic-text shaping (same pass
        // as drawing), so the caret and IME anchor line up with the glyphs
        // regardless of ASCII/CJK widths. Cached for the frame so
        // `measure_query_width` can read it back under `&self`.
        // The widths are retained in `control_text_cache` and re-measured only
        // when the text or DPI scale changes; the shaped quads themselves are
        // reused verbatim while the anchors are unchanged.
        if let Some(m) = self.text.as_mut() {
            let mut measure = |s: &str| -> f32 {
                if s.is_empty() {
                    return 0.0;
                }
                let spec = text::CenteredLineSpec {
                    text: s,
                    font_size: QUERY_LABEL_SIZE,
                    line_height: QUERY_LABEL_LINE,
                    family: QUERY_LABEL_FONT,
                    color: [1.0, 1.0, 1.0, 1.0],
                    center: (0.0, 0.0),
                    scale_factor: scale,
                };
                m.measure_text(&spec)
            };
            let cache = self
                .control_text_cache
                .get_or_insert_with(ControlTextCache::new);
            if cache.texts_changed(scale, &self.control) {
                // The caret sits after *all visible text*: the committed query
                // plus the in-flight IME preedit. Without the preedit width,
                // the caret stays put while the user types Japanese.
                let query_w = measure(&self.control.query);
                let preedit_w = measure(&self.control.preedit);
                let done_w = measure(DONE_LABEL);
                cache.set_widths(query_w, preedit_w, done_w);
                cache.note_texts_measured(scale, &self.control);
            }
            self.cached_query_width = Some(cache.query_width + cache.preedit_width);
            self.cached_done_width = Some(cache.done_width);
        }
        let (geom, layers) = self.resolve_control()?;
        let query_width = self.measure_query_width();
        let caret_blink = caret_visibility(&self.control);
        let edit_visual_progress = self.edit_visual_progress();

        // 1) Procedural overlay instances (magnifier, dots, caret, close).
        // While the Done-width morph is active, keep normal pill contents hidden
        // so they don't overflow the narrower capsule.
        let instances = if edit_visual_progress > 0.0 {
            Vec::new()
        } else {
            build_overlay_instances(&geom, &layers, query_width, caret_blink)
        };

        // 2) Text glyphs (label / query / placeholder). Built via the shared
        // text renderer so they share the glyph atlas. The shaping is retained
        // in `control_text_cache`; per frame only the alpha recolors the small
        // quad set unless the text/anchors actually changed. Done before
        // touching the renderer so the atlas upload + dirty clear happen in
        // one place.
        let (quads, atlas_dirty) = if let Some(t) = self.text.as_mut() {
            let cache = self
                .control_text_cache
                .get_or_insert_with(ControlTextCache::new);
            let q = control_text_quads(
                cache,
                t,
                &geom,
                &layers,
                scale,
                &self.control,
                edit_visual_progress,
            );
            (q, t.atlas_dirty)
        } else {
            (Vec::new(), false)
        };
        if atlas_dirty {
            if let Some(t) = self.text.as_mut() {
                t.atlas_dirty = false;
            }
        }

        // 3) Upload the control ink/text and return its glass shape to the
        // caller. `tick_frame` immediately passes it to `render_gear`, keeping
        // the transient GPU-facing value out of persistent app state.
        let control_shape = control_glass_shape(&geom);
        self.upload_control_overlay(atlas_dirty, &instances, &quads);
        control_shape
    }

    /// Upload the control ink/text. Glass submission waits until
    /// [`render_gear`] has resolved both members of the overlay lane.
    fn upload_control_overlay(
        &mut self,
        atlas_dirty: bool,
        instances: &[InkView],
        quads: &[text::GlyphQuad],
    ) {
        if atlas_dirty {
            if let (Some(r), Some(t)) = (self.renderer.as_mut(), self.text.as_ref()) {
                let (aw, ah) = t.atlas_dimensions();
                r.upload_atlas(t.atlas_rgba(), aw, ah);
            }
        }
        self.render_model
            .set_ink_batch(InkLane::BottomControl, instances.to_vec());
        self.render_model.set_glyph_batch(
            GlyphLane::BottomControl,
            glyph_views(UiId::bottom_control(), quads),
        );
    }

    pub(crate) fn render_gear(&mut self, control_shape: Option<GlassSurface>) {
        // The gear only appears in edit mode, alongside the Done capsule.
        let edit_progress = self.edit_visual_progress();
        let show = self.visible && edit_progress > 0.0 && !self.settings_panel_active();
        // Resolve the gear geometry once (it yields both the glass shape and
        // the ink instance).
        let gear_geom = if show {
            let viewport = self.viewport_phys();
            let frame_bottom = self.frame_bottom_y();
            let scale = self.scale_factor;
            let done_hw = self
                .cached_done_width
                .map(|w| bottom_control::done_half_width(w, scale))
                .unwrap_or_else(|| bottom_control::done_half_width(0.0, scale));
            bottom_control::edit_gear_geometry(
                viewport,
                frame_bottom,
                scale,
                done_hw,
                edit_progress,
            )
        } else {
            None
        };
        let gear_shape = gear_geom.map(|(geom, _)| edit_gear_glass_shape(&geom));
        let gear_instance = gear_geom.map(|(geom, alpha)| edit_gear_instance(&geom, alpha));
        self.render_model.set_glass_batch(
            GlassLayer::Overlay,
            self.interaction_glass
                .iter()
                .cloned()
                .chain(control_shape)
                .chain(gear_shape)
                .collect(),
        );
        self.render_model
            .set_ink_batch(InkLane::Gear, gear_instance.into_iter().collect());
    }

    pub(crate) fn step_edit_control_width(&mut self, dt: f32) -> bool {
        let target = if self.editing { 1.0 } else { 0.0 };
        let duration = if self.editing {
            bottom_control::EXPAND_DURATION
        } else {
            bottom_control::COLLAPSE_DURATION
        };
        let before = self.edit_control_progress;
        self.edit_control_progress =
            advance_unit_toward(self.edit_control_progress, target, dt, duration);
        (self.edit_control_progress - before).abs() > 0.0001
            || (self.edit_control_progress - target).abs() > 0.0001
    }

    pub(crate) fn frame_control_cy(&self) -> f32 {
        self.resolve_control()
            .map(|(geom, _)| geom.center.1)
            .unwrap_or(0.0)
    }

    /// Keep the OS IME in sync with the search field: enable it (and point the
    /// composition window at the caret) while the field is focused, disable it
    /// otherwise. Called every frame; `set_ime_allowed` is cheap.
    pub(crate) fn update_ime_state(&self) {
        let Some(r) = self.renderer.as_ref() else {
            return;
        };
        let want_ime = self.folders.rename.is_some() || self.control.ime_attached();
        r.window.set_ime_allowed(want_ime);
        if want_ime {
            // Park the IME composition window at the caret so Japanese/IME
            // candidates appear right next to the typed text.
            let scale = self.scale_factor;
            let (caret_x, caret_y) = if self.folders.rename.is_some() {
                self.folder_layout
                    .as_ref()
                    .map(|layout| (layout.title_rect.center().x, layout.title_rect.max_y()))
                    .unwrap_or((0.0, 0.0))
            } else {
                (self.control_caret_screen_x(), self.frame_control_cy())
            };
            r.window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(caret_x as f64, caret_y as f64),
                winit::dpi::PhysicalSize::new(1.0, (16.0 * scale) as f64),
            );
        }
    }

    /// Screen-space X of the text caret inside the search field (physical px),
    /// used to anchor the IME composition window.
    pub(crate) fn control_caret_screen_x(&self) -> f32 {
        let Some((geom, _)) = self.resolve_control() else {
            return 0.0;
        };
        let origin = bottom_control::field_text_origin_x(&geom);
        origin + self.measure_query_width()
    }
}

const CONTROL_INK: [f32; 4] = [1.0, 1.0, 1.0, 0.92];
const DOT_ACTIVE: [f32; 4] = [1.0, 1.0, 1.0, 0.96];
const DOT_IDLE: [f32; 4] = [1.0, 1.0, 1.0, 0.40];

fn build_overlay_instances(
    geom: &bottom_control::ControlGeometry,
    layers: &[bottom_control::ControlLayer],
    query_width: f32,
    caret_blink: f32,
) -> Vec<InkView> {
    let mut out = Vec::new();
    let (cx, cy) = geom.center;
    let hw = geom.half_size.0;
    let scale = crate::layout::control_geometry::control_scale(geom);

    for layer in layers {
        let alpha = layer.alpha;
        if alpha <= 0.01 {
            continue;
        }
        match layer.visual {
            bottom_control::Visual::SearchPill => {
                let (mag_cx, _) = bottom_control::search_pill_content_centers(geom);
                let size = crate::layout::control_geometry::search_magnifier_size(scale);
                out.push(ink_view(
                    UiId::bottom_control(),
                    [mag_cx, cy],
                    size,
                    alpha,
                    0.0,
                    0.0,
                    CONTROL_INK,
                    ControlKind::Magnifier,
                ));
            }
            bottom_control::Visual::PageIndicator => {
                let dots = geom.page_count.max(1);
                let active_r = 3.2 * scale;
                let idle_r = 2.4 * scale;
                let gap = 8.0 * scale;
                let total = dots as f32 * active_r * 2.0 + (dots.saturating_sub(1)) as f32 * gap;
                let start_x = cx - total * 0.5 + active_r;
                for index in 0..dots {
                    let active = index == geom.page;
                    out.push(ink_view(
                        UiId::bottom_control(),
                        [start_x + index as f32 * (active_r * 2.0 + gap), cy],
                        if active { active_r } else { idle_r },
                        alpha,
                        0.0,
                        0.0,
                        if active { DOT_ACTIVE } else { DOT_IDLE },
                        ControlKind::Dot,
                    ));
                }
            }
            bottom_control::Visual::SearchField => {
                let size = 11.0 * scale;
                let mag_cx = cx - hw + size + 10.0 * scale;
                out.push(ink_view(
                    UiId::bottom_control(),
                    [mag_cx, cy],
                    size,
                    alpha,
                    0.0,
                    0.0,
                    CONTROL_INK,
                    ControlKind::Magnifier,
                ));
                if caret_blink > 0.01 {
                    out.push(ink_view(
                        UiId::bottom_control(),
                        [mag_cx + size + 6.0 * scale + query_width, cy],
                        8.0 * scale,
                        alpha * caret_blink,
                        scale,
                        0.0,
                        CONTROL_INK,
                        ControlKind::Caret,
                    ));
                }
                out.push(ink_view(
                    UiId::bottom_control_close(),
                    [cx + hw - 20.0 * scale, cy],
                    7.0 * scale,
                    alpha,
                    1.4 * scale,
                    0.0,
                    CONTROL_INK,
                    ControlKind::CloseButton,
                ));
            }
        }
    }
    out
}

fn edit_gear_instance(
    geom: &crate::layout::control_geometry::EditGearGeometry,
    alpha: f32,
) -> InkView {
    ink_view(
        UiId::edit_settings_gear(),
        [geom.center.0, geom.center.1],
        geom.radius * 0.62,
        alpha,
        0.0,
        0.0,
        [1.0, 1.0, 1.0, 1.0],
        ControlKind::SettingsGear,
    )
}

#[allow(clippy::too_many_arguments)]
fn ink_view(
    id: UiId,
    center: [f32; 2],
    extent: f32,
    opacity: f32,
    stroke: f32,
    corner_radius: f32,
    color: [f32; 4],
    kind: ControlKind,
) -> InkView {
    InkView {
        id,
        center: Point::new(center[0], center[1]),
        extent,
        opacity,
        scene_blur: 0.0,
        stroke,
        corner_radius,
        color: Color::rgba(color[0], color[1], color[2], color[3]),
        kind,
        z: 0,
        clip: None,
    }
}

fn glyph_views(id: UiId, quads: &[text::GlyphQuad]) -> Vec<GlyphView> {
    quads
        .iter()
        .map(|quad| GlyphView {
            presentation_pivot: None,
            id: id.clone(),
            rect: Rect::new(quad.x, quad.y, quad.w, quad.h),
            uv: UvRect {
                u0: quad.u0,
                v0: quad.v0,
                u1: quad.u1,
                v1: quad.v1,
            },
            color: Color::rgba(quad.color[0], quad.color[1], quad.color[2], quad.color[3]),
            z: 0,
            clip: None,
        })
        .collect()
}

fn control_glass_shape(geom: &bottom_control::ControlGeometry) -> Option<GlassSurface> {
    (geom.half_size.0 >= 1.0).then(|| GlassSurface {
        id: UiId::bottom_control(),
        rect: Rect::new(
            geom.center.0 - geom.half_size.0,
            geom.center.1 - geom.half_size.1,
            geom.half_size.0 * 2.0,
            geom.half_size.1 * 2.0,
        ),
        radius: geom.radius,
        material: GlassMaterial::Regular,
        behavior: GlassBehavior::Control,
        z: 0,
        clip: None,
        activation: 0.0,
        blur_radius: None,
        backdrop_replacement: 0.0,
        tint: None,
    })
}

fn edit_gear_glass_shape(geom: &crate::layout::control_geometry::EditGearGeometry) -> GlassSurface {
    GlassSurface {
        id: UiId::edit_settings_gear(),
        rect: Rect::new(
            geom.center.0 - geom.glass_radius,
            geom.center.1 - geom.glass_radius,
            geom.glass_radius * 2.0,
            geom.glass_radius * 2.0,
        ),
        radius: geom.glass_radius,
        material: GlassMaterial::Regular,
        behavior: GlassBehavior::Control,
        z: 10,
        clip: None,
        activation: 0.0,
        blur_radius: None,
        backdrop_replacement: 0.0,
        tint: None,
    }
}

const QUERY_LABEL_FONT: &str = text::UI_FONT_FAMILY;
const QUERY_LABEL_SIZE: f32 = 13.0;
const QUERY_LABEL_LINE: f32 = 18.0;
const DONE_LABEL: &str = "完了";

// ---- settings overlay (placeholder panel) ----------------------------------

/// Retained shaping for the bottom-control text lines.
///
/// The bottom control is rendered every frame (the caret blinks and the
/// edit morph animates), but the text content and its on-screen anchors only
/// change on input, DPI scale, or window-resize. Re-shaping the same short
/// strings with cosmic-text on every frame is the dominant CPU cost of the
/// control; this cache stores the shaped quads keyed by every value they
/// depend on, so steady-state frames only recolor the small quad set.
#[derive(Debug)]
pub(crate) struct ControlTextCache {
    key: ControlTextCacheKey,
    done_width: f32,
    query_width: f32,
    preedit_width: f32,
    done: Vec<text::GlyphQuad>,
    pill: Vec<text::GlyphQuad>,
    placeholder: Vec<text::GlyphQuad>,
    query: Vec<text::GlyphQuad>,
    preedit: Vec<text::GlyphQuad>,
}

#[derive(Debug, Clone, PartialEq)]
struct ControlTextCacheKey {
    scale_bits: u32,
    query: String,
    preedit: String,
    /// `edit_visual_progress > 0` → only the Done label is drawn.
    edit_morph: bool,
    done_center: (f32, f32),
    pill_center: (f32, f32),
    placeholder_center: (f32, f32),
    query_center: (f32, f32),
    preedit_center: (f32, f32),
}

impl ControlTextCache {
    fn new() -> Self {
        Self {
            key: ControlTextCacheKey {
                scale_bits: 0,
                query: String::new(),
                preedit: String::new(),
                edit_morph: false,
                done_center: (0.0, 0.0),
                pill_center: (0.0, 0.0),
                placeholder_center: (0.0, 0.0),
                query_center: (0.0, 0.0),
                preedit_center: (0.0, 0.0),
            },
            done_width: 0.0,
            query_width: 0.0,
            preedit_width: 0.0,
            done: Vec::new(),
            pill: Vec::new(),
            placeholder: Vec::new(),
            query: Vec::new(),
            preedit: Vec::new(),
        }
    }

    /// Whether the measured widths are stale (text content or DPI scale
    /// changed since the last shaping pass).
    fn texts_changed(&self, scale: f32, control: &bottom_control::BottomControl) -> bool {
        self.key.scale_bits != scale.to_bits()
            || self.key.query != control.query
            || self.key.preedit != control.preedit
    }
}

/// Shape or reuse the control's text pieces for one frame, returning the
/// glyph quads in draw order. On a cache hit only the quad colors are
/// refreshed (the layer/opacity animations are per-frame); the shaping is
/// re-run only when the text, scale, or on-screen anchors changed.
#[allow(clippy::too_many_arguments)]
fn control_text_quads(
    cache: &mut ControlTextCache,
    t: &mut text::TextRenderer,
    geom: &bottom_control::ControlGeometry,
    layers: &[bottom_control::ControlLayer],
    scale: f32,
    control: &bottom_control::BottomControl,
    edit_visual_progress: f32,
) -> Vec<text::GlyphQuad> {
    const INK: [f32; 4] = [1.0, 1.0, 1.0, 0.92];
    const PLACEHOLDER: [f32; 4] = [1.0, 1.0, 1.0, 0.45];
    /// Preedit (in-flight IME composition) is shown slightly dimmer to hint
    /// it isn't committed yet.
    const PREEDIT_INK: [f32; 4] = [0.85, 0.92, 1.0, 0.88];

    let edit_morph = edit_visual_progress > 0.0;
    let cy = geom.center.1;
    let origin_x = bottom_control::field_text_origin_x(geom);
    let (_, pill_label_x) = bottom_control::search_pill_content_centers(geom);
    let query_w = cache.query_width;
    let preedit_w = cache.preedit_width;
    let done_center = geom.center;
    let pill_center = (pill_label_x, cy);
    let placeholder_center = (origin_x + 14.0 * scale, cy);
    let query_center = (origin_x + query_w * 0.5, cy);
    let preedit_center = (origin_x + query_w + preedit_w * 0.5, cy);

    let hit = cache.key.scale_bits == scale.to_bits()
        && cache.key.query == control.query
        && cache.key.preedit == control.preedit
        && cache.key.edit_morph == edit_morph
        && cache.key.done_center == done_center
        && cache.key.pill_center == pill_center
        && cache.key.placeholder_center == placeholder_center
        && cache.key.query_center == query_center
        && cache.key.preedit_center == preedit_center;

    let mut quads = Vec::new();
    if edit_morph {
        // While edit-mode width is morphing, keep the Done label centered and
        // skip normal pill/indicator/field content so it cannot overflow the
        // narrower capsule.
        let color = mul_alpha(INK, edit_visual_progress.clamp(0.0, 1.0));
        if hit {
            recolor_quads(&mut cache.done, color);
            quads.extend_from_slice(&cache.done);
        } else {
            let mut q = t.layout_centered_line(&text::CenteredLineSpec {
                text: DONE_LABEL,
                font_size: QUERY_LABEL_SIZE,
                line_height: QUERY_LABEL_LINE,
                family: QUERY_LABEL_FONT,
                color,
                center: done_center,
                scale_factor: scale,
            });
            cache.done = q.clone();
            quads.append(&mut q);
        }
        cache.key = ControlTextCacheKey {
            scale_bits: scale.to_bits(),
            query: control.query.clone(),
            preedit: control.preedit.clone(),
            edit_morph,
            done_center,
            pill_center,
            placeholder_center,
            query_center,
            preedit_center,
        };
        return quads;
    }

    for layer in layers {
        let a = layer.alpha;
        if a <= 0.01 {
            continue;
        }
        match layer.visual {
            bottom_control::Visual::SearchPill => {
                // "検索" label to the right of the magnifier.
                let color = mul_alpha(INK, a);
                if hit {
                    recolor_quads(&mut cache.pill, color);
                    quads.extend_from_slice(&cache.pill);
                } else {
                    let mut q = t.layout_centered_line(&text::CenteredLineSpec {
                        text: "検索",
                        font_size: QUERY_LABEL_SIZE,
                        line_height: QUERY_LABEL_LINE,
                        family: QUERY_LABEL_FONT,
                        color,
                        center: pill_center,
                        scale_factor: scale,
                    });
                    cache.pill = q.clone();
                    quads.append(&mut q);
                }
            }
            bottom_control::Visual::PageIndicator => {
                // No text.
            }
            bottom_control::Visual::SearchField => {
                if control.query.is_empty() && control.preedit.is_empty() {
                    let color = mul_alpha(PLACEHOLDER, a);
                    if hit {
                        recolor_quads(&mut cache.placeholder, color);
                        quads.extend_from_slice(&cache.placeholder);
                    } else {
                        let mut q = t.layout_centered_line(&text::CenteredLineSpec {
                            text: "検索",
                            font_size: QUERY_LABEL_SIZE,
                            line_height: QUERY_LABEL_LINE,
                            family: QUERY_LABEL_FONT,
                            color,
                            center: placeholder_center,
                            scale_factor: scale,
                        });
                        cache.placeholder = q.clone();
                        quads.append(&mut q);
                    }
                } else {
                    // Committed text: left-anchored at origin_x, so center on
                    // its own half-width. The preedit starts right after the
                    // committed query.
                    if query_w > 0.0 {
                        let color = mul_alpha(INK, a);
                        if hit {
                            recolor_quads(&mut cache.query, color);
                            quads.extend_from_slice(&cache.query);
                        } else {
                            let mut q = t.layout_centered_line(&text::CenteredLineSpec {
                                text: &control.query,
                                font_size: QUERY_LABEL_SIZE,
                                line_height: QUERY_LABEL_LINE,
                                family: QUERY_LABEL_FONT,
                                color,
                                center: query_center,
                                scale_factor: scale,
                            });
                            cache.query = q.clone();
                            quads.append(&mut q);
                        }
                    }
                    if preedit_w > 0.0 {
                        let color = mul_alpha(PREEDIT_INK, a);
                        if hit {
                            recolor_quads(&mut cache.preedit, color);
                            quads.extend_from_slice(&cache.preedit);
                        } else {
                            let mut q = t.layout_centered_line(&text::CenteredLineSpec {
                                text: &control.preedit,
                                font_size: QUERY_LABEL_SIZE,
                                line_height: QUERY_LABEL_LINE,
                                family: QUERY_LABEL_FONT,
                                color,
                                center: preedit_center,
                                scale_factor: scale,
                            });
                            cache.preedit = q.clone();
                            quads.append(&mut q);
                        }
                    }
                }
            }
        }
    }

    if !hit {
        cache.key = ControlTextCacheKey {
            scale_bits: scale.to_bits(),
            query: control.query.clone(),
            preedit: control.preedit.clone(),
            edit_morph,
            done_center,
            pill_center,
            placeholder_center,
            query_center,
            preedit_center,
        };
    }
    quads
}

/// Overwrite the alpha channel of every quad (the RGB channels are already
/// the piece's base color from shaping time).
fn recolor_quads(quads: &mut [text::GlyphQuad], color: [f32; 4]) {
    for quad in quads {
        quad.color[0] = color[0];
        quad.color[1] = color[1];
        quad.color[2] = color[2];
        quad.color[3] = color[3];
    }
}

impl ControlTextCache {
    /// Store freshly measured line widths (physical px). Called only when the
    /// text or scale changed; the quads are re-shaped on the same transition.
    fn set_widths(&mut self, query_width: f32, preedit_width: f32, done_width: f32) {
        self.query_width = query_width;
        self.preedit_width = preedit_width;
        self.done_width = done_width;
    }

    /// Record the text inputs the widths were measured for so
    /// [`texts_changed`](Self::texts_changed) reports them current.
    fn note_texts_measured(&mut self, scale: f32, control: &bottom_control::BottomControl) {
        self.key.scale_bits = scale.to_bits();
        self.key.query = control.query.clone();
        self.key.preedit = control.preedit.clone();
    }
}

fn caret_visibility(control: &bottom_control::BottomControl) -> f32 {
    // Only blink when the field is the focus.
    if !matches!(control.mode, bottom_control::Mode::Field) {
        return 1.0;
    }
    crate::layout::control_geometry::caret_blink_opacity(control.caret_phase)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_geom(center: (f32, f32)) -> bottom_control::ControlGeometry {
        bottom_control::ControlGeometry {
            center,
            half_size: (160.0, 24.0),
            radius: 24.0,
            expand: 1.0,
            indicator: 0.0,
            page: 0,
            page_count: 1,
        }
    }

    fn quads_eq(a: &[text::GlyphQuad], b: &[text::GlyphQuad]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b.iter()).all(|(x, y)| {
                x.x == y.x
                    && x.y == y.y
                    && x.w == y.w
                    && x.h == y.h
                    && x.u0 == y.u0
                    && x.v0 == y.v0
                    && x.u1 == y.u1
                    && x.v1 == y.v1
                    && x.color == y.color
                    && x.clip_rect == y.clip_rect
                    && x.clip_radius == y.clip_radius
            })
    }

    fn field_layers() -> Vec<bottom_control::ControlLayer> {
        vec![bottom_control::ControlLayer {
            visual: bottom_control::Visual::SearchField,
            alpha: 1.0,
        }]
    }

    #[test]
    fn control_text_cache_reuses_quads_while_inputs_are_unchanged() {
        let mut t = text::TextRenderer::new();
        let mut cache = ControlTextCache::new();
        let g = test_geom((640.0, 700.0));
        let layers = field_layers();
        let mut control = bottom_control::BottomControl::new();
        control.query = "テスト".to_string();

        // Mirror the real flow: widths are measured and retained before the
        // quads are shaped.
        let measure = |t: &mut text::TextRenderer, s: &str| -> f32 {
            t.measure_text(&text::CenteredLineSpec {
                text: s,
                font_size: QUERY_LABEL_SIZE,
                line_height: QUERY_LABEL_LINE,
                family: QUERY_LABEL_FONT,
                color: [1.0; 4],
                center: (0.0, 0.0),
                scale_factor: 1.0,
            })
        };
        cache.set_widths(measure(&mut t, "テスト"), 0.0, measure(&mut t, DONE_LABEL));
        cache.note_texts_measured(1.0, &control);

        let first = control_text_quads(&mut cache, &mut t, &g, &layers, 1.0, &control, 0.0);
        assert!(!first.is_empty(), "a non-empty query must shape glyphs");

        // Identical inputs: the retained quads are reused verbatim (no
        // re-shaping, no atlas growth on the hit).
        let atlas_after_first = t.atlas_dimensions();
        let second = control_text_quads(&mut cache, &mut t, &g, &layers, 1.0, &control, 0.0);
        assert!(
            quads_eq(&first, &second),
            "same inputs must reuse the same quads"
        );
        assert_eq!(
            t.atlas_dimensions(),
            atlas_after_first,
            "hit must not rasterize"
        );

        // Changing the query re-shapes (different text, different quads).
        control.query = "別のクエリ".to_string();
        let changed = control_text_quads(&mut cache, &mut t, &g, &layers, 1.0, &control, 0.0);
        assert!(!quads_eq(&first, &changed), "query change must re-shape");
        let unchanged = control_text_quads(&mut cache, &mut t, &g, &layers, 1.0, &control, 0.0);
        assert!(quads_eq(&changed, &unchanged));

        // Changing the DPI scale re-shapes (new physical font size).
        let scaled = control_text_quads(&mut cache, &mut t, &g, &layers, 1.25, &control, 0.0);
        assert!(!quads_eq(&changed, &scaled), "scale change must re-shape");

        // Moving the control anchor re-shapes (positions embed the anchor).
        let moved = control_text_quads(
            &mut cache,
            &mut t,
            &test_geom((700.0, 700.0)),
            &layers,
            1.25,
            &control,
            0.0,
        );
        assert!(!quads_eq(&scaled, &moved), "anchor change must re-shape");
    }

    #[test]
    fn control_text_cache_recolors_alpha_without_reshaping() {
        let mut t = text::TextRenderer::new();
        let mut cache = ControlTextCache::new();
        let g = test_geom((640.0, 700.0));
        let control = bottom_control::BottomControl::new();
        let layers = field_layers();

        let first = control_text_quads(&mut cache, &mut t, &g, &layers, 1.0, &control, 0.0);
        assert!(!first.is_empty(), "empty query draws the placeholder");

        // A lower layer alpha (cross-fade) recolors the cached quads without
        // changing their positions or UVs.
        let faded = control_text_quads(
            &mut cache,
            &mut t,
            &g,
            &[bottom_control::ControlLayer {
                visual: bottom_control::Visual::SearchField,
                alpha: 0.5,
            }],
            1.0,
            &control,
            0.0,
        );
        assert_eq!(first.len(), faded.len());
        for (a, b) in first.iter().zip(faded.iter()) {
            assert_eq!(a.x, b.x);
            assert_eq!(a.y, b.y);
            assert_eq!(a.u0, b.u0);
            assert_eq!(a.color[0..3], b.color[0..3]);
            assert!((b.color[3] - a.color[3] * 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn control_text_cache_widths_follow_text_and_scale() {
        let mut t = text::TextRenderer::new();
        let mut cache = ControlTextCache::new();
        let mut control = bottom_control::BottomControl::new();
        control.query = "abc".to_string();

        assert!(cache.texts_changed(1.0, &control));
        let measure = |t: &mut text::TextRenderer, s: &str| -> f32 {
            t.measure_text(&text::CenteredLineSpec {
                text: s,
                font_size: QUERY_LABEL_SIZE,
                line_height: QUERY_LABEL_LINE,
                family: QUERY_LABEL_FONT,
                color: [1.0; 4],
                center: (0.0, 0.0),
                scale_factor: 1.0,
            })
        };
        let qw = measure(&mut t, "abc");
        let dw = measure(&mut t, DONE_LABEL);
        cache.set_widths(qw, 0.0, dw);
        cache.note_texts_measured(1.0, &control);
        assert!(
            !cache.texts_changed(1.0, &control),
            "texts unchanged after set"
        );

        control.query = "abcdef".to_string();
        assert!(
            cache.texts_changed(1.0, &control),
            "query change must invalidate"
        );
        assert!(
            cache.texts_changed(2.0, &control),
            "scale change must invalidate"
        );
    }
}

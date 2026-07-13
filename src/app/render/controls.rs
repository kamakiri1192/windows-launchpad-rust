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
        if let Some(m) = self.text.as_mut() {
            self.cached_query_width = {
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
                // The caret sits after *all visible text*: the committed query
                // plus the in-flight IME preedit. Without the preedit width,
                // the caret stays put while the user types Japanese.
                let w = measure(&self.control.query) + measure(&self.control.preedit);
                Some(w)
            };
            let spec = text::CenteredLineSpec {
                text: DONE_LABEL,
                font_size: QUERY_LABEL_SIZE,
                line_height: QUERY_LABEL_LINE,
                family: QUERY_LABEL_FONT,
                color: [1.0, 1.0, 1.0, 1.0],
                center: (0.0, 0.0),
                scale_factor: scale,
            };
            self.cached_done_width = Some(m.measure_text(&spec));
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
        // text renderer so they share the glyph atlas. Done before touching the
        // renderer so the atlas upload + dirty clear happen in one place.
        let (quads, atlas_dirty) = if let Some(t) = self.text.as_mut() {
            let q = self_layout_control_text(
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
                r.upload_atlas(t.atlas_rgba());
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
        let want_ime = self.folders.rename.is_some() || self.control.wants_keyboard();
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
    }
}

fn glyph_views(id: UiId, quads: &[text::GlyphQuad]) -> Vec<GlyphView> {
    quads
        .iter()
        .map(|quad| GlyphView {
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
        z: 1,
    }
}

const QUERY_LABEL_FONT: &str = "Yu Gothic UI";
const QUERY_LABEL_SIZE: f32 = 13.0;
const QUERY_LABEL_LINE: f32 = 18.0;
const DONE_LABEL: &str = "完了";

// ---- settings overlay (placeholder panel) ----------------------------------

fn self_layout_control_text(
    t: &mut text::TextRenderer,
    geom: &bottom_control::ControlGeometry,
    layers: &[bottom_control::ControlLayer],
    scale: f32,
    control: &bottom_control::BottomControl,
    edit_visual_progress: f32,
) -> Vec<text::GlyphQuad> {
    let mut quads = Vec::new();
    const INK: [f32; 4] = [1.0, 1.0, 1.0, 0.92];
    const PLACEHOLDER: [f32; 4] = [1.0, 1.0, 1.0, 0.45];
    /// Preedit (in-flight IME composition) is shown slightly dimmer to hint
    /// it isn't committed yet.
    const PREEDIT_INK: [f32; 4] = [0.85, 0.92, 1.0, 0.88];

    // While edit-mode width is morphing, keep the Done label centered and skip
    // normal pill/indicator/field content so it cannot overflow the capsule.
    if edit_visual_progress > 0.0 {
        let mut q = t.layout_centered_line(&text::CenteredLineSpec {
            text: DONE_LABEL,
            font_size: QUERY_LABEL_SIZE,
            line_height: QUERY_LABEL_LINE,
            family: QUERY_LABEL_FONT,
            color: mul_alpha(INK, edit_visual_progress.clamp(0.0, 1.0)),
            center: (geom.center.0, geom.center.1),
            scale_factor: scale,
        });
        quads.append(&mut q);
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
                let (_, label_center_x) = bottom_control::search_pill_content_centers(geom);
                let mut q = t.layout_centered_line(&text::CenteredLineSpec {
                    text: "検索",
                    font_size: QUERY_LABEL_SIZE,
                    line_height: QUERY_LABEL_LINE,
                    family: QUERY_LABEL_FONT,
                    color: mul_alpha(INK, a),
                    center: (label_center_x, geom.center.1),
                    scale_factor: scale,
                });
                quads.append(&mut q);
            }
            bottom_control::Visual::PageIndicator => {
                // No text.
            }
            bottom_control::Visual::SearchField => {
                let origin_x = bottom_control::field_text_origin_x(geom);
                if control.query.is_empty() && control.preedit.is_empty() {
                    let mut q = t.layout_centered_line(&text::CenteredLineSpec {
                        text: "検索",
                        font_size: QUERY_LABEL_SIZE,
                        line_height: QUERY_LABEL_LINE,
                        family: QUERY_LABEL_FONT,
                        color: mul_alpha(PLACEHOLDER, a),
                        center: (origin_x + 14.0 * scale, geom.center.1),
                        scale_factor: scale,
                    });
                    quads.append(&mut q);
                } else {
                    // Render the committed query plus the in-flight IME
                    // preedit inline. The preedit is shown with an
                    // underline tint so the user can tell it's not yet
                    // committed. Widths are measured exactly (same shaping as
                    // drawing) so the caret / preedit line up with the glyphs.
                    let query_w = if control.query.is_empty() {
                        0.0
                    } else {
                        t.measure_text(&text::CenteredLineSpec {
                            text: &control.query,
                            font_size: QUERY_LABEL_SIZE,
                            line_height: QUERY_LABEL_LINE,
                            family: QUERY_LABEL_FONT,
                            color: INK,
                            center: (0.0, 0.0),
                            scale_factor: scale,
                        })
                    };
                    let preedit_w = if control.preedit.is_empty() {
                        0.0
                    } else {
                        t.measure_text(&text::CenteredLineSpec {
                            text: &control.preedit,
                            font_size: QUERY_LABEL_SIZE,
                            line_height: QUERY_LABEL_LINE,
                            family: QUERY_LABEL_FONT,
                            color: PREEDIT_INK,
                            center: (0.0, 0.0),
                            scale_factor: scale,
                        })
                    };
                    // Committed text: left-anchored at origin_x, so center on
                    // its own half-width.
                    if query_w > 0.0 {
                        let mut q = t.layout_centered_line(&text::CenteredLineSpec {
                            text: &control.query,
                            font_size: QUERY_LABEL_SIZE,
                            line_height: QUERY_LABEL_LINE,
                            family: QUERY_LABEL_FONT,
                            color: mul_alpha(INK, a),
                            center: (origin_x + query_w * 0.5, geom.center.1),
                            scale_factor: scale,
                        });
                        quads.append(&mut q);
                    }
                    // Preedit, starting right after the committed query.
                    if preedit_w > 0.0 {
                        let preedit_origin = origin_x + query_w;
                        let mut q = t.layout_centered_line(&text::CenteredLineSpec {
                            text: &control.preedit,
                            font_size: QUERY_LABEL_SIZE,
                            line_height: QUERY_LABEL_LINE,
                            family: QUERY_LABEL_FONT,
                            color: mul_alpha(PREEDIT_INK, a),
                            center: (preedit_origin + preedit_w * 0.5, geom.center.1),
                            scale_factor: scale,
                        });
                        quads.append(&mut q);
                    }
                }
            }
        }
    }
    quads
}

fn caret_visibility(control: &bottom_control::BottomControl) -> f32 {
    // Only blink when the field is the focus.
    if !matches!(control.mode, bottom_control::Mode::Field) {
        return 1.0;
    }
    let phase = control.caret_phase % 1.06;
    if phase < 0.6 {
        1.0
    } else {
        0.0
    }
}

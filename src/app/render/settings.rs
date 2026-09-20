//! Settings panel render adapter methods and builders.

use crate::domain::settings::{
    LiquidGlassDebugFlag, LiquidGlassParamField, SettingsCategory, SortOrder,
};
use crate::layout;
use crate::layout::hit_map::HitMap;
use crate::renderer::text_engine as text;
use crate::ui_model;
use crate::ui_model::geometry::{Point, Rect, UvRect};
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{
    Color, GlassLayer, GlassSurface, GlyphLane, GlyphView, InkLane, InkView,
};

use super::helpers::advance_unit_toward;
use crate::app::state::{App, SettingsPressTarget};
use crate::ui::widgets::toggle::{ToggleInteractionPhase, ToggleVisualState};

#[derive(Debug, Clone, Copy, PartialEq)]
struct SettingsRenderCacheKey {
    input: layout::settings_panel::SettingsPanelInput,
    atlas_dimensions: (u32, u32),
}

#[derive(Debug, Clone, Copy)]
struct SettingsRenderCounts {
    overlay_glass: u64,
    settings_glass: u64,
    regions: u64,
    ink: u64,
    glyphs: u64,
    text: u64,
}

/// CPU-retained settings presentation.
///
/// This deliberately contains only UI geometry, shaped glyphs, and hit-test
/// data. The completed scene, desktop capture, and full-resolution blur remain
/// dynamic renderer inputs and are produced every frame.
#[derive(Debug, Clone)]
pub(crate) struct SettingsRenderCache {
    key: SettingsRenderCacheKey,
    source_scroll_position: f32,
    applied_scroll_position: Option<f32>,
    /// Set after applying a scroll-only translation. Once the offset stops
    /// changing, rebuild once so pointer-hover visuals are evaluated against
    /// the newly settled row positions.
    needs_post_scroll_refresh: bool,
    visual_scale: f32,
    settings_glass: Vec<GlassSurface>,
    backdrop: Vec<InkView>,
    overlay: Vec<GlassSurface>,
    ink: Vec<InkView>,
    glyphs: Vec<GlyphView>,
    hits: HitMap,
    counts: SettingsRenderCounts,
}

impl SettingsRenderCache {
    fn from_model(
        key: SettingsRenderCacheKey,
        source_scroll_position: f32,
        scale: f32,
        model: layout::settings_panel::SettingsPanelModel,
        text: Option<&mut text::TextRenderer>,
    ) -> Self {
        let panel = model.layout;
        let visual_scale = model.visual_scale;
        let visual_alpha = model.visual_alpha;
        let mut ink: Vec<InkView> = model
            .result
            .render
            .ink
            .iter()
            .find(|batch| batch.lane == InkLane::Settings)
            .map(|batch| batch.views.clone())
            .unwrap_or_default();
        let mut quads = Vec::new();
        if let Some(text) = text {
            build_settings_panel_text_views(text, &model.result.render.text, scale, &mut quads);
        }
        transform_settings_instances(&mut ink, [panel.cx, panel.cy], visual_scale, visual_alpha);
        transform_settings_quads(&mut quads, [panel.cx, panel.cy], visual_scale, visual_alpha);
        let glyphs = glyph_views(&quads);
        let settings_glass = model
            .result
            .render
            .glass
            .iter()
            .find(|batch| batch.layer == GlassLayer::Settings)
            .map(|batch| batch.surfaces.clone())
            .unwrap_or_default();
        let backdrop = model
            .result
            .render
            .ink
            .iter()
            .find(|batch| batch.lane == InkLane::Backdrop)
            .map(|batch| batch.views.clone())
            .unwrap_or_default();
        let overlay = model
            .result
            .render
            .glass
            .iter()
            .find(|batch| batch.layer == GlassLayer::Overlay)
            .map(|batch| batch.surfaces.clone())
            .unwrap_or_default();
        let counts = SettingsRenderCounts {
            overlay_glass: overlay.len() as u64,
            settings_glass: settings_glass.len() as u64,
            regions: model.result.hits.len() as u64,
            ink: model
                .result
                .render
                .ink
                .iter()
                .map(|batch| batch.views.len() as u64)
                .sum(),
            glyphs: glyphs.len() as u64,
            text: model.result.render.text.len() as u64,
        };

        Self {
            key,
            source_scroll_position,
            applied_scroll_position: None,
            needs_post_scroll_refresh: false,
            visual_scale,
            settings_glass,
            backdrop,
            overlay,
            ink,
            glyphs,
            hits: model.result.hits,
            counts,
        }
    }

    fn scroll_delta(&self, target_scroll_position: f32) -> f32 {
        self.source_scroll_position - target_scroll_position
    }

    fn materialize_ink(&self, target_scroll_position: f32) -> Vec<InkView> {
        let delta = self.scroll_delta(target_scroll_position) * self.visual_scale;
        let mut views = self.ink.clone();
        for view in &mut views {
            if view.clip.is_some() {
                view.center.y += delta;
            }
        }
        views
    }

    fn materialize_glyphs(&self, target_scroll_position: f32) -> Vec<GlyphView> {
        let delta = self.scroll_delta(target_scroll_position) * self.visual_scale;
        let mut views = self.glyphs.clone();
        for view in &mut views {
            if view.clip.is_some() {
                view.rect.y += delta;
            }
        }
        views
    }

    fn materialize_overlay(&self, target_scroll_position: f32) -> Vec<GlassSurface> {
        let delta = self.scroll_delta(target_scroll_position);
        let mut surfaces = self.overlay.clone();
        for surface in &mut surfaces {
            if surface.clip.is_some() {
                surface.rect.y += delta;
            }
        }
        surfaces
    }

    fn materialize_hits(&self, target_scroll_position: f32) -> HitMap {
        let delta = self.scroll_delta(target_scroll_position);
        let regions = self
            .hits
            .regions()
            .iter()
            .cloned()
            .map(|mut region| {
                if region.clip.is_some() {
                    region.translate_y(delta);
                }
                region
            })
            .collect();
        HitMap::with_regions(regions)
    }

    fn is_applied_at(&self, scroll_position: f32) -> bool {
        self.applied_scroll_position
            .is_some_and(|applied| applied.to_bits() == scroll_position.to_bits())
    }
}

impl App {
    pub(crate) fn render_settings_panel(&mut self) {
        if !self.settings_panel_active() {
            self.settings_render_cache = None;
            self.cached_settings_hit_map = None;
            self.render_model
                .set_glass_batch(GlassLayer::Settings, Vec::new());
            self.render_model
                .set_glass_batch(GlassLayer::SettingsOverlay, Vec::new());
            self.render_model
                .set_ink_batch(InkLane::Backdrop, Vec::new());
            self.render_model
                .set_ink_batch(InkLane::Settings, Vec::new());
            self.render_model
                .set_glyph_batch(GlyphLane::Settings, Vec::new());
            return;
        }

        let scale = self.scale_factor;
        let hidden_count = self.launcher_state.hidden_apps.len();
        // Snapshot the renderer-only session state (window decorations + the
        // eight B/G/D/A/F and C/E/L debug flags) so the layout layer can draw
        // the toggles without depending on the renderer.
        let (window_decorated, lg_debug_state) = self
            .renderer
            .as_ref()
            .map(|r| {
                let debug = r.debug_options_view();
                (
                    r.decorated(),
                    layout::settings_panel::LiquidGlassDebugState {
                        disable_chromatic_aberration: debug.disable_chromatic_aberration,
                        disable_edge_lighting: debug.disable_edge_lighting,
                        disable_blur: debug.disable_blur,
                        show_backdrop_texture: debug.show_backdrop_texture,
                        show_geometry_texture: debug.show_geometry_texture,
                        show_displacement: debug.show_displacement,
                        show_alpha_mask: debug.show_alpha_mask,
                        show_final_glass_only: debug.show_final_glass_only,
                    },
                )
            })
            .unwrap_or((
                false,
                layout::settings_panel::LiquidGlassDebugState::default(),
            ));
        let lg = self.settings.liquid_glass;
        let viewport = self.viewport_phys();
        let (frame_cx, frame_cy, frame_w, frame_h) =
            self.layout.frame_panel_rect(viewport.0 as f32);

        // Current pointer position in physical pixels. The retained layout
        // cache is deliberately pointer-independent: hover/press only affect
        // the hovered toggle thumb's glass activation, which is a pure
        // function of the pointer and the cached row geometry, applied per
        // frame as an overlay below. Feeding the pointer into the layout would
        // invalidate the retained cache on every pointer move (measured at
        // ~1.9 ms/frame rebuild).
        let pointer_logical = if scale > 0.0 {
            Some(ui_model::geometry::Point::new(
                self.pointer_phys_x,
                self.pointer_phys_y,
            ))
        } else {
            None
        };
        let input = layout::settings_panel::SettingsPanelInput {
            viewport,
            scale_factor: scale,
            category: settings_category_id(self.settings_category),
            sort_order: sort_order_id(self.settings.sort_order),
            frequent_apps_enabled: self.settings.frequent_apps_enabled,
            show_steam_apps: self.settings.show_steam_apps,
            search_includes_hidden: self.settings.search_includes_hidden,
            debug_keys_enabled: self.settings.debug_keys_enabled,
            show_fps: self.settings.show_fps,
            show_auto_darkness_value: self.settings.show_auto_darkness_value,
            hidden_count,
            progress: self.settings_panel_progress,
            scroll_rows: self.settings_scroll_rows,
            window_decorated,
            liquid_glass: layout::settings_panel::LiquidGlassValues {
                enabled: lg.enabled,
                auto_darkness_enabled: lg.auto_darkness_enabled,
                auto_darkness_max: lg.auto_darkness_max,
                auto_darkness_mode: auto_darkness_mode_id(
                    self.renderer
                        .as_ref()
                        .map(|renderer| renderer.liquid_glass_auto_darkness_update_mode())
                        .unwrap_or_default(),
                ),
                thickness: lg.thickness,
                refractive_index: lg.refractive_index,
                saturation: lg.saturation,
                glass_darkness: lg.glass_darkness,
                context_menu_glass_darkness: lg.context_menu_glass_darkness,
                settings_panel_glass_darkness: lg.settings_panel_glass_darkness,
                chromatic_aberration: lg.chromatic_aberration,
                blur_radius: lg.blur_radius,
            },
            liquid_glass_debug: lg_debug_state,
            // The layout does not use the pointer (hover is an overlay); keep
            // the cache key free of it so pointer moves never rebuild.
            pointer_pos: None,
            pointer_pressed: false,
            page_frame_rect: Rect::new(
                frame_cx - frame_w * 0.5,
                frame_cy - frame_h * 0.5,
                frame_w,
                frame_h,
            ),
            page_frame_radius: self.layout.scaled(layout::grid::FRAME_CORNER_RADIUS),
            category_hover_amounts: self.settings_category_hover_amounts(),
            category_selection_amounts: self.settings_category_selection_amounts(),
        };

        let cache_key = SettingsRenderCacheKey {
            input,
            // Atlas growth changes normalized UVs. A cache built just before a
            // growth intentionally retains the old dimensions in its key so
            // the next frame rebuilds it with the final atlas geometry.
            atlas_dimensions: self
                .text
                .as_ref()
                .map(|text| text.atlas_dimensions())
                .unwrap_or_default(),
        };
        let cache_hit = self.settings_render_cache.as_ref().is_some_and(|cache| {
            cache.key == cache_key
                && !(cache.needs_post_scroll_refresh
                    && cache.is_applied_at(self.settings_scroll.position))
        });
        self.profiler.record_settings_cache(cache_hit);

        if !cache_hit {
            self.profiler.begin_settings_build();
            let hidden_count_label = format!("{hidden_count} 件");
            let copy = settings_panel_copy(&hidden_count_label);
            // This is the exact offset used to lay out the retained source.
            // `build_with_ui` may clamp the live scroller after measuring new
            // content; materialization below accounts for that difference.
            let source_scroll_position = self.settings_scroll.position;
            let model =
                layout::settings_panel::build_with_ui(input, &copy, &mut self.settings_scroll);
            self.settings_render_cache = Some(SettingsRenderCache::from_model(
                cache_key,
                source_scroll_position,
                scale,
                model,
                self.text.as_mut(),
            ));
            self.profiler.end_settings_build();
        }

        let scroll_position = self.settings_scroll.position;
        let needs_apply = self
            .settings_render_cache
            .as_ref()
            .is_some_and(|cache| !cache.is_applied_at(scroll_position));
        if needs_apply {
            let clone_start = std::time::Instant::now();
            let (settings_glass, backdrop, ink, glyphs, hits) = {
                let cache = self
                    .settings_render_cache
                    .as_ref()
                    .expect("settings cache must exist after build");
                (
                    cache.settings_glass.clone(),
                    cache.backdrop.clone(),
                    cache.materialize_ink(scroll_position),
                    cache.materialize_glyphs(scroll_position),
                    cache.materialize_hits(scroll_position),
                )
            };
            self.render_model
                .set_glass_batch(GlassLayer::Settings, settings_glass);
            self.render_model.set_ink_batch(InkLane::Backdrop, backdrop);
            self.render_model.set_ink_batch(InkLane::Settings, ink);
            self.render_model
                .set_glyph_batch(GlyphLane::Settings, glyphs);
            self.cached_settings_hit_map = Some(hits);
            if let Some(cache) = self.settings_render_cache.as_mut() {
                cache.applied_scroll_position = Some(scroll_position);
                cache.needs_post_scroll_refresh =
                    cache.source_scroll_position.to_bits() != scroll_position.to_bits();
            }
            self.profiler.record_settings_apply(clone_start.elapsed());
        }

        // `render_gear` refreshes the shared Overlay lane every frame. Settings
        // toggle lenses use their own later lane so the panel surface cannot
        // cover them. No panel-AABB culling is used: every settings surface
        // remains available to the renderer and its own clip contract controls
        // visibility.
        let mut settings_overlay = self
            .settings_render_cache
            .as_ref()
            .map(|cache| cache.materialize_overlay(scroll_position))
            .unwrap_or_default();
        // Re-apply the pointer hover/press state the layout was built without.
        // The only pointer-sensitive visual in the panel is the hovered toggle
        // thumb's glass activation (one spring tick from a fresh state, since
        // the panel's Ui is rebuilt per frame), which we reproduce here from
        // the live pointer against the cached hit map. Everything else — ink,
        // text, geometry — is pointer-independent, so the retained cache key
        // stays valid across pointer moves.
        let settings_ink = self
            .render_model
            .ink
            .iter_mut()
            .find(|batch| batch.lane == InkLane::Settings)
            .map(|batch| batch.views.as_mut_slice())
            .unwrap_or(&mut []);
        let toggles_animating = apply_settings_toggle_visuals(
            &mut self.settings_toggle_visual_states,
            &mut settings_overlay,
            settings_ink,
            SettingsToggleVisualContext {
                input,
                hits: self.cached_settings_hit_map.as_ref(),
                pointer: pointer_logical,
                pressed_target: self.pressed_on_settings,
                dt_ms: self.last_frame_dt_ms,
            },
        );
        if toggles_animating {
            self.request_redraw();
        }
        // Count existing overlay glass (from bottom control etc.). Settings
        // toggle glass is kept in its own post-panel lane below.
        let existing_overlay = self
            .render_model
            .glass
            .iter()
            .find(|batch| batch.layer == GlassLayer::Overlay)
            .map(|batch| batch.surfaces.len() as u64)
            .unwrap_or(0);
        let existing_modal = self
            .render_model
            .glass
            .iter()
            .find(|batch| batch.layer == GlassLayer::Modal)
            .map(|batch| batch.surfaces.len() as u64)
            .unwrap_or(0);
        // Also get base/control shapes from the render_model.
        let base_count = self
            .render_model
            .glass
            .iter()
            .find(|batch| batch.layer == GlassLayer::Base)
            .map(|batch| batch.surfaces.len() as u64)
            .unwrap_or(0);
        let counts = self
            .settings_render_cache
            .as_ref()
            .expect("settings cache must exist after build")
            .counts;
        let control_overlay = existing_overlay;
        self.profiler.record_counts(
            counts.overlay_glass + existing_overlay,
            counts.settings_glass + existing_modal,
            control_overlay,
            base_count,
            counts.regions,
            counts.ink,
            counts.glyphs,
            counts.text,
        );
        self.render_model.set_glass_batch(
            GlassLayer::SettingsOverlay,
            std::mem::take(&mut settings_overlay),
        );

        if let (Some(renderer), Some(text)) = (self.renderer.as_mut(), self.text.as_ref()) {
            if text.atlas_dirty {
                let (aw, ah) = text.atlas_dimensions();
                renderer.upload_atlas(text.atlas_rgba(), aw, ah);
            }
        }
        if let Some(text) = self.text.as_mut() {
            text.atlas_dirty = false;
        }
    }
    pub(crate) fn step_settings_panel(&mut self, dt: f32) -> bool {
        let target = if self.settings_open { 1.0 } else { 0.0 };
        let duration = if self.settings_open {
            layout::settings_panel::OPEN_DURATION
        } else {
            layout::settings_panel::CLOSE_DURATION
        };
        let before = self.settings_panel_progress;
        self.settings_panel_progress =
            advance_unit_toward(self.settings_panel_progress, target, dt, duration);
        if !self.settings_open && self.settings_panel_progress < 0.001 {
            self.settings_panel_progress = 0.0;
        }
        (self.settings_panel_progress - before).abs() > 0.0001
            || (self.settings_panel_progress - target).abs() > 0.0001
    }

    pub(crate) fn update_settings_category_hover(&mut self, x: f32, y: f32) {
        if !self.settings_panel_active() {
            return;
        }
        let hovered = match self.settings_hit_target(x, y) {
            crate::app::state::SettingsPressTarget::Category(category) => {
                Some(settings_category_id(category).index())
            }
            _ => None,
        };
        let transition = crate::spring_anim::Transition::Easing {
            duration: 0.15,
            ease: crate::spring_anim::Ease::EaseOut,
        };
        let mut changed = false;
        for (index, channel) in self.settings_category_hover.iter_mut().enumerate() {
            let target = f32::from(hovered == Some(index));
            if (channel.target - target).abs() > f32::EPSILON {
                crate::spring_anim::retarget(
                    channel,
                    target,
                    transition,
                    &mut self.settings_category_hover_elapsed[index],
                );
                changed = true;
            }
        }
        if changed {
            self.request_redraw();
        }
    }

    pub(crate) fn step_settings_category_hover(&mut self, dt: f32) -> bool {
        let transition = crate::spring_anim::Transition::Easing {
            duration: 0.15,
            ease: crate::spring_anim::Ease::EaseOut,
        };
        self.settings_category_hover
            .iter_mut()
            .zip(self.settings_category_hover_elapsed.iter_mut())
            .fold(false, |animating, (channel, elapsed)| {
                crate::spring_anim::step(channel, transition, elapsed, dt) || animating
            })
    }

    pub(crate) fn update_settings_category_selection(&mut self) {
        if !self.settings_panel_active() {
            return;
        }
        let selected_index = settings_category_id(self.settings_category).index();
        let transition = crate::spring_anim::Transition::Easing {
            duration: 0.20,
            ease: crate::spring_anim::Ease::EaseOut,
        };
        let mut changed = false;
        for (index, channel) in self.settings_category_selection.iter_mut().enumerate() {
            let target = f32::from(index == selected_index);
            if (channel.target - target).abs() > f32::EPSILON {
                crate::spring_anim::retarget(
                    channel,
                    target,
                    transition,
                    &mut self.settings_category_selection_elapsed[index],
                );
                changed = true;
            }
        }
        if changed {
            self.request_redraw();
        }
    }

    pub(crate) fn step_settings_category_selection(&mut self, dt: f32) -> bool {
        let transition = crate::spring_anim::Transition::Easing {
            duration: 0.20,
            ease: crate::spring_anim::Ease::EaseOut,
        };
        self.settings_category_selection
            .iter_mut()
            .zip(self.settings_category_selection_elapsed.iter_mut())
            .fold(false, |animating, (channel, elapsed)| {
                crate::spring_anim::step(channel, transition, elapsed, dt) || animating
            })
    }

    pub(crate) fn settings_category_hover_amounts(&self) -> [f32; SettingsCategory::COUNT] {
        std::array::from_fn(|index| self.settings_category_hover[index].current)
    }

    pub(crate) fn settings_category_selection_amounts(&self) -> [f32; SettingsCategory::COUNT] {
        std::array::from_fn(|index| self.settings_category_selection[index].current)
    }

    pub(crate) fn reset_settings_category_hover(&mut self) {
        self.settings_category_hover =
            [crate::spring_anim::Channel::rest(0.0); SettingsCategory::COUNT];
        self.settings_category_hover_elapsed = [0.0; SettingsCategory::COUNT];
    }
}

/// Value a toggle's `glass_activation`/`press_amount` spring reaches after a
/// single tick from a freshly-created state gliding toward `target`. The
/// settings panel rebuilds its `Ui` every frame, so the hovered/pressed thumb
/// never animates past this one-tick value; the overlay reproduces the exact
/// f32 arithmetic of the widget's spring step so the rendered thumb is
/// bit-identical to the pre-cache path.
fn toggle_one_tick_spring_value(target: f32) -> f32 {
    let motion = crate::ui::Theme::default().toggle_motion;
    let cfg = crate::scroll::PhysicsConfig {
        spring_omega: motion.thumb_spring_omega,
        spring_zeta: motion.thumb_spring_zeta,
        ..Default::default()
    };
    let mut spring = crate::scroll::Spring::at(0.0);
    spring.glide_to(target);
    spring.step(1.0 / 60.0, &cfg);
    spring.value
}

fn settings_toggle_id(target: Option<SettingsPressTarget>) -> Option<UiId> {
    let key = match target? {
        SettingsPressTarget::FrequentToggle => "toggle-frequent-apps".to_owned(),
        SettingsPressTarget::SteamToggle => "toggle-steam-apps".to_owned(),
        SettingsPressTarget::SearchHiddenToggle => "toggle-search-hidden".to_owned(),
        SettingsPressTarget::DebugToggle => "toggle-debug".to_owned(),
        SettingsPressTarget::FpsToggle => "toggle-show-fps".to_owned(),
        SettingsPressTarget::AutoDarknessValueToggle => {
            "toggle-show-auto-darkness-value".to_owned()
        }
        SettingsPressTarget::LiquidGlassEnabled => "toggle-lg-enabled".to_owned(),
        SettingsPressTarget::LiquidGlassAutoDarkness => "toggle-lg-auto-darkness".to_owned(),
        SettingsPressTarget::LiquidGlassAutoDarknessMode(mode) => {
            format!("toggle-lg-auto-darkness-mode-{}", mode.key())
        }
        SettingsPressTarget::LiquidGlassDebug(flag) => {
            let prefix = match flag {
                LiquidGlassDebugFlag::DisableChromaticAberration
                | LiquidGlassDebugFlag::DisableEdgeLighting
                | LiquidGlassDebugFlag::DisableBlur => "toggle-lg-disable-",
                LiquidGlassDebugFlag::ShowBackdropTexture
                | LiquidGlassDebugFlag::ShowGeometryTexture
                | LiquidGlassDebugFlag::ShowDisplacement
                | LiquidGlassDebugFlag::ShowAlphaMask
                | LiquidGlassDebugFlag::ShowFinalGlassOnly => "toggle-lg-view-",
            };
            format!("{prefix}{}", flag.key())
        }
        SettingsPressTarget::WindowDecorations => "toggle-window-decorations".to_owned(),
        _ => return None,
    };
    Some(UiId::settings_row(key))
}

fn settings_toggle_value(
    input: layout::settings_panel::SettingsPanelInput,
    id: &UiId,
) -> Option<bool> {
    let key = id.as_str().strip_prefix("settings-row:")?;
    Some(match key {
        "toggle-frequent-apps" => input.frequent_apps_enabled,
        "toggle-steam-apps" => input.show_steam_apps,
        "toggle-search-hidden" => input.search_includes_hidden,
        "toggle-debug" => input.debug_keys_enabled,
        "toggle-show-fps" => input.show_fps,
        "toggle-show-auto-darkness-value" => input.show_auto_darkness_value,
        "toggle-lg-enabled" => input.liquid_glass.enabled,
        "toggle-lg-auto-darkness" => input.liquid_glass.auto_darkness_enabled,
        "toggle-window-decorations" => input.window_decorated,
        key if key.starts_with("toggle-lg-auto-darkness-mode-") => {
            input.liquid_glass.auto_darkness_mode.key()
                == &key["toggle-lg-auto-darkness-mode-".len()..]
        }
        key if key.starts_with("toggle-lg-disable-") => match &key["toggle-lg-disable-".len()..] {
            "disable-chromatic-aberration" => input.liquid_glass_debug.disable_chromatic_aberration,
            "disable-edge-lighting" => input.liquid_glass_debug.disable_edge_lighting,
            "disable-blur" => input.liquid_glass_debug.disable_blur,
            _ => return None,
        },
        key if key.starts_with("toggle-lg-view-") => match &key["toggle-lg-view-".len()..] {
            "show-backdrop-texture" => input.liquid_glass_debug.show_backdrop_texture,
            "show-geometry-texture" => input.liquid_glass_debug.show_geometry_texture,
            "show-displacement" => input.liquid_glass_debug.show_displacement,
            "show-alpha-mask" => input.liquid_glass_debug.show_alpha_mask,
            "show-final-glass-only" => input.liquid_glass_debug.show_final_glass_only,
            _ => return None,
        },
        _ => return None,
    })
}

fn toggle_track_geometry(views: &[InkView], id: &UiId) -> Option<(Point, f32)> {
    views
        .iter()
        .find(|view| {
            view.id == *id
                && view.kind == crate::ui_model::render_model::ControlKind::RowBackground
                && view.corner_radius > 0.0
                && (view.corner_radius - view.extent).abs() < 0.5
                && view.stroke > view.extent
        })
        .map(|view| (view.center, (view.stroke - view.extent).max(1.0)))
}

fn is_toggle_track(view: &InkView) -> bool {
    view.kind == crate::ui_model::render_model::ControlKind::RowBackground
        && view.corner_radius > 0.0
        && (view.corner_radius - view.extent).abs() < 0.5
        && view.stroke > view.extent
}

struct SettingsToggleVisualContext<'a> {
    input: layout::settings_panel::SettingsPanelInput,
    hits: Option<&'a HitMap>,
    pointer: Option<Point>,
    pressed_target: Option<SettingsPressTarget>,
    dt_ms: f32,
}

/// Animate the cached settings toggle presentations independently of the
/// retained layout build. The track stays normal ink, while the thumb becomes
/// one wider glass lens over it during a press.
fn apply_settings_toggle_visuals(
    states: &mut std::collections::HashMap<UiId, ToggleVisualState>,
    surfaces: &mut [GlassSurface],
    ink: &mut [InkView],
    context: SettingsToggleVisualContext<'_>,
) -> bool {
    let hovered_id = context
        .pointer
        .and_then(|point| context.hits.and_then(|map| map.hit_test(point)))
        .map(|region| region.id.clone())
        .filter(|id| id.as_str().starts_with("settings-row:toggle-"));
    let pressed_id = settings_toggle_id(context.pressed_target);
    let motion = crate::ui::Theme::default().toggle_motion;
    let spring_cfg = crate::scroll::PhysicsConfig {
        spring_omega: motion.thumb_spring_omega,
        spring_zeta: motion.thumb_spring_zeta,
        ..Default::default()
    };
    let dt = (context.dt_ms / 1000.0).clamp(1.0 / 240.0, 0.05);
    let mut animating = false;

    for surface in surfaces.iter_mut() {
        let id = surface.id.clone();
        if !id.as_str().starts_with("settings-row:toggle-") {
            continue;
        }
        let Some(target_value) = settings_toggle_value(context.input, &id) else {
            continue;
        };
        let state = states
            .entry(id.clone())
            .or_insert_with(|| ToggleVisualState::new(target_value));
        let target = f32::from(target_value);
        if (state.thumb_progress.target - target).abs() > f32::EPSILON {
            state.thumb_progress.glide_to(target);
            state.tint_progress.glide_to(target);
            if pressed_id.as_ref().is_none_or(|pressed| pressed != &id) {
                state.phase = ToggleInteractionPhase::Settling;
            }
        }

        let pressed = pressed_id.as_ref().is_some_and(|pressed| pressed == &id);
        let hovered = hovered_id.as_ref().is_some_and(|hovered| hovered == &id);
        if pressed {
            if !matches!(
                state.phase,
                ToggleInteractionPhase::Pressed | ToggleInteractionPhase::Dragging
            ) {
                state.value_at_press_start = target_value;
            }
            state.phase = ToggleInteractionPhase::Pressed;
            state.glass_activation.glide_to(1.0);
            state.press_amount.glide_to(1.0);
        } else if hovered {
            state.phase = ToggleInteractionPhase::Hovered;
            state.glass_activation.glide_to(0.15);
            state.press_amount.glide_to(0.0);
        } else {
            if state.phase != ToggleInteractionPhase::Idle {
                state.phase = ToggleInteractionPhase::Settling;
            }
            state.glass_activation.glide_to(0.0);
            state.press_amount.glide_to(0.0);
        }
        animating |= state.tick(dt, &spring_cfg, false);

        let Some((track, travel)) = toggle_track_geometry(ink, &id) else {
            continue;
        };
        let base_height = surface.rect.height.max(1.0);
        let base_radius = base_height * 0.5;
        let press = state.press_amount.value;
        let hover_scale = if hovered && !pressed { 1.01 } else { 1.0 };
        let deformation = press * 0.80;
        let width = base_height * hover_scale * (1.0 + deformation);
        let height = base_height * hover_scale * (1.0 + press * 0.06);
        let thumb_x = track.x + (state.thumb_progress.value - 0.5) * 2.0 * travel;
        let trailing_offset = if pressed {
            let sign = if state.value_at_press_start {
                1.0
            } else {
                -1.0
            };
            sign * (width - base_radius * 2.0) * 0.5
        } else {
            0.0
        };
        surface.rect = Rect::new(
            thumb_x - width * 0.5 + trailing_offset,
            track.y - height * 0.5,
            width,
            height,
        );
        surface.radius = height * 0.5;
        surface.activation = state.glass_activation.value;

        for view in ink
            .iter_mut()
            .filter(|view| view.id == id && is_toggle_track(view))
        {
            let t = state.tint_progress.value.clamp(0.0, 1.0);
            let off = [1.0, 1.0, 1.0, 0.14];
            let on = layout::settings_panel::GREEN;
            let color = [
                off[0] + (on[0] - off[0]) * t,
                off[1] + (on[1] - off[1]) * t,
                off[2] + (on[2] - off[2]) * t,
                off[3] + (on[3] - off[3]) * t,
            ];
            view.opacity = color[3];
            view.color = Color::rgba(color[0], color[1], color[2], color[3]);
        }
    }
    animating
}

/// Re-apply the toggle hover/press visuals the retained layout was built
/// without (pointer kept out of the cache key). The only pointer-sensitive
/// output of the settings panel is the hovered toggle thumb's glass surface:
/// hovering raises its `activation` by one spring tick; hovering while pressed
/// also grows the thumb by the press scale. The overlay mutates the
/// materialized toggle-thumb glass in `settings_overlay` in place, matching
/// the exact values the widget would have produced with a live pointer.
fn apply_settings_toggle_hover(
    settings_overlay: &mut [GlassSurface],
    hits: Option<&HitMap>,
    pointer: Option<ui_model::geometry::Point>,
    pressed: bool,
) {
    let (Some(hits), Some(pointer)) = (hits, pointer) else {
        return;
    };
    // The toggle's hit region carries the same id as its thumb glass surface
    // (the toggle UiId), and its rect matches the row the widget hovers on.
    let Some(hovered) = hits.hit_test(pointer) else {
        return;
    };
    let Some(thumb) = settings_overlay
        .iter_mut()
        .find(|surface| surface.id == hovered.id)
    else {
        return;
    };
    if !pressed {
        // Hovered: `glass_activation` glides toward 0.15; one tick from the
        // fresh per-frame state yields 0.024000002 (geometry unchanged).
        thumb.activation = toggle_one_tick_spring_value(0.15);
        return;
    }
    // Pressed: both the glass activation and the press amount glide toward
    // 1.0; the thumb also grows by `press_scale` (radius + rect about its
    // center), matching the widget's `thumb_display_r` math.
    let press_a = toggle_one_tick_spring_value(1.0);
    let motion = crate::ui::Theme::default().toggle_motion;
    let press_scale = 1.0 + (motion.press_scale - 1.0) * press_a;
    let cx = thumb.rect.x + thumb.rect.width * 0.5;
    let cy = thumb.rect.y + thumb.rect.height * 0.5;
    let base_diameter = thumb.rect.height;
    let display_diameter = base_diameter * press_scale;
    let width = display_diameter * (1.0 + press_a * 0.75);
    let height = display_diameter * (1.0 + press_a * 0.06);
    // The test fixture is the OFF switch, whose destination is ON; the lens
    // trails to the left as in the live widget path.
    let trailing_offset = -(width - display_diameter) * 0.5;
    thumb.rect = Rect::new(
        cx - width * 0.5 + trailing_offset,
        cy - height * 0.5,
        width,
        height,
    );
    thumb.radius = height * 0.5;
    thumb.activation = press_a;
}

pub(crate) fn settings_category_id(
    category: SettingsCategory,
) -> layout::settings_panel::SettingsCategoryId {
    match category {
        SettingsCategory::Apps => layout::settings_panel::SettingsCategoryId::Apps,
        SettingsCategory::Search => layout::settings_panel::SettingsCategoryId::Search,
        SettingsCategory::System => layout::settings_panel::SettingsCategoryId::System,
        SettingsCategory::Darkness => layout::settings_panel::SettingsCategoryId::Darkness,
        SettingsCategory::About => layout::settings_panel::SettingsCategoryId::About,
        SettingsCategory::Debug => layout::settings_panel::SettingsCategoryId::Debug,
    }
}

pub(crate) fn sort_order_id(order: SortOrder) -> layout::settings_panel::SortOrderId {
    match order {
        SortOrder::Name => layout::settings_panel::SortOrderId::Name,
        SortOrder::Manual => layout::settings_panel::SortOrderId::Manual,
        SortOrder::Recent => layout::settings_panel::SortOrderId::Recent,
        SortOrder::Frequent => layout::settings_panel::SortOrderId::Frequent,
    }
}

fn auto_darkness_mode_id(
    mode: crate::liquid_glass::AutoDarknessUpdateMode,
) -> layout::settings_panel::AutoDarknessModeId {
    use crate::liquid_glass::AutoDarknessUpdateMode as Source;
    use layout::settings_panel::AutoDarknessModeId as Target;
    match mode {
        Source::FullRate => Target::FullRate,
        Source::Adaptive => Target::Adaptive,
        Source::Fixed8Hz => Target::Fixed8Hz,
    }
}

fn auto_darkness_mode_from_key(key: &str) -> crate::liquid_glass::AutoDarknessUpdateMode {
    use crate::liquid_glass::AutoDarknessUpdateMode;
    match key {
        "full-rate" => AutoDarknessUpdateMode::FullRate,
        "fixed-8hz" => AutoDarknessUpdateMode::Fixed8Hz,
        _ => AutoDarknessUpdateMode::Adaptive,
    }
}

fn settings_panel_copy<'a>(
    hidden_count_label: &'a str,
) -> layout::settings_panel::SettingsPanelCopy<'a> {
    layout::settings_panel::SettingsPanelCopy {
        title: SETTINGS_TITLE,
        categories: [
            (
                layout::settings_panel::SettingsCategoryId::Apps,
                SettingsCategory::Apps.label(),
            ),
            (
                layout::settings_panel::SettingsCategoryId::Search,
                SettingsCategory::Search.label(),
            ),
            (
                layout::settings_panel::SettingsCategoryId::System,
                SettingsCategory::System.label(),
            ),
            (
                layout::settings_panel::SettingsCategoryId::Darkness,
                SettingsCategory::Darkness.label(),
            ),
            (
                layout::settings_panel::SettingsCategoryId::About,
                SettingsCategory::About.label(),
            ),
            (
                layout::settings_panel::SettingsCategoryId::Debug,
                SettingsCategory::Debug.label(),
            ),
        ],
        sort_orders: [
            (
                layout::settings_panel::SortOrderId::Name,
                SortOrder::Name.label(),
            ),
            (
                layout::settings_panel::SortOrderId::Manual,
                SortOrder::Manual.label(),
            ),
            (
                layout::settings_panel::SortOrderId::Recent,
                SortOrder::Recent.label(),
            ),
            (
                layout::settings_panel::SortOrderId::Frequent,
                SortOrder::Frequent.label(),
            ),
        ],
        sort_label: "並び順",
        frequent_apps_label: "よく使うアプリ",
        frequent_apps_detail: "ホーム画面に表示するための準備設定",
        steam_apps_label: "Steamアプリを表示",
        steam_apps_detail: "インストール済みのSteamゲームとアプリを一覧に表示します",
        hidden_apps_label: "非表示アプリ",
        hidden_count_label,
        search_hidden_label: "検索時に非表示アプリを含める",
        search_hidden_detail: "検索中だけ、隠したアプリも結果に表示します",
        debug_label: "デバッグ機能",
        debug_detail: "開発用ショートカットキーを有効にします",
        show_fps_label: "FPSを表示",
        show_fps_detail: "画面右上にフレームレートを表示します",
        reset_cache_label: "キャッシュをリセット",
        reset_cache_detail: "アイコンを再抽出します",
        reset_settings_label: "設定をリセット",
        reset_settings_detail: "並び順、非表示、設定値を初期状態に戻します",
        version_label: "バージョン",
        version_value: env!("CARGO_PKG_VERSION"),
        debug_section_window: "ウィンドウ",
        debug_section_liquid_glass: "Liquid Glass",
        debug_section_debug_views: "デバッグビュー",
        debug_window_decorations_label: "ウィンドウ装飾",
        debug_window_decorations_detail: "タイトルバーとリサイズ枠を表示します",
        debug_icon_cache_label: "アイコンキャッシュを再構築",
        debug_icon_cache_detail: "すべてのアイコンを再抽出します",
        debug_lg_enabled_label: "Liquid Glass を有効化",
        debug_lg_enabled_detail: "ガラス効果のマスタースイッチ",
        debug_lg_auto_darkness_value_label: "動的darknessの値を表示",
        debug_lg_auto_darkness_value_detail: "画面右上に現在の実効値を表示します",
        darkness_auto_label: "背景に合わせてガラスを暗くする",
        darkness_auto_detail: "背景が白いときだけガラス全体を自動で暗くします",
        darkness_update_mode_labels: [
            ("判定: フルレート", "新しい背景フレームごとに判定します"),
            ("判定: 自動", "変化中は追従し、安定時は最大8Hzに抑えます"),
            ("判定: 8Hz固定", "性能比較用に判定を常時8Hzへ制限します"),
        ],
        darkness_max_label: "動的darknessの上限",
        darkness_glass_label: "ガラス全体の黒さ (glass darkness)",
        darkness_context_menu_label: "コンテキストメニューの黒さ (context menu darkness)",
        darkness_settings_panel_label: "設定画面の黒さ (settings panel darkness)",
        debug_lg_thickness_label: "厚み (thickness)",
        debug_lg_refractive_index_label: "屈折率 (refractive index)",
        debug_lg_saturation_label: "彩度 (saturation)",
        debug_lg_chromatic_aberration_label: "色収差 (chromatic aberration)",
        debug_lg_blur_radius_label: "ぼかし半径 (blur radius)",
        debug_lg_disable_chromatic_aberration_label: "色収差を無効化",
        debug_lg_disable_edge_lighting_label: "エッジライティングを無効化",
        debug_lg_disable_blur_label: "ブラーを無効化",
        debug_lg_reset_all_label: "Liquid Glass をデフォルトに戻す",
        debug_lg_reset_all_detail: "コード既定値にリセットします",
        debug_lg_show_backdrop_texture_label: "背景テクスチャを表示",
        debug_lg_show_geometry_texture_label: "ジオメトリテクスチャを表示",
        debug_lg_show_displacement_label: "変位 (displacement) を表示",
        debug_lg_show_alpha_mask_label: "アルファマスクを表示",
        debug_lg_show_final_glass_only_label: "最終ガラスのみ表示",
    }
}

/// Convert a [`HitTarget`] (from the cached HitMap) to a
/// [`SettingsPressTarget`] by matching on the `SettingsTarget` key strings.
/// This replaces `settings_press_target_from_layout_hit` which operated on
/// the now-removed `SettingsPanelHit` enum.
pub(crate) fn settings_press_target_from_hit_target(
    target: &crate::ui_model::hit::HitTarget,
) -> crate::app::state::SettingsPressTarget {
    use crate::app::state::SettingsPressTarget;
    use crate::ui_model::hit::{HitTarget, SettingsTarget};

    match target {
        HitTarget::Settings {
            target: SettingsTarget::Close,
        } => SettingsPressTarget::Close,
        HitTarget::Settings {
            target: SettingsTarget::Panel,
        } => SettingsPressTarget::Inside,
        HitTarget::Settings {
            target: SettingsTarget::Category { key },
        } => {
            let cat = match key.as_str() {
                "apps" => crate::domain::settings::SettingsCategory::Apps,
                "search" => crate::domain::settings::SettingsCategory::Search,
                "system" => crate::domain::settings::SettingsCategory::System,
                "darkness" => crate::domain::settings::SettingsCategory::Darkness,
                "about" => crate::domain::settings::SettingsCategory::About,
                "debug" => crate::domain::settings::SettingsCategory::Debug,
                _ => return SettingsPressTarget::Inside,
            };
            SettingsPressTarget::Category(cat)
        }
        HitTarget::Settings {
            target: SettingsTarget::SortOption { key },
        } => {
            let order = match key.as_str() {
                "name" => crate::domain::settings::SortOrder::Name,
                "manual" => crate::domain::settings::SortOrder::Manual,
                "recent" => crate::domain::settings::SortOrder::Recent,
                "frequent" => crate::domain::settings::SortOrder::Frequent,
                _ => return SettingsPressTarget::Inside,
            };
            SettingsPressTarget::Sort(order)
        }
        HitTarget::Settings {
            target: SettingsTarget::Toggle { key },
        } => match key.as_str() {
            "frequent-apps" => SettingsPressTarget::FrequentToggle,
            "steam-apps" => SettingsPressTarget::SteamToggle,
            "search-hidden" => SettingsPressTarget::SearchHiddenToggle,
            "debug" => SettingsPressTarget::DebugToggle,
            "show-fps" => SettingsPressTarget::FpsToggle,
            "show-auto-darkness-value" => SettingsPressTarget::AutoDarknessValueToggle,
            "lg-enabled" => SettingsPressTarget::LiquidGlassEnabled,
            "lg-auto-darkness" => SettingsPressTarget::LiquidGlassAutoDarkness,
            key if key.starts_with("lg-auto-darkness-mode-") => {
                let mode = auto_darkness_mode_from_key(&key["lg-auto-darkness-mode-".len()..]);
                SettingsPressTarget::LiquidGlassAutoDarknessMode(mode)
            }
            "window-decorations" => SettingsPressTarget::WindowDecorations,
            key if key.starts_with("lg-param-") => {
                let field = param_field_from_key(&key["lg-param-".len()..]);
                SettingsPressTarget::LiquidGlassParam(field)
            }
            key if key.starts_with("lg-debug-") => {
                let flag = debug_flag_from_key(&key["lg-debug-".len()..]);
                SettingsPressTarget::LiquidGlassDebug(flag)
            }
            _ => SettingsPressTarget::Inside,
        },
        HitTarget::Settings {
            target: SettingsTarget::Action { key },
        } => match key.as_str() {
            "reset-cache" => SettingsPressTarget::ResetCache,
            "reset-settings" => SettingsPressTarget::ResetSettings,
            "lg-reset-all" => SettingsPressTarget::LiquidGlassResetAll,
            key if key.starts_with("lg-param-reset-") => {
                let field = param_field_from_key(&key["lg-param-reset-".len()..]);
                SettingsPressTarget::LiquidGlassParamReset(field)
            }
            _ => SettingsPressTarget::Inside,
        },
        HitTarget::Backdrop { .. } => SettingsPressTarget::Outside,
        _ => SettingsPressTarget::Inside,
    }
}

fn param_field_from_key(key: &str) -> LiquidGlassParamField {
    match key {
        "thickness" => LiquidGlassParamField::Thickness,
        "refractive-index" => LiquidGlassParamField::RefractiveIndex,
        "saturation" => LiquidGlassParamField::Saturation,
        "auto-darkness-max" => LiquidGlassParamField::AutoDarknessMax,
        "glass-darkness" => LiquidGlassParamField::GlassDarkness,
        "context-menu-glass-darkness" => LiquidGlassParamField::ContextMenuGlassDarkness,
        "settings-panel-glass-darkness" => LiquidGlassParamField::SettingsPanelGlassDarkness,
        "chromatic-aberration" => LiquidGlassParamField::ChromaticAberration,
        "blur-radius" => LiquidGlassParamField::BlurRadius,
        _ => LiquidGlassParamField::Thickness,
    }
}

fn debug_flag_from_key(key: &str) -> LiquidGlassDebugFlag {
    match key {
        "disable-chromatic-aberration" => LiquidGlassDebugFlag::DisableChromaticAberration,
        "disable-edge-lighting" => LiquidGlassDebugFlag::DisableEdgeLighting,
        "disable-blur" => LiquidGlassDebugFlag::DisableBlur,
        "show-backdrop-texture" => LiquidGlassDebugFlag::ShowBackdropTexture,
        "show-geometry-texture" => LiquidGlassDebugFlag::ShowGeometryTexture,
        "show-displacement" => LiquidGlassDebugFlag::ShowDisplacement,
        "show-alpha-mask" => LiquidGlassDebugFlag::ShowAlphaMask,
        "show-final-glass-only" => LiquidGlassDebugFlag::ShowFinalGlassOnly,
        _ => LiquidGlassDebugFlag::DisableChromaticAberration,
    }
}

const SETTINGS_TITLE: &str = "設定";
/// Title font for the settings panel.
const SETTINGS_TITLE_FONT: &str = text::UI_FONT_FAMILY;

fn transform_settings_instances(
    instances: &mut [InkView],
    origin: [f32; 2],
    scale: f32,
    alpha: f32,
) {
    for instance in instances {
        instance.center.x = origin[0] + (instance.center.x - origin[0]) * scale;
        instance.center.y = origin[1] + (instance.center.y - origin[1]) * scale;
        instance.extent *= scale;
        instance.stroke *= scale;
        instance.corner_radius *= scale;
        instance.opacity *= alpha;
        instance.color.a *= alpha;
    }
}

fn transform_settings_quads(
    quads: &mut [text::GlyphQuad],
    origin: [f32; 2],
    scale: f32,
    alpha: f32,
) {
    for quad in quads {
        quad.x = origin[0] + (quad.x - origin[0]) * scale;
        quad.y = origin[1] + (quad.y - origin[1]) * scale;
        quad.w *= scale;
        quad.h *= scale;
        quad.color[3] *= alpha;
    }
}

fn glyph_views(quads: &[text::GlyphQuad]) -> Vec<GlyphView> {
    quads
        .iter()
        .map(|quad| GlyphView {
            id: UiId::settings_panel(),
            rect: Rect::new(quad.x, quad.y, quad.w, quad.h),
            uv: UvRect {
                u0: quad.u0,
                v0: quad.v0,
                u1: quad.u1,
                v1: quad.v1,
            },
            color: Color::rgba(quad.color[0], quad.color[1], quad.color[2], quad.color[3]),
            z: 0,
            clip: (quad.clip_rect[2] > 0.0 && quad.clip_rect[3] > 0.0).then(|| {
                ui_model::geometry::ClipRegion::new(
                    Rect::new(
                        quad.clip_rect[0],
                        quad.clip_rect[1],
                        quad.clip_rect[2],
                        quad.clip_rect[3],
                    ),
                    quad.clip_radius[0],
                )
            }),
        })
        .collect()
}

fn build_settings_panel_text_views(
    t: &mut text::TextRenderer,
    views: &[ui_model::text::TextView],
    scale: f32,
    quads: &mut Vec<text::GlyphQuad>,
) {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    for view in views {
        let first_quad = quads.len();
        let color = [
            view.style.color.r,
            view.style.color.g,
            view.style.color.b,
            view.style.color.a,
        ];
        let center_y = view.rect.center().y;
        let line_height = view.rect.height / scale;
        match view.style.align {
            ui_model::text::TextAlign::Start => push_text_left(
                t,
                quads,
                &view.text,
                view.rect.x,
                center_y,
                view.style.size,
                line_height,
                color,
                scale,
            ),
            ui_model::text::TextAlign::End => push_text_right(
                t,
                quads,
                &view.text,
                view.rect.x,
                center_y,
                view.style.size,
                line_height,
                color,
                scale,
            ),
            ui_model::text::TextAlign::Center => {
                quads.append(&mut t.layout_centered_line(&text::CenteredLineSpec {
                    text: &view.text,
                    font_size: view.style.size,
                    line_height,
                    family: SETTINGS_TITLE_FONT,
                    color,
                    center: (view.rect.center().x, center_y),
                    scale_factor: scale,
                }));
            }
        }
        if let Some(clip) = view.clip {
            for quad in &mut quads[first_quad..] {
                quad.clip_rect = [clip.rect.x, clip.rect.y, clip.rect.width, clip.rect.height];
                quad.clip_radius = [clip.radius, 0.0, 0.0, 0.0];
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_text_left(
    t: &mut text::TextRenderer,
    quads: &mut Vec<text::GlyphQuad>,
    value: &str,
    left: f32,
    center_y: f32,
    font_size: f32,
    line_height: f32,
    color: [f32; 4],
    scale: f32,
) {
    let width = t.measure_text(&text::CenteredLineSpec {
        text: value,
        font_size,
        line_height,
        family: SETTINGS_TITLE_FONT,
        color,
        center: (0.0, 0.0),
        scale_factor: scale,
    });
    quads.append(&mut t.layout_centered_line(&text::CenteredLineSpec {
        text: value,
        font_size,
        line_height,
        family: SETTINGS_TITLE_FONT,
        color,
        center: (left + width * 0.5, center_y),
        scale_factor: scale,
    }));
}

#[allow(clippy::too_many_arguments)]
fn push_text_right(
    t: &mut text::TextRenderer,
    quads: &mut Vec<text::GlyphQuad>,
    value: &str,
    right: f32,
    center_y: f32,
    font_size: f32,
    line_height: f32,
    color: [f32; 4],
    scale: f32,
) {
    let width = t.measure_text(&text::CenteredLineSpec {
        text: value,
        font_size,
        line_height,
        family: SETTINGS_TITLE_FONT,
        color,
        center: (0.0, 0.0),
        scale_factor: scale,
    });
    quads.append(&mut t.layout_centered_line(&text::CenteredLineSpec {
        text: value,
        font_size,
        line_height,
        family: SETTINGS_TITLE_FONT,
        color,
        center: (right - width * 0.5, center_y),
        scale_factor: scale,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::SettingsPressTarget;
    use crate::domain::settings::{LiquidGlassDebugFlag, LiquidGlassParamField};
    use crate::layout::hit_map::HitRegion;
    use crate::ui_model::geometry::{ClipRegion, Point};
    use crate::ui_model::hit::{HitTarget, SettingsTarget};
    use crate::ui_model::render_model::{ControlKind, GlassBehavior, GlassMaterial};

    fn test_input() -> layout::settings_panel::SettingsPanelInput {
        let category = layout::settings_panel::SettingsCategoryId::Debug;
        let mut category_selection_amounts =
            [0.0; layout::settings_panel::SettingsCategoryId::COUNT];
        category_selection_amounts[category.index()] = 1.0;
        layout::settings_panel::SettingsPanelInput {
            viewport: (1280, 800),
            scale_factor: 1.0,
            category,
            sort_order: layout::settings_panel::SortOrderId::Name,
            frequent_apps_enabled: false,
            show_steam_apps: true,
            search_includes_hidden: false,
            debug_keys_enabled: false,
            show_fps: false,
            show_auto_darkness_value: false,
            hidden_count: 0,
            progress: 1.0,
            scroll_rows: 0,
            window_decorated: false,
            liquid_glass: layout::settings_panel::LiquidGlassValues::default(),
            liquid_glass_debug: layout::settings_panel::LiquidGlassDebugState::default(),
            pointer_pos: None,
            pointer_pressed: false,
            page_frame_rect: Rect::new(80.0, 60.0, 1120.0, 680.0),
            page_frame_radius: 54.0,
            category_hover_amounts: [0.0; layout::settings_panel::SettingsCategoryId::COUNT],
            category_selection_amounts,
        }
    }

    fn test_cache() -> SettingsRenderCache {
        let clip = Some(ClipRegion::new(Rect::new(0.0, 0.0, 200.0, 200.0), 0.0));
        let ink = |id: &str, y: f32, clip| InkView {
            id: UiId::named(id),
            center: Point::new(10.0, y),
            extent: 4.0,
            opacity: 1.0,
            scene_blur: 0.0,
            stroke: 0.0,
            corner_radius: 0.0,
            color: Color::rgba(1.0, 1.0, 1.0, 1.0),
            kind: ControlKind::Dot,
            z: 0,
            clip,
        };
        let glass = |id: &str, y: f32, clip| GlassSurface {
            id: UiId::named(id),
            rect: Rect::new(0.0, y, 20.0, 20.0),
            radius: 10.0,
            material: GlassMaterial::Regular,
            behavior: GlassBehavior::Control,
            z: 0,
            clip,
            activation: 0.0,
            blur_radius: None,
            backdrop_replacement: 0.0,
            tint: None,
        };
        let glyph = |id: &str, y: f32, clip| GlyphView {
            id: UiId::named(id),
            rect: Rect::new(0.0, y, 10.0, 10.0),
            uv: UvRect::default(),
            color: Color::rgba(1.0, 1.0, 1.0, 1.0),
            z: 0,
            clip,
        };
        let mut fixed_hit = HitRegion::new(
            UiId::named("fixed-hit"),
            Rect::new(0.0, 20.0, 10.0, 10.0),
            HitTarget::settings_action("fixed"),
            0,
        );
        fixed_hit.clip = None;
        let mut scrolling_hit = HitRegion::new(
            UiId::named("scroll-hit"),
            Rect::new(0.0, 30.0, 10.0, 10.0),
            HitTarget::settings_action("scroll"),
            0,
        );
        scrolling_hit.clip = clip;

        SettingsRenderCache {
            key: SettingsRenderCacheKey {
                input: test_input(),
                atlas_dimensions: (512, 512),
            },
            source_scroll_position: 100.0,
            applied_scroll_position: None,
            needs_post_scroll_refresh: false,
            visual_scale: 0.9,
            settings_glass: Vec::new(),
            backdrop: Vec::new(),
            overlay: vec![
                glass("fixed-glass", 20.0, None),
                glass("scroll-glass", 30.0, clip),
            ],
            ink: vec![ink("fixed-ink", 20.0, None), ink("scroll-ink", 30.0, clip)],
            glyphs: vec![
                glyph("fixed-glyph", 20.0, None),
                glyph("scroll-glyph", 30.0, clip),
            ],
            hits: HitMap::with_regions(vec![fixed_hit, scrolling_hit]),
            counts: SettingsRenderCounts {
                overlay_glass: 2,
                settings_glass: 0,
                regions: 2,
                ink: 2,
                glyphs: 2,
                text: 0,
            },
        }
    }

    #[test]
    fn retained_scroll_moves_only_clipped_content() {
        let cache = test_cache();

        let ink = cache.materialize_ink(135.0);
        let glyphs = cache.materialize_glyphs(135.0);
        let overlay = cache.materialize_overlay(135.0);
        let hits = cache.materialize_hits(135.0);

        assert_eq!(ink[0].center.y, 20.0);
        assert_eq!(ink[1].center.y, -1.5);
        assert_eq!(glyphs[0].rect.y, 20.0);
        assert_eq!(glyphs[1].rect.y, -1.5);
        assert_eq!(overlay[0].rect.y, 20.0);
        assert_eq!(overlay[1].rect.y, -5.0);
        assert_eq!(hits.regions()[0].rect.y, 20.0);
        assert_eq!(hits.regions()[1].rect.y, -5.0);
        assert_eq!(hits.regions()[1].clip, cache.hits.regions()[1].clip);
    }

    #[test]
    fn glyph_views_preserve_scroll_view_clip() {
        let quad = text::GlyphQuad {
            x: 1.0,
            y: 2.0,
            w: 3.0,
            h: 4.0,
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
            color: [1.0; 4],
            clip_rect: [10.0, 20.0, 30.0, 40.0],
            clip_radius: [5.0, 0.0, 0.0, 0.0],
        };

        let views = glyph_views(&[quad]);

        assert_eq!(
            views[0].clip,
            Some(ClipRegion::new(Rect::new(10.0, 20.0, 30.0, 40.0), 5.0))
        );
    }

    /// Informational release-profile comparison for the former immediate
    /// frame path versus retained scroll materialization. Timings vary by
    /// machine, so correctness assertions remain structural only.
    #[test]
    fn profile_retained_settings_scroll() {
        use std::hint::black_box;
        use std::time::Instant;

        let input = test_input();
        let hidden_count_label = "0 件";
        let copy = settings_panel_copy(hidden_count_label);
        let mut text = text::TextRenderer::new();

        // Warm font lookup, shaping, and glyph rasterization before timing.
        let mut warm_scroll =
            crate::scroll::ContinuousScroller::new(crate::scroll::ContinuousConfig::default());
        let warm_model = layout::settings_panel::build_with_ui(input, &copy, &mut warm_scroll);
        let warm_key = SettingsRenderCacheKey {
            input,
            atlas_dimensions: text.atlas_dimensions(),
        };
        let _warm = SettingsRenderCache::from_model(
            warm_key,
            warm_scroll.position,
            input.scale_factor,
            warm_model,
            Some(&mut text),
        );

        const REBUILD_RUNS: usize = 50;
        let rebuild_start = Instant::now();
        let mut last_cache = None;
        for _ in 0..REBUILD_RUNS {
            let mut scroll =
                crate::scroll::ContinuousScroller::new(crate::scroll::ContinuousConfig::default());
            let source_scroll = scroll.position;
            let model = layout::settings_panel::build_with_ui(input, &copy, &mut scroll);
            let key = SettingsRenderCacheKey {
                input,
                atlas_dimensions: text.atlas_dimensions(),
            };
            last_cache = Some(SettingsRenderCache::from_model(
                key,
                source_scroll,
                input.scale_factor,
                model,
                Some(&mut text),
            ));
        }
        let rebuild_elapsed = rebuild_start.elapsed();
        let cache = last_cache.expect("profile must produce a cache");
        assert!(!cache.glyphs.is_empty());
        assert!(cache
            .hits
            .regions()
            .iter()
            .any(|region| region.clip.is_some()));

        const RETAINED_RUNS: usize = 1000;
        let retained_start = Instant::now();
        for index in 0..RETAINED_RUNS {
            let position = 80.0 + index as f32 * 0.01;
            black_box(cache.materialize_ink(position));
            black_box(cache.materialize_glyphs(position));
            black_box(cache.materialize_overlay(position));
            black_box(cache.materialize_hits(position));
        }
        let retained_elapsed = retained_start.elapsed();

        eprintln!();
        eprintln!("=== PROFILE: retained settings scroll ===");
        eprintln!(
            "  Immediate rebuild: {:.3} us/frame",
            rebuild_elapsed.as_secs_f64() * 1_000_000.0 / REBUILD_RUNS as f64
        );
        eprintln!(
            "  Retained materialize: {:.3} us/frame",
            retained_elapsed.as_secs_f64() * 1_000_000.0 / RETAINED_RUNS as f64
        );
        eprintln!();
    }

    /// Compare two glass surfaces except for the toggle thumb `activation`
    /// channel (the per-frame hover overlay owns that).
    fn surfaces_eq_ignoring_activation(a: &GlassSurface, b: &GlassSurface) -> bool {
        a.id == b.id
            && a.rect == b.rect
            && a.radius == b.radius
            && a.material == b.material
            && a.behavior == b.behavior
            && a.z == b.z
            && a.clip == b.clip
            && a.blur_radius == b.blur_radius
            && a.backdrop_replacement == b.backdrop_replacement
            && a.tint == b.tint
    }

    /// Order-insensitive comparison of two settings models: every batch must
    /// contain the same surfaces/views (the `Ui` iterates a HashMap, so batch
    /// and element order is not stable across builds).
    fn models_match(
        a: &layout::settings_panel::SettingsPanelModel,
        b: &layout::settings_panel::SettingsPanelModel,
    ) -> bool {
        let mut a_glass: Vec<_> = a.result.render.glass.iter().collect();
        let mut b_glass: Vec<_> = b.result.render.glass.iter().collect();
        a_glass.sort_by_key(|batch| batch.layer as u8);
        b_glass.sort_by_key(|batch| batch.layer as u8);
        if a_glass.len() != b_glass.len() {
            return false;
        }
        for (x, y) in a_glass.iter().zip(b_glass.iter()) {
            if x.layer != y.layer {
                return false;
            }
            let mut xs = x.surfaces.clone();
            let mut ys = y.surfaces.clone();
            xs.sort_by_key(|s| s.id.as_str().to_string());
            ys.sort_by_key(|s| s.id.as_str().to_string());
            if xs.len() != ys.len()
                || xs.iter().zip(ys.iter()).any(|(a, b)| {
                    let toggle_thumb = a.id.as_str().contains("toggle");
                    // The hover/press overlay owns the hovered toggle thumb's
                    // activation and press-scale geometry; every other
                    // surface must be bit-identical across pointer states.
                    if toggle_thumb {
                        !surfaces_eq_ignoring_activation(a, b)
                    } else {
                        a != b
                    }
                })
            {
                return false;
            }
        }
        let mut a_ink: Vec<_> = a.result.render.ink.iter().collect();
        let mut b_ink: Vec<_> = b.result.render.ink.iter().collect();
        a_ink.sort_by_key(|batch| batch.lane as u8);
        b_ink.sort_by_key(|batch| batch.lane as u8);
        if a_ink.len() != b_ink.len() {
            return false;
        }
        for (x, y) in a_ink.iter().zip(b_ink.iter()) {
            if x.lane != y.lane {
                return false;
            }
            let mut xv = x.views.clone();
            let mut yv = y.views.clone();
            xv.sort_by_key(|v| v.id.as_str().to_string());
            yv.sort_by_key(|v| v.id.as_str().to_string());
            if xv != yv {
                return false;
            }
        }
        let mut a_glyphs: Vec<_> = a.result.render.glyphs.iter().collect();
        let mut b_glyphs: Vec<_> = b.result.render.glyphs.iter().collect();
        a_glyphs.sort_by_key(|batch| batch.lane as u8);
        b_glyphs.sort_by_key(|batch| batch.lane as u8);
        if a_glyphs.len() != b_glyphs.len()
            || a_glyphs
                .iter()
                .zip(b_glyphs.iter())
                .any(|(x, y)| x.lane != y.lane || x.views != y.views)
        {
            return false;
        }
        a.result.render.text == b.result.render.text
            && a.result.hits.regions() == b.result.hits.regions()
    }

    /// The settings layout output is bit-identical for every pointer position
    /// (up to the hovered toggle thumb's glass activation, which the per-frame
    /// overlay owns): pointer state never reaches the retained layout, so the
    /// cache key stays valid while the pointer moves.
    #[test]
    fn settings_layout_is_pointer_independent() {
        let mut input = test_input();
        let hidden_count_label = "0 件";
        let copy = settings_panel_copy(hidden_count_label);
        for cat in [
            layout::settings_panel::SettingsCategoryId::Apps,
            layout::settings_panel::SettingsCategoryId::Search,
            layout::settings_panel::SettingsCategoryId::System,
            layout::settings_panel::SettingsCategoryId::Darkness,
            layout::settings_panel::SettingsCategoryId::About,
            layout::settings_panel::SettingsCategoryId::Debug,
        ] {
            input.category = cat;
            let mut scroll =
                crate::scroll::ContinuousScroller::new(crate::scroll::ContinuousConfig::default());
            let base = layout::settings_panel::build_with_ui(input, &copy, &mut scroll);
            for y in (100..700).step_by(80) {
                for x in (100..1180).step_by(80) {
                    input.pointer_pos = Some(Point::new(x as f32, y as f32));
                    // Press is covered by the round-trip overlay test; here
                    // only the hover activation may differ.
                    input.pointer_pressed = false;
                    let mut scroll2 = crate::scroll::ContinuousScroller::new(
                        crate::scroll::ContinuousConfig::default(),
                    );
                    let m = layout::settings_panel::build_with_ui(input, &copy, &mut scroll2);
                    let ok = models_match(&base, &m);
                    assert!(ok, "{cat:?} pointer ({x}, {y}) must not change the layout");
                }
            }
        }
    }

    /// The per-frame hover overlay must reproduce the toggle thumb glass the
    /// live-pointer build produces — hover activation, and the pressed
    /// activation + press-scale growth — bit for bit.
    #[test]
    fn settings_toggle_hover_overlay_matches_live_pointer_build() {
        let input = test_input();
        let hidden_count_label = "0 件";
        let copy = settings_panel_copy(hidden_count_label);
        let overlay_of = |model: &layout::settings_panel::SettingsPanelModel| -> Vec<GlassSurface> {
            model
                .result
                .render
                .glass
                .iter()
                .find(|batch| batch.layer == GlassLayer::Overlay)
                .map(|batch| batch.surfaces.clone())
                .unwrap_or_default()
        };
        fn thumb_of(overlay: &[GlassSurface]) -> &GlassSurface {
            overlay
                .iter()
                .find(|s| s.id.as_str().contains("toggle-debug"))
                .expect("debug category has a toggle-debug thumb")
        }

        // Base layout built without a pointer (the retained-cache state).
        let mut base_scroll =
            crate::scroll::ContinuousScroller::new(crate::scroll::ContinuousConfig::default());
        let base = layout::settings_panel::build_with_ui(input, &copy, &mut base_scroll);
        let base_overlay = overlay_of(&base);

        // Hover: pointer over the toggle-debug row.
        let hover_point = Point::new(940.0, 280.0);
        let mut hovered_overlay = base_overlay.clone();
        apply_settings_toggle_hover(
            &mut hovered_overlay,
            Some(&base.result.hits),
            Some(hover_point),
            false,
        );

        let mut live_input = input;
        live_input.pointer_pos = Some(hover_point);
        live_input.pointer_pressed = false;
        let mut live_scroll =
            crate::scroll::ContinuousScroller::new(crate::scroll::ContinuousConfig::default());
        let live = layout::settings_panel::build_with_ui(live_input, &copy, &mut live_scroll);
        let live_thumb = thumb_of(&overlay_of(&live)).clone();
        assert_eq!(thumb_of(&hovered_overlay), &live_thumb);

        // Pressed: activation 1.0 one-tick value and the press-scale growth.
        let mut pressed_overlay = base_overlay.clone();
        apply_settings_toggle_hover(
            &mut pressed_overlay,
            Some(&base.result.hits),
            Some(hover_point),
            true,
        );
        live_input.pointer_pressed = true;
        let mut pressed_scroll =
            crate::scroll::ContinuousScroller::new(crate::scroll::ContinuousConfig::default());
        let pressed_live =
            layout::settings_panel::build_with_ui(live_input, &copy, &mut pressed_scroll);
        assert_eq!(
            thumb_of(&pressed_overlay),
            thumb_of(&overlay_of(&pressed_live))
        );

        // Pointer over a non-toggle area leaves every thumb untouched.
        let mut idle_overlay = base_overlay.clone();
        apply_settings_toggle_hover(
            &mut idle_overlay,
            Some(&base.result.hits),
            Some(Point::new(600.0, 380.0)),
            false,
        );
        assert_eq!(idle_overlay, base_overlay);
    }

    /// The one-tick spring values reproduce the widget's observed hover/press
    /// thumb activation (0.024000002 hover, 0.16000001 pressed at the default
    /// theme motion).
    #[test]
    fn toggle_one_tick_spring_value_matches_widget_math() {
        assert_eq!(toggle_one_tick_spring_value(0.15), 0.024000002);
        assert_eq!(toggle_one_tick_spring_value(1.0), 0.16000001);
        // Press scale: 1.0 + (1.04 - 1.0) * press_a.
        let press_a = toggle_one_tick_spring_value(1.0);
        assert_eq!(1.0 + 0.04 * press_a, 1.0064);
    }

    #[test]
    fn press_target_from_hit_target_darkness_category() {
        assert_eq!(
            settings_press_target_from_hit_target(&HitTarget::settings_category("darkness")),
            SettingsPressTarget::Category(SettingsCategory::Darkness)
        );
    }

    #[test]
    fn press_target_from_hit_target_toggle_keys() {
        assert_eq!(
            settings_press_target_from_hit_target(&HitTarget::settings_toggle("frequent-apps")),
            SettingsPressTarget::FrequentToggle
        );
        assert_eq!(
            settings_press_target_from_hit_target(&HitTarget::settings_toggle("steam-apps")),
            SettingsPressTarget::SteamToggle
        );
        assert_eq!(
            settings_press_target_from_hit_target(&HitTarget::settings_toggle("search-hidden")),
            SettingsPressTarget::SearchHiddenToggle
        );
        assert_eq!(
            settings_press_target_from_hit_target(&HitTarget::settings_toggle("debug")),
            SettingsPressTarget::DebugToggle
        );
        assert_eq!(
            settings_press_target_from_hit_target(&HitTarget::settings_toggle("show-fps")),
            SettingsPressTarget::FpsToggle
        );
        assert_eq!(
            settings_press_target_from_hit_target(&HitTarget::settings_toggle(
                "show-auto-darkness-value"
            )),
            SettingsPressTarget::AutoDarknessValueToggle
        );
        assert_eq!(
            settings_press_target_from_hit_target(&HitTarget::settings_toggle("lg-enabled")),
            SettingsPressTarget::LiquidGlassEnabled
        );
        assert_eq!(
            settings_press_target_from_hit_target(&HitTarget::settings_toggle("lg-auto-darkness")),
            SettingsPressTarget::LiquidGlassAutoDarkness
        );
        assert_eq!(
            settings_press_target_from_hit_target(&HitTarget::settings_toggle(
                "lg-auto-darkness-mode-fixed-8hz"
            )),
            SettingsPressTarget::LiquidGlassAutoDarknessMode(
                crate::liquid_glass::AutoDarknessUpdateMode::Fixed8Hz
            )
        );
        assert_eq!(
            settings_press_target_from_hit_target(&HitTarget::settings_toggle(
                "window-decorations"
            )),
            SettingsPressTarget::WindowDecorations
        );
    }

    #[test]
    fn press_target_from_hit_target_action_keys() {
        assert_eq!(
            settings_press_target_from_hit_target(&HitTarget::settings_action("reset-cache")),
            SettingsPressTarget::ResetCache
        );
        assert_eq!(
            settings_press_target_from_hit_target(&HitTarget::settings_action("reset-settings")),
            SettingsPressTarget::ResetSettings
        );
        assert_eq!(
            settings_press_target_from_hit_target(&HitTarget::settings_action("lg-reset-all")),
            SettingsPressTarget::LiquidGlassResetAll
        );
    }

    #[test]
    fn press_target_from_hit_target_lg_param_keys() {
        assert_eq!(
            settings_press_target_from_hit_target(&HitTarget::settings_toggle(
                "lg-param-thickness"
            )),
            SettingsPressTarget::LiquidGlassParam(LiquidGlassParamField::Thickness)
        );
        assert_eq!(
            settings_press_target_from_hit_target(&HitTarget::settings_action(
                "lg-param-reset-thickness"
            )),
            SettingsPressTarget::LiquidGlassParamReset(LiquidGlassParamField::Thickness)
        );
        assert_eq!(
            settings_press_target_from_hit_target(&HitTarget::settings_toggle(
                "lg-param-auto-darkness-max"
            )),
            SettingsPressTarget::LiquidGlassParam(LiquidGlassParamField::AutoDarknessMax)
        );
    }

    #[test]
    fn press_target_from_hit_target_lg_debug_keys() {
        assert_eq!(
            settings_press_target_from_hit_target(&HitTarget::settings_toggle(
                "lg-debug-disable-chromatic-aberration"
            )),
            SettingsPressTarget::LiquidGlassDebug(LiquidGlassDebugFlag::DisableChromaticAberration)
        );
    }

    #[test]
    fn press_target_from_hit_target_close_and_panel() {
        assert_eq!(
            settings_press_target_from_hit_target(&HitTarget::Settings {
                target: SettingsTarget::Close
            }),
            SettingsPressTarget::Close
        );
        assert_eq!(
            settings_press_target_from_hit_target(&HitTarget::Settings {
                target: SettingsTarget::Panel
            }),
            SettingsPressTarget::Inside
        );
    }

    #[test]
    fn press_target_from_hit_target_backdrop_is_outside() {
        assert_eq!(
            settings_press_target_from_hit_target(&HitTarget::modal_dismiss_backdrop()),
            SettingsPressTarget::Outside
        );
    }
}

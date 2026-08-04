//! Settings panel render adapter methods and builders.

use crate::domain::settings::{
    LiquidGlassDebugFlag, LiquidGlassParamField, SettingsCategory, SortOrder,
};
use crate::layout;
use crate::renderer::text_engine as text;
use crate::ui_model;
use crate::ui_model::geometry::{Rect, UvRect};
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{Color, GlassLayer, GlyphLane, GlyphView, InkLane, InkView};

use super::helpers::advance_unit_toward;
use crate::app::state::App;

impl App {
    pub(crate) fn render_settings_panel(&mut self) {
        if !self.settings_panel_active() {
            self.render_model
                .set_glass_batch(GlassLayer::Settings, Vec::new());
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
        let hidden_count_label = format!("{hidden_count} 件");
        let copy = settings_panel_copy(&hidden_count_label);
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

        // Current pointer position in logical pixels (for widget hover/press states)
        let pointer_logical = if scale > 0.0 {
            Some(ui_model::geometry::Point::new(
                self.pointer_phys_x,
                self.pointer_phys_y,
            ))
        } else {
            None
        };
        let pointer_pressed = self.pressed_on_settings.is_some()
            || self.pending_press.is_some()
            || self.settings_slider_drag.is_some();
        // Capture the settings_scroll into a local ref for the builder.
        // We use a trick: split borrow of settings_scroll while we still need
        // other fields of self. Build the input first, then pass &mut scroll.
        self.profiler.begin_settings_build();
        let model = {
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
                hidden_count,
                progress: self.settings_panel_progress,
                scroll_rows: self.settings_scroll_rows,
                window_decorated,
                liquid_glass: layout::settings_panel::LiquidGlassValues {
                    enabled: lg.enabled,
                    thickness: lg.thickness,
                    refractive_index: lg.refractive_index,
                    saturation: lg.saturation,
                    adaptive_darkness: lg.adaptive_darkness,
                    chromatic_aberration: lg.chromatic_aberration,
                    blur_radius: lg.blur_radius,
                },
                liquid_glass_debug: lg_debug_state,
                pointer_pos: pointer_logical,
                pointer_pressed,
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
            layout::settings_panel::build_with_ui(input, &copy, &mut self.settings_scroll)
        };
        self.profiler.end_settings_build();

        // Cache the HitMap for next frame's input processing (1-frame delay).
        let clone_start = std::time::Instant::now();
        self.cached_settings_hit_map = Some(model.result.hits.clone());
        self.profiler.record_hitmap_clone(clone_start.elapsed());

        // Record shape/model counts for profiling.
        let overlay_glass = model
            .result
            .render
            .glass
            .iter()
            .find(|b| b.layer == GlassLayer::Overlay)
            .map(|b| b.surfaces.len() as u64)
            .unwrap_or(0);
        let modal_glass = model
            .result
            .render
            .glass
            .iter()
            .find(|b| b.layer == GlassLayer::Settings)
            .map(|b| b.surfaces.len() as u64)
            .unwrap_or(0);
        let ink_count = model
            .result
            .render
            .ink
            .iter()
            .map(|b| b.views.len() as u64)
            .sum::<u64>();
        let glyph_count = model
            .result
            .render
            .glyphs
            .iter()
            .map(|b| b.views.len() as u64)
            .sum::<u64>();
        let text_count = model.result.render.text.len() as u64;
        // Count existing overlay glass that's merged in (from bottom control etc.)
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
        let control_overlay = existing_overlay; // Overlay = control + settings glass
        let region_count = model.result.hits.len() as u64;
        self.profiler.record_counts(
            overlay_glass + existing_overlay,
            modal_glass + existing_modal,
            control_overlay,
            base_count,
            region_count,
            ink_count,
            glyph_count,
            text_count,
        );

        let panel = model.layout;
        let visual_scale = model.visual_scale;
        let visual_alpha = model.visual_alpha;

        // Extract ink instances from the builder's output.
        let mut instances: Vec<InkView> = model
            .result
            .render
            .ink
            .iter()
            .find(|b| b.lane == InkLane::Settings)
            .map(|b| b.views.clone())
            .unwrap_or_default();

        // Text views from the builder → glyph quads.
        let mut quads = Vec::new();
        if let Some(text) = self.text.as_mut() {
            build_settings_panel_text_views(text, &model.result.render.text, scale, &mut quads);
        }

        // Transform for pop animation.
        transform_settings_instances(
            &mut instances,
            [panel.cx, panel.cy],
            visual_scale,
            visual_alpha,
        );
        transform_settings_quads(&mut quads, [panel.cx, panel.cy], visual_scale, visual_alpha);

        // Glass from the builder's output. The panel background lives on the
        // Settings layer; the Liquid Glass toggle thumbs live on the Overlay
        // layer (so they render as independent glass lenses, not a union with
        // the panel capsule). Overlay is merged with whatever the rest of the
        // frame already pushed there (e.g. the bottom control capsule) because
        // `set_glass_batch` replaces per-layer.
        let settings_glass = model
            .result
            .render
            .glass
            .iter()
            .find(|batch| batch.layer == GlassLayer::Settings)
            .map(|batch| batch.surfaces.clone())
            .unwrap_or_default();
        self.render_model
            .set_glass_batch(GlassLayer::Settings, settings_glass);

        let backdrop = model
            .result
            .render
            .ink
            .iter()
            .find(|batch| batch.lane == InkLane::Backdrop)
            .map(|batch| batch.views.clone())
            .unwrap_or_default();
        self.render_model.set_ink_batch(InkLane::Backdrop, backdrop);

        let ui_overlay = model
            .result
            .render
            .glass
            .iter()
            .find(|batch| batch.layer == GlassLayer::Overlay)
            .map(|batch| batch.surfaces.clone())
            .unwrap_or_default();
        if !ui_overlay.is_empty() {
            let existing_overlay = self
                .render_model
                .glass
                .iter()
                .find(|batch| batch.layer == GlassLayer::Overlay)
                .map(|batch| batch.surfaces.clone())
                .unwrap_or_default();
            // Cull only the settings-panel glass (toggle thumbs) that has
            // scrolled fully past the panel AABB. The existing_overlay
            // (bottom control capsule, gear, etc.) is left untouched —
            // the list-virtualization idea applies to our own scrollable
            // content, not to ambient UI that happens to share the Overlay
            // lane.
            let panel_rect = model.layout.rect();
            let ui_overlay: Vec<_> = ui_overlay
                .into_iter()
                .filter(|s| s.rect.intersects(panel_rect))
                .collect();
            let merged: Vec<_> = existing_overlay.into_iter().chain(ui_overlay).collect();
            self.render_model
                .set_glass_batch(GlassLayer::Overlay, merged);
        }
        self.render_model
            .set_ink_batch(InkLane::Settings, instances);
        self.render_model
            .set_glyph_batch(GlyphLane::Settings, glyph_views(&quads));

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

    pub(crate) fn settings_category_hover_amounts(&self) -> [f32; 5] {
        std::array::from_fn(|index| self.settings_category_hover[index].current)
    }

    pub(crate) fn settings_category_selection_amounts(&self) -> [f32; 5] {
        std::array::from_fn(|index| self.settings_category_selection[index].current)
    }

    pub(crate) fn reset_settings_category_hover(&mut self) {
        self.settings_category_hover = [crate::spring_anim::Channel::rest(0.0); 5];
        self.settings_category_hover_elapsed = [0.0; 5];
    }
}

pub(crate) fn settings_category_id(
    category: SettingsCategory,
) -> layout::settings_panel::SettingsCategoryId {
    match category {
        SettingsCategory::Apps => layout::settings_panel::SettingsCategoryId::Apps,
        SettingsCategory::Search => layout::settings_panel::SettingsCategoryId::Search,
        SettingsCategory::System => layout::settings_panel::SettingsCategoryId::System,
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
        debug_lg_thickness_label: "厚み (thickness)",
        debug_lg_refractive_index_label: "屈折率 (refractive index)",
        debug_lg_saturation_label: "彩度 (saturation)",
        debug_lg_adaptive_darkness_label: "白背景の黒さ (adaptive darkness)",
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
            "lg-enabled" => SettingsPressTarget::LiquidGlassEnabled,
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
        "adaptive-darkness" => LiquidGlassParamField::AdaptiveDarkness,
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
            clip: None,
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
    use crate::ui_model::hit::{HitTarget, SettingsTarget};

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
            settings_press_target_from_hit_target(&HitTarget::settings_toggle("lg-enabled")),
            SettingsPressTarget::LiquidGlassEnabled
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

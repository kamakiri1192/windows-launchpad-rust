//! Settings panel render adapter methods and builders.

use crate::domain::settings::{
    LiquidGlassDebugFlag, LiquidGlassParamField, SettingsCategory, SortOrder,
};
use crate::layout;
use crate::layout::settings_panel::{LiquidGlassDebugId, LiquidGlassParamId};
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
                .set_glass_batch(GlassLayer::Modal, Vec::new());
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
        let model = {
            let input = layout::settings_panel::SettingsPanelInput {
                viewport: self.viewport_phys(),
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
                    chromatic_aberration: lg.chromatic_aberration,
                    blur_radius: lg.blur_radius,
                },
                liquid_glass_debug: lg_debug_state,
                pointer_pos: pointer_logical,
                pointer_pressed,
            };
            layout::settings_panel::build_with_ui(input, &copy, &mut self.settings_scroll)
        };
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

        // Glass from the builder's output.
        let modal = model
            .result
            .render
            .glass
            .iter()
            .find(|batch| batch.layer == GlassLayer::Modal)
            .map(|batch| batch.surfaces.clone())
            .unwrap_or_default();
        self.render_model.set_glass_batch(GlassLayer::Modal, modal);
        self.render_model
            .set_ink_batch(InkLane::Settings, instances);
        self.render_model
            .set_glyph_batch(GlyphLane::Settings, glyph_views(&quads));

        if let (Some(renderer), Some(text)) = (self.renderer.as_mut(), self.text.as_ref()) {
            if text.atlas_dirty {
                renderer.upload_atlas(text.atlas_rgba());
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

pub(crate) fn settings_category_from_id(
    category: layout::settings_panel::SettingsCategoryId,
) -> SettingsCategory {
    match category {
        layout::settings_panel::SettingsCategoryId::Apps => SettingsCategory::Apps,
        layout::settings_panel::SettingsCategoryId::Search => SettingsCategory::Search,
        layout::settings_panel::SettingsCategoryId::System => SettingsCategory::System,
        layout::settings_panel::SettingsCategoryId::About => SettingsCategory::About,
        layout::settings_panel::SettingsCategoryId::Debug => SettingsCategory::Debug,
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

pub(crate) fn sort_order_from_id(order: layout::settings_panel::SortOrderId) -> SortOrder {
    match order {
        layout::settings_panel::SortOrderId::Name => SortOrder::Name,
        layout::settings_panel::SortOrderId::Manual => SortOrder::Manual,
        layout::settings_panel::SortOrderId::Recent => SortOrder::Recent,
        layout::settings_panel::SortOrderId::Frequent => SortOrder::Frequent,
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

pub(crate) fn settings_press_target_from_layout_hit(
    hit: layout::settings_panel::SettingsPanelHit,
) -> crate::app::state::SettingsPressTarget {
    match hit {
        layout::settings_panel::SettingsPanelHit::Close => {
            crate::app::state::SettingsPressTarget::Close
        }
        layout::settings_panel::SettingsPanelHit::Category(category) => {
            crate::app::state::SettingsPressTarget::Category(settings_category_from_id(category))
        }
        layout::settings_panel::SettingsPanelHit::Sort(order) => {
            crate::app::state::SettingsPressTarget::Sort(sort_order_from_id(order))
        }
        layout::settings_panel::SettingsPanelHit::FrequentToggle => {
            crate::app::state::SettingsPressTarget::FrequentToggle
        }
        layout::settings_panel::SettingsPanelHit::SteamToggle => {
            crate::app::state::SettingsPressTarget::SteamToggle
        }
        layout::settings_panel::SettingsPanelHit::SearchHiddenToggle => {
            crate::app::state::SettingsPressTarget::SearchHiddenToggle
        }
        layout::settings_panel::SettingsPanelHit::DebugToggle => {
            crate::app::state::SettingsPressTarget::DebugToggle
        }
        layout::settings_panel::SettingsPanelHit::FpsToggle => {
            crate::app::state::SettingsPressTarget::FpsToggle
        }
        layout::settings_panel::SettingsPanelHit::ResetCache => {
            crate::app::state::SettingsPressTarget::ResetCache
        }
        layout::settings_panel::SettingsPanelHit::ResetSettings => {
            crate::app::state::SettingsPressTarget::ResetSettings
        }
        layout::settings_panel::SettingsPanelHit::LiquidGlassEnabled => {
            crate::app::state::SettingsPressTarget::LiquidGlassEnabled
        }
        layout::settings_panel::SettingsPanelHit::LiquidGlassParam(id) => {
            crate::app::state::SettingsPressTarget::LiquidGlassParam(param_field_from_id(id))
        }
        layout::settings_panel::SettingsPanelHit::LiquidGlassParamReset(id) => {
            crate::app::state::SettingsPressTarget::LiquidGlassParamReset(param_field_from_id(id))
        }
        layout::settings_panel::SettingsPanelHit::LiquidGlassResetAll => {
            crate::app::state::SettingsPressTarget::LiquidGlassResetAll
        }
        layout::settings_panel::SettingsPanelHit::LiquidGlassDebug(id) => {
            crate::app::state::SettingsPressTarget::LiquidGlassDebug(debug_flag_from_id(id))
        }
        layout::settings_panel::SettingsPanelHit::WindowDecorations => {
            crate::app::state::SettingsPressTarget::WindowDecorations
        }
        layout::settings_panel::SettingsPanelHit::ScrollUp => {
            crate::app::state::SettingsPressTarget::SettingsScrollUp
        }
        layout::settings_panel::SettingsPanelHit::ScrollDown => {
            crate::app::state::SettingsPressTarget::SettingsScrollDown
        }
        layout::settings_panel::SettingsPanelHit::Inside => {
            crate::app::state::SettingsPressTarget::Inside
        }
        layout::settings_panel::SettingsPanelHit::Outside => {
            crate::app::state::SettingsPressTarget::Outside
        }
    }
}

fn param_field_from_id(id: LiquidGlassParamId) -> LiquidGlassParamField {
    match id {
        LiquidGlassParamId::Thickness => LiquidGlassParamField::Thickness,
        LiquidGlassParamId::RefractiveIndex => LiquidGlassParamField::RefractiveIndex,
        LiquidGlassParamId::Saturation => LiquidGlassParamField::Saturation,
        LiquidGlassParamId::ChromaticAberration => LiquidGlassParamField::ChromaticAberration,
        LiquidGlassParamId::BlurRadius => LiquidGlassParamField::BlurRadius,
    }
}

fn debug_flag_from_id(id: LiquidGlassDebugId) -> LiquidGlassDebugFlag {
    match id {
        LiquidGlassDebugId::DisableChromaticAberration => {
            LiquidGlassDebugFlag::DisableChromaticAberration
        }
        LiquidGlassDebugId::DisableEdgeLighting => LiquidGlassDebugFlag::DisableEdgeLighting,
        LiquidGlassDebugId::DisableBlur => LiquidGlassDebugFlag::DisableBlur,
        LiquidGlassDebugId::ShowBackdropTexture => LiquidGlassDebugFlag::ShowBackdropTexture,
        LiquidGlassDebugId::ShowGeometryTexture => LiquidGlassDebugFlag::ShowGeometryTexture,
        LiquidGlassDebugId::ShowDisplacement => LiquidGlassDebugFlag::ShowDisplacement,
        LiquidGlassDebugId::ShowAlphaMask => LiquidGlassDebugFlag::ShowAlphaMask,
        LiquidGlassDebugId::ShowFinalGlassOnly => LiquidGlassDebugFlag::ShowFinalGlassOnly,
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

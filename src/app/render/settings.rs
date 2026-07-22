//! Settings panel render adapter methods and builders.

use crate::domain::settings::{Settings, SettingsCategory, SortOrder};
use crate::layout;
use crate::renderer::text_engine as text;
use crate::ui_model;
use crate::ui_model::geometry::{Point, Rect, UvRect};
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{
    Color, ControlKind, GlassLayer, GlyphLane, GlyphView, InkLane, InkView,
};

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
        let model = layout::settings_panel::build_with_copy(
            layout::settings_panel::SettingsPanelInput {
                viewport: self.viewport_phys(),
                scale_factor: scale,
                category: settings_category_id(self.settings_category),
                sort_order: sort_order_id(self.settings.sort_order),
                frequent_apps_enabled: self.settings.frequent_apps_enabled,
                show_steam_apps: self.settings.show_steam_apps,
                search_includes_hidden: self.settings.search_includes_hidden,
                hidden_count,
                progress: self.settings_panel_progress,
            },
            &copy,
        );
        let panel = model.layout;
        let visual_scale = model.visual_scale;
        let visual_alpha = model.visual_alpha;

        let btn_r = layout::settings_panel::CLOSE_HALF * scale;
        let close = control_icon(
            panel.left + panel.hw * 2.0 - btn_r * 2.0,
            panel.top + btn_r * 2.0,
            btn_r,
            ControlKind::CloseButton,
            layout::settings_panel::INK,
        );

        let mut instances = Vec::new();
        let mut quads = Vec::new();
        build_settings_panel_instances(
            &panel,
            scale,
            self.settings_category,
            &self.settings,
            hidden_count,
            &mut instances,
        );
        instances.push(close);

        if let Some(text) = self.text.as_mut() {
            build_settings_panel_text_views(text, &model.result.render.text, scale, &mut quads);
        }

        transform_settings_instances(
            &mut instances,
            [panel.cx, panel.cy],
            visual_scale,
            visual_alpha,
        );
        transform_settings_quads(&mut quads, [panel.cx, panel.cy], visual_scale, visual_alpha);

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
        reset_cache_label: "キャッシュをリセット",
        reset_cache_detail: "アイコンを再抽出します",
        reset_settings_label: "設定をリセット",
        reset_settings_detail: "並び順、非表示、設定値を初期状態に戻します",
        version_label: "バージョン",
        version_value: env!("CARGO_PKG_VERSION"),
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
        layout::settings_panel::SettingsPanelHit::ResetCache => {
            crate::app::state::SettingsPressTarget::ResetCache
        }
        layout::settings_panel::SettingsPanelHit::ResetSettings => {
            crate::app::state::SettingsPressTarget::ResetSettings
        }
        layout::settings_panel::SettingsPanelHit::Inside => {
            crate::app::state::SettingsPressTarget::Inside
        }
        layout::settings_panel::SettingsPanelHit::Outside => {
            crate::app::state::SettingsPressTarget::Outside
        }
    }
}

const SETTINGS_TITLE: &str = "設定";
/// Title font for the settings panel.
const SETTINGS_TITLE_FONT: &str = "Yu Gothic UI";
const SETTINGS_SIDEBAR_W: f32 = 210.0;
const SETTINGS_SIDEBAR_TOP: f32 = 78.0;
const SETTINGS_SIDEBAR_ROW_H: f32 = 38.0;
const SETTINGS_SIDEBAR_STEP: f32 = 44.0;
const SETTINGS_CONTENT_PAD: f32 = 34.0;
const SETTINGS_CONTENT_TOP: f32 = 92.0;
const SETTINGS_ROW_H: f32 = 46.0;
const SETTINGS_ROW_STEP: f32 = 62.0;
const SETTINGS_SEGMENT_H: f32 = 32.0;
const SETTINGS_SEGMENT_GAP: f32 = 8.0;
const SETTINGS_INK: [f32; 4] = [1.0, 1.0, 1.0, 0.92];
const SETTINGS_MUTED: [f32; 4] = [1.0, 1.0, 1.0, 0.58];
const SETTINGS_DIM: [f32; 4] = [1.0, 1.0, 1.0, 0.34];
const SETTINGS_ACCENT: [f32; 4] = [0.35, 0.68, 1.0, 0.42];
const SETTINGS_GREEN: [f32; 4] = [0.28, 0.82, 0.48, 0.78];

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

fn control_icon(x: f32, y: f32, radius: f32, kind: ControlKind, color: [f32; 4]) -> InkView {
    ink_view([x, y], radius, color[3], 1.6, 0.0, color, kind)
}

#[allow(clippy::too_many_arguments)]
fn ink_view(
    center: [f32; 2],
    extent: f32,
    opacity: f32,
    stroke: f32,
    corner_radius: f32,
    color: [f32; 4],
    kind: ControlKind,
) -> InkView {
    InkView {
        id: UiId::settings_panel(),
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
        })
        .collect()
}

fn round_rect_instance(
    center: [f32; 2],
    half_width: f32,
    half_height: f32,
    radius: f32,
    color: [f32; 4],
) -> InkView {
    ink_view(
        center,
        half_height,
        color[3],
        half_width,
        radius,
        color,
        ControlKind::RowBackground,
    )
}

fn divider_instance(center: [f32; 2], half_width: f32, half_height: f32) -> InkView {
    round_rect_instance(center, half_width, half_height, half_height, SETTINGS_DIM)
}

fn toggle_instances(center: [f32; 2], enabled: bool, scale: f32, instances: &mut Vec<InkView>) {
    let track_hw = 22.0 * scale;
    let track_hh = 11.0 * scale;
    let track_color = if enabled {
        SETTINGS_GREEN
    } else {
        [1.0, 1.0, 1.0, 0.14]
    };
    instances.push(round_rect_instance(
        center,
        track_hw,
        track_hh,
        track_hh,
        track_color,
    ));
    if enabled {
        instances.push(control_icon(
            center[0] + 10.0 * scale,
            center[1],
            6.0 * scale,
            ControlKind::Dot,
            [1.0, 1.0, 1.0, 0.78],
        ));
    }
}

fn build_settings_panel_instances(
    layout: &crate::layout::settings_panel::SettingsPanelLayout,
    scale: f32,
    category: SettingsCategory,
    settings: &Settings,
    _hidden_count: usize,
    instances: &mut Vec<InkView>,
) {
    let panel_right = layout.left + layout.hw * 2.0;
    let panel_bottom = layout.top + layout.hh * 2.0;
    instances.push(divider_instance(
        [layout.right_left, layout.cy],
        0.55 * scale,
        layout.hh - 28.0 * scale,
    ));

    for (i, item) in SettingsCategory::ALL.iter().copied().enumerate() {
        if item == category {
            let row_top = layout.top
                + SETTINGS_SIDEBAR_TOP * scale
                + i as f32 * SETTINGS_SIDEBAR_STEP * scale;
            instances.push(round_rect_instance(
                [
                    layout.left + layout.sidebar_w * 0.5,
                    row_top + SETTINGS_SIDEBAR_ROW_H * scale * 0.5,
                ],
                layout.sidebar_w * 0.5 - 14.0 * scale,
                SETTINGS_SIDEBAR_ROW_H * scale * 0.5,
                10.0 * scale,
                SETTINGS_ACCENT,
            ));
        }
    }

    let content_left = layout.right_left + SETTINGS_CONTENT_PAD * scale;
    let content_right = panel_right - SETTINGS_CONTENT_PAD * scale;
    let row_w = content_right - content_left;
    let row_h = SETTINGS_ROW_H * scale;
    let first_top = layout.top + SETTINGS_CONTENT_TOP * scale;

    match category {
        SettingsCategory::Apps => {
            let segment_top = first_top + 44.0 * scale;
            let gap = SETTINGS_SEGMENT_GAP * scale;
            let each_w = (row_w - gap * 3.0) / 4.0;
            for (i, order) in SortOrder::ALL.iter().copied().enumerate() {
                let left = content_left + i as f32 * (each_w + gap);
                let selected = settings.sort_order == order;
                instances.push(round_rect_instance(
                    [
                        left + each_w * 0.5,
                        segment_top + SETTINGS_SEGMENT_H * scale * 0.5,
                    ],
                    each_w * 0.5,
                    SETTINGS_SEGMENT_H * scale * 0.5,
                    10.0 * scale,
                    if selected {
                        SETTINGS_ACCENT
                    } else {
                        [1.0, 1.0, 1.0, 0.14]
                    },
                ));
                if selected {
                    instances.push(control_icon(
                        left + 15.0 * scale,
                        segment_top + SETTINGS_SEGMENT_H * scale * 0.5,
                        8.0 * scale,
                        ControlKind::Checkmark,
                        SETTINGS_INK,
                    ));
                }
            }
            settings_row_backgrounds(
                content_left,
                first_top + SETTINGS_ROW_STEP * scale,
                row_w,
                row_h,
                scale,
                instances,
                3,
            );
            toggle_instances(
                [
                    content_right - 28.0 * scale,
                    first_top + SETTINGS_ROW_STEP * scale + row_h * 0.5,
                ],
                settings.frequent_apps_enabled,
                scale,
                instances,
            );
            toggle_instances(
                [
                    content_right - 28.0 * scale,
                    first_top + SETTINGS_ROW_STEP * 2.0 * scale + row_h * 0.5,
                ],
                settings.show_steam_apps,
                scale,
                instances,
            );
            instances.push(control_icon(
                content_right - 14.0 * scale,
                first_top + SETTINGS_ROW_STEP * 3.0 * scale + row_h * 0.5,
                9.0 * scale,
                ControlKind::Chevron,
                SETTINGS_MUTED,
            ));
        }
        SettingsCategory::Search => {
            settings_row_backgrounds(content_left, first_top, row_w, row_h, scale, instances, 1);
            toggle_instances(
                [content_right - 28.0 * scale, first_top + row_h * 0.5],
                settings.search_includes_hidden,
                scale,
                instances,
            );
        }
        SettingsCategory::System => {
            settings_row_backgrounds(content_left, first_top, row_w, row_h, scale, instances, 2);
            for i in 0..2 {
                instances.push(control_icon(
                    content_right - 14.0 * scale,
                    first_top + i as f32 * SETTINGS_ROW_STEP * scale + row_h * 0.5,
                    9.0 * scale,
                    ControlKind::Chevron,
                    SETTINGS_MUTED,
                ));
            }
        }
        SettingsCategory::About => {
            settings_row_backgrounds(content_left, first_top, row_w, row_h, scale, instances, 1);
        }
    }

    instances.push(divider_instance(
        [layout.cx, panel_bottom - 56.0 * scale],
        layout.hw - 26.0 * scale,
        0.45 * scale,
    ));
}

fn settings_row_backgrounds(
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    scale: f32,
    instances: &mut Vec<InkView>,
    count: usize,
) {
    for i in 0..count {
        instances.push(round_rect_instance(
            [
                left + width * 0.5,
                top + i as f32 * SETTINGS_ROW_STEP * scale + height * 0.5,
            ],
            width * 0.5,
            height * 0.5,
            12.0 * scale,
            [1.0, 1.0, 1.0, 0.12],
        ));
    }
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

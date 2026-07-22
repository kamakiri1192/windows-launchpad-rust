use crate::layout::hit_map::{HitMap, HitRegion};
use crate::layout::LayoutResult;
use crate::ui_model::geometry::{Point, Rect};
use crate::ui_model::hit::{HitTarget, SettingsTarget};
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{
    Color, ControlKind, ControlView, GlassBatch, GlassBehavior, GlassLayer, GlassMaterial,
    GlassSurface, InkLane, RenderModel,
};
use crate::ui_model::text::{TextAlign, TextRole, TextStyle, TextView, TextWeight};

#[cfg(target_os = "macos")]
pub const TITLE_FONT: &str = ".SF NS";
#[cfg(not(target_os = "macos"))]
pub const TITLE_FONT: &str = "Yu Gothic UI";
pub const TITLE_SIZE: f32 = 22.0;
pub const TITLE_LINE: f32 = 26.0;
pub const CLOSE_HALF: f32 = 10.0;
/// Half-size of the invisible close-button hit circle. The visible × glyph is
/// only `CLOSE_HALF` (radius 10 logical px), which is too tight to tap/click
/// reliably. We keep the visible size small but enlarge the hit target to the
/// Windows-recommended minimum touch size (diameter 32 px).
pub const CLOSE_HIT_HALF: f32 = 16.0;
pub const HEADER_SIZE: f32 = 21.0;
pub const HEADER_LINE: f32 = 28.0;
pub const LABEL_SIZE: f32 = 14.0;
pub const LABEL_LINE: f32 = 20.0;
pub const DETAIL_SIZE: f32 = 12.0;
pub const DETAIL_LINE: f32 = 18.0;
pub const OPEN_DURATION: f32 = 0.28;
pub const CLOSE_DURATION: f32 = 0.18;

const PANEL_HALF_W: f32 = 380.0;
const PANEL_HALF_H: f32 = 255.0;
const PANEL_RADIUS: f32 = 28.0;
const SIDEBAR_W: f32 = 210.0;
const SIDEBAR_TOP: f32 = 78.0;
const SIDEBAR_ROW_H: f32 = 38.0;
const SIDEBAR_STEP: f32 = 44.0;
const CONTENT_PAD: f32 = 34.0;
const CONTENT_TOP: f32 = 92.0;
const ROW_H: f32 = 46.0;
const ROW_STEP: f32 = 62.0;
const SEGMENT_H: f32 = 32.0;
const SEGMENT_GAP: f32 = 8.0;

pub const INK: [f32; 4] = [1.0, 1.0, 1.0, 0.92];
pub const MUTED: [f32; 4] = [1.0, 1.0, 1.0, 0.58];
pub const DIM: [f32; 4] = [1.0, 1.0, 1.0, 0.34];
pub const ACCENT: [f32; 4] = [0.35, 0.68, 1.0, 0.42];
pub const GREEN: [f32; 4] = [0.28, 0.82, 0.48, 0.78];

const Z_BACKDROP: i16 = 80;
const Z_PANEL: i16 = 90;
const Z_CONTROL: i16 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsCategoryId {
    Apps,
    Search,
    System,
    About,
}

impl SettingsCategoryId {
    pub const ALL: [Self; 4] = [Self::Apps, Self::Search, Self::System, Self::About];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Apps => "apps",
            Self::Search => "search",
            Self::System => "system",
            Self::About => "about",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrderId {
    Name,
    Manual,
    Recent,
    Frequent,
}

impl SortOrderId {
    pub const ALL: [Self; 4] = [Self::Name, Self::Manual, Self::Recent, Self::Frequent];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Manual => "manual",
            Self::Recent => "recent",
            Self::Frequent => "frequent",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPanelHit {
    Close,
    Category(SettingsCategoryId),
    Sort(SortOrderId),
    FrequentToggle,
    SteamToggle,
    SearchHiddenToggle,
    ResetCache,
    ResetSettings,
    Inside,
    Outside,
}

impl SettingsPanelHit {
    pub fn target(self) -> HitTarget {
        match self {
            Self::Close => HitTarget::Settings {
                target: SettingsTarget::Close,
            },
            Self::Category(category) => HitTarget::settings_category(category.key()),
            Self::Sort(order) => HitTarget::settings_sort_option(order.key()),
            Self::FrequentToggle => HitTarget::settings_toggle("frequent-apps"),
            Self::SteamToggle => HitTarget::settings_toggle("steam-apps"),
            Self::SearchHiddenToggle => HitTarget::settings_toggle("search-hidden"),
            Self::ResetCache => HitTarget::settings_action("reset-cache"),
            Self::ResetSettings => HitTarget::settings_action("reset-settings"),
            Self::Inside => HitTarget::Settings {
                target: SettingsTarget::Panel,
            },
            Self::Outside => HitTarget::modal_dismiss_backdrop(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SettingsPanelLayout {
    pub cx: f32,
    pub cy: f32,
    pub hw: f32,
    pub hh: f32,
    pub radius: f32,
    pub left: f32,
    pub top: f32,
    pub sidebar_w: f32,
    pub right_left: f32,
}

impl SettingsPanelLayout {
    pub fn rect(&self) -> Rect {
        Rect::new(self.left, self.top, self.hw * 2.0, self.hh * 2.0)
    }

    pub fn panel_right(&self) -> f32 {
        self.left + self.hw * 2.0
    }

    pub fn panel_bottom(&self) -> f32 {
        self.top + self.hh * 2.0
    }

    pub fn content_left(&self, scale: f32) -> f32 {
        self.right_left + CONTENT_PAD * scale
    }

    pub fn content_right(&self, scale: f32) -> f32 {
        self.panel_right() - CONTENT_PAD * scale
    }

    pub fn first_row_top(&self, scale: f32) -> f32 {
        self.top + CONTENT_TOP * scale
    }

    pub fn row_size(&self, scale: f32) -> (f32, f32) {
        let left = self.content_left(scale);
        (self.content_right(scale) - left, ROW_H * scale)
    }

    pub fn close_center(&self, scale: f32) -> (f32, f32) {
        let button_radius = CLOSE_HALF * scale;
        (
            self.left + self.hw * 2.0 - button_radius * 2.0,
            self.top + button_radius * 2.0,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SettingsPanelInput {
    pub viewport: (u32, u32),
    pub scale_factor: f32,
    /// Fixed page-frame geometry used to clip the Glass Focus Veil.
    pub page_frame_rect: Rect,
    pub page_frame_radius: f32,
    pub category: SettingsCategoryId,
    pub sort_order: SortOrderId,
    pub frequent_apps_enabled: bool,
    pub show_steam_apps: bool,
    pub search_includes_hidden: bool,
    pub hidden_count: usize,
    pub progress: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct SettingsPanelCopy<'a> {
    pub title: &'a str,
    pub categories: [(SettingsCategoryId, &'a str); 4],
    pub sort_orders: [(SortOrderId, &'a str); 4],
    pub sort_label: &'a str,
    pub frequent_apps_label: &'a str,
    pub frequent_apps_detail: &'a str,
    pub steam_apps_label: &'a str,
    pub steam_apps_detail: &'a str,
    pub hidden_apps_label: &'a str,
    pub hidden_count_label: &'a str,
    pub search_hidden_label: &'a str,
    pub search_hidden_detail: &'a str,
    pub reset_cache_label: &'a str,
    pub reset_cache_detail: &'a str,
    pub reset_settings_label: &'a str,
    pub reset_settings_detail: &'a str,
    pub version_label: &'a str,
    pub version_value: &'a str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SettingsPanelModel {
    pub layout: SettingsPanelLayout,
    pub visual_scale: f32,
    pub visual_alpha: f32,
    pub result: LayoutResult,
}

pub fn panel_layout(viewport: (u32, u32), scale_factor: f32) -> SettingsPanelLayout {
    let scale = sanitize_scale(scale_factor);
    let (width, height) = viewport;
    let cx = width as f32 * 0.5;
    let cy = height as f32 * 0.5;
    let hw = PANEL_HALF_W * scale;
    let hh = PANEL_HALF_H * scale;
    let radius = PANEL_RADIUS * scale;
    let left = cx - hw;
    let top = cy - hh;
    let sidebar_w = SIDEBAR_W * scale;

    SettingsPanelLayout {
        cx,
        cy,
        hw,
        hh,
        radius,
        left,
        top,
        sidebar_w,
        right_left: left + sidebar_w,
    }
}

pub fn contains(layout: &SettingsPanelLayout, point: Point) -> bool {
    point.x >= layout.left
        && point.x <= layout.panel_right()
        && point.y >= layout.top
        && point.y <= layout.panel_bottom()
}

pub fn hit_close(layout: &SettingsPanelLayout, scale_factor: f32, point: Point) -> bool {
    let scale = sanitize_scale(scale_factor);
    let hit_radius = CLOSE_HIT_HALF * scale;
    let (button_x, button_y) = layout.close_center(scale);
    let dx = point.x - button_x;
    let dy = point.y - button_y;
    dx * dx + dy * dy <= hit_radius * hit_radius
}

pub fn hit_test(
    layout: &SettingsPanelLayout,
    scale_factor: f32,
    category: SettingsCategoryId,
    point: Point,
) -> SettingsPanelHit {
    let scale = sanitize_scale(scale_factor);
    if !contains(layout, point) {
        return SettingsPanelHit::Outside;
    }
    if hit_close(layout, scale, point) {
        return SettingsPanelHit::Close;
    }

    for (index, category) in SettingsCategoryId::ALL.iter().copied().enumerate() {
        let row_top = layout.top + SIDEBAR_TOP * scale + index as f32 * SIDEBAR_STEP * scale;
        if point.x >= layout.left + 12.0 * scale
            && point.x <= layout.right_left - 12.0 * scale
            && point.y >= row_top
            && point.y <= row_top + SIDEBAR_ROW_H * scale
        {
            return SettingsPanelHit::Category(category);
        }
    }

    if point.x < layout.right_left {
        return SettingsPanelHit::Inside;
    }

    let content_left = layout.content_left(scale);
    let (row_w, row_h) = layout.row_size(scale);
    let first_top = layout.first_row_top(scale);

    match category {
        SettingsCategoryId::Apps => {
            let segment_top = first_top + 44.0 * scale;
            let segment_h = SEGMENT_H * scale;
            if point.y >= segment_top && point.y <= segment_top + segment_h {
                let gap = SEGMENT_GAP * scale;
                let each_w = (row_w - gap * 3.0) / 4.0;
                for (index, order) in SortOrderId::ALL.iter().copied().enumerate() {
                    let left = content_left + index as f32 * (each_w + gap);
                    if point.x >= left && point.x <= left + each_w {
                        return SettingsPanelHit::Sort(order);
                    }
                }
            }
            let frequent_top = first_top + ROW_STEP * scale;
            if point_in_row(point, content_left, frequent_top, row_w, row_h) {
                return SettingsPanelHit::FrequentToggle;
            }
            let hidden_top = first_top + ROW_STEP * 2.0 * scale;
            if point_in_row(point, content_left, hidden_top, row_w, row_h) {
                return SettingsPanelHit::SteamToggle;
            }
            let hidden_top = first_top + ROW_STEP * 3.0 * scale;
            if point_in_row(point, content_left, hidden_top, row_w, row_h) {
                return SettingsPanelHit::Inside;
            }
        }
        SettingsCategoryId::Search => {
            if point_in_row(point, content_left, first_top, row_w, row_h) {
                return SettingsPanelHit::SearchHiddenToggle;
            }
        }
        SettingsCategoryId::System => {
            if point_in_row(point, content_left, first_top, row_w, row_h) {
                return SettingsPanelHit::ResetCache;
            }
            let reset_top = first_top + ROW_STEP * scale;
            if point_in_row(point, content_left, reset_top, row_w, row_h) {
                return SettingsPanelHit::ResetSettings;
            }
        }
        SettingsCategoryId::About => {}
    }

    SettingsPanelHit::Inside
}

pub fn build(input: SettingsPanelInput) -> SettingsPanelModel {
    let hidden_count_label = format!("{} hidden", input.hidden_count);
    let copy = SettingsPanelCopy {
        title: "Settings",
        categories: [
            (SettingsCategoryId::Apps, "Apps"),
            (SettingsCategoryId::Search, "Search"),
            (SettingsCategoryId::System, "System"),
            (SettingsCategoryId::About, "About"),
        ],
        sort_orders: [
            (SortOrderId::Name, "Name"),
            (SortOrderId::Manual, "Manual"),
            (SortOrderId::Recent, "Recent"),
            (SortOrderId::Frequent, "Frequent"),
        ],
        sort_label: "Sort",
        frequent_apps_label: "Frequent apps",
        frequent_apps_detail: "Show frequently used apps on the home screen.",
        steam_apps_label: "Steam apps",
        steam_apps_detail: "Show installed Steam games and applications.",
        hidden_apps_label: "Hidden apps",
        hidden_count_label: &hidden_count_label,
        search_hidden_label: "Include hidden apps in search",
        search_hidden_detail: "Show hidden apps only while searching.",
        reset_cache_label: "Reset cache",
        reset_cache_detail: "Extract icons again.",
        reset_settings_label: "Reset settings",
        reset_settings_detail: "Restore order, hidden apps, and settings.",
        version_label: "Version",
        version_value: env!("CARGO_PKG_VERSION"),
    };
    build_with_copy(input, &copy)
}

pub fn build_with_copy(
    input: SettingsPanelInput,
    copy: &SettingsPanelCopy<'_>,
) -> SettingsPanelModel {
    let scale = sanitize_scale(input.scale_factor);
    let layout = panel_layout(input.viewport, scale);
    let raw_progress = input.progress.clamp(0.0, 1.0);
    let pop = pop_progress(raw_progress);
    let visual_scale = 0.935 + 0.065 * pop;
    let visual_alpha = alpha(raw_progress);
    let mut render = RenderModel::new();
    let mut hits = HitMap::new();

    render.glass.push(GlassBatch {
        layer: GlassLayer::Modal,
        surfaces: vec![GlassSurface {
            id: UiId::settings_panel(),
            rect: scaled_rect_around_center(&layout, visual_scale),
            radius: layout.radius * visual_scale,
            material: GlassMaterial::Prominent,
            behavior: GlassBehavior::Control,
            z: Z_PANEL,
        }],
    });
    render.set_ink_batch(
        InkLane::Backdrop,
        vec![super::focus_veil::view(
            input.page_frame_rect,
            input.page_frame_radius,
            visual_alpha,
        )],
    );

    hits.push(HitRegion::rect_inclusive(
        UiId::backdrop("settings-modal"),
        Rect::new(0.0, 0.0, input.viewport.0 as f32, input.viewport.1 as f32),
        HitTarget::modal_dismiss_backdrop(),
        Z_BACKDROP,
    ));
    hits.push(HitRegion::rect_inclusive(
        UiId::settings_panel(),
        layout.rect(),
        HitTarget::Settings {
            target: SettingsTarget::Panel,
        },
        Z_PANEL,
    ));

    push_static_controls(&mut render, &layout, scale, input);
    push_text_views(&mut render, &layout, scale, input, copy);
    push_hit_regions(&mut hits, &layout, scale, input.category);

    SettingsPanelModel {
        layout,
        visual_scale,
        visual_alpha,
        result: LayoutResult::new(render, hits),
    }
}

pub fn alpha(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn pop_progress(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let inv = t - 1.0;
    1.0 + inv * inv * ((1.45 + 1.0) * inv + 1.45)
}

pub fn row_step(scale_factor: f32) -> f32 {
    ROW_STEP * sanitize_scale(scale_factor)
}

pub fn content_top(scale_factor: f32) -> f32 {
    CONTENT_TOP * sanitize_scale(scale_factor)
}

pub fn segment_h(scale_factor: f32) -> f32 {
    SEGMENT_H * sanitize_scale(scale_factor)
}

pub fn segment_gap(scale_factor: f32) -> f32 {
    SEGMENT_GAP * sanitize_scale(scale_factor)
}

pub fn sidebar_top(scale_factor: f32) -> f32 {
    SIDEBAR_TOP * sanitize_scale(scale_factor)
}

pub fn sidebar_row_h(scale_factor: f32) -> f32 {
    SIDEBAR_ROW_H * sanitize_scale(scale_factor)
}

pub fn sidebar_step(scale_factor: f32) -> f32 {
    SIDEBAR_STEP * sanitize_scale(scale_factor)
}

fn push_static_controls(
    render: &mut RenderModel,
    layout: &SettingsPanelLayout,
    scale: f32,
    input: SettingsPanelInput,
) {
    render.controls.push(ControlView {
        id: UiId::settings_row("sidebar-divider"),
        rect: centered_rect(
            layout.right_left,
            layout.cy,
            1.1 * scale,
            layout.hh * 2.0 - 56.0 * scale,
        ),
        kind: ControlKind::Divider,
        opacity: DIM[3],
        z: Z_CONTROL,
    });

    for (index, category) in SettingsCategoryId::ALL.iter().copied().enumerate() {
        if category == input.category {
            let row_top = layout.top + SIDEBAR_TOP * scale + index as f32 * SIDEBAR_STEP * scale;
            render.controls.push(ControlView {
                id: UiId::settings_row(format!("category-{}", category.key())),
                rect: centered_rect(
                    layout.left + layout.sidebar_w * 0.5,
                    row_top + SIDEBAR_ROW_H * scale * 0.5,
                    layout.sidebar_w - 28.0 * scale,
                    SIDEBAR_ROW_H * scale,
                ),
                kind: ControlKind::RowBackground,
                opacity: ACCENT[3],
                z: Z_CONTROL,
            });
        }
    }

    let (close_x, close_y) = layout.close_center(scale);
    let close_size = CLOSE_HALF * scale * 2.0;
    render.controls.push(ControlView {
        id: UiId::settings_close(),
        rect: centered_rect(close_x, close_y, close_size, close_size),
        kind: ControlKind::CloseButton,
        opacity: INK[3],
        z: Z_CONTROL,
    });
}

fn push_text_views(
    render: &mut RenderModel,
    layout: &SettingsPanelLayout,
    scale: f32,
    input: SettingsPanelInput,
    copy: &SettingsPanelCopy<'_>,
) {
    let content_left = layout.content_left(scale);
    let content_right = layout.content_right(scale);
    let first_top = layout.first_row_top(scale);
    let row_h = ROW_H * scale;

    push_text(
        render,
        "title",
        copy.title,
        layout.left + 24.0 * scale,
        layout.top + 36.0 * scale,
        TITLE_SIZE,
        TITLE_LINE * scale,
        INK,
        TextRole::SettingsTitle,
        TextAlign::Start,
    );

    for (index, (category, label)) in copy.categories.iter().copied().enumerate() {
        let y = layout.top
            + SIDEBAR_TOP * scale
            + index as f32 * SIDEBAR_STEP * scale
            + SIDEBAR_ROW_H * scale * 0.5;
        push_text(
            render,
            format!("category-{}", category.key()),
            label,
            layout.left + 28.0 * scale,
            y,
            LABEL_SIZE,
            LABEL_LINE * scale,
            if category == input.category {
                INK
            } else {
                MUTED
            },
            TextRole::SettingsSidebar,
            TextAlign::Start,
        );
    }

    let category_label = copy
        .categories
        .iter()
        .find_map(|(category, label)| (*category == input.category).then_some(*label))
        .unwrap_or(input.category.key());
    push_text(
        render,
        "category-heading",
        category_label,
        content_left,
        layout.top + 46.0 * scale,
        HEADER_SIZE,
        HEADER_LINE * scale,
        INK,
        TextRole::SettingsHeader,
        TextAlign::Start,
    );

    match input.category {
        SettingsCategoryId::Apps => {
            push_text(
                render,
                "sort-label",
                copy.sort_label,
                content_left,
                first_top + 12.0 * scale,
                LABEL_SIZE,
                LABEL_LINE * scale,
                INK,
                TextRole::SettingsRow,
                TextAlign::Start,
            );

            let gap = SEGMENT_GAP * scale;
            let row_w = content_right - content_left;
            let each_w = (row_w - gap * 3.0) / 4.0;
            let segment_top = first_top + 44.0 * scale;
            for (index, (order, label)) in copy.sort_orders.iter().copied().enumerate() {
                let left = content_left + index as f32 * (each_w + gap);
                let x = if input.sort_order == order {
                    left + 30.0 * scale
                } else {
                    left + 14.0 * scale
                };
                push_text(
                    render,
                    format!("sort-{}", order.key()),
                    label,
                    x,
                    segment_top + SEGMENT_H * scale * 0.5,
                    DETAIL_SIZE,
                    DETAIL_LINE * scale,
                    INK,
                    TextRole::SettingsDetail,
                    TextAlign::Start,
                );
            }

            let frequent_y = first_top + ROW_STEP * scale + row_h * 0.5;
            push_text(
                render,
                "frequent-apps-label",
                copy.frequent_apps_label,
                content_left + 16.0 * scale,
                frequent_y,
                LABEL_SIZE,
                LABEL_LINE * scale,
                INK,
                TextRole::SettingsRow,
                TextAlign::Start,
            );
            push_text(
                render,
                "frequent-apps-detail",
                copy.frequent_apps_detail,
                content_left + 16.0 * scale,
                frequent_y + 16.0 * scale,
                DETAIL_SIZE,
                DETAIL_LINE * scale,
                MUTED,
                TextRole::SettingsDetail,
                TextAlign::Start,
            );

            let steam_y = first_top + ROW_STEP * 2.0 * scale + row_h * 0.5;
            push_text(
                render,
                "steam-apps-label",
                copy.steam_apps_label,
                content_left + 16.0 * scale,
                steam_y,
                LABEL_SIZE,
                LABEL_LINE * scale,
                INK,
                TextRole::SettingsRow,
                TextAlign::Start,
            );
            push_text(
                render,
                "steam-apps-detail",
                copy.steam_apps_detail,
                content_left + 16.0 * scale,
                steam_y + 16.0 * scale,
                DETAIL_SIZE,
                DETAIL_LINE * scale,
                MUTED,
                TextRole::SettingsDetail,
                TextAlign::Start,
            );

            let hidden_y = first_top + ROW_STEP * 3.0 * scale + row_h * 0.5;
            push_text(
                render,
                "hidden-apps-label",
                copy.hidden_apps_label,
                content_left + 16.0 * scale,
                hidden_y,
                LABEL_SIZE,
                LABEL_LINE * scale,
                INK,
                TextRole::SettingsRow,
                TextAlign::Start,
            );
            push_text(
                render,
                "hidden-apps-count",
                copy.hidden_count_label,
                content_right - 32.0 * scale,
                hidden_y,
                DETAIL_SIZE,
                DETAIL_LINE * scale,
                MUTED,
                TextRole::SettingsDetail,
                TextAlign::End,
            );
        }
        SettingsCategoryId::Search => {
            let y = first_top + row_h * 0.5;
            push_text(
                render,
                "search-hidden-label",
                copy.search_hidden_label,
                content_left + 16.0 * scale,
                y,
                LABEL_SIZE,
                LABEL_LINE * scale,
                INK,
                TextRole::SettingsRow,
                TextAlign::Start,
            );
            push_text(
                render,
                "search-hidden-detail",
                copy.search_hidden_detail,
                content_left + 16.0 * scale,
                y + 16.0 * scale,
                DETAIL_SIZE,
                DETAIL_LINE * scale,
                MUTED,
                TextRole::SettingsDetail,
                TextAlign::Start,
            );
        }
        SettingsCategoryId::System => {
            let y0 = first_top + row_h * 0.5;
            push_text(
                render,
                "reset-cache-label",
                copy.reset_cache_label,
                content_left + 16.0 * scale,
                y0,
                LABEL_SIZE,
                LABEL_LINE * scale,
                INK,
                TextRole::SettingsRow,
                TextAlign::Start,
            );
            push_text(
                render,
                "reset-cache-detail",
                copy.reset_cache_detail,
                content_left + 16.0 * scale,
                y0 + 16.0 * scale,
                DETAIL_SIZE,
                DETAIL_LINE * scale,
                MUTED,
                TextRole::SettingsDetail,
                TextAlign::Start,
            );

            let y1 = first_top + ROW_STEP * scale + row_h * 0.5;
            push_text(
                render,
                "reset-settings-label",
                copy.reset_settings_label,
                content_left + 16.0 * scale,
                y1,
                LABEL_SIZE,
                LABEL_LINE * scale,
                INK,
                TextRole::SettingsRow,
                TextAlign::Start,
            );
            push_text(
                render,
                "reset-settings-detail",
                copy.reset_settings_detail,
                content_left + 16.0 * scale,
                y1 + 16.0 * scale,
                DETAIL_SIZE,
                DETAIL_LINE * scale,
                MUTED,
                TextRole::SettingsDetail,
                TextAlign::Start,
            );
        }
        SettingsCategoryId::About => {
            let y = first_top + row_h * 0.5;
            push_text(
                render,
                "version-label",
                copy.version_label,
                content_left + 16.0 * scale,
                y,
                LABEL_SIZE,
                LABEL_LINE * scale,
                INK,
                TextRole::SettingsRow,
                TextAlign::Start,
            );
            push_text(
                render,
                "version-value",
                copy.version_value,
                content_right - 16.0 * scale,
                y,
                LABEL_SIZE,
                LABEL_LINE * scale,
                MUTED,
                TextRole::SettingsDetail,
                TextAlign::End,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_text(
    render: &mut RenderModel,
    id: impl AsRef<str>,
    value: &str,
    anchor_x: f32,
    center_y: f32,
    font_size: f32,
    line_height: f32,
    color: [f32; 4],
    role: TextRole,
    align: TextAlign,
) {
    render.text.push(TextView {
        id: UiId::settings_row(format!("text-{}", id.as_ref())),
        text: value.to_owned(),
        rect: Rect::new(anchor_x, center_y - line_height * 0.5, 0.0, line_height),
        style: TextStyle::new(
            role,
            font_size,
            Color::rgba(color[0], color[1], color[2], color[3]),
            TextWeight::Regular,
            align,
        ),
        z: Z_CONTROL + 1,
    });
}

fn push_hit_regions(
    hits: &mut HitMap,
    layout: &SettingsPanelLayout,
    scale: f32,
    category: SettingsCategoryId,
) {
    let (close_x, close_y) = layout.close_center(scale);
    hits.push(HitRegion::circle(
        UiId::settings_close(),
        Point::new(close_x, close_y),
        CLOSE_HIT_HALF * scale,
        SettingsPanelHit::Close.target(),
        Z_CONTROL + 3,
    ));

    for (index, category) in SettingsCategoryId::ALL.iter().copied().enumerate() {
        let row_top = layout.top + SIDEBAR_TOP * scale + index as f32 * SIDEBAR_STEP * scale;
        hits.push(HitRegion::rect_inclusive(
            UiId::settings_row(format!("category-{}", category.key())),
            Rect::new(
                layout.left + 12.0 * scale,
                row_top,
                layout.sidebar_w - 24.0 * scale,
                SIDEBAR_ROW_H * scale,
            ),
            SettingsPanelHit::Category(category).target(),
            Z_CONTROL + 1,
        ));
    }

    let content_left = layout.content_left(scale);
    let (row_w, row_h) = layout.row_size(scale);
    let first_top = layout.first_row_top(scale);

    match category {
        SettingsCategoryId::Apps => {
            let segment_top = first_top + 44.0 * scale;
            let gap = SEGMENT_GAP * scale;
            let each_w = (row_w - gap * 3.0) / 4.0;
            for (index, order) in SortOrderId::ALL.iter().copied().enumerate() {
                let left = content_left + index as f32 * (each_w + gap);
                hits.push(HitRegion::rect_inclusive(
                    UiId::settings_row(format!("sort-{}", order.key())),
                    Rect::new(left, segment_top, each_w, SEGMENT_H * scale),
                    SettingsPanelHit::Sort(order).target(),
                    Z_CONTROL + 2,
                ));
            }
            hits.push(HitRegion::rect_inclusive(
                UiId::settings_row("toggle-frequent-apps"),
                Rect::new(content_left, first_top + ROW_STEP * scale, row_w, row_h),
                SettingsPanelHit::FrequentToggle.target(),
                Z_CONTROL + 1,
            ));
            hits.push(HitRegion::rect_inclusive(
                UiId::settings_row("toggle-steam-apps"),
                Rect::new(
                    content_left,
                    first_top + ROW_STEP * 2.0 * scale,
                    row_w,
                    row_h,
                ),
                SettingsPanelHit::SteamToggle.target(),
                Z_CONTROL + 1,
            ));
        }
        SettingsCategoryId::Search => {
            hits.push(HitRegion::rect_inclusive(
                UiId::settings_row("toggle-search-hidden"),
                Rect::new(content_left, first_top, row_w, row_h),
                SettingsPanelHit::SearchHiddenToggle.target(),
                Z_CONTROL + 1,
            ));
        }
        SettingsCategoryId::System => {
            hits.push(HitRegion::rect_inclusive(
                UiId::settings_row("reset-cache"),
                Rect::new(content_left, first_top, row_w, row_h),
                SettingsPanelHit::ResetCache.target(),
                Z_CONTROL + 1,
            ));
            hits.push(HitRegion::rect_inclusive(
                UiId::settings_row("reset-settings"),
                Rect::new(content_left, first_top + ROW_STEP * scale, row_w, row_h),
                SettingsPanelHit::ResetSettings.target(),
                Z_CONTROL + 1,
            ));
        }
        SettingsCategoryId::About => {}
    }
}

fn scaled_rect_around_center(layout: &SettingsPanelLayout, scale: f32) -> Rect {
    let width = layout.hw * 2.0 * scale;
    let height = layout.hh * 2.0 * scale;
    Rect::new(
        layout.cx - width * 0.5,
        layout.cy - height * 0.5,
        width,
        height,
    )
}

fn centered_rect(cx: f32, cy: f32, width: f32, height: f32) -> Rect {
    Rect::new(cx - width * 0.5, cy - height * 0.5, width, height)
}

fn point_in_row(point: Point, left: f32, top: f32, width: f32, height: f32) -> bool {
    point.x >= left && point.x <= left + width && point.y >= top && point.y <= top + height
}

fn sanitize_scale(scale_factor: f32) -> f32 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_model::hit::{BackdropKind, HitTarget, SettingsTarget};
    use crate::ui_model::text::{TextAlign, TextRole};

    fn layout() -> SettingsPanelLayout {
        panel_layout((1280, 800), 1.0)
    }

    fn copy<'a>(hidden_count_label: &'a str) -> SettingsPanelCopy<'a> {
        SettingsPanelCopy {
            title: "Settings",
            categories: [
                (SettingsCategoryId::Apps, "Apps"),
                (SettingsCategoryId::Search, "Search"),
                (SettingsCategoryId::System, "System"),
                (SettingsCategoryId::About, "About"),
            ],
            sort_orders: [
                (SortOrderId::Name, "Name"),
                (SortOrderId::Manual, "Manual"),
                (SortOrderId::Recent, "Recent"),
                (SortOrderId::Frequent, "Frequent"),
            ],
            sort_label: "Sort",
            frequent_apps_label: "Frequent apps",
            frequent_apps_detail: "Frequent detail",
            steam_apps_label: "Steam apps",
            steam_apps_detail: "Steam detail",
            hidden_apps_label: "Hidden apps",
            hidden_count_label,
            search_hidden_label: "Search hidden",
            search_hidden_detail: "Search hidden detail",
            reset_cache_label: "Reset cache",
            reset_cache_detail: "Reset cache detail",
            reset_settings_label: "Reset settings",
            reset_settings_detail: "Reset settings detail",
            version_label: "Version",
            version_value: "0.1.0",
        }
    }

    fn input(category: SettingsCategoryId) -> SettingsPanelInput {
        SettingsPanelInput {
            viewport: (1280, 800),
            scale_factor: 1.0,
            page_frame_rect: Rect::new(80.0, 60.0, 1120.0, 680.0),
            page_frame_radius: 54.0,
            category,
            sort_order: SortOrderId::Name,
            frequent_apps_enabled: false,
            show_steam_apps: true,
            search_includes_hidden: false,
            hidden_count: 0,
            progress: 1.0,
        }
    }

    fn assert_hit_map_matches_hit_test(model: &SettingsPanelModel, point: Point) {
        let expected = hit_test(&model.layout, 1.0, SettingsCategoryId::Apps, point).target();
        let actual = model
            .result
            .hits
            .hit_test(point)
            .expect("modeled hit")
            .target
            .clone();

        assert_eq!(actual, expected);
    }

    #[test]
    fn panel_layout_matches_current_centered_geometry() {
        let layout = layout();

        assert_eq!(layout.cx, 640.0);
        assert_eq!(layout.cy, 400.0);
        assert_eq!(layout.hw, 380.0);
        assert_eq!(layout.hh, 255.0);
        assert_eq!(layout.left, 260.0);
        assert_eq!(layout.top, 145.0);
        assert_eq!(layout.right_left, 470.0);
    }

    #[test]
    fn hit_test_distinguishes_modal_outside_from_panel_inside() {
        let layout = layout();

        assert_eq!(
            hit_test(
                &layout,
                1.0,
                SettingsCategoryId::Apps,
                Point::new(100.0, 100.0)
            ),
            SettingsPanelHit::Outside
        );
        assert_eq!(
            hit_test(
                &layout,
                1.0,
                SettingsCategoryId::Apps,
                Point::new(
                    layout.content_left(1.0) + 10.0,
                    layout.first_row_top(1.0) + ROW_STEP * 3.0 + ROW_H * 0.5
                )
            ),
            SettingsPanelHit::Inside
        );
    }

    #[test]
    fn panel_contains_matches_current_inclusive_bounds() {
        let layout = layout();

        assert!(contains(
            &layout,
            Point::new(layout.panel_right(), layout.panel_bottom())
        ));
    }

    #[test]
    fn hit_test_finds_close_button() {
        let layout = layout();
        let (x, y) = layout.close_center(1.0);

        assert_eq!(
            hit_test(&layout, 1.0, SettingsCategoryId::Apps, Point::new(x, y)),
            SettingsPanelHit::Close
        );
    }

    #[test]
    fn hit_close_enlarges_target_beyond_visible_glyph() {
        let layout = layout();
        let (cx, cy) = layout.close_center(1.0);

        // CLOSE_HIT_HALF > CLOSE_HALF by design (invisible slop around the
        // smaller visible glyph). The visible × glyph spans ±CLOSE_HALF.
        // Verify every cardinal point on the glyph boundary is still a hit,
        // then that the slop ring just outside the glyph but inside the hit
        // radius registers as Close.
        let dirs = [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)];
        for (dx, dy) in dirs {
            let on_glyph = Point::new(cx + dx * CLOSE_HALF, cy + dy * CLOSE_HALF);
            assert!(
                hit_close(&layout, 1.0, on_glyph),
                "glyph boundary should hit: ({dx}, {dy})"
            );

            let in_slop = Point::new(
                cx + dx * ((CLOSE_HALF + CLOSE_HIT_HALF) * 0.5),
                cy + dy * ((CLOSE_HALF + CLOSE_HIT_HALF) * 0.5),
            );
            assert!(
                hit_close(&layout, 1.0, in_slop),
                "slop ring should hit: ({dx}, {dy})"
            );

            let beyond = Point::new(
                cx + dx * (CLOSE_HIT_HALF + 0.5),
                cy + dy * (CLOSE_HIT_HALF + 0.5),
            );
            assert!(
                !hit_close(&layout, 1.0, beyond),
                "point beyond hit radius should miss: ({dx}, {dy})"
            );
        }
    }

    #[test]
    fn hit_close_scales_hit_radius_with_dpi() {
        let layout = layout();
        let (cx, cy) = layout.close_center(1.5);

        // At 150% DPI the hit radius grows to CLOSE_HIT_HALF * 1.5 = 24 px.
        // A point 20 px from the center is inside the 24 px hit radius but
        // would be outside a non-scaled glyph (radius 10 * 1.5 = 15 px).
        let point = Point::new(cx + 20.0, cy);
        assert!(hit_close(&layout, 1.5, point));
    }

    #[test]
    fn hit_test_finds_category_rows() {
        let layout = layout();
        let y = layout.top + SIDEBAR_TOP + SIDEBAR_ROW_H * 0.5;

        assert_eq!(
            hit_test(
                &layout,
                1.0,
                SettingsCategoryId::Search,
                Point::new(layout.left + 30.0, y)
            ),
            SettingsPanelHit::Category(SettingsCategoryId::Apps)
        );
    }

    #[test]
    fn hit_test_finds_apps_category_actions() {
        let layout = layout();
        let content_left = layout.content_left(1.0);
        let segment_y = layout.first_row_top(1.0) + 44.0 + SEGMENT_H * 0.5;
        let frequent_y = layout.first_row_top(1.0) + ROW_STEP + ROW_H * 0.5;
        let steam_y = layout.first_row_top(1.0) + ROW_STEP * 2.0 + ROW_H * 0.5;

        assert_eq!(
            hit_test(
                &layout,
                1.0,
                SettingsCategoryId::Apps,
                Point::new(content_left + 10.0, segment_y)
            ),
            SettingsPanelHit::Sort(SortOrderId::Name)
        );
        assert_eq!(
            hit_test(
                &layout,
                1.0,
                SettingsCategoryId::Apps,
                Point::new(content_left + 10.0, frequent_y)
            ),
            SettingsPanelHit::FrequentToggle
        );
        assert_eq!(
            hit_test(
                &layout,
                1.0,
                SettingsCategoryId::Apps,
                Point::new(content_left + 10.0, steam_y)
            ),
            SettingsPanelHit::SteamToggle
        );
    }

    #[test]
    fn hit_test_finds_search_and_system_actions() {
        let layout = layout();
        let x = layout.content_left(1.0) + 10.0;
        let y0 = layout.first_row_top(1.0) + ROW_H * 0.5;
        let y1 = layout.first_row_top(1.0) + ROW_STEP + ROW_H * 0.5;

        assert_eq!(
            hit_test(&layout, 1.0, SettingsCategoryId::Search, Point::new(x, y0)),
            SettingsPanelHit::SearchHiddenToggle
        );
        assert_eq!(
            hit_test(&layout, 1.0, SettingsCategoryId::System, Point::new(x, y0)),
            SettingsPanelHit::ResetCache
        );
        assert_eq!(
            hit_test(&layout, 1.0, SettingsCategoryId::System, Point::new(x, y1)),
            SettingsPanelHit::ResetSettings
        );
    }

    #[test]
    fn model_hit_map_prefers_panel_controls_over_backdrop() {
        let model = build(input(SettingsCategoryId::Apps));
        let (close_x, close_y) = model.layout.close_center(1.0);

        let close_hit = model
            .result
            .hits
            .hit_test(Point::new(close_x, close_y))
            .expect("close hit");
        assert_eq!(
            close_hit.target,
            HitTarget::Settings {
                target: SettingsTarget::Close
            }
        );

        let outside_hit = model
            .result
            .hits
            .hit_test(Point::new(10.0, 10.0))
            .expect("outside hit");
        assert_eq!(
            outside_hit.target,
            HitTarget::Backdrop {
                kind: BackdropKind::ModalDismiss
            }
        );
    }

    #[test]
    fn model_emits_focus_veil_for_settings_progress() {
        let mut input = input(SettingsCategoryId::Apps);
        input.progress = 0.5;
        let model = build(input);
        let veil = &model
            .result
            .render
            .ink
            .iter()
            .find(|batch| batch.lane == InkLane::Backdrop)
            .expect("settings focus veil")
            .views[0];

        assert_eq!(veil.id, UiId::backdrop("glass-focus-veil"));
        assert_eq!(veil.center, Point::new(640.0, 400.0));
        assert_eq!(veil.stroke, 560.0);
        assert_eq!(veil.extent, 340.0);
        assert_eq!(veil.corner_radius, 54.0);
        assert!((veil.opacity - crate::layout::focus_veil::OPACITY * 0.5).abs() < 0.001);
        assert!((veil.scene_blur - 0.5).abs() < 0.001);
    }

    #[test]
    fn settings_panel_uses_prominent_glass_material() {
        let model = build(input(SettingsCategoryId::Apps));
        let panel = &model
            .result
            .render
            .glass
            .iter()
            .find(|batch| batch.layer == GlassLayer::Modal)
            .expect("settings modal glass")
            .surfaces[0];

        assert_eq!(panel.material, GlassMaterial::Prominent);
    }

    #[test]
    fn model_hit_map_uses_circular_close_region() {
        let model = build(input(SettingsCategoryId::Apps));
        let (close_x, close_y) = model.layout.close_center(1.0);

        // Point sits just outside the visible glyph (radius = CLOSE_HALF = 10)
        // but inside the enlarged hit circle (radius = CLOSE_HIT_HALF = 16),
        // so the close target should win thanks to the hit slop.
        let dist = (CLOSE_HALF + 3.0).min(CLOSE_HIT_HALF - 1.0);
        let point = Point::new(close_x + dist, close_y);

        assert_eq!(
            hit_test(&model.layout, 1.0, SettingsCategoryId::Apps, point),
            SettingsPanelHit::Close
        );
        assert_eq!(
            model.result.hits.hit_test(point).expect("close hit").target,
            SettingsPanelHit::Close.target()
        );

        // A point beyond the hit radius falls through to the panel interior.
        let outside_point = Point::new(close_x + CLOSE_HIT_HALF + 1.0, close_y);
        assert_eq!(
            hit_test(&model.layout, 1.0, SettingsCategoryId::Apps, outside_point),
            SettingsPanelHit::Inside
        );
    }

    #[test]
    fn model_hit_map_matches_current_inclusive_edges() {
        let model = build(input(SettingsCategoryId::Apps));
        assert_hit_map_matches_hit_test(
            &model,
            Point::new(model.layout.panel_right(), model.layout.panel_bottom()),
        );

        let row_bottom = model.layout.top + SIDEBAR_TOP + SIDEBAR_ROW_H;
        assert_hit_map_matches_hit_test(
            &model,
            Point::new(model.layout.right_left - 12.0, row_bottom),
        );

        let content_left = model.layout.content_left(1.0);
        let row_w = model.layout.content_right(1.0) - content_left;
        let each_w = (row_w - SEGMENT_GAP * 3.0) / 4.0;
        let segment_top = model.layout.first_row_top(1.0) + 44.0;
        assert_hit_map_matches_hit_test(
            &model,
            Point::new(content_left + each_w, segment_top + SEGMENT_H),
        );
    }

    #[test]
    fn model_emits_settings_text_views_from_layout_positions() {
        let copy = copy("3 hidden");
        let model = build_with_copy(
            SettingsPanelInput {
                viewport: (1280, 800),
                scale_factor: 1.0,
                page_frame_rect: Rect::new(80.0, 60.0, 1120.0, 680.0),
                page_frame_radius: 54.0,
                category: SettingsCategoryId::Apps,
                sort_order: SortOrderId::Manual,
                frequent_apps_enabled: false,
                show_steam_apps: true,
                search_includes_hidden: false,
                hidden_count: 3,
                progress: 1.0,
            },
            &copy,
        );

        let title = model
            .result
            .render
            .text
            .iter()
            .find(|view| view.id.as_str() == "settings-row:text-title")
            .expect("title text");
        assert_eq!(title.text, "Settings");
        assert_eq!(title.style.role, TextRole::SettingsTitle);
        assert_eq!(title.style.align, TextAlign::Start);
        assert_eq!(title.rect.x, model.layout.left + 24.0);

        let manual = model
            .result
            .render
            .text
            .iter()
            .find(|view| view.id.as_str() == "settings-row:text-sort-manual")
            .expect("manual sort text");
        assert_eq!(manual.text, "Manual");
        let row_w = model.layout.content_right(1.0) - model.layout.content_left(1.0);
        let each_w = (row_w - SEGMENT_GAP * 3.0) / 4.0;
        assert_eq!(
            manual.rect.x,
            model.layout.content_left(1.0) + each_w + SEGMENT_GAP + 30.0
        );

        let hidden_count = model
            .result
            .render
            .text
            .iter()
            .find(|view| view.id.as_str() == "settings-row:text-hidden-apps-count")
            .expect("hidden count text");
        assert_eq!(hidden_count.text, "3 hidden");
        assert_eq!(hidden_count.style.align, TextAlign::End);
        assert_eq!(hidden_count.rect.x, model.layout.content_right(1.0) - 32.0);
    }

    #[test]
    fn animation_helpers_match_endpoints() {
        assert_eq!(alpha(0.0), 0.0);
        assert_eq!(alpha(1.0), 1.0);
        assert_eq!(pop_progress(0.0), 0.0);
        assert_eq!(pop_progress(1.0), 1.0);
    }
}

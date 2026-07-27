use crate::layout::hit_map::{HitMap, HitRegion};
use crate::layout::LayoutResult;
use crate::ui_model::geometry::{Point, Rect};
use crate::ui_model::hit::{HitTarget, SettingsTarget};
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{
    Color, ControlKind, ControlView, GlassBatch, GlassBehavior, GlassLayer, GlassMaterial,
    GlassSurface, RenderModel,
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
    Debug,
}

impl SettingsCategoryId {
    pub const ALL: [Self; 5] = [
        Self::Apps,
        Self::Search,
        Self::System,
        Self::About,
        Self::Debug,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Apps => "apps",
            Self::Search => "search",
            Self::System => "system",
            Self::About => "about",
            Self::Debug => "debug",
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
    DebugToggle,
    FpsToggle,
    ResetCache,
    ResetSettings,
    /// Liquid Glass master switch (keyboard `V`).
    LiquidGlassEnabled,
    /// One of the numeric Liquid Glass slider rows.
    LiquidGlassParam(LiquidGlassParamId),
    /// Per-parameter reset arrow on a slider row.
    LiquidGlassParamReset(LiquidGlassParamId),
    /// "Reset Liquid Glass to defaults" button.
    LiquidGlassResetAll,
    /// One of the session-only B/G/D/A/F or C/E/L debug flags.
    LiquidGlassDebug(LiquidGlassDebugId),
    /// Window decorations toggle (keyboard `M`).
    WindowDecorations,
    /// Wheel-up / wheel-down hit regions for content scrolling.
    ScrollUp,
    ScrollDown,
    Inside,
    Outside,
}

/// Layout-side mirror of `domain::settings::LiquidGlassParamField`. Kept
/// separate so this module stays free of the `domain` layer; the App
/// converts between the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiquidGlassParamId {
    Thickness,
    RefractiveIndex,
    Saturation,
    ChromaticAberration,
    BlurRadius,
}

impl LiquidGlassParamId {
    pub const ALL: [Self; 5] = [
        Self::Thickness,
        Self::RefractiveIndex,
        Self::Saturation,
        Self::ChromaticAberration,
        Self::BlurRadius,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Thickness => "thickness",
            Self::RefractiveIndex => "refractive-index",
            Self::Saturation => "saturation",
            Self::ChromaticAberration => "chromatic-aberration",
            Self::BlurRadius => "blur-radius",
        }
    }

    /// `(min, max)` clamp range for the slider, matching the keyboard handler.
    pub const fn range(self) -> (f32, f32) {
        match self {
            Self::Thickness => (6.0, 48.0),
            Self::RefractiveIndex => (1.02, 1.75),
            Self::Saturation => (0.5, 2.0),
            Self::ChromaticAberration => (0.0, 0.18),
            Self::BlurRadius => (0.0, 40.0),
        }
    }
}

/// Layout-side mirror of `domain::settings::LiquidGlassDebugFlag`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiquidGlassDebugId {
    DisableChromaticAberration,
    DisableEdgeLighting,
    DisableBlur,
    ShowBackdropTexture,
    ShowGeometryTexture,
    ShowDisplacement,
    ShowAlphaMask,
    ShowFinalGlassOnly,
}

impl LiquidGlassDebugId {
    pub const fn key(self) -> &'static str {
        match self {
            Self::DisableChromaticAberration => "disable-chromatic-aberration",
            Self::DisableEdgeLighting => "disable-edge-lighting",
            Self::DisableBlur => "disable-blur",
            Self::ShowBackdropTexture => "show-backdrop-texture",
            Self::ShowGeometryTexture => "show-geometry-texture",
            Self::ShowDisplacement => "show-displacement",
            Self::ShowAlphaMask => "show-alpha-mask",
            Self::ShowFinalGlassOnly => "show-final-glass-only",
        }
    }
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
            Self::DebugToggle => HitTarget::settings_toggle("debug"),
            Self::FpsToggle => HitTarget::settings_toggle("show-fps"),
            Self::ResetCache => HitTarget::settings_action("reset-cache"),
            Self::ResetSettings => HitTarget::settings_action("reset-settings"),
            Self::LiquidGlassEnabled => HitTarget::settings_toggle("lg-enabled"),
            Self::LiquidGlassParam(p) => {
                HitTarget::settings_toggle(format!("lg-param-{}", p.key()))
            }
            Self::LiquidGlassParamReset(p) => {
                HitTarget::settings_action(format!("lg-param-reset-{}", p.key()))
            }
            Self::LiquidGlassResetAll => HitTarget::settings_action("lg-reset-all"),
            Self::LiquidGlassDebug(f) => {
                HitTarget::settings_toggle(format!("lg-debug-{}", f.key()))
            }
            Self::WindowDecorations => HitTarget::settings_toggle("window-decorations"),
            Self::ScrollUp => HitTarget::settings_action("scroll-up"),
            Self::ScrollDown => HitTarget::settings_action("scroll-down"),
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
    pub category: SettingsCategoryId,
    pub sort_order: SortOrderId,
    pub frequent_apps_enabled: bool,
    pub show_steam_apps: bool,
    pub search_includes_hidden: bool,
    pub debug_keys_enabled: bool,
    pub show_fps: bool,
    pub hidden_count: usize,
    pub progress: f32,
    /// Vertical scroll offset (in rows) for the Debug category.
    pub scroll_rows: i32,
    /// Window decoration state (M-equivalent). Session-only.
    pub window_decorated: bool,
    /// Liquid Glass persisted snapshot (the six user-facing fields).
    pub liquid_glass: LiquidGlassValues,
    /// Per-flag session state for the B/G/D/A/F and C/E/L toggles, in the
    /// order given by `LiquidGlassDebugId` (disable flags first, then view
    /// overlays). `true` = the flag is currently on.
    pub liquid_glass_debug: LiquidGlassDebugState,
}

/// Persisted Liquid Glass values forwarded from
/// `domain::settings::LiquidGlassSettings`. Layout-side mirror so the layout
/// module stays independent of `domain`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiquidGlassValues {
    pub enabled: bool,
    pub thickness: f32,
    pub refractive_index: f32,
    pub saturation: f32,
    pub chromatic_aberration: f32,
    pub blur_radius: f32,
}

impl Default for LiquidGlassValues {
    fn default() -> Self {
        Self {
            enabled: true,
            thickness: 26.0,
            refractive_index: 1.42,
            saturation: 1.34,
            chromatic_aberration: 0.075,
            blur_radius: 16.0,
        }
    }
}

impl LiquidGlassValues {
    pub fn get(self, id: LiquidGlassParamId) -> f32 {
        match id {
            LiquidGlassParamId::Thickness => self.thickness,
            LiquidGlassParamId::RefractiveIndex => self.refractive_index,
            LiquidGlassParamId::Saturation => self.saturation,
            LiquidGlassParamId::ChromaticAberration => self.chromatic_aberration,
            LiquidGlassParamId::BlurRadius => self.blur_radius,
        }
    }
}

/// Session-only state for the eight Liquid Glass debug toggles, keyed by id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LiquidGlassDebugState {
    pub disable_chromatic_aberration: bool,
    pub disable_edge_lighting: bool,
    pub disable_blur: bool,
    pub show_backdrop_texture: bool,
    pub show_geometry_texture: bool,
    pub show_displacement: bool,
    pub show_alpha_mask: bool,
    pub show_final_glass_only: bool,
}

impl LiquidGlassDebugState {
    pub fn get(self, id: LiquidGlassDebugId) -> bool {
        match id {
            LiquidGlassDebugId::DisableChromaticAberration => self.disable_chromatic_aberration,
            LiquidGlassDebugId::DisableEdgeLighting => self.disable_edge_lighting,
            LiquidGlassDebugId::DisableBlur => self.disable_blur,
            LiquidGlassDebugId::ShowBackdropTexture => self.show_backdrop_texture,
            LiquidGlassDebugId::ShowGeometryTexture => self.show_geometry_texture,
            LiquidGlassDebugId::ShowDisplacement => self.show_displacement,
            LiquidGlassDebugId::ShowAlphaMask => self.show_alpha_mask,
            LiquidGlassDebugId::ShowFinalGlassOnly => self.show_final_glass_only,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SettingsPanelCopy<'a> {
    pub title: &'a str,
    pub categories: [(SettingsCategoryId, &'a str); 5],
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
    pub debug_label: &'a str,
    pub debug_detail: &'a str,
    pub show_fps_label: &'a str,
    pub show_fps_detail: &'a str,
    pub reset_cache_label: &'a str,
    pub reset_cache_detail: &'a str,
    pub reset_settings_label: &'a str,
    pub reset_settings_detail: &'a str,
    pub version_label: &'a str,
    pub version_value: &'a str,
    // ----- Debug-category section headers and row labels -----
    pub debug_section_window: &'a str,
    pub debug_section_liquid_glass: &'a str,
    pub debug_section_debug_views: &'a str,
    pub debug_window_decorations_label: &'a str,
    pub debug_window_decorations_detail: &'a str,
    pub debug_icon_cache_label: &'a str,
    pub debug_icon_cache_detail: &'a str,
    pub debug_lg_enabled_label: &'a str,
    pub debug_lg_enabled_detail: &'a str,
    pub debug_lg_thickness_label: &'a str,
    pub debug_lg_refractive_index_label: &'a str,
    pub debug_lg_saturation_label: &'a str,
    pub debug_lg_chromatic_aberration_label: &'a str,
    pub debug_lg_blur_radius_label: &'a str,
    pub debug_lg_disable_chromatic_aberration_label: &'a str,
    pub debug_lg_disable_edge_lighting_label: &'a str,
    pub debug_lg_disable_blur_label: &'a str,
    pub debug_lg_reset_all_label: &'a str,
    pub debug_lg_reset_all_detail: &'a str,
    pub debug_lg_show_backdrop_texture_label: &'a str,
    pub debug_lg_show_geometry_texture_label: &'a str,
    pub debug_lg_show_displacement_label: &'a str,
    pub debug_lg_show_alpha_mask_label: &'a str,
    pub debug_lg_show_final_glass_only_label: &'a str,
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
    scroll_rows: i32,
    point: Point,
) -> SettingsPanelHit {
    let scale = sanitize_scale(scale_factor);
    let input_debug_scroll = scroll_rows;
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
        SettingsCategoryId::Debug => {
            let scroll = input_debug_scroll;
            // Walk every Debug row; the first visible row whose on-screen Y
            // contains the point wins. Section headers are not interactive.
            // Slider rows additionally expose a reset-arrow hit area on the
            // right and a track hit area covering the rest of the row.
            for i in 0..DEBUG_CATEGORY_ROW_COUNT {
                if !debug_row_is_visible(layout, scale, scroll, i) {
                    continue;
                }
                let row_y = debug_row_y(layout, scale, scroll, i);
                if !point_in_row(point, content_left, row_y, row_w, row_h) {
                    continue;
                }
                return debug_classify_row_hit(layout, scale, i, point);
            }
            // Scroll affordances: a narrow hit strip along the panel's bottom
            // edge scrolls down, and along the top edge (just below the title)
            // scrolls up. Only active when scrolling is possible.
            let max_scroll = debug_category_overflow_rows();
            if max_scroll > 0 {
                let top_strip = first_top - 6.0 * scale;
                let bottom_strip = layout.panel_bottom() - 12.0 * scale;
                if point.x >= content_left
                    && point.x <= layout.content_right(scale)
                    && point.y >= top_strip
                    && point.y < first_top
                {
                    return SettingsPanelHit::ScrollUp;
                }
                if point.x >= content_left
                    && point.x <= layout.content_right(scale)
                    && point.y >= bottom_strip
                    && point.y <= layout.panel_bottom()
                {
                    return SettingsPanelHit::ScrollDown;
                }
            }
        }
        SettingsCategoryId::System => {
            // Row 0: FPS overlay toggle.
            if point_in_row(point, content_left, first_top, row_w, row_h) {
                return SettingsPanelHit::FpsToggle;
            }
            // Row 1: Reset cache action.
            let reset_cache_top = first_top + ROW_STEP * scale;
            if point_in_row(point, content_left, reset_cache_top, row_w, row_h) {
                return SettingsPanelHit::ResetCache;
            }
            // Row 2: Reset settings action.
            let reset_settings_top = first_top + ROW_STEP * scale * 2.0;
            if point_in_row(point, content_left, reset_settings_top, row_w, row_h) {
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
            (SettingsCategoryId::Debug, "Debug"),
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
        debug_label: "Developer shortcuts",
        debug_detail: "Enable single-key debug shortcuts.",
        show_fps_label: "Show FPS",
        show_fps_detail: "Display the frame rate in the top-right corner.",
        reset_cache_label: "Reset cache",
        reset_cache_detail: "Extract icons again.",
        reset_settings_label: "Reset settings",
        reset_settings_detail: "Restore order, hidden apps, and settings.",
        version_label: "Version",
        version_value: env!("CARGO_PKG_VERSION"),
        debug_section_window: "Window",
        debug_section_liquid_glass: "Liquid Glass",
        debug_section_debug_views: "Debug views",
        debug_window_decorations_label: "Window decorations",
        debug_window_decorations_detail: "Show the OS title bar and resize edges.",
        debug_icon_cache_label: "Rebuild icon cache",
        debug_icon_cache_detail: "Re-extract every icon live.",
        debug_lg_enabled_label: "Enable Liquid Glass",
        debug_lg_enabled_detail: "Master switch for the glass effect.",
        debug_lg_thickness_label: "Thickness",
        debug_lg_refractive_index_label: "Refractive index",
        debug_lg_saturation_label: "Saturation",
        debug_lg_chromatic_aberration_label: "Chromatic aberration",
        debug_lg_blur_radius_label: "Blur radius",
        debug_lg_disable_chromatic_aberration_label: "Disable chromatic aberration",
        debug_lg_disable_edge_lighting_label: "Disable edge lighting",
        debug_lg_disable_blur_label: "Disable blur",
        debug_lg_reset_all_label: "Reset Liquid Glass to defaults",
        debug_lg_reset_all_detail: "Restore the coded baseline parameters.",
        debug_lg_show_backdrop_texture_label: "Show backdrop texture",
        debug_lg_show_geometry_texture_label: "Show geometry texture",
        debug_lg_show_displacement_label: "Show displacement",
        debug_lg_show_alpha_mask_label: "Show alpha mask",
        debug_lg_show_final_glass_only_label: "Show final glass only",
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
            material: GlassMaterial::Regular,
            behavior: GlassBehavior::Control,
            z: Z_PANEL,
            clip: None,
        }],
    });

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
    push_hit_regions(&mut hits, &layout, scale, input.category, input.scroll_rows);

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

// ----- Debug-category content layout ----------------------------------
//
// The Debug category has more rows than fit in the fixed panel, so it is the
// only scrollable category. Rows are addressed by a stable index `i` that is
// independent of the scroll offset; the rendering/hit-test layers subtract
// `scroll_rows` to produce the on-screen Y. Section headers are rendered at
// half-row offsets and do not consume a full row slot.
//
// Row index map:
//   0  debug_keys_enabled toggle
//   1  window decorations (M)
//   2  icon cache rebuild (R)
//   3  Liquid Glass enabled (V)
//   4  thickness slider
//   5  refractive_index slider
//   6  saturation slider
//   7  chromatic_aberration slider
//   8  blur_radius slider
//   9  disable chromatic aberration (C)
//  10  disable edge lighting (E)
//  11  disable blur (L)
//  12  "Reset Liquid Glass to defaults" button
//  13  show backdrop texture (B)
//  14  show geometry texture (G)
//  15  show displacement (D)
//  16  show alpha mask (A)
//  17  show final glass only (F)

/// Total number of full-row slots the Debug category uses.
pub const DEBUG_CATEGORY_ROW_COUNT: i32 = 18;

/// Half-row offset (in row units) contributed by the three section headers.
pub const DEBUG_CATEGORY_SECTION_HEADER_ROW_OFFSET: f32 = 1.5;

/// How many rows are visible without scrolling. Matches the fixed panel
/// height: `(PANEL_HALF_H*2 - CONTENT_TOP) / ROW_STEP ≈ 6.7`.
pub const DEBUG_CATEGORY_VISIBLE_ROWS: i32 = 6;

/// Maximum scroll offset (in whole rows) for the Debug category. Returns 0
/// when everything fits.
pub fn debug_category_overflow_rows() -> i32 {
    let total = DEBUG_CATEGORY_ROW_COUNT as f32 + DEBUG_CATEGORY_SECTION_HEADER_ROW_OFFSET;
    let visible = DEBUG_CATEGORY_VISIBLE_ROWS as f32;
    (total - visible).ceil() as i32 - 1
}

/// Y position (in logical content space, *before* the scroll offset is
/// applied) of the top of row `i` within the Debug category.
pub fn debug_row_y_unscrolled(layout: &SettingsPanelLayout, scale: f32, i: i32) -> f32 {
    layout.first_row_top(scale) + i as f32 * row_step(scale)
}

/// On-screen Y of row `i` after applying the scroll offset. Negative values
/// mean the row is scrolled above the visible region (and must be skipped
/// during rendering so it does not leak into the title bar).
pub fn debug_row_y(layout: &SettingsPanelLayout, scale: f32, scroll_rows: i32, i: i32) -> f32 {
    debug_row_y_unscrolled(layout, scale, i) - scroll_rows as f32 * row_step(scale)
}

/// Y position of the section header that sits between row `after` and
/// `after + 1` (e.g. `after = 0` for the first header). Rendered at a
/// half-row offset above row `after + 1`.
pub fn debug_section_header_y(
    layout: &SettingsPanelLayout,
    scale: f32,
    scroll_rows: i32,
    after: i32,
) -> f32 {
    let step = row_step(scale);
    let unscrolled = layout.first_row_top(scale) + (after as f32 + 1.0) * step - step * 0.5;
    unscrolled - scroll_rows as f32 * step
}

/// True when a row's on-screen Y is within the visible content region (i.e.
/// not scrolled out of view). Used to suppress rendering / hit-testing of
/// clipped rows in lieu of a real clip primitive.
pub fn debug_row_is_visible(
    layout: &SettingsPanelLayout,
    scale: f32,
    scroll_rows: i32,
    i: i32,
) -> bool {
    let y = debug_row_y(layout, scale, scroll_rows, i);
    let top = layout.first_row_top(scale) - 2.0;
    let bottom = layout.panel_bottom() - ROW_H * scale;
    y >= top && y <= bottom
}

// ----- Debug-category row index map -----------------------------------

/// Row index of the master debug-keys toggle (`debug_keys_enabled`).
pub const DEBUG_ROW_KEYS: i32 = 0;
/// Row index of the window-decorations toggle (M).
pub const DEBUG_ROW_WINDOW_DECORATIONS: i32 = 1;
/// Row index of the icon-cache rebuild action (R).
pub const DEBUG_ROW_ICON_CACHE: i32 = 2;
/// Row index of the Liquid Glass master toggle (V).
pub const DEBUG_ROW_LG_ENABLED: i32 = 3;
/// First slider row index (thickness). The five parameters occupy rows
/// `DEBUG_ROW_LG_PARAM_FIRST .. DEBUG_ROW_LG_PARAM_FIRST + 5`.
pub const DEBUG_ROW_LG_PARAM_FIRST: i32 = 4;
/// Row index of the "disable chromatic aberration" toggle (C).
pub const DEBUG_ROW_LG_DISABLE_CHROMA: i32 = 9;
/// Row index of the "disable edge lighting" toggle (E).
pub const DEBUG_ROW_LG_DISABLE_EDGE: i32 = 10;
/// Row index of the "disable blur" toggle (L).
pub const DEBUG_ROW_LG_DISABLE_BLUR: i32 = 11;
/// Row index of the "reset Liquid Glass to defaults" button.
pub const DEBUG_ROW_LG_RESET_ALL: i32 = 12;
/// First debug-view row index (show backdrop texture). The five view toggles
/// occupy rows `DEBUG_ROW_LG_VIEW_FIRST .. DEBUG_ROW_LG_VIEW_FIRST + 5`.
pub const DEBUG_ROW_LG_VIEW_FIRST: i32 = 13;

/// Row index for a slider parameter.
pub fn debug_param_row(id: LiquidGlassParamId) -> i32 {
    DEBUG_ROW_LG_PARAM_FIRST
        + match id {
            LiquidGlassParamId::Thickness => 0,
            LiquidGlassParamId::RefractiveIndex => 1,
            LiquidGlassParamId::Saturation => 2,
            LiquidGlassParamId::ChromaticAberration => 3,
            LiquidGlassParamId::BlurRadius => 4,
        }
}

/// Row index for a debug-view toggle (B/G/D/A/F).
pub fn debug_view_row(id: LiquidGlassDebugId) -> i32 {
    match id {
        LiquidGlassDebugId::ShowBackdropTexture => DEBUG_ROW_LG_VIEW_FIRST,
        LiquidGlassDebugId::ShowGeometryTexture => DEBUG_ROW_LG_VIEW_FIRST + 1,
        LiquidGlassDebugId::ShowDisplacement => DEBUG_ROW_LG_VIEW_FIRST + 2,
        LiquidGlassDebugId::ShowAlphaMask => DEBUG_ROW_LG_VIEW_FIRST + 3,
        LiquidGlassDebugId::ShowFinalGlassOnly => DEBUG_ROW_LG_VIEW_FIRST + 4,
        _ => DEBUG_ROW_LG_VIEW_FIRST,
    }
}

/// Slider X geometry for a row: `(track_left, track_width, knob_radius,
/// reset_center_x, reset_radius)`. All in logical px relative to the panel
/// content area. The slider lives in the right half of a row, with the reset
/// arrow just to the right of the track.
pub fn debug_slider_geometry(
    layout: &SettingsPanelLayout,
    scale: f32,
) -> (f32, f32, f32, f32, f32) {
    let content_right = layout.content_right(scale);
    let reset_radius = 9.0 * scale;
    let gap = 10.0 * scale;
    let track_right = content_right - reset_radius * 2.0 - gap;
    let track_width = 120.0 * scale;
    let track_left = track_right - track_width;
    let knob_radius = 7.5 * scale;
    (
        track_left,
        track_width,
        knob_radius,
        content_right - reset_radius,
        reset_radius,
    )
}

/// Slider half-height (the track's vertical radius), in logical px.
pub fn debug_slider_track_half_h(scale: f32) -> f32 {
    2.5 * scale
}

/// Convert a pointer X (logical, content-space) to a slider value for the
/// given parameter id and current Liquid Glass values. Returns the clamped
/// value.
pub fn debug_slider_value_from_pointer(
    layout: &SettingsPanelLayout,
    scale: f32,
    pointer_x: f32,
    id: LiquidGlassParamId,
) -> f32 {
    let (track_left, track_width, _, _, _) = debug_slider_geometry(layout, scale);
    let (min, max) = id.range();
    let t = ((pointer_x - track_left) / track_width).clamp(0.0, 1.0);
    min + (max - min) * t
}

/// Emit a section header text at the half-row offset above row `after + 1`.
/// No-op when the header is scrolled out of view.
fn debug_section_text(
    render: &mut RenderModel,
    layout: &SettingsPanelLayout,
    scale: f32,
    scroll: i32,
    label: &str,
    after: i32,
) {
    let y = debug_section_header_y(layout, scale, scroll, after);
    let top = layout.first_row_top(scale) - 2.0;
    let bottom = layout.panel_bottom() - ROW_H * scale;
    if y < top || y > bottom {
        return;
    }
    let id_str = format!("debug-section-{}", after);
    let id = UiId::settings_row(&id_str);
    render.text.push(TextView {
        id,
        text: label.to_string(),
        rect: Rect::new(
            layout.content_left(scale),
            y,
            layout.content_right(scale) - layout.content_left(scale),
            HEADER_LINE * scale,
        ),
        style: TextStyle {
            role: TextRole::SettingsHeader,
            size: HEADER_SIZE,
            color: Color::rgba(MUTED[0], MUTED[1], MUTED[2], MUTED[3]),
            weight: TextWeight::Medium,
            align: TextAlign::Start,
        },
        z: Z_CONTROL + 1,
        clip: None,
    });
}

/// Classify a hit that landed inside Debug row `i`. Slider rows split into
/// the reset-arrow hit area (right) and the slider track hit area (the rest
/// of the row); all other rows map 1:1 to a hit variant.
pub fn debug_classify_row_hit(
    layout: &SettingsPanelLayout,
    scale: f32,
    i: i32,
    point: Point,
) -> SettingsPanelHit {
    // Slider rows: DEBUG_ROW_LG_PARAM_FIRST .. +5
    if (DEBUG_ROW_LG_PARAM_FIRST..DEBUG_ROW_LG_PARAM_FIRST + 5).contains(&i) {
        let id = LiquidGlassParamId::ALL[(i - DEBUG_ROW_LG_PARAM_FIRST) as usize];
        let (_, _, _, reset_cx, reset_r) = debug_slider_geometry(layout, scale);
        let dx = point.x - reset_cx;
        let row_center_y = debug_row_y(layout, scale, /*scroll=*/ 0, i) + ROW_H * scale * 0.5;
        let dy = point.y - row_center_y;
        if dx * dx + dy * dy <= (reset_r * 1.6) * (reset_r * 1.6) {
            return SettingsPanelHit::LiquidGlassParamReset(id);
        }
        return SettingsPanelHit::LiquidGlassParam(id);
    }
    match i {
        DEBUG_ROW_KEYS => SettingsPanelHit::DebugToggle,
        DEBUG_ROW_WINDOW_DECORATIONS => SettingsPanelHit::WindowDecorations,
        DEBUG_ROW_ICON_CACHE => SettingsPanelHit::ResetCache,
        DEBUG_ROW_LG_ENABLED => SettingsPanelHit::LiquidGlassEnabled,
        DEBUG_ROW_LG_DISABLE_CHROMA => {
            SettingsPanelHit::LiquidGlassDebug(LiquidGlassDebugId::DisableChromaticAberration)
        }
        DEBUG_ROW_LG_DISABLE_EDGE => {
            SettingsPanelHit::LiquidGlassDebug(LiquidGlassDebugId::DisableEdgeLighting)
        }
        DEBUG_ROW_LG_DISABLE_BLUR => {
            SettingsPanelHit::LiquidGlassDebug(LiquidGlassDebugId::DisableBlur)
        }
        DEBUG_ROW_LG_RESET_ALL => SettingsPanelHit::LiquidGlassResetAll,
        other => {
            // Debug-view rows.
            let view_first = DEBUG_ROW_LG_VIEW_FIRST;
            if other >= view_first && other < view_first + 5 {
                let id = [
                    LiquidGlassDebugId::ShowBackdropTexture,
                    LiquidGlassDebugId::ShowGeometryTexture,
                    LiquidGlassDebugId::ShowDisplacement,
                    LiquidGlassDebugId::ShowAlphaMask,
                    LiquidGlassDebugId::ShowFinalGlassOnly,
                ][(other - view_first) as usize];
                SettingsPanelHit::LiquidGlassDebug(id)
            } else {
                SettingsPanelHit::Inside
            }
        }
    }
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
        SettingsCategoryId::Debug => {
            let scroll = input.scroll_rows;
            // Helper closures for this category. Each row is only emitted if
            // it is visible after scrolling, since we have no clip primitive.
            let label_for = |i: i32| -> f32 { debug_row_y(layout, scale, scroll, i) + row_h * 0.5 };
            // Row 0: master debug-keys toggle.
            if debug_row_is_visible(layout, scale, scroll, DEBUG_ROW_KEYS) {
                let y = label_for(DEBUG_ROW_KEYS);
                push_text(
                    render,
                    "debug-label",
                    copy.debug_label,
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
                    "debug-detail",
                    copy.debug_detail,
                    content_left + 16.0 * scale,
                    y + 16.0 * scale,
                    DETAIL_SIZE,
                    DETAIL_LINE * scale,
                    MUTED,
                    TextRole::SettingsDetail,
                    TextAlign::Start,
                );
            }
            // Section: Window
            debug_section_text(render, layout, scale, scroll, copy.debug_section_window, 0);
            if debug_row_is_visible(layout, scale, scroll, DEBUG_ROW_WINDOW_DECORATIONS) {
                let y = label_for(DEBUG_ROW_WINDOW_DECORATIONS);
                push_text(
                    render,
                    "debug-window-decorations-label",
                    copy.debug_window_decorations_label,
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
                    "debug-window-decorations-detail",
                    copy.debug_window_decorations_detail,
                    content_left + 16.0 * scale,
                    y + 16.0 * scale,
                    DETAIL_SIZE,
                    DETAIL_LINE * scale,
                    MUTED,
                    TextRole::SettingsDetail,
                    TextAlign::Start,
                );
            }
            if debug_row_is_visible(layout, scale, scroll, DEBUG_ROW_ICON_CACHE) {
                let y = label_for(DEBUG_ROW_ICON_CACHE);
                push_text(
                    render,
                    "debug-icon-cache-label",
                    copy.debug_icon_cache_label,
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
                    "debug-icon-cache-detail",
                    copy.debug_icon_cache_detail,
                    content_left + 16.0 * scale,
                    y + 16.0 * scale,
                    DETAIL_SIZE,
                    DETAIL_LINE * scale,
                    MUTED,
                    TextRole::SettingsDetail,
                    TextAlign::Start,
                );
            }
            // Section: Liquid Glass
            debug_section_text(
                render,
                layout,
                scale,
                scroll,
                copy.debug_section_liquid_glass,
                2,
            );
            if debug_row_is_visible(layout, scale, scroll, DEBUG_ROW_LG_ENABLED) {
                let y = label_for(DEBUG_ROW_LG_ENABLED);
                push_text(
                    render,
                    "debug-lg-enabled-label",
                    copy.debug_lg_enabled_label,
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
                    "debug-lg-enabled-detail",
                    copy.debug_lg_enabled_detail,
                    content_left + 16.0 * scale,
                    y + 16.0 * scale,
                    DETAIL_SIZE,
                    DETAIL_LINE * scale,
                    MUTED,
                    TextRole::SettingsDetail,
                    TextAlign::Start,
                );
            }
            // Slider rows: label on the left, current value on the right of the label.
            let slider_labels = [
                copy.debug_lg_thickness_label,
                copy.debug_lg_refractive_index_label,
                copy.debug_lg_saturation_label,
                copy.debug_lg_chromatic_aberration_label,
                copy.debug_lg_blur_radius_label,
            ];
            for (k, id) in LiquidGlassParamId::ALL.iter().copied().enumerate() {
                let i = DEBUG_ROW_LG_PARAM_FIRST + k as i32;
                if !debug_row_is_visible(layout, scale, scroll, i) {
                    continue;
                }
                let y = label_for(i);
                push_text(
                    render,
                    format!("debug-lg-param-{}-label", id.key()),
                    slider_labels[k],
                    content_left + 16.0 * scale,
                    y,
                    LABEL_SIZE,
                    LABEL_LINE * scale,
                    INK,
                    TextRole::SettingsRow,
                    TextAlign::Start,
                );
                let value = input.liquid_glass.get(id);
                let value_text = format!("{:.3}", value);
                let (track_left, track_width, _, _, _) = debug_slider_geometry(layout, scale);
                push_text(
                    render,
                    format!("debug-lg-param-{}-value", id.key()),
                    &value_text,
                    track_left - 8.0 * scale,
                    y,
                    LABEL_SIZE,
                    LABEL_LINE * scale,
                    MUTED,
                    TextRole::SettingsRow,
                    TextAlign::End,
                );
                let _ = track_width;
            }
            // Disable-flag toggles (C/E/L).
            let disable_rows = [
                (
                    DEBUG_ROW_LG_DISABLE_CHROMA,
                    copy.debug_lg_disable_chromatic_aberration_label,
                ),
                (
                    DEBUG_ROW_LG_DISABLE_EDGE,
                    copy.debug_lg_disable_edge_lighting_label,
                ),
                (DEBUG_ROW_LG_DISABLE_BLUR, copy.debug_lg_disable_blur_label),
            ];
            for (i, label) in disable_rows {
                if debug_row_is_visible(layout, scale, scroll, i) {
                    let y = label_for(i);
                    push_text(
                        render,
                        format!("debug-lg-disable-{}", i),
                        label,
                        content_left + 16.0 * scale,
                        y,
                        LABEL_SIZE,
                        LABEL_LINE * scale,
                        INK,
                        TextRole::SettingsRow,
                        TextAlign::Start,
                    );
                }
            }
            // Reset-all button.
            if debug_row_is_visible(layout, scale, scroll, DEBUG_ROW_LG_RESET_ALL) {
                let y = label_for(DEBUG_ROW_LG_RESET_ALL);
                push_text(
                    render,
                    "debug-lg-reset-all-label",
                    copy.debug_lg_reset_all_label,
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
                    "debug-lg-reset-all-detail",
                    copy.debug_lg_reset_all_detail,
                    content_left + 16.0 * scale,
                    y + 16.0 * scale,
                    DETAIL_SIZE,
                    DETAIL_LINE * scale,
                    MUTED,
                    TextRole::SettingsDetail,
                    TextAlign::Start,
                );
            }
            // Section: Debug views
            debug_section_text(
                render,
                layout,
                scale,
                scroll,
                copy.debug_section_debug_views,
                12,
            );
            let view_rows = [
                (
                    DEBUG_ROW_LG_VIEW_FIRST,
                    copy.debug_lg_show_backdrop_texture_label,
                ),
                (
                    DEBUG_ROW_LG_VIEW_FIRST + 1,
                    copy.debug_lg_show_geometry_texture_label,
                ),
                (
                    DEBUG_ROW_LG_VIEW_FIRST + 2,
                    copy.debug_lg_show_displacement_label,
                ),
                (
                    DEBUG_ROW_LG_VIEW_FIRST + 3,
                    copy.debug_lg_show_alpha_mask_label,
                ),
                (
                    DEBUG_ROW_LG_VIEW_FIRST + 4,
                    copy.debug_lg_show_final_glass_only_label,
                ),
            ];
            for (i, label) in view_rows {
                if debug_row_is_visible(layout, scale, scroll, i) {
                    let y = label_for(i);
                    push_text(
                        render,
                        format!("debug-lg-view-{}", i),
                        label,
                        content_left + 16.0 * scale,
                        y,
                        LABEL_SIZE,
                        LABEL_LINE * scale,
                        INK,
                        TextRole::SettingsRow,
                        TextAlign::Start,
                    );
                }
            }
        }
        SettingsCategoryId::System => {
            // Row 0: FPS overlay toggle.
            let y0 = first_top + row_h * 0.5;
            push_text(
                render,
                "show-fps-label",
                copy.show_fps_label,
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
                "show-fps-detail",
                copy.show_fps_detail,
                content_left + 16.0 * scale,
                y0 + 16.0 * scale,
                DETAIL_SIZE,
                DETAIL_LINE * scale,
                MUTED,
                TextRole::SettingsDetail,
                TextAlign::Start,
            );

            // Row 1: Reset cache action.
            let y1 = first_top + ROW_STEP * scale + row_h * 0.5;
            push_text(
                render,
                "reset-cache-label",
                copy.reset_cache_label,
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
                "reset-cache-detail",
                copy.reset_cache_detail,
                content_left + 16.0 * scale,
                y1 + 16.0 * scale,
                DETAIL_SIZE,
                DETAIL_LINE * scale,
                MUTED,
                TextRole::SettingsDetail,
                TextAlign::Start,
            );

            // Row 2: Reset settings action.
            let y2 = first_top + ROW_STEP * scale * 2.0 + row_h * 0.5;
            push_text(
                render,
                "reset-settings-label",
                copy.reset_settings_label,
                content_left + 16.0 * scale,
                y2,
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
                y2 + 16.0 * scale,
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
        clip: None,
    });
}

fn push_hit_regions(
    hits: &mut HitMap,
    layout: &SettingsPanelLayout,
    scale: f32,
    category: SettingsCategoryId,
    scroll_rows: i32,
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
        SettingsCategoryId::Debug => {
            // Register one rect per visible Debug row. Slider rows get a
            // single covering rect (the track); their inner reset-arrow
            // sub-region is resolved at hit-time via `hit_test`.
            for i in 0..DEBUG_CATEGORY_ROW_COUNT {
                if !debug_row_is_visible(layout, scale, scroll_rows, i) {
                    continue;
                }
                let row_y = debug_row_y(layout, scale, scroll_rows, i);
                let target = debug_classify_row_hit(
                    layout,
                    scale,
                    i,
                    Point::new(content_left + 1.0, row_y + row_h * 0.5),
                )
                .target();
                hits.push(HitRegion::rect_inclusive(
                    UiId::settings_row(format!("debug-row-{}", i)),
                    Rect::new(content_left, row_y, row_w, row_h),
                    target,
                    Z_CONTROL + 1,
                ));
            }
            // Scroll affordances.
            let max_scroll = debug_category_overflow_rows();
            if max_scroll > 0 {
                let top_strip_y = first_top - 6.0 * scale;
                let bottom_strip_y = layout.panel_bottom() - 12.0 * scale;
                hits.push(HitRegion::rect_inclusive(
                    UiId::settings_row("debug-scroll-up"),
                    Rect::new(content_left, top_strip_y, row_w, 6.0 * scale),
                    SettingsPanelHit::ScrollUp.target(),
                    Z_CONTROL + 1,
                ));
                hits.push(HitRegion::rect_inclusive(
                    UiId::settings_row("debug-scroll-down"),
                    Rect::new(content_left, bottom_strip_y, row_w, 12.0 * scale),
                    SettingsPanelHit::ScrollDown.target(),
                    Z_CONTROL + 1,
                ));
            }
        }
        SettingsCategoryId::System => {
            hits.push(HitRegion::rect_inclusive(
                UiId::settings_row("toggle-show-fps"),
                Rect::new(content_left, first_top, row_w, row_h),
                SettingsPanelHit::FpsToggle.target(),
                Z_CONTROL + 1,
            ));
            hits.push(HitRegion::rect_inclusive(
                UiId::settings_row("reset-cache"),
                Rect::new(content_left, first_top + ROW_STEP * scale, row_w, row_h),
                SettingsPanelHit::ResetCache.target(),
                Z_CONTROL + 1,
            ));
            hits.push(HitRegion::rect_inclusive(
                UiId::settings_row("reset-settings"),
                Rect::new(
                    content_left,
                    first_top + ROW_STEP * scale * 2.0,
                    row_w,
                    row_h,
                ),
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
                (SettingsCategoryId::Debug, "Debug"),
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
            debug_label: "Developer shortcuts",
            debug_detail: "Debug detail",
            show_fps_label: "Show FPS",
            show_fps_detail: "Show FPS detail",
            reset_cache_label: "Reset cache",
            reset_cache_detail: "Reset cache detail",
            reset_settings_label: "Reset settings",
            reset_settings_detail: "Reset settings detail",
            version_label: "Version",
            version_value: "0.1.0",
            debug_section_window: "Window",
            debug_section_liquid_glass: "Liquid Glass",
            debug_section_debug_views: "Debug views",
            debug_window_decorations_label: "Window decorations",
            debug_window_decorations_detail: "Window detail",
            debug_icon_cache_label: "Rebuild icon cache",
            debug_icon_cache_detail: "Cache detail",
            debug_lg_enabled_label: "Enable Liquid Glass",
            debug_lg_enabled_detail: "LG detail",
            debug_lg_thickness_label: "Thickness",
            debug_lg_refractive_index_label: "Refractive index",
            debug_lg_saturation_label: "Saturation",
            debug_lg_chromatic_aberration_label: "Chromatic aberration",
            debug_lg_blur_radius_label: "Blur radius",
            debug_lg_disable_chromatic_aberration_label: "Disable chromatic aberration",
            debug_lg_disable_edge_lighting_label: "Disable edge lighting",
            debug_lg_disable_blur_label: "Disable blur",
            debug_lg_reset_all_label: "Reset Liquid Glass to defaults",
            debug_lg_reset_all_detail: "Reset detail",
            debug_lg_show_backdrop_texture_label: "Show backdrop texture",
            debug_lg_show_geometry_texture_label: "Show geometry texture",
            debug_lg_show_displacement_label: "Show displacement",
            debug_lg_show_alpha_mask_label: "Show alpha mask",
            debug_lg_show_final_glass_only_label: "Show final glass only",
        }
    }

    fn input(category: SettingsCategoryId) -> SettingsPanelInput {
        SettingsPanelInput {
            viewport: (1280, 800),
            scale_factor: 1.0,
            category,
            sort_order: SortOrderId::Name,
            frequent_apps_enabled: false,
            show_steam_apps: true,
            search_includes_hidden: false,
            debug_keys_enabled: false,
            show_fps: false,
            hidden_count: 0,
            progress: 1.0,
            scroll_rows: 0,
            window_decorated: false,
            liquid_glass: LiquidGlassValues::default(),
            liquid_glass_debug: LiquidGlassDebugState::default(),
        }
    }

    fn assert_hit_map_matches_hit_test(model: &SettingsPanelModel, point: Point) {
        let expected = hit_test(&model.layout, 1.0, SettingsCategoryId::Apps, 0, point).target();
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
                0,
                Point::new(100.0, 100.0)
            ),
            SettingsPanelHit::Outside
        );
        assert_eq!(
            hit_test(
                &layout,
                1.0,
                SettingsCategoryId::Apps,
                0,
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
            hit_test(&layout, 1.0, SettingsCategoryId::Apps, 0, Point::new(x, y)),
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
                0,
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
                0,
                Point::new(content_left + 10.0, segment_y)
            ),
            SettingsPanelHit::Sort(SortOrderId::Name)
        );
        assert_eq!(
            hit_test(
                &layout,
                1.0,
                SettingsCategoryId::Apps,
                0,
                Point::new(content_left + 10.0, frequent_y)
            ),
            SettingsPanelHit::FrequentToggle
        );
        assert_eq!(
            hit_test(
                &layout,
                1.0,
                SettingsCategoryId::Apps,
                0,
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
        let y2 = layout.first_row_top(1.0) + ROW_STEP * 2.0 + ROW_H * 0.5;

        assert_eq!(
            hit_test(
                &layout,
                1.0,
                SettingsCategoryId::Search,
                0,
                Point::new(x, y0)
            ),
            SettingsPanelHit::SearchHiddenToggle
        );
        assert_eq!(
            hit_test(
                &layout,
                1.0,
                SettingsCategoryId::System,
                0,
                Point::new(x, y0)
            ),
            SettingsPanelHit::FpsToggle
        );
        assert_eq!(
            hit_test(
                &layout,
                1.0,
                SettingsCategoryId::System,
                0,
                Point::new(x, y1)
            ),
            SettingsPanelHit::ResetCache
        );
        assert_eq!(
            hit_test(
                &layout,
                1.0,
                SettingsCategoryId::System,
                0,
                Point::new(x, y2)
            ),
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
    fn model_hit_map_uses_circular_close_region() {
        let model = build(input(SettingsCategoryId::Apps));
        let (close_x, close_y) = model.layout.close_center(1.0);

        // Point sits just outside the visible glyph (radius = CLOSE_HALF = 10)
        // but inside the enlarged hit circle (radius = CLOSE_HIT_HALF = 16),
        // so the close target should win thanks to the hit slop.
        let dist = (CLOSE_HALF + 3.0).min(CLOSE_HIT_HALF - 1.0);
        let point = Point::new(close_x + dist, close_y);

        assert_eq!(
            hit_test(&model.layout, 1.0, SettingsCategoryId::Apps, 0, point),
            SettingsPanelHit::Close
        );
        assert_eq!(
            model.result.hits.hit_test(point).expect("close hit").target,
            SettingsPanelHit::Close.target()
        );

        // A point beyond the hit radius falls through to the panel interior.
        let outside_point = Point::new(close_x + CLOSE_HIT_HALF + 1.0, close_y);
        assert_eq!(
            hit_test(
                &model.layout,
                1.0,
                SettingsCategoryId::Apps,
                0,
                outside_point
            ),
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
                category: SettingsCategoryId::Apps,
                sort_order: SortOrderId::Manual,
                frequent_apps_enabled: false,
                show_steam_apps: true,
                search_includes_hidden: false,
                debug_keys_enabled: false,
                show_fps: false,
                hidden_count: 3,
                progress: 1.0,
                scroll_rows: 0,
                window_decorated: false,
                liquid_glass: LiquidGlassValues::default(),
                liquid_glass_debug: LiquidGlassDebugState::default(),
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

    // ----- Debug-category layout / hit-test (issue #112) -----

    #[test]
    fn debug_overflow_rows_is_positive() {
        // The Debug category has more rows than fit, so scrolling must be
        // possible. Sanity-check the magnitude is in a reasonable range.
        let n = debug_category_overflow_rows();
        assert!(n > 0, "Debug category should overflow, got {n}");
        assert!(n < DEBUG_CATEGORY_ROW_COUNT, "overflow exceeds row count");
    }

    #[test]
    fn debug_row_y_advances_with_scroll() {
        let layout = layout();
        let scale = 1.0;
        let y_top = debug_row_y(&layout, scale, 0, 0);
        let y_scrolled = debug_row_y(&layout, scale, 3, 0);
        // Scrolling by 3 rows moves the row up by 3 * row_step.
        assert_eq!(y_top - y_scrolled, 3.0 * ROW_STEP);
    }

    #[test]
    fn debug_row_visibility_respects_scroll() {
        let layout = layout();
        let scale = 1.0;
        // Row 0 is visible at scroll 0, hidden when scrolled well past it.
        assert!(debug_row_is_visible(&layout, scale, 0, 0));
        assert!(!debug_row_is_visible(&layout, scale, 10, 0));
    }

    #[test]
    fn hit_test_debug_master_toggle_at_row_zero() {
        let layout = layout();
        let x = layout.content_left(1.0) + 10.0;
        let y = layout.first_row_top(1.0) + ROW_H * 0.5;
        assert_eq!(
            hit_test(&layout, 1.0, SettingsCategoryId::Debug, 0, Point::new(x, y)),
            SettingsPanelHit::DebugToggle
        );
    }

    #[test]
    fn hit_test_debug_lg_enabled_toggle() {
        let layout = layout();
        let x = layout.content_left(1.0) + 10.0;
        let y = debug_row_y(&layout, 1.0, 0, DEBUG_ROW_LG_ENABLED) + ROW_H * 0.5;
        assert_eq!(
            hit_test(&layout, 1.0, SettingsCategoryId::Debug, 0, Point::new(x, y)),
            SettingsPanelHit::LiquidGlassEnabled
        );
    }

    #[test]
    fn hit_test_debug_slider_row_resolves_to_param() {
        let layout = layout();
        // Click the left part of the thickness slider row → track hit.
        let row_y = debug_row_y(&layout, 1.0, 0, DEBUG_ROW_LG_PARAM_FIRST) + ROW_H * 0.5;
        let (track_left, _, _, _, _) = debug_slider_geometry(&layout, 1.0);
        let x = track_left + 5.0;
        assert_eq!(
            hit_test(
                &layout,
                1.0,
                SettingsCategoryId::Debug,
                0,
                Point::new(x, row_y)
            ),
            SettingsPanelHit::LiquidGlassParam(LiquidGlassParamId::Thickness)
        );
    }

    #[test]
    fn hit_test_debug_slider_reset_arrow() {
        let layout = layout();
        let row_y = debug_row_y(&layout, 1.0, 0, DEBUG_ROW_LG_PARAM_FIRST) + ROW_H * 0.5;
        let (_, _, _, reset_cx, _) = debug_slider_geometry(&layout, 1.0);
        assert_eq!(
            hit_test(
                &layout,
                1.0,
                SettingsCategoryId::Debug,
                0,
                Point::new(reset_cx, row_y)
            ),
            SettingsPanelHit::LiquidGlassParamReset(LiquidGlassParamId::Thickness)
        );
    }

    #[test]
    fn hit_test_debug_reset_all_button() {
        let layout = layout();
        // Row 12 only becomes visible once scrolled; at scroll 0 it is below
        // the fold and would be clipped.
        let scroll = 6;
        let x = layout.content_left(1.0) + 10.0;
        let y = debug_row_y(&layout, 1.0, scroll, DEBUG_ROW_LG_RESET_ALL) + ROW_H * 0.5;
        assert_eq!(
            hit_test(
                &layout,
                1.0,
                SettingsCategoryId::Debug,
                scroll,
                Point::new(x, y)
            ),
            SettingsPanelHit::LiquidGlassResetAll
        );
    }

    #[test]
    fn hit_test_debug_disabled_rows_when_scrolled() {
        let layout = layout();
        // Row 0 scrolled out of view: a click at its old Y must not resolve
        // to DebugToggle (it falls through to Inside / Outside / next row).
        let x = layout.content_left(1.0) + 10.0;
        // Scroll so row 0 is above the viewport.
        let big_scroll = DEBUG_CATEGORY_ROW_COUNT;
        let y_row0 = debug_row_y(&layout, 1.0, big_scroll, 0) + ROW_H * 0.5;
        let hit = hit_test(
            &layout,
            1.0,
            SettingsCategoryId::Debug,
            big_scroll,
            Point::new(x, y_row0),
        );
        // Row 0's old position is now off-panel (negative Y) → Outside.
        assert_eq!(hit, SettingsPanelHit::Outside);
    }

    #[test]
    fn slider_value_from_pointer_is_clamped() {
        let layout = layout();
        let (track_left, track_width, _, _, _) = debug_slider_geometry(&layout, 1.0);
        let (min, max) = LiquidGlassParamId::Thickness.range();
        // Pointer far left of the track → min value.
        let v_left = debug_slider_value_from_pointer(
            &layout,
            1.0,
            track_left - 50.0,
            LiquidGlassParamId::Thickness,
        );
        assert_eq!(v_left, min);
        // Pointer far right of the track → max value.
        let v_right = debug_slider_value_from_pointer(
            &layout,
            1.0,
            track_left + track_width + 50.0,
            LiquidGlassParamId::Thickness,
        );
        assert_eq!(v_right, max);
        // Pointer at the midpoint → midpoint value (within float tolerance).
        let v_mid = debug_slider_value_from_pointer(
            &layout,
            1.0,
            track_left + track_width * 0.5,
            LiquidGlassParamId::Thickness,
        );
        assert!((v_mid - (min + max) * 0.5).abs() < 1e-3);
    }

    #[test]
    fn debug_param_and_view_row_indices_are_stable() {
        // Ensure the row-index helpers agree with the constants used in the
        // layout code (a regression guard against accidental renumbering).
        assert_eq!(
            debug_param_row(LiquidGlassParamId::Thickness),
            DEBUG_ROW_LG_PARAM_FIRST
        );
        assert_eq!(
            debug_param_row(LiquidGlassParamId::BlurRadius),
            DEBUG_ROW_LG_PARAM_FIRST + 4
        );
        assert_eq!(
            debug_view_row(LiquidGlassDebugId::ShowBackdropTexture),
            DEBUG_ROW_LG_VIEW_FIRST
        );
        assert_eq!(
            debug_view_row(LiquidGlassDebugId::ShowFinalGlassOnly),
            DEBUG_ROW_LG_VIEW_FIRST + 4
        );
    }
}

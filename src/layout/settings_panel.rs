use crate::layout::context_menu::{
    CONTEXT_MENU_BASE_BLUR, CONTEXT_MENU_TINT_ALPHA, FOCUS_ROW_OPACITY, FOCUS_ROW_VERTICAL_INSET,
    MENU_LABEL_RGB, SYSTEM_BLUE_RGB,
};
use crate::layout::hit_map::HitRegion;
use crate::layout::LayoutResult;
use crate::scroll::ContinuousScroller;
use crate::ui::context::Ui;
use crate::ui::theme::Theme;
use crate::ui::widgets::color_from_array;
use crate::ui::widgets::{Button, ButtonStyle, IconButton, Label, Slider, Toggle};
use crate::ui_model::geometry::{Insets, Point, Rect};
use crate::ui_model::hit::{HitTarget, SettingsTarget};
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{
    Color, ControlKind, GlassBehavior, GlassLayer, GlassMaterial, GlassSurface, InkLane, InkView,
};
use crate::ui_model::text::{TextAlign, TextRole, TextStyle, TextWeight};

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

pub const INK: [f32; 4] = [
    MENU_LABEL_RGB[0],
    MENU_LABEL_RGB[1],
    MENU_LABEL_RGB[2],
    0.92,
];
pub const MUTED: [f32; 4] = [
    MENU_LABEL_RGB[0],
    MENU_LABEL_RGB[1],
    MENU_LABEL_RGB[2],
    0.58,
];
pub const DIM: [f32; 4] = [
    MENU_LABEL_RGB[0],
    MENU_LABEL_RGB[1],
    MENU_LABEL_RGB[2],
    0.34,
];
/// Neutral hover treatment for sidebar categories. Selection uses the
/// context-menu system blue below, so hover remains distinguishable from the
/// currently selected category while keeping the same animated pill geometry.
const SETTINGS_HOVER_ROW_RGB: [f32; 3] = MENU_LABEL_RGB;
const SETTINGS_SELECTED_ROW_RGB: [f32; 3] = SYSTEM_BLUE_RGB;
/// Strong enough to read as selection while retaining the glass surface below.
const SETTINGS_SELECTED_ROW_OPACITY: f32 = 0.28;
pub const ACCENT: [f32; 4] = [
    SYSTEM_BLUE_RGB[0],
    SYSTEM_BLUE_RGB[1],
    SYSTEM_BLUE_RGB[2],
    0.20,
];
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

    pub const fn index(self) -> usize {
        match self {
            Self::Apps => 0,
            Self::Search => 1,
            Self::System => 2,
            Self::About => 3,
            Self::Debug => 4,
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
    AdaptiveDarkness,
    ChromaticAberration,
    BlurRadius,
}

impl LiquidGlassParamId {
    pub const ALL: [Self; 6] = [
        Self::Thickness,
        Self::RefractiveIndex,
        Self::Saturation,
        Self::AdaptiveDarkness,
        Self::ChromaticAberration,
        Self::BlurRadius,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Thickness => "thickness",
            Self::RefractiveIndex => "refractive-index",
            Self::Saturation => "saturation",
            Self::AdaptiveDarkness => "adaptive-darkness",
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
            Self::AdaptiveDarkness => (0.0, 1.0),
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
    /// Deprecated: Phase 4 migrates to pixel-based ContinuousScroller.
    pub scroll_rows: i32,
    /// Window decoration state (M-equivalent). Session-only.
    pub window_decorated: bool,
    /// Liquid Glass persisted snapshot (the seven user-facing fields).
    pub liquid_glass: LiquidGlassValues,
    /// Per-flag session state for the B/G/D/A/F and C/E/L toggles, in the
    /// order given by `LiquidGlassDebugId` (disable flags first, then view
    /// overlays). `true` = the flag is currently on.
    pub liquid_glass_debug: LiquidGlassDebugState,
    /// Current pointer position in logical pixels (for widget hover/press).
    pub pointer_pos: Option<Point>,
    /// Whether the primary pointer button is currently pressed.
    pub pointer_pressed: bool,
    /// Physical page-frame geometry used by the Glass Focus Veil outside the
    /// settings surface.
    pub page_frame_rect: Rect,
    pub page_frame_radius: f32,
    /// Per-category hover amounts, animated by the app shell with the same
    /// easing used by context-menu rows.
    pub category_hover_amounts: [f32; 5],
    /// Per-category selection amounts, animated independently so switching
    /// categories fades the blue selection pill between rows.
    pub category_selection_amounts: [f32; 5],
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
    pub adaptive_darkness: f32,
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
            adaptive_darkness: 0.65,
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
            LiquidGlassParamId::AdaptiveDarkness => self.adaptive_darkness,
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
    pub debug_lg_adaptive_darkness_label: &'a str,
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

/// Build the settings panel using the new Liquid Glass UI foundation
/// (Phase 4). This replaces the manual coordinate-calculation approach of
/// `build_with_copy` with an immediate-mode `Ui` context and widgets.
///
/// `scroll` is the Debug-category's continuous scroller (owned by `App`).
/// The output `SettingsPanelModel` is compatible with the existing app-side
/// rendering adapter, which transforms the ink/text views by `visual_scale`
/// and `visual_alpha` for the pop animation.
pub fn build_with_ui(
    input: SettingsPanelInput,
    copy: &SettingsPanelCopy<'_>,
    scroll: &mut ContinuousScroller,
) -> SettingsPanelModel {
    let scale = sanitize_scale(input.scale_factor);
    let layout = panel_layout(input.viewport, scale);
    let raw_progress = input.progress.clamp(0.0, 1.0);
    let pop = pop_progress(raw_progress);
    let visual_scale = 0.935 + 0.065 * pop;
    let visual_alpha = alpha(raw_progress);

    let theme = Theme {
        scale_factor: scale,
        ..Default::default()
    };
    let mut ui = Ui::new(theme, input.viewport.0 as f32, input.viewport.1 as f32);
    ui.set_pointer(input.pointer_pos);
    ui.set_pointer_pressed(input.pointer_pressed);

    // ------------------------------------------------------------------
    // Panel glass background (scaled for pop animation)
    // ------------------------------------------------------------------
    ui.push_glass(
        GlassLayer::Settings,
        GlassSurface {
            id: UiId::settings_panel(),
            rect: scaled_rect_around_center(&layout, visual_scale),
            radius: layout.radius * visual_scale,
            material: GlassMaterial::Regular,
            behavior: GlassBehavior::Control,
            z: Z_PANEL,
            clip: None,
            activation: 0.0,
            blur_radius: Some(CONTEXT_MENU_BASE_BLUR),
            backdrop_replacement: visual_alpha,
            tint: Some(Color::rgba(
                0.93,
                0.94,
                0.96,
                CONTEXT_MENU_TINT_ALPHA * visual_alpha,
            )),
        },
    );

    // ------------------------------------------------------------------
    // Glass Focus Veil outside the settings surface
    // ------------------------------------------------------------------
    let page_frame_radius = input
        .page_frame_radius
        .max(0.0)
        .min(input.page_frame_rect.width * 0.5)
        .min(input.page_frame_rect.height * 0.5);
    ui.push_ink_with_lane(
        InkLane::Backdrop,
        InkView {
            id: UiId::backdrop("glass-focus-veil"),
            center: input.page_frame_rect.center(),
            extent: input.page_frame_rect.height * 0.5,
            opacity: crate::layout::GLASS_FOCUS_VEIL_OPACITY * raw_progress,
            scene_blur: raw_progress,
            stroke: input.page_frame_rect.width * 0.5,
            corner_radius: page_frame_radius,
            color: Color::rgba(0.12, 0.15, 0.20, 1.0),
            kind: ControlKind::RowBackground,
            z: Z_BACKDROP,
            clip: None,
        },
    );

    // ------------------------------------------------------------------
    // Backdrop + panel hit regions
    // ------------------------------------------------------------------
    ui.push_hit(HitRegion::rect_inclusive(
        UiId::backdrop("settings-modal"),
        Rect::new(0.0, 0.0, input.viewport.0 as f32, input.viewport.1 as f32),
        HitTarget::modal_dismiss_backdrop(),
        Z_BACKDROP,
    ));
    ui.push_hit(HitRegion::rect_inclusive(
        UiId::settings_panel(),
        layout.rect(),
        HitTarget::Settings {
            target: SettingsTarget::Panel,
        },
        Z_PANEL,
    ));

    // ------------------------------------------------------------------
    // Title
    // ------------------------------------------------------------------
    ui.begin_absolute_placement();
    ui.set_cursor(layout.left + 24.0 * scale, layout.top + 18.0 * scale);
    ui.set_available_width(layout.hw * 2.0 - 48.0 * scale);
    ui.label(
        &Label::new(copy.title)
            .id(UiId::settings_row("text-title"))
            .style(TextStyle::new(
                TextRole::SettingsTitle,
                TITLE_SIZE,
                color_from_array(INK),
                TextWeight::Regular,
                TextAlign::Start,
            ))
            .color(INK),
    );

    // ------------------------------------------------------------------
    // Close button
    // ------------------------------------------------------------------
    let (close_cx, close_cy) = layout.close_center(scale);
    let close_hit_r = CLOSE_HIT_HALF * scale;
    ui.begin_absolute_placement();
    ui.set_cursor(close_cx - close_hit_r, close_cy - close_hit_r);
    ui.icon_button(
        &IconButton::new(ControlKind::CloseButton)
            .id(UiId::settings_close())
            .visual_radius(CLOSE_HALF)
            .hit_radius(CLOSE_HIT_HALF)
            .tint(INK)
            .hit_target(SettingsPanelHit::Close.target()),
    );

    // ------------------------------------------------------------------
    // Sidebar vertical divider (InkView directly — Divider widget is
    // horizontal only)
    // ------------------------------------------------------------------
    let sidebar_div = InkView {
        id: UiId::settings_row("sidebar-divider"),
        center: Point::new(layout.right_left, layout.cy),
        extent: layout.hh - 28.0 * scale,
        opacity: DIM[3],
        scene_blur: 0.0,
        stroke: 0.55 * scale,
        corner_radius: 0.55 * scale,
        color: color_from_array(DIM),
        kind: ControlKind::Divider,
        z: Z_CONTROL,
        clip: None,
    };
    ui.push_ink(sidebar_div);

    // ------------------------------------------------------------------
    // Sidebar category buttons
    // ------------------------------------------------------------------
    for (index, (cat_id, label)) in copy.categories.iter().copied().enumerate() {
        let row_top = layout.top + SIDEBAR_TOP * scale + index as f32 * SIDEBAR_STEP * scale;
        let sidebar_w = layout.sidebar_w - 24.0 * scale;
        let row_rect = Rect::new(
            layout.left + 12.0 * scale,
            row_top,
            sidebar_w,
            SIDEBAR_ROW_H * scale,
        );
        let hover_amount = input.category_hover_amounts[cat_id.index()].clamp(0.0, 1.0);
        let selection_amount = input.category_selection_amounts[cat_id.index()].clamp(0.0, 1.0);
        let focus_amount = hover_amount.max(selection_amount).clamp(0.0, 1.0);
        let row_color = [
            SETTINGS_HOVER_ROW_RGB[0]
                + (SETTINGS_SELECTED_ROW_RGB[0] - SETTINGS_HOVER_ROW_RGB[0]) * selection_amount,
            SETTINGS_HOVER_ROW_RGB[1]
                + (SETTINGS_SELECTED_ROW_RGB[1] - SETTINGS_HOVER_ROW_RGB[1]) * selection_amount,
            SETTINGS_HOVER_ROW_RGB[2]
                + (SETTINGS_SELECTED_ROW_RGB[2] - SETTINGS_HOVER_ROW_RGB[2]) * selection_amount,
        ];
        let vertical_inset = (FOCUS_ROW_VERTICAL_INSET * scale).min(row_rect.height * 0.5);
        let focus_rect = row_rect.inset(Insets::symmetric(0.0, vertical_inset));
        let focus_scale = 0.96 + 0.04 * focus_amount;
        let focus_center = focus_rect.center();
        let focus_rect = Rect::new(
            focus_center.x - focus_rect.width * focus_scale * 0.5,
            focus_center.y - focus_rect.height * focus_scale * 0.5,
            focus_rect.width * focus_scale,
            focus_rect.height * focus_scale,
        );
        ui.push_ink(InkView {
            id: UiId::settings_row(format!("category-focus-{}", cat_id.key())),
            center: focus_rect.center(),
            extent: focus_rect.height * 0.5,
            opacity: (SETTINGS_SELECTED_ROW_OPACITY * selection_amount)
                .max(FOCUS_ROW_OPACITY * hover_amount),
            scene_blur: 0.0,
            stroke: focus_rect.width * 0.5,
            corner_radius: focus_rect.height * 0.5,
            color: Color::rgba(row_color[0], row_color[1], row_color[2], 1.0),
            kind: ControlKind::RowBackground,
            z: Z_CONTROL,
            clip: None,
        });
        let id = UiId::settings_row(format!("category-{}", cat_id.key()));
        ui.push_hit(HitRegion::new(
            id.clone(),
            row_rect,
            SettingsPanelHit::Category(cat_id).target(),
            Z_CONTROL + 2,
        ));
        ui.begin_absolute_placement();
        ui.set_cursor(
            row_rect.x + 16.0 * scale,
            row_top + (SIDEBAR_ROW_H - LABEL_SIZE) * 0.5 * scale,
        );
        ui.set_available_width(sidebar_w);
        ui.label(
            &Label::new(label)
                .id(id)
                .style(TextStyle::new(
                    TextRole::SettingsRow,
                    LABEL_SIZE,
                    Color::rgba(MENU_LABEL_RGB[0], MENU_LABEL_RGB[1], MENU_LABEL_RGB[2], 1.0),
                    TextWeight::Regular,
                    TextAlign::Start,
                ))
                .color([MENU_LABEL_RGB[0], MENU_LABEL_RGB[1], MENU_LABEL_RGB[2], 1.0]),
        );
    }

    // ------------------------------------------------------------------
    // Content area
    // ------------------------------------------------------------------
    let content_left = layout.content_left(scale);
    let content_right = layout.content_right(scale);
    let content_w = content_right - content_left;
    let first_top = layout.first_row_top(scale);
    let row_h = ROW_H * scale;

    // Category heading
    let cat_label = copy
        .categories
        .iter()
        .find_map(|(cat, lbl)| (*cat == input.category).then_some(*lbl))
        .unwrap_or(input.category.key());
    ui.begin_absolute_placement();
    ui.set_cursor(content_left, layout.top + 32.0 * scale);
    ui.set_available_width(content_w);
    ui.label(
        &Label::new(cat_label)
            .id(UiId::settings_row("category-heading"))
            .style(TextStyle::new(
                TextRole::SettingsHeader,
                HEADER_SIZE,
                Color::rgba(MENU_LABEL_RGB[0], MENU_LABEL_RGB[1], MENU_LABEL_RGB[2], 1.0),
                TextWeight::Bold,
                TextAlign::Start,
            ))
            .color([MENU_LABEL_RGB[0], MENU_LABEL_RGB[1], MENU_LABEL_RGB[2], 1.0]),
    );

    match input.category {
        // ==============================================================
        // Apps
        // ==============================================================
        SettingsCategoryId::Apps => {
            let segment_top = first_top + 44.0 * scale;
            let gap = SEGMENT_GAP * scale;
            let each_w = (content_w - gap * 3.0) / 4.0;

            // Sort label
            ui.begin_absolute_placement();
            ui.set_cursor(content_left, first_top - 8.0 * scale);
            ui.set_available_width(content_w);
            ui.label(
                &Label::new(copy.sort_label)
                    .id(UiId::settings_row("text-sort-label"))
                    .color(INK),
            );

            // Sort segment buttons
            for (seg_idx, (order, label)) in copy.sort_orders.iter().copied().enumerate() {
                let left = content_left + seg_idx as f32 * (each_w + gap);
                ui.begin_absolute_placement();
                ui.set_cursor(left, segment_top);
                ui.set_available_width(each_w);
                ui.button(
                    &Button::new(label)
                        .id(UiId::settings_row(format!("sort-{}", order.key())))
                        .style(if input.sort_order == order {
                            ButtonStyle::Prominent
                        } else {
                            ButtonStyle::Plain
                        })
                        .chevron_opt(false)
                        .hit_target(SettingsPanelHit::Sort(order).target()),
                );
            }

            // Frequent apps toggle
            let y = first_top + ROW_STEP * scale;
            ui.begin_absolute_placement();
            ui.set_cursor(content_left, y);
            ui.set_available_width(content_w);
            ui.toggle(
                &Toggle::new(input.frequent_apps_enabled)
                    .id(UiId::settings_row("toggle-frequent-apps"))
                    .label(copy.frequent_apps_label)
                    .detail(copy.frequent_apps_detail)
                    .hit_target(SettingsPanelHit::FrequentToggle.target()),
            );

            // Steam apps toggle
            let y = first_top + ROW_STEP * 2.0 * scale;
            ui.begin_absolute_placement();
            ui.set_cursor(content_left, y);
            ui.set_available_width(content_w);
            ui.toggle(
                &Toggle::new(input.show_steam_apps)
                    .id(UiId::settings_row("toggle-steam-apps"))
                    .label(copy.steam_apps_label)
                    .detail(copy.steam_apps_detail)
                    .hit_target(SettingsPanelHit::SteamToggle.target()),
            );

            // Hidden apps row (chevron, no toggle)
            let y = first_top + ROW_STEP * 3.0 * scale;
            ui.begin_absolute_placement();
            ui.set_cursor(content_left, y);
            ui.set_available_width(content_w);
            ui.button(
                &Button::new(copy.hidden_apps_label)
                    .id(UiId::settings_row("hidden-apps"))
                    .chevron_opt(true),
            );
            // Hidden count text (right-aligned, overlaid on the row)
            ui.begin_absolute_placement();
            ui.set_cursor(
                content_right - 32.0 * scale,
                y + row_h * 0.5 - LABEL_LINE * scale * 0.5,
            );
            ui.label(
                &Label::new(copy.hidden_count_label)
                    .id(UiId::settings_row("text-hidden-apps-count"))
                    .color(MUTED)
                    .align(TextAlign::End),
            );
        }

        // ==============================================================
        // Search
        // ==============================================================
        SettingsCategoryId::Search => {
            let y = first_top;
            ui.begin_absolute_placement();
            ui.set_cursor(content_left, y);
            ui.set_available_width(content_w);
            ui.toggle(
                &Toggle::new(input.search_includes_hidden)
                    .id(UiId::settings_row("toggle-search-hidden"))
                    .label(copy.search_hidden_label)
                    .detail(copy.search_hidden_detail)
                    .hit_target(SettingsPanelHit::SearchHiddenToggle.target()),
            );
        }

        // ==============================================================
        // System
        // ==============================================================
        SettingsCategoryId::System => {
            // FPS toggle
            let y = first_top;
            ui.begin_absolute_placement();
            ui.set_cursor(content_left, y);
            ui.set_available_width(content_w);
            ui.toggle(
                &Toggle::new(input.show_fps)
                    .id(UiId::settings_row("toggle-show-fps"))
                    .label(copy.show_fps_label)
                    .detail(copy.show_fps_detail)
                    .hit_target(SettingsPanelHit::FpsToggle.target()),
            );

            // Reset cache button
            let y = first_top + ROW_STEP * scale;
            ui.begin_absolute_placement();
            ui.set_cursor(content_left, y);
            ui.set_available_width(content_w);
            ui.button(
                &Button::new(copy.reset_cache_label)
                    .id(UiId::settings_row("reset-cache"))
                    .detail(copy.reset_cache_detail)
                    .chevron_opt(true)
                    .hit_target(SettingsPanelHit::ResetCache.target()),
            );

            // Reset settings button
            let y = first_top + ROW_STEP * 2.0 * scale;
            ui.begin_absolute_placement();
            ui.set_cursor(content_left, y);
            ui.set_available_width(content_w);
            ui.button(
                &Button::new(copy.reset_settings_label)
                    .id(UiId::settings_row("reset-settings"))
                    .detail(copy.reset_settings_detail)
                    .chevron_opt(true)
                    .hit_target(SettingsPanelHit::ResetSettings.target()),
            );
        }

        // ==============================================================
        // About
        // ==============================================================
        SettingsCategoryId::About => {
            let y = first_top;
            // Version row background
            ui.begin_absolute_placement();
            ui.set_cursor(content_left, y);
            ui.set_available_width(content_w);
            ui.button(
                &Button::new(copy.version_label)
                    .id(UiId::settings_row("version"))
                    .chevron_opt(false),
            );
            // Version value (right-aligned)
            ui.begin_absolute_placement();
            ui.set_cursor(
                content_right - 16.0 * scale,
                y + row_h * 0.5 - LABEL_LINE * scale * 0.5,
            );
            ui.label(
                &Label::new(copy.version_value)
                    .id(UiId::settings_row("text-version-value"))
                    .color(MUTED)
                    .align(TextAlign::End),
            );
        }

        // ==============================================================
        // Debug (scrollable)
        // ==============================================================
        SettingsCategoryId::Debug => {
            let scroll_id = UiId::named("settings.debug.scroll");
            let debug_viewport_h = layout.panel_bottom() - first_top;

            ui.begin_absolute_placement();
            ui.set_cursor(content_left, first_top);
            ui.set_available_width(content_w);
            ui.set_available_height(Some(debug_viewport_h));

            ui.scroll_view(scroll_id, scroll, |ui| {
                let cw = ui.available_width;

                // Row 0: Debug keys toggle
                ui.begin_absolute_placement();
                ui.set_available_width(cw);
                ui.toggle(
                    &Toggle::new(input.debug_keys_enabled)
                        .id(UiId::settings_row("toggle-debug"))
                        .label(copy.debug_label)
                        .detail(copy.debug_detail)
                        .hit_target(SettingsPanelHit::DebugToggle.target()),
                );

                // Section header: Window
                ui.begin_absolute_placement();
                ui.set_available_width(cw);
                ui.spacer(8.0 * scale);
                ui.label(
                    &Label::new(copy.debug_section_window)
                        .id(UiId::settings_row("debug-section-0"))
                        .style(TextStyle::new(
                            TextRole::SettingsHeader,
                            HEADER_SIZE,
                            Color::rgba(
                                MENU_LABEL_RGB[0],
                                MENU_LABEL_RGB[1],
                                MENU_LABEL_RGB[2],
                                1.0,
                            ),
                            TextWeight::Bold,
                            TextAlign::Start,
                        ))
                        .color([MENU_LABEL_RGB[0], MENU_LABEL_RGB[1], MENU_LABEL_RGB[2], 1.0]),
                );
                ui.spacer(2.0 * scale);

                // Row 1: Window decorations toggle
                ui.begin_absolute_placement();
                ui.set_available_width(cw);
                ui.toggle(
                    &Toggle::new(input.window_decorated)
                        .id(UiId::settings_row("toggle-window-decorations"))
                        .label(copy.debug_window_decorations_label)
                        .detail(copy.debug_window_decorations_detail)
                        .hit_target(SettingsPanelHit::WindowDecorations.target()),
                );

                // Row 2: Icon cache rebuild button
                ui.begin_absolute_placement();
                ui.set_available_width(cw);
                ui.button(
                    &Button::new(copy.debug_icon_cache_label)
                        .id(UiId::settings_row("debug-icon-cache"))
                        .detail(copy.debug_icon_cache_detail)
                        .chevron_opt(true)
                        .hit_target(SettingsPanelHit::ResetCache.target()),
                );

                // Section header: Liquid Glass
                ui.begin_absolute_placement();
                ui.set_available_width(cw);
                ui.spacer(8.0 * scale);
                ui.label(
                    &Label::new(copy.debug_section_liquid_glass)
                        .id(UiId::settings_row("debug-section-1"))
                        .style(TextStyle::new(
                            TextRole::SettingsHeader,
                            HEADER_SIZE,
                            Color::rgba(
                                MENU_LABEL_RGB[0],
                                MENU_LABEL_RGB[1],
                                MENU_LABEL_RGB[2],
                                1.0,
                            ),
                            TextWeight::Bold,
                            TextAlign::Start,
                        ))
                        .color([MENU_LABEL_RGB[0], MENU_LABEL_RGB[1], MENU_LABEL_RGB[2], 1.0]),
                );
                ui.spacer(2.0 * scale);

                // Row 3: Liquid Glass master toggle
                ui.begin_absolute_placement();
                ui.set_available_width(cw);
                ui.toggle(
                    &Toggle::new(input.liquid_glass.enabled)
                        .id(UiId::settings_row("toggle-lg-enabled"))
                        .label(copy.debug_lg_enabled_label)
                        .detail(copy.debug_lg_enabled_detail)
                        .hit_target(SettingsPanelHit::LiquidGlassEnabled.target()),
                );

                // Rows 4-9: Liquid Glass parameter sliders
                let param_labels: [(LiquidGlassParamId, &str); 6] = [
                    (LiquidGlassParamId::Thickness, copy.debug_lg_thickness_label),
                    (
                        LiquidGlassParamId::RefractiveIndex,
                        copy.debug_lg_refractive_index_label,
                    ),
                    (
                        LiquidGlassParamId::Saturation,
                        copy.debug_lg_saturation_label,
                    ),
                    (
                        LiquidGlassParamId::AdaptiveDarkness,
                        copy.debug_lg_adaptive_darkness_label,
                    ),
                    (
                        LiquidGlassParamId::ChromaticAberration,
                        copy.debug_lg_chromatic_aberration_label,
                    ),
                    (
                        LiquidGlassParamId::BlurRadius,
                        copy.debug_lg_blur_radius_label,
                    ),
                ];
                for (param_id, label) in param_labels {
                    let value = input.liquid_glass.get(param_id);
                    let (min, max) = param_id.range();
                    ui.begin_absolute_placement();
                    ui.set_available_width(cw);
                    ui.slider(
                        &Slider::new(value, (min, max))
                            .id(UiId::settings_row(format!("slider-{}", param_id.key())))
                            .label(label)
                            .reset_opt(true)
                            .hit_target(SettingsPanelHit::LiquidGlassParam(param_id).target())
                            .hit_target_reset(
                                SettingsPanelHit::LiquidGlassParamReset(param_id).target(),
                            ),
                    );
                }

                // Rows 9-11: Disable-flag toggles
                let disable_items = [
                    (
                        LiquidGlassDebugId::DisableChromaticAberration,
                        copy.debug_lg_disable_chromatic_aberration_label,
                        input.liquid_glass_debug.disable_chromatic_aberration,
                    ),
                    (
                        LiquidGlassDebugId::DisableEdgeLighting,
                        copy.debug_lg_disable_edge_lighting_label,
                        input.liquid_glass_debug.disable_edge_lighting,
                    ),
                    (
                        LiquidGlassDebugId::DisableBlur,
                        copy.debug_lg_disable_blur_label,
                        input.liquid_glass_debug.disable_blur,
                    ),
                ];
                for (flag_id, label, state) in disable_items {
                    ui.begin_absolute_placement();
                    ui.set_available_width(cw);
                    ui.toggle(
                        &Toggle::new(state)
                            .id(UiId::settings_row(format!(
                                "toggle-lg-disable-{}",
                                flag_id.key()
                            )))
                            .label(label)
                            .hit_target(SettingsPanelHit::LiquidGlassDebug(flag_id).target()),
                    );
                }

                // Row 12: Reset-all button
                ui.begin_absolute_placement();
                ui.set_available_width(cw);
                ui.button(
                    &Button::new(copy.debug_lg_reset_all_label)
                        .id(UiId::settings_row("reset-lg-all"))
                        .detail(copy.debug_lg_reset_all_detail)
                        .chevron_opt(true)
                        .hit_target(SettingsPanelHit::LiquidGlassResetAll.target()),
                );

                // Section header: Debug views
                ui.begin_absolute_placement();
                ui.set_available_width(cw);
                ui.spacer(8.0 * scale);
                ui.label(
                    &Label::new(copy.debug_section_debug_views)
                        .id(UiId::settings_row("debug-section-2"))
                        .style(TextStyle::new(
                            TextRole::SettingsHeader,
                            HEADER_SIZE,
                            Color::rgba(
                                MENU_LABEL_RGB[0],
                                MENU_LABEL_RGB[1],
                                MENU_LABEL_RGB[2],
                                1.0,
                            ),
                            TextWeight::Bold,
                            TextAlign::Start,
                        ))
                        .color([MENU_LABEL_RGB[0], MENU_LABEL_RGB[1], MENU_LABEL_RGB[2], 1.0]),
                );
                ui.spacer(2.0 * scale);

                // Rows 13-17: Debug-view toggles
                let view_items = [
                    (
                        LiquidGlassDebugId::ShowBackdropTexture,
                        copy.debug_lg_show_backdrop_texture_label,
                        input.liquid_glass_debug.show_backdrop_texture,
                    ),
                    (
                        LiquidGlassDebugId::ShowGeometryTexture,
                        copy.debug_lg_show_geometry_texture_label,
                        input.liquid_glass_debug.show_geometry_texture,
                    ),
                    (
                        LiquidGlassDebugId::ShowDisplacement,
                        copy.debug_lg_show_displacement_label,
                        input.liquid_glass_debug.show_displacement,
                    ),
                    (
                        LiquidGlassDebugId::ShowAlphaMask,
                        copy.debug_lg_show_alpha_mask_label,
                        input.liquid_glass_debug.show_alpha_mask,
                    ),
                    (
                        LiquidGlassDebugId::ShowFinalGlassOnly,
                        copy.debug_lg_show_final_glass_only_label,
                        input.liquid_glass_debug.show_final_glass_only,
                    ),
                ];
                for (flag_id, label, state) in view_items {
                    ui.begin_absolute_placement();
                    ui.set_available_width(cw);
                    ui.toggle(
                        &Toggle::new(state)
                            .id(UiId::settings_row(format!(
                                "toggle-lg-view-{}",
                                flag_id.key()
                            )))
                            .label(label)
                            .hit_target(SettingsPanelHit::LiquidGlassDebug(flag_id).target()),
                    );
                }
            });
        }
    }

    // ------------------------------------------------------------------
    // Assemble the model
    // ------------------------------------------------------------------
    let result = ui.into_layout_result();

    SettingsPanelModel {
        layout,
        visual_scale,
        visual_alpha,
        result,
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
//   7  adaptive_darkness slider
//   8  chromatic_aberration slider
//   9  blur_radius slider
//  10  disable chromatic aberration (C)
//  11  disable edge lighting (E)
//  12  disable blur (L)
//  13  "Reset Liquid Glass to defaults" button
//  14  show backdrop texture (B)
//  15  show geometry texture (G)
//  16  show displacement (D)
//  17  show alpha mask (A)
//  18  show final glass only (F)

/// Total number of full-row slots the Debug category uses.
pub const DEBUG_CATEGORY_ROW_COUNT: i32 = 19;

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

/// Slider X geometry for a row: `(track_left, track_width, knob_radius,
/// reset_center_x, reset_radius)`. All in logical px relative to the panel
/// content area. Delegates to the widget-side [`Slider::geometry`] so hit
/// testing and rendering share identical coordinate math.
pub fn debug_slider_geometry(
    layout: &SettingsPanelLayout,
    scale: f32,
) -> (f32, f32, f32, f32, f32) {
    let content_right = layout.content_right(scale);
    let (track_left, track_width, knob_radius, reset_cx, reset_r, _track_hh) =
        Slider::geometry(content_right, scale);
    (track_left, track_width, knob_radius, reset_cx, reset_r)
}

/// Convert a pointer X (logical, content-space) to a slider value for the
/// given parameter id. Delegates to the widget-side [`Slider::value_from_pointer`]
/// so drag and hit testing share identical coordinate math.
pub fn debug_slider_value_from_pointer(
    layout: &SettingsPanelLayout,
    scale: f32,
    pointer_x: f32,
    id: LiquidGlassParamId,
) -> f32 {
    let (track_left, track_width, _, _, _) = debug_slider_geometry(layout, scale);
    Slider::value_from_pointer(pointer_x, track_left, track_width, id.range())
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
    use crate::layout::hit_map::HitMap;
    use crate::scroll::{ContinuousConfig, ContinuousScroller};
    use crate::ui_model::hit::{HitTarget, SettingsTarget};

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
            debug_lg_adaptive_darkness_label: "Adaptive darkness",
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
        let mut category_selection_amounts = [0.0; 5];
        category_selection_amounts[category.index()] = 1.0;
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
            pointer_pos: None,
            pointer_pressed: false,
            page_frame_rect: Rect::new(80.0, 60.0, 1120.0, 680.0),
            page_frame_radius: 54.0,
            category_hover_amounts: [0.0; 5],
            category_selection_amounts,
        }
    }

    /// Build the settings panel and return the HitMap.
    fn hit_map_for_category(category: SettingsCategoryId) -> HitMap {
        let inp = input(category);
        let c = copy("0");
        let mut scroll = ContinuousScroller::new(ContinuousConfig::default());
        let model = build_with_ui(inp, &c, &mut scroll);
        model.result.hits
    }

    /// Hit-test and extract SettingsTarget.
    fn hit_settings_target(hm: &HitMap, point: Point) -> Option<SettingsTarget> {
        hm.hit_test(point).and_then(|r| match &r.target {
            HitTarget::Settings { target } => Some(target.clone()),
            _ => None,
        })
    }

    // ------------------------------------------------------------------
    // Layout
    // ------------------------------------------------------------------

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
    fn panel_contains_matches_current_inclusive_bounds() {
        let layout = layout();
        assert!(contains(
            &layout,
            Point::new(layout.panel_right(), layout.panel_bottom())
        ));
    }

    #[test]
    fn settings_matches_context_menu_glass_and_focus_contract() {
        let mut inp = input(SettingsCategoryId::Apps);
        inp.category_hover_amounts[SettingsCategoryId::Search.index()] = 1.0;
        let c = copy("0");
        let mut scroll = ContinuousScroller::new(ContinuousConfig::default());
        let model = build_with_ui(inp, &c, &mut scroll);

        let surface = model
            .result
            .render
            .glass
            .iter()
            .find(|batch| batch.layer == GlassLayer::Settings)
            .and_then(|batch| batch.surfaces.first())
            .expect("settings glass surface");
        assert_eq!(surface.blur_radius, Some(CONTEXT_MENU_BASE_BLUR));
        assert_eq!(surface.backdrop_replacement, 1.0);
        assert_eq!(surface.tint, Some(Color::rgba(0.93, 0.94, 0.96, 0.68)));

        let veil = model
            .result
            .render
            .ink
            .iter()
            .find(|batch| batch.lane == InkLane::Backdrop)
            .and_then(|batch| batch.views.first())
            .expect("settings Glass Focus Veil");
        assert_eq!(veil.id, UiId::backdrop("glass-focus-veil"));
        assert_eq!(veil.scene_blur, 1.0);
        assert_eq!(veil.center, Point::new(640.0, 400.0));

        let focus = model
            .result
            .render
            .ink
            .iter()
            .find(|batch| batch.lane == InkLane::Settings)
            .and_then(|batch| {
                batch
                    .views
                    .iter()
                    .find(|view| view.id == UiId::settings_row("category-focus-search"))
            })
            .expect("focused settings category pill");
        assert_eq!(focus.opacity, FOCUS_ROW_OPACITY);

        let search_label = model
            .result
            .render
            .text
            .iter()
            .find(|view| view.id == UiId::settings_row("category-search"))
            .expect("settings category label");
        assert_eq!(
            search_label.style.color,
            Color::rgba(MENU_LABEL_RGB[0], MENU_LABEL_RGB[1], MENU_LABEL_RGB[2], 1.0)
        );

        let apps_selection = model
            .result
            .render
            .ink
            .iter()
            .find(|batch| batch.lane == InkLane::Settings)
            .and_then(|batch| {
                batch
                    .views
                    .iter()
                    .find(|view| view.id == UiId::settings_row("category-focus-apps"))
            })
            .expect("selected settings category pill");
        assert_eq!(apps_selection.opacity, SETTINGS_SELECTED_ROW_OPACITY);
        let expected_selection_color = Color::rgba(
            SETTINGS_SELECTED_ROW_RGB[0],
            SETTINGS_SELECTED_ROW_RGB[1],
            SETTINGS_SELECTED_ROW_RGB[2],
            1.0,
        );
        assert!((apps_selection.color.r - expected_selection_color.r).abs() < 1e-6);
        assert!((apps_selection.color.g - expected_selection_color.g).abs() < 1e-6);
        assert!((apps_selection.color.b - expected_selection_color.b).abs() < 1e-6);
        assert_eq!(
            focus.color,
            Color::rgba(
                SETTINGS_HOVER_ROW_RGB[0],
                SETTINGS_HOVER_ROW_RGB[1],
                SETTINGS_HOVER_ROW_RGB[2],
                1.0
            )
        );
    }

    #[test]
    fn settings_selection_pill_fades_with_selection_amount() {
        let mut inp = input(SettingsCategoryId::Apps);
        inp.category_selection_amounts[SettingsCategoryId::Apps.index()] = 0.5;
        let c = copy("0");
        let mut scroll = ContinuousScroller::new(ContinuousConfig::default());
        let model = build_with_ui(inp, &c, &mut scroll);
        let selection = model
            .result
            .render
            .ink
            .iter()
            .find(|batch| batch.lane == InkLane::Settings)
            .and_then(|batch| {
                batch
                    .views
                    .iter()
                    .find(|view| view.id == UiId::settings_row("category-focus-apps"))
            })
            .expect("transitioning settings category pill");

        assert!((selection.opacity - SETTINGS_SELECTED_ROW_OPACITY * 0.5).abs() < 1e-6);
        assert!(selection.color.r > SETTINGS_SELECTED_ROW_RGB[0]);
        assert!(selection.color.g < SETTINGS_SELECTED_ROW_RGB[1]);
    }

    #[test]
    fn settings_dividers_match_column_layout() {
        let inp = input(SettingsCategoryId::Apps);
        let c = copy("0");
        let mut scroll = ContinuousScroller::new(ContinuousConfig::default());
        let model = build_with_ui(inp, &c, &mut scroll);
        let settings_ink = model
            .result
            .render
            .ink
            .iter()
            .find(|batch| batch.lane == InkLane::Settings)
            .expect("settings ink batch");
        let sidebar_divider = settings_ink
            .views
            .iter()
            .find(|view| view.id == UiId::settings_row("sidebar-divider"))
            .expect("sidebar divider");
        let expected_layout = layout();

        assert!((sidebar_divider.extent - (expected_layout.hh - 28.0)).abs() < f32::EPSILON);
        assert!((sidebar_divider.stroke - 0.55).abs() < f32::EPSILON);
        assert!(!settings_ink
            .views
            .iter()
            .any(|view| view.id == UiId::settings_row("bottom-divider")));
    }

    // ------------------------------------------------------------------
    // HitMap-based hit tests (unified rendering + hit target)
    // ------------------------------------------------------------------

    #[test]
    fn hit_map_backdrop_outside_panel() {
        let hm = hit_map_for_category(SettingsCategoryId::Apps);
        let hit = hm.hit_test(Point::new(100.0, 100.0)).unwrap();
        assert!(matches!(hit.target, HitTarget::Backdrop { .. }));
    }

    #[test]
    fn hit_map_panel_inside() {
        // The panel area has a HitRegion at z=90 covering the panel rect.
        // Widgets at higher z overlay it, but the panel region is still present.
        let hm = hit_map_for_category(SettingsCategoryId::Apps);
        let has_panel = hm.regions().iter().any(|r| {
            matches!(
                &r.target,
                HitTarget::Settings {
                    target: SettingsTarget::Panel
                }
            )
        });
        assert!(has_panel, "Panel hit region should exist");
    }

    #[test]
    fn hit_map_close_button() {
        let layout = layout();
        let (x, y) = layout.close_center(1.0);
        let hm = hit_map_for_category(SettingsCategoryId::Apps);
        let target = hit_settings_target(&hm, Point::new(x, y)).unwrap();
        assert_eq!(target, SettingsTarget::Close);
    }

    #[test]
    fn hit_map_category_sidebar() {
        let layout = layout();
        let y = layout.top + SIDEBAR_TOP + SIDEBAR_ROW_H * 0.5;
        let hm = hit_map_for_category(SettingsCategoryId::Apps);
        let target = hit_settings_target(&hm, Point::new(layout.left + 30.0, y)).unwrap();
        assert_eq!(target, SettingsTarget::Category { key: "apps".into() });
    }

    #[test]
    fn hit_map_sort_segment() {
        let layout = layout();
        let hm = hit_map_for_category(SettingsCategoryId::Apps);
        let content_left = layout.content_left(1.0);
        let segment_y = layout.first_row_top(1.0) + 44.0 + SEGMENT_H * 0.5;
        let target = hit_settings_target(&hm, Point::new(content_left + 10.0, segment_y)).unwrap();
        assert_eq!(target, SettingsTarget::SortOption { key: "name".into() });
    }

    #[test]
    fn hit_map_frequent_toggle() {
        let layout = layout();
        let hm = hit_map_for_category(SettingsCategoryId::Apps);
        let x = layout.content_left(1.0) + 10.0;
        let y = layout.first_row_top(1.0) + ROW_STEP + ROW_H * 0.5;
        let target = hit_settings_target(&hm, Point::new(x, y)).unwrap();
        assert_eq!(
            target,
            SettingsTarget::Toggle {
                key: "frequent-apps".into()
            }
        );
    }

    #[test]
    fn hit_map_steam_toggle() {
        let layout = layout();
        let hm = hit_map_for_category(SettingsCategoryId::Apps);
        let x = layout.content_left(1.0) + 10.0;
        let y = layout.first_row_top(1.0) + ROW_STEP * 2.0 + ROW_H * 0.5;
        let target = hit_settings_target(&hm, Point::new(x, y)).unwrap();
        assert_eq!(
            target,
            SettingsTarget::Toggle {
                key: "steam-apps".into()
            }
        );
    }

    #[test]
    fn hit_map_search_hidden_toggle() {
        let layout = layout();
        let hm = hit_map_for_category(SettingsCategoryId::Search);
        let x = layout.content_left(1.0) + 10.0;
        let y = layout.first_row_top(1.0) + ROW_H * 0.5;
        let target = hit_settings_target(&hm, Point::new(x, y)).unwrap();
        assert_eq!(
            target,
            SettingsTarget::Toggle {
                key: "search-hidden".into()
            }
        );
    }

    #[test]
    fn hit_map_system_fps_toggle() {
        let layout = layout();
        let hm = hit_map_for_category(SettingsCategoryId::System);
        let x = layout.content_left(1.0) + 10.0;
        let y = layout.first_row_top(1.0) + ROW_H * 0.5;
        let target = hit_settings_target(&hm, Point::new(x, y)).unwrap();
        assert_eq!(
            target,
            SettingsTarget::Toggle {
                key: "show-fps".into()
            }
        );
    }

    #[test]
    fn hit_map_system_reset_cache() {
        let layout = layout();
        let hm = hit_map_for_category(SettingsCategoryId::System);
        let x = layout.content_left(1.0) + 10.0;
        let y = layout.first_row_top(1.0) + ROW_STEP + ROW_H * 0.5;
        let target = hit_settings_target(&hm, Point::new(x, y)).unwrap();
        assert_eq!(
            target,
            SettingsTarget::Action {
                key: "reset-cache".into()
            }
        );
    }

    #[test]
    fn hit_map_system_reset_settings() {
        let layout = layout();
        let hm = hit_map_for_category(SettingsCategoryId::System);
        let x = layout.content_left(1.0) + 10.0;
        let y = layout.first_row_top(1.0) + ROW_STEP * 2.0 + ROW_H * 0.5;
        let target = hit_settings_target(&hm, Point::new(x, y)).unwrap();
        assert_eq!(
            target,
            SettingsTarget::Action {
                key: "reset-settings".into()
            }
        );
    }

    #[test]
    fn hit_map_debug_toggle() {
        let layout = layout();
        let hm = hit_map_for_category(SettingsCategoryId::Debug);
        let x = layout.content_left(1.0) + 10.0;
        let y = layout.first_row_top(1.0) + ROW_H * 0.5;
        let target = hit_settings_target(&hm, Point::new(x, y)).unwrap();
        assert_eq!(
            target,
            SettingsTarget::Toggle {
                key: "debug".into()
            }
        );
    }

    #[test]
    fn hit_map_debug_lg_enabled_toggle_exists() {
        let hm = hit_map_for_category(SettingsCategoryId::Debug);
        let found = hm
            .regions()
            .iter()
            .any(|r| r.target == HitTarget::settings_toggle("lg-enabled"));
        assert!(found, "LG enabled toggle should exist");
    }

    #[test]
    fn hit_map_debug_lg_param_slider_track_exists() {
        let hm = hit_map_for_category(SettingsCategoryId::Debug);
        let found = hm
            .regions()
            .iter()
            .any(|r| r.target == HitTarget::settings_toggle("lg-param-thickness"));
        assert!(found, "LG thickness slider track should exist");
    }

    #[test]
    fn hit_map_debug_lg_param_slider_reset_exists() {
        let hm = hit_map_for_category(SettingsCategoryId::Debug);
        let found = hm
            .regions()
            .iter()
            .any(|r| r.target == HitTarget::settings_action("lg-param-reset-thickness"));
        assert!(found, "LG thickness slider reset should exist");
    }

    #[test]
    fn hit_map_debug_lg_disable_flag_toggle_exists() {
        let hm = hit_map_for_category(SettingsCategoryId::Debug);
        let found = hm.regions().iter().any(|r| {
            r.target == HitTarget::settings_toggle("lg-debug-disable-chromatic-aberration")
        });
        assert!(found, "LG disable flag toggle should exist");
    }

    #[test]
    fn hit_map_debug_lg_view_flag_toggle_exists() {
        let hm = hit_map_for_category(SettingsCategoryId::Debug);
        let found = hm
            .regions()
            .iter()
            .any(|r| r.target == HitTarget::settings_toggle("lg-debug-show-backdrop-texture"));
        assert!(found, "LG view flag toggle should exist");
    }

    #[test]
    fn hit_map_debug_lg_reset_all_button_exists() {
        let hm = hit_map_for_category(SettingsCategoryId::Debug);
        let found = hm
            .regions()
            .iter()
            .any(|r| r.target == HitTarget::settings_action("lg-reset-all"));
        assert!(found, "LG reset all button should exist");
    }

    #[test]
    fn hit_map_debug_window_decorations_toggle_exists() {
        let hm = hit_map_for_category(SettingsCategoryId::Debug);
        let found = hm
            .regions()
            .iter()
            .any(|r| r.target == HitTarget::settings_toggle("window-decorations"));
        assert!(found, "Window decorations toggle should exist");
    }

    #[test]
    fn hit_map_category_switch_changes_hit_regions() {
        let hm_apps = hit_map_for_category(SettingsCategoryId::Apps);
        let hm_system = hit_map_for_category(SettingsCategoryId::System);
        let apps_has_frequent = hm_apps
            .regions()
            .iter()
            .any(|r| r.target == HitTarget::settings_toggle("frequent-apps"));
        assert!(apps_has_frequent);
        let system_has_fps = hm_system
            .regions()
            .iter()
            .any(|r| r.target == HitTarget::settings_toggle("show-fps"));
        assert!(system_has_fps);
        let system_has_frequent = hm_system
            .regions()
            .iter()
            .any(|r| r.target == HitTarget::settings_toggle("frequent-apps"));
        assert!(!system_has_frequent);
    }

    // ------------------------------------------------------------------
    // Close button
    // ------------------------------------------------------------------

    #[test]
    fn hit_close_enlarges_target_beyond_visible_glyph() {
        let layout = layout();
        let (cx, cy) = layout.close_center(1.0);
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
        assert!(hit_close(&layout, 1.5, Point::new(cx + 20.0, cy)));
    }

    // ------------------------------------------------------------------
    // Animation / geometry / slider helpers
    // ------------------------------------------------------------------

    #[test]
    fn animation_helpers_match_endpoints() {
        assert_eq!(alpha(0.0), 0.0);
        assert_eq!(alpha(1.0), 1.0);
        assert_eq!(pop_progress(0.0), 0.0);
        assert_eq!(pop_progress(1.0), 1.0);
    }

    #[test]
    fn debug_overflow_rows_is_positive() {
        let n = debug_category_overflow_rows();
        assert!(n > 0, "Debug category should overflow, got {n}");
        assert!(n < DEBUG_CATEGORY_ROW_COUNT, "overflow exceeds row count");
    }

    #[test]
    fn slider_value_from_pointer_is_clamped() {
        let layout = layout();
        let (track_left, track_width, _, _, _) = debug_slider_geometry(&layout, 1.0);
        let (min, max) = LiquidGlassParamId::Thickness.range();
        let v_left = debug_slider_value_from_pointer(
            &layout,
            1.0,
            track_left - 50.0,
            LiquidGlassParamId::Thickness,
        );
        assert_eq!(v_left, min);
        let v_right = debug_slider_value_from_pointer(
            &layout,
            1.0,
            track_left + track_width + 50.0,
            LiquidGlassParamId::Thickness,
        );
        assert_eq!(v_right, max);
        let v_mid = debug_slider_value_from_pointer(
            &layout,
            1.0,
            track_left + track_width * 0.5,
            LiquidGlassParamId::Thickness,
        );
        assert!((v_mid - (min + max) * 0.5).abs() < 1e-3);
    }

    #[test]
    fn build_with_ui_produces_glass_surface() {
        let inp = input(SettingsCategoryId::Apps);
        let c = copy("0");
        let mut scroll = ContinuousScroller::new(ContinuousConfig::default());
        let model = build_with_ui(inp, &c, &mut scroll);
        let glass_surfaces: Vec<_> = model
            .result
            .render
            .glass
            .iter()
            .flat_map(|b| &b.surfaces)
            .collect();
        assert!(
            !glass_surfaces.is_empty(),
            "build_with_ui should produce glass surfaces"
        );
    }

    #[test]
    fn build_with_ui_debug_has_six_sliders() {
        let inp = input(SettingsCategoryId::Debug);
        let c = copy("0");
        let mut scroll = ContinuousScroller::new(ContinuousConfig::default());
        let model = build_with_ui(inp, &c, &mut scroll);
        let all_ink: Vec<_> = model
            .result
            .render
            .ink
            .iter()
            .flat_map(|b| &b.views)
            .collect();
        let knobs: Vec<_> = all_ink
            .iter()
            .filter(|v| v.kind == ControlKind::SliderKnob)
            .collect();
        assert_eq!(knobs.len(), 6, "6 slider knobs");
    }

    // ------------------------------------------------------------------
    // Profiling benchmarks: measure build_with_ui CPU cost and model counts.
    // These are informational tests that print timings; they do not assert
    // specific values (timings vary by machine).
    // ------------------------------------------------------------------

    /// Measure `build_with_ui` for the Debug category, which has the most
    /// toggles (~11). Prints timing and shape/view counts for analysis.
    #[test]
    fn profile_build_with_ui_debug_category() {
        use std::time::Instant;

        let inp = input(SettingsCategoryId::Debug);
        let c = copy("0");
        let mut scroll = ContinuousScroller::new(ContinuousConfig::default());

        // Warm-up: run once to avoid first-run allocation noise.
        let _warm = build_with_ui(inp, &c, &mut scroll);
        let mut scroll = ContinuousScroller::new(ContinuousConfig::default());

        // Timed runs.
        const RUNS: usize = 100;
        let start = Instant::now();
        for _ in 0..RUNS {
            let _model = build_with_ui(inp, &c, &mut scroll);
            // Reset scroll for next iteration (build_with_ui mutates it).
            scroll = ContinuousScroller::new(ContinuousConfig::default());
        }
        let elapsed = start.elapsed();
        let avg_us = elapsed.as_micros() as f64 / RUNS as f64;

        // Run one more time to extract counts.
        let mut scroll = ContinuousScroller::new(ContinuousConfig::default());
        let model = build_with_ui(inp, &c, &mut scroll);

        let modal_glass = model
            .result
            .render
            .glass
            .iter()
            .find(|b| b.layer == GlassLayer::Settings)
            .map(|b| b.surfaces.len())
            .unwrap_or(0);
        let overlay_glass = model
            .result
            .render
            .glass
            .iter()
            .find(|b| b.layer == GlassLayer::Overlay)
            .map(|b| b.surfaces.len())
            .unwrap_or(0);
        let ink_count: usize = model.result.render.ink.iter().map(|b| b.views.len()).sum();
        let glyph_count: usize = model
            .result
            .render
            .glyphs
            .iter()
            .map(|b| b.views.len())
            .sum();
        let text_count = model.result.render.text.len();
        let region_count = model.result.hits.len();

        eprintln!();
        eprintln!("=== PROFILE: build_with_ui (Debug category) ===");
        eprintln!("  Runs:               {}", RUNS);
        eprintln!(
            "  Total time:         {:.3} ms",
            elapsed.as_secs_f64() * 1000.0
        );
        eprintln!(
            "  Avg per call:       {:.3} us ({:.3} ms)",
            avg_us,
            avg_us / 1000.0
        );
        eprintln!("  Modal glass shapes: {}", modal_glass);
        eprintln!("  Overlay glass:      {} (toggle thumbs)", overlay_glass);
        eprintln!("  Ink views:          {}", ink_count);
        eprintln!("  Glyph views:        {}", glyph_count);
        eprintln!("  Text views:         {}", text_count);
        eprintln!("  Hit regions:        {}", region_count);
        eprintln!();

        // The Debug category should produce ~11 toggle thumbs (glass overlay
        // shapes) plus helper shapes. Overlay >= 11 is an indicator.
        assert!(
            overlay_glass >= 11,
            "expected at least 11 toggle glass thumbs"
        );
    }

    /// Measure `build_with_ui` for the Apps category (fewest toggles: 1-2),
    /// to compare against the Debug category. The ratio reveals the overhead
    /// of ~11 toggles vs ~1 toggle.
    #[test]
    fn profile_build_with_ui_apps_category() {
        use std::time::Instant;

        let inp = input(SettingsCategoryId::Apps);
        let c = copy("0");
        let mut scroll = ContinuousScroller::new(ContinuousConfig::default());

        // Warm-up.
        let _warm = build_with_ui(inp, &c, &mut scroll);
        let mut scroll = ContinuousScroller::new(ContinuousConfig::default());

        const RUNS: usize = 100;
        let start = Instant::now();
        for _ in 0..RUNS {
            let _model = build_with_ui(inp, &c, &mut scroll);
            scroll = ContinuousScroller::new(ContinuousConfig::default());
        }
        let elapsed = start.elapsed();
        let avg_us = elapsed.as_micros() as f64 / RUNS as f64;

        // Extract counts.
        let mut scroll = ContinuousScroller::new(ContinuousConfig::default());
        let model = build_with_ui(inp, &c, &mut scroll);

        let modal_glass = model
            .result
            .render
            .glass
            .iter()
            .find(|b| b.layer == GlassLayer::Settings)
            .map(|b| b.surfaces.len())
            .unwrap_or(0);
        let overlay_glass = model
            .result
            .render
            .glass
            .iter()
            .find(|b| b.layer == GlassLayer::Overlay)
            .map(|b| b.surfaces.len())
            .unwrap_or(0);

        eprintln!();
        eprintln!("=== PROFILE: build_with_ui (Apps category) ===");
        eprintln!("  Runs:               {}", RUNS);
        eprintln!(
            "  Total time:         {:.3} ms",
            elapsed.as_secs_f64() * 1000.0
        );
        eprintln!(
            "  Avg per call:       {:.3} us ({:.3} ms)",
            avg_us,
            avg_us / 1000.0
        );
        eprintln!("  Modal glass shapes: {}", modal_glass);
        eprintln!("  Overlay glass:      {}", overlay_glass);
        eprintln!();
    }

    /// Measure `HitMap::clone()` cost for the Debug category's hit map.
    #[test]
    fn profile_hitmap_clone_debug() {
        use std::time::Instant;

        let inp = input(SettingsCategoryId::Debug);
        let c = copy("0");
        let mut scroll = ContinuousScroller::new(ContinuousConfig::default());
        let model = build_with_ui(inp, &c, &mut scroll);
        let hits = &model.result.hits;

        eprintln!("  HitMap regions: {}", hits.len());

        const RUNS: usize = 1000;
        let start = Instant::now();
        for _ in 0..RUNS {
            let _clone = hits.clone();
        }
        let elapsed = start.elapsed();
        let avg_us = elapsed.as_micros() as f64 / RUNS as f64;

        eprintln!();
        eprintln!(
            "=== PROFILE: HitMap::clone() (Debug, {} regions) ===",
            hits.len()
        );
        eprintln!("  Runs:               {}", RUNS);
        eprintln!(
            "  Total time:         {:.3} ms",
            elapsed.as_secs_f64() * 1000.0
        );
        eprintln!(
            "  Avg per clone:      {:.3} us ({:.6} ms)",
            avg_us,
            avg_us / 1000.0
        );
        eprintln!();
    }

    /// Measure Ui::new() overhead (HashMap allocation etc.).
    #[test]
    fn profile_ui_construction() {
        use crate::ui::context::Ui;
        use crate::ui::theme::Theme;
        use std::time::Instant;

        let theme = Theme::default();
        const RUNS: usize = 1000;
        let start = Instant::now();
        for _ in 0..RUNS {
            let _ui = Ui::new(theme, 1280.0, 800.0);
        }
        let elapsed = start.elapsed();
        let avg_us = elapsed.as_micros() as f64 / RUNS as f64;

        eprintln!();
        eprintln!("=== PROFILE: Ui::new() ===");
        eprintln!("  Runs:               {}", RUNS);
        eprintln!(
            "  Total time:         {:.3} ms",
            elapsed.as_secs_f64() * 1000.0
        );
        eprintln!("  Avg per call:       {:.3} us", avg_us);
        eprintln!();
    }
}

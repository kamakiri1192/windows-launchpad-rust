//! Pure context-menu geometry.
//!
//! Emits the renderer-neutral glass surface, item rows (icon + label), and hit
//! regions for the app-icon right-click menu. Every animated property (panel
//! position, size, corner radius, content scale/opacity/blur, glass activation)
//! is provided by the caller via [`ContextMenuInput`]; this module only lays it
//! out, mirroring the contract of [`crate::layout::folder_panel`].

use crate::layout::hit_map::{HitMap, HitRegion};
use crate::layout::LayoutResult;
use crate::ui_model::geometry::{Insets, Point, Rect};
use crate::ui_model::hit::HitTarget;
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{
    Color, ControlKind, GlassBehavior, GlassLayer, GlassMaterial, GlassSurface, InkLane, InkView,
    RenderModel,
};
use crate::ui_model::text::{TextAlign, TextRole, TextStyle, TextView, TextWeight};

/// Reference demo row metrics (logical px at 1× DPI).
const ROW_HEIGHT: f32 = 40.0;
const ROW_INNER_MARGIN_X: f32 = 10.0;
const CONTENT_PAD_X: f32 = 30.0;
const CONTENT_PAD_Y: f32 = 20.0;
const ICON_SIZE: f32 = 24.0;
const ICON_GAP: f32 = 17.0;
/// Content-driven panel width is clamped to these bounds (logical px at 1× DPI)
/// so very short or very long localized labels still render comfortably. The
/// app shell measures the longest label and feeds it to
/// [`open_panel_size_logical`].
const MIN_MENU_WIDTH: f32 = 200.0;
const MAX_MENU_WIDTH: f32 = 320.0;
/// Conservative fallback for the longest label width (logical px) when the text
/// engine is unavailable at open time. Roughly the width of the longest current
/// label ("ChatGPTで使い方を調べる") at 14 px.
pub const FALLBACK_MAX_LABEL_WIDTH: f32 = 200.0;
/// Number of rows currently emitted by the context menu. Kept public so the
/// feature animation and renderer-neutral input model share one array size.
pub const CONTEXT_MENU_ITEM_COUNT: usize = 7;
/// Number of rows omitted when the menu targets a folder. A folder cannot be
/// hidden (no hide-app row), has no file to reveal (no reveal row), and has no
/// app metadata for a ChatGPT lookup (no chatgpt-help row), so three rows are
/// omitted from the app menu.
pub const FOLDER_OMITTED_ITEM_COUNT: usize = 3;
/// Font size in logical px (1× DPI), matching the app-icon label size. The
/// renderer's `scale_factor` converts this to physical px.
const FONT_SIZE: f32 = 14.0;
const CONTEXT_MENU_TINT_ALPHA: f32 = 0.68;
/// System-blue hover tint used by the focused row. The low opacity keeps the
/// material and its blurred backdrop visible through the pill, matching the
/// translucent highlight used by iOS/macOS menus.
const FOCUS_ROW_RGB: [f32; 3] = [0.039, 0.518, 1.0];
const FOCUS_ROW_OPACITY: f32 = 0.20;
/// Leave a little breathing room between the focused pill and adjacent rows.
const FOCUS_ROW_VERTICAL_INSET: f32 = 3.0;
/// Keep the menu body visibly blurred even after the content-reveal animation
/// reaches its resting value of zero. The animated `content_blur` widens the
/// dedicated context-menu blur kernel on top of this baseline.
const CONTEXT_MENU_BASE_BLUR: f32 = 24.0;

/// iOS/macOS-style primary label color on a light material. This is the
/// familiar near-black `#1C1C1E`, rather than absolute black.
pub const MENU_LABEL_RGB: [f32; 3] = [0.11, 0.11, 0.118];
/// iOS-style destructive action color (`systemRed`, approximately `#FF3B30`).
pub const MENU_DESTRUCTIVE_RGB: [f32; 3] = [1.0, 0.231, 0.188];

/// One of the context-menu actions. Selecting an action runs it and closes the
/// menu; the mock rows (icon size / app info) still close only.
///
/// Defined here (in the pure layout layer) rather than in the feature module
/// so the renderer-neutral geometry does not depend on the binary-only
/// features crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContextMenuItem {
    /// ホーム画面を編集
    EditHome,
    /// アプリを非表示
    HideApp,
    /// Finderで開く / Reveal in Explorer
    RevealInFinder,
    /// ChatGPTで使い方を調べる
    ChatGptHelp,
    /// アイコンを大きくする
    IconLarger,
    /// アイコンを小さくする
    IconSmaller,
    /// アプリの概要
    AppInfo,
}

impl ContextMenuItem {
    /// All items in display order.
    pub const ALL: [Self; CONTEXT_MENU_ITEM_COUNT] = [
        Self::EditHome,
        Self::HideApp,
        Self::RevealInFinder,
        Self::ChatGptHelp,
        Self::IconLarger,
        Self::IconSmaller,
        Self::AppInfo,
    ];

    /// Number of rows shown when the menu targets a folder. A folder has no
    /// hide action (it cannot be hidden), no file to reveal (it is a virtual
    /// launcher group with no file on disk to open), and no app metadata for a
    /// ChatGPT lookup, so [`FOLDER_OMITTED_ITEM_COUNT`] rows are omitted — the
    /// folder menu is that many rows shorter than the app menu.
    pub const FOLDER_ITEM_COUNT: usize = CONTEXT_MENU_ITEM_COUNT - FOLDER_OMITTED_ITEM_COUNT;

    /// Items shown when the menu targets a folder. See [`FOLDER_ITEM_COUNT`]:
    /// a folder has no hide action, no file to reveal, and no app metadata for
    /// a ChatGPT lookup, so "アプリを非表示", "Finderで開く", and "ChatGPTで
    /// 使い方を調べる" are all omitted.
    pub const FOLDER_ITEMS: [Self; Self::FOLDER_ITEM_COUNT] = [
        Self::EditHome,
        Self::IconLarger,
        Self::IconSmaller,
        Self::AppInfo,
    ];

    /// Japanese display label for the item.
    pub const fn label(self) -> &'static str {
        match self {
            Self::EditHome => "ホーム画面を編集",
            Self::HideApp => "アプリを非表示",
            Self::RevealInFinder => {
                if cfg!(target_os = "macos") {
                    "Finderで開く"
                } else {
                    "エクスプローラーで開く"
                }
            }
            Self::ChatGptHelp => "ChatGPTで使い方を調べる",
            Self::IconLarger => "アイコンを大きくする",
            Self::IconSmaller => "アイコンを小さくする",
            Self::AppInfo => "アプリの概要",
        }
    }

    /// Display labels for [`ALL`], in the same order. Precomputed at compile
    /// time so callers (the context-menu renderer, which runs every frame)
    /// can borrow a static slice instead of reallocating a `Vec` each frame.
    ///
    /// [`ALL`]: ContextMenuItem::ALL
    pub const ALL_LABELS: [&'static str; CONTEXT_MENU_ITEM_COUNT] = [
        Self::EditHome.label(),
        Self::HideApp.label(),
        Self::RevealInFinder.label(),
        Self::ChatGptHelp.label(),
        Self::IconLarger.label(),
        Self::IconSmaller.label(),
        Self::AppInfo.label(),
    ];

    /// Display labels for [`FOLDER_ITEMS`], in the same order.
    pub const FOLDER_ITEMS_LABELS: [&'static str; Self::FOLDER_ITEM_COUNT] = [
        Self::EditHome.label(),
        Self::IconLarger.label(),
        Self::IconSmaller.label(),
        Self::AppInfo.label(),
    ];

    /// Foreground color for this menu action, shared by its label and icon.
    pub const fn foreground_rgb(self) -> [f32; 3] {
        match self {
            Self::HideApp => MENU_DESTRUCTIVE_RGB,
            _ => MENU_LABEL_RGB,
        }
    }
}

/// The `(items, labels)` pair to display for a menu target, in display order.
/// Folder targets omit both the hide-app row (a folder cannot be hidden) and
/// the reveal row (a folder is a virtual group with no file on disk); app
/// targets show all six rows. The app shell picks this once at open time and
/// reuses it every frame, so the rendered rows and the release-time row
/// resolution always agree.
pub fn menu_rows(is_folder_target: bool) -> (&'static [ContextMenuItem], &'static [&'static str]) {
    if is_folder_target {
        (
            &ContextMenuItem::FOLDER_ITEMS[..],
            &ContextMenuItem::FOLDER_ITEMS_LABELS[..],
        )
    } else {
        (&ContextMenuItem::ALL[..], &ContextMenuItem::ALL_LABELS[..])
    }
}

/// Inputs resolved by the app shell from the live [`ContextMenuState`].
#[derive(Debug, Clone)]
pub struct ContextMenuInput<'a> {
    pub viewport: (u32, u32),
    pub scale_factor: f32,
    /// Stable key of the right-clicked launcher item (e.g. `app:{id}` /
    /// `folder:{id}`). Used only as the opaque UiId key for the rendered rows.
    pub target: &'a str,
    /// Current animated panel top-left (physical px).
    pub pos: (f32, f32),
    /// Current animated panel size (physical px).
    pub size: (f32, f32),
    /// Fully-open panel size (physical px). Content is laid out against this
    /// fixed target and then scaled by `content_scale` toward the animated
    /// center, so item positions stay stable regardless of where the menu
    /// was opened.
    pub open_size: (f32, f32),
    /// Current animated corner radius (physical px).
    pub radius: f32,
    /// Content reveal 0..1 (drives icon/label opacity + scale).
    pub content_scale: f32,
    pub content_opacity: f32,
    pub content_blur: f32,
    /// Per-surface glass activation 0..1 (the open-time optics bump).
    pub activation: f32,
    /// Per-row focus amounts in the range 0..1. The app shell animates these
    /// values from pointer hit changes; the layout only applies the resulting
    /// opacity and scale to the focused pill.
    pub focus_amounts: [f32; CONTEXT_MENU_ITEM_COUNT],
    pub items: &'a [ContextMenuItem],
    /// Localized label for each item, in display order.
    pub labels: &'a [&'a str],
}

/// Per-item layout geometry, kept for hit-test resolution by the app shell.
#[derive(Debug, Clone)]
pub struct ContextMenuItemRow {
    pub rect: Rect,
    pub icon_center: Point,
    pub label_rect: Rect,
}

#[derive(Debug, Clone)]
pub struct ContextMenuModel {
    pub result: LayoutResult,
    pub panel_rect: Rect,
    pub rows: Vec<ContextMenuItemRow>,
    /// Effective DPI scale used to size rows/icons/text.
    pub scale: f32,
}

/// Fully-open panel size for a given item count, in logical px at 1× DPI.
/// Used by the app shell to build the open [`MenuTarget`](crate::features::context_menu::MenuTarget).
///
/// The width is content-driven: `max_label_width_logical` is the measured width
/// of the longest localized label (logical px at 1× DPI), and the panel fits
/// `left content origin (pad_x + icon + gap) + label + right pad_x`, clamped to
/// [`MIN_MENU_WIDTH`]..=[`MAX_MENU_WIDTH`]. Passing [`FALLBACK_MAX_LABEL_WIDTH`]
/// keeps the menu reasonably sized when the text engine is unavailable.
pub fn open_panel_size_logical(item_count: usize, max_label_width_logical: f32) -> (f32, f32) {
    let rows = item_count.max(1);
    let width = (CONTENT_PAD_X + ICON_SIZE + ICON_GAP + max_label_width_logical + CONTENT_PAD_X)
        .clamp(MIN_MENU_WIDTH, MAX_MENU_WIDTH);
    let height = CONTENT_PAD_Y * 2.0 + rows as f32 * ROW_HEIGHT;
    (width, height)
}

/// Gap between the icon edge and the menu edge when attached (physical px).
const ATTACH_GAP: f32 = 4.0;
/// Viewport safety margin kept around the menu (physical px).
const VIEWPORT_MARGIN: f32 = 8.0;

/// Attach the open panel to the icon's edge, flipping sides when there is no
/// room — an iOS-style placement. Returns `(panel_top_left, seed_anchor)` in
/// physical px, where `seed_anchor` is the icon center the 40×40 seed should be
/// centered on so the menu appears to bloom out of the icon regardless of which
/// side it lands on (no fly-through).
///
/// Priority: right of icon (top-aligned) → left of icon (top-aligned) →
/// bottom-aligned on the chosen side → centered clamp when neither side fits.
pub fn open_panel_origin(
    icon: Rect,
    size: (f32, f32),
    viewport: (u32, u32),
) -> ((f32, f32), (f32, f32)) {
    let (vw, vh) = (viewport.0 as f32, viewport.1 as f32);
    let (w, h) = size;

    // --- Horizontal: prefer the icon's right edge, flip to the left, else clamp.
    let right_x = icon.max_x() + ATTACH_GAP; // menu left edge at icon's right
    let fits_right = right_x + w <= vw - VIEWPORT_MARGIN;
    let left_x = icon.min_x() - ATTACH_GAP - w; // menu right edge at icon's left
    let fits_left = left_x >= VIEWPORT_MARGIN;

    let x = if fits_right {
        right_x
    } else if fits_left {
        left_x
    } else {
        // Neither side fits: center on the icon, clamped into the viewport.
        (icon.center().x - w * 0.5).clamp(
            VIEWPORT_MARGIN,
            (vw - VIEWPORT_MARGIN - w).max(VIEWPORT_MARGIN),
        )
    };

    // --- Vertical: top-align with the icon first, flip to bottom-align, else clamp.
    let top_y = icon.min_y();
    let fits_top = top_y + h <= vh - VIEWPORT_MARGIN;
    let bottom_y = icon.max_y() - h; // menu bottom aligns with icon bottom
    let fits_bottom = bottom_y >= VIEWPORT_MARGIN;

    let y = if fits_top {
        top_y
    } else if fits_bottom {
        bottom_y
    } else {
        (icon.center().y - h * 0.5).clamp(
            VIEWPORT_MARGIN,
            (vh - VIEWPORT_MARGIN - h).max(VIEWPORT_MARGIN),
        )
    };

    let center = icon.center();
    ((x, y), (center.x, center.y))
}

/// Build the renderer-neutral model for the context menu.
pub fn build(input: &ContextMenuInput<'_>) -> ContextMenuModel {
    let scale = input.scale_factor.max(0.01);
    let (vw, vh) = (input.viewport.0 as f32, input.viewport.1 as f32);

    let panel_rect = Rect::new(input.pos.0, input.pos.1, input.size.0, input.size.1);

    let mut render = RenderModel::new();

    // Content reveal 0..1. This drives the icons/labels *and* the background
    // body alpha, so on close the collapsed disc fades out instead of lingering
    // as an opaque dot (mirrors the folder panel's `progress`-driven opacity).
    let content_opacity = input.content_opacity.clamp(0.0, 1.0);
    let reveal = content_opacity;

    // --- Glass surface (the menu body) --------------------------------------
    render.set_glass_batch(
        GlassLayer::ContextMenu,
        vec![GlassSurface {
            id: UiId::context_menu_panel(),
            rect: panel_rect,
            radius: input.radius,
            material: GlassMaterial::Regular,
            behavior: GlassBehavior::Control,
            z: 100,
            clip: None,
            activation: input.activation,
            blur_radius: Some((CONTEXT_MENU_BASE_BLUR + input.content_blur).max(0.0)),
            // The renderer flattens the completed pre-menu scene over the
            // desktop and uses that as the material under the menu. Replace
            // the real DWM backdrop while open, then release it with the same
            // reveal channel so the closing seed does not remain opaque.
            backdrop_replacement: content_opacity,
            // The context menu is intentionally brighter than the global glass tint.
            // Fade the override with the menu reveal so the collapsed seed does not
            // leave a white disc behind during close.
            tint: Some(Color::rgba(
                0.93,
                0.94,
                0.96,
                CONTEXT_MENU_TINT_ALPHA * content_opacity,
            )),
        }],
    );

    // --- Item rows (icon + label ink) ---------------------------------------
    // Row geometry is computed against the *fully-open* panel size and then
    // scaled by `content_scale` about the current animated panel center. Using
    // the fixed open size (not the animated one) keeps item positions stable
    // regardless of where the menu was opened.
    let open_w = input.open_size.0.max(1.0);
    let open_h = input.open_size.1.max(1.0);
    let row_h = ROW_HEIGHT * scale;
    let pad_x = CONTENT_PAD_X * scale;
    let pad_y = CONTENT_PAD_Y * scale;
    let icon_size = ICON_SIZE * scale;
    let icon_gap = ICON_GAP * scale;
    // Font size stays in logical px; the text renderer applies `scale`.
    let font_size = FONT_SIZE;

    // Content is centered inside the animated panel and scaled by
    // content_scale around that center (matching the reference's origin 0.5/0.5
    // transform). We compute child positions in open-panel space first, then
    // map into the current animated rect.
    let content_scale = input.content_scale.max(0.0);
    let panel_center_x = panel_rect.x + panel_rect.width * 0.5;
    let panel_center_y = panel_rect.y + panel_rect.height * 0.5;

    let mut ink: Vec<InkView> = Vec::new();
    let mut text_views: Vec<TextView> = Vec::new();
    let mut rows: Vec<ContextMenuItemRow> = Vec::new();

    for (index, (item, label)) in input.items.iter().zip(input.labels.iter()).enumerate() {
        // Position in fully-open panel space.
        let open_row_y = pad_y + index as f32 * row_h;
        let open_icon_x = pad_x;
        let open_label_x = open_icon_x + icon_size + icon_gap;
        let open_row_cy = open_row_y + row_h * 0.5;

        // Map open-panel point -> current animated panel via scale about center.
        let map = |ox: f32, oy: f32| -> (f32, f32) {
            let dx = (ox - open_w * 0.5) * content_scale;
            let dy = (oy - open_h * 0.5) * content_scale;
            (panel_center_x + dx, panel_center_y + dy)
        };

        let (icon_cx, icon_cy) = map(open_icon_x + icon_size * 0.5, open_row_cy);
        let (label_cx, label_cy) = map(open_label_x, open_row_cy);
        let color = item.foreground_rgb();

        // Hit-test row rect (in current panel space).
        let (row_left, row_top) = map(ROW_INNER_MARGIN_X * scale, open_row_y);
        let (row_right, row_bottom) = map(open_w - ROW_INNER_MARGIN_X * scale, open_row_y + row_h);
        let row_rect = Rect::new(
            row_left.min(row_right),
            row_top.min(row_bottom),
            (row_right - row_left).abs(),
            (row_bottom - row_top).abs(),
        );

        // Draw the focus treatment before the row's icon and label so the
        // foreground stays crisp above the translucent blue material. The
        // row hit rect intentionally remains unchanged: the visual inset is
        // only for the breathing room visible in the reference design.
        let focus_amount = input.focus_amounts.get(index).copied().unwrap_or(0.0);
        if focus_amount > 0.001 {
            let vertical_inset = (FOCUS_ROW_VERTICAL_INSET * scale).min(row_rect.height * 0.5);
            let focus_rect = row_rect.inset(Insets::symmetric(0.0, vertical_inset));
            let focus_scale = 0.96 + 0.04 * focus_amount.clamp(0.0, 1.0);
            let focus_center = focus_rect.center();
            let focus_rect = Rect::new(
                focus_center.x - focus_rect.width * focus_scale * 0.5,
                focus_center.y - focus_rect.height * focus_scale * 0.5,
                focus_rect.width * focus_scale,
                focus_rect.height * focus_scale,
            );
            ink.push(InkView {
                id: UiId::context_menu_item(input.target, index),
                center: focus_rect.center(),
                extent: focus_rect.height * 0.5,
                opacity: reveal * FOCUS_ROW_OPACITY * focus_amount.clamp(0.0, 1.0),
                scene_blur: 0.0,
                stroke: focus_rect.width * 0.5,
                corner_radius: focus_rect.height * 0.5,
                color: Color::rgba(FOCUS_ROW_RGB[0], FOCUS_ROW_RGB[1], FOCUS_ROW_RGB[2], 1.0),
                kind: ControlKind::RowBackground,
                z: 130,
                clip: None,
            });
        }

        ink.push(InkView {
            id: UiId::context_menu_item(input.target, index),
            center: Point::new(icon_cx, icon_cy),
            extent: icon_size * 0.5 * content_scale,
            opacity: reveal,
            scene_blur: 0.0,
            stroke: 1.8 * scale * content_scale,
            corner_radius: 0.0,
            color: Color::rgba(color[0], color[1], color[2], 1.0),
            kind: item_icon_kind(input.items[index]),
            z: 140,
            clip: None,
        });

        let label_width = (open_w - open_label_x - pad_x).max(1.0);
        text_views.push(TextView {
            id: UiId::context_menu_item(input.target, index),
            text: (*label).to_string(),
            rect: Rect::new(
                label_cx,
                label_cy - font_size * 0.5 * content_scale,
                label_width * content_scale,
                row_h * content_scale,
            ),
            style: TextStyle::new(
                TextRole::ControlLabel,
                font_size,
                Color::rgba(color[0], color[1], color[2], reveal),
                TextWeight::Regular,
                TextAlign::Start,
            ),
            z: 141,
            clip: None,
        });

        rows.push(ContextMenuItemRow {
            rect: row_rect,
            icon_center: Point::new(icon_cx, icon_cy),
            label_rect: Rect::new(label_cx, label_cy, label_width, font_size),
        });
    }

    render.set_ink_batch(InkLane::ContextMenu, ink);
    // TextViews are not pushed to `render.text` (which routes through
    // GlyphLane::Grid); the render adapter shapes labels into GlyphQuads and
    // submits them on GlyphLane::ContextMenu instead.

    // --- Hit map ------------------------------------------------------------
    let mut hits = HitMap::new();
    // Backdrop dismiss covers the whole viewport and sits below the panel.
    hits.push(HitRegion::new(
        UiId::backdrop("context-menu-modal"),
        Rect::new(0.0, 0.0, vw, vh),
        HitTarget::modal_dismiss_backdrop(),
        90,
    ));
    hits.push(HitRegion::new(
        UiId::context_menu_panel(),
        panel_rect,
        HitTarget::context_menu_panel(),
        100,
    ));
    // Items only become hittable once the content has mostly revealed, matching
    // the folder panel's `progress > 0.9` gate.
    if content_opacity > 0.5 {
        for (index, row) in rows.iter().enumerate() {
            hits.push(HitRegion::new(
                UiId::context_menu_item(input.target, index),
                row.rect,
                HitTarget::context_menu_item(input.target, index),
                140,
            ));
        }
    }

    ContextMenuModel {
        result: LayoutResult::new(render, hits),
        panel_rect,
        rows,
        scale,
    }
}

/// Map a menu item to its procedural icon kind.
fn item_icon_kind(item: ContextMenuItem) -> ControlKind {
    match item {
        ContextMenuItem::EditHome => ControlKind::Pencil,
        ContextMenuItem::HideApp => ControlKind::EyeOff,
        ContextMenuItem::RevealInFinder => ControlKind::FolderIcon,
        ContextMenuItem::ChatGptHelp => ControlKind::ChatGptLogo,
        ContextMenuItem::IconLarger => ControlKind::Plus,
        ContextMenuItem::IconSmaller => ControlKind::Minus,
        ContextMenuItem::AppInfo => ControlKind::Info,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build, menu_rows, open_panel_origin, Color, ContextMenuInput, ContextMenuItem, GlassLayer,
        InkLane, Rect, CONTEXT_MENU_ITEM_COUNT, FOCUS_ROW_OPACITY, MENU_DESTRUCTIVE_RGB,
        MENU_LABEL_RGB,
    };
    use crate::ui_model::render_model::ControlKind;

    /// A folder target has no hide action, no file to reveal, and no app
    /// metadata for a ChatGPT lookup, so its menu omits all three rows (three
    /// rows shorter than the app menu).
    #[test]
    fn folder_menu_omits_the_hide_reveal_and_chatgpt_rows() {
        let (items, labels) = menu_rows(true);
        assert_eq!(items, &ContextMenuItem::FOLDER_ITEMS[..]);
        assert_eq!(labels, &ContextMenuItem::FOLDER_ITEMS_LABELS[..]);
        assert_eq!(items.len(), ContextMenuItem::ALL.len() - 3);
        assert!(!items.contains(&ContextMenuItem::HideApp));
        assert!(!items.contains(&ContextMenuItem::RevealInFinder));
        assert!(!items.contains(&ContextMenuItem::ChatGptHelp));

        // App targets keep all seven rows, in the same order as `ALL`.
        let (app_items, app_labels) = menu_rows(false);
        assert_eq!(app_items, &ContextMenuItem::ALL[..]);
        assert_eq!(app_labels, &ContextMenuItem::ALL_LABELS[..]);
    }

    /// Icon near the right of the viewport so the menu cannot fit on its right
    /// side, but has ample room on the left: it must flip to the left edge,
    /// top-aligned with the icon.
    #[test]
    fn menu_flips_left_when_no_room_on_the_right() {
        let icon = Rect::new(200.0, 100.0, 60.0, 60.0); // max_x = 260
        let ((x, y), _seed) = open_panel_origin(icon, (120.0, 80.0), (350, 300));
        // Right side: 264 + 120 = 384 > 350 − 8 → no fit. Left side:
        // menu right edge = icon left edge (200) − gap (4) → x = 200 − 4 − 120.
        assert!((x - (200.0 - 4.0 - 120.0)).abs() < 0.5);
        // Top-aligned with the icon.
        assert!((y - 100.0).abs() < 0.5);
    }

    /// Icon near the left with ample room on the right: menu attaches to the
    /// right edge, top-aligned.
    #[test]
    fn menu_attaches_to_the_right_by_default() {
        let icon = Rect::new(40.0, 40.0, 60.0, 60.0);
        let ((x, y), seed) = open_panel_origin(icon, (120.0, 80.0), (400, 300));
        // Right side: menu left edge = icon right edge (100) + gap (4).
        assert!((x - (40.0 + 60.0 + 4.0)).abs() < 0.5);
        assert!((y - 40.0).abs() < 0.5);
        // Seed is the icon center.
        assert!((seed.0 - 70.0).abs() < 0.5);
        assert!((seed.1 - 70.0).abs() < 0.5);
    }

    /// Menu taller than the room below the icon: flip to bottom-aligned so the
    /// menu grows upward from the icon's bottom edge.
    #[test]
    fn menu_flips_to_bottom_alignment_when_top_overflows() {
        let icon = Rect::new(40.0, 240.0, 60.0, 60.0); // icon bottom = 300 = vh
        let ((_, y), _) = open_panel_origin(icon, (120.0, 80.0), (400, 300));
        // Top (240) + 80 = 320 > 300 − margin → flip to bottom-aligned:
        // menu bottom = icon bottom (300) → y = 300 − 80.
        assert!((y - (300.0 - 80.0)).abs() < 0.5);
    }

    /// Menu wider than the viewport on both sides of the icon: clamp centered.
    #[test]
    fn menu_clamps_centered_when_neither_side_fits() {
        let icon = Rect::new(90.0, 40.0, 20.0, 20.0); // tiny viewport, big menu
        let ((x, _), _) = open_panel_origin(icon, (300.0, 80.0), (200, 300));
        // Centered on icon center (100), clamped: 100 − 150 = −50 → clamp to margin.
        assert!((x - 8.0).abs() < 0.5);
    }

    #[test]
    fn menu_uses_per_surface_tint_without_opaque_fallback() {
        let mut input = ContextMenuInput {
            viewport: (1280, 800),
            scale_factor: 1.0,
            target: "app:qa-context-menu",
            pos: (320.0, 180.0),
            size: (280.0, 280.0),
            open_size: (280.0, 280.0),
            radius: 28.0,
            content_scale: 1.0,
            content_opacity: 1.0,
            content_blur: 0.0,
            activation: 0.0,
            focus_amounts: [0.0; CONTEXT_MENU_ITEM_COUNT],
            items: &ContextMenuItem::ALL,
            labels: &ContextMenuItem::ALL_LABELS,
        };

        let model = build(&input);
        assert!(
            model.result.render.context_menu_tiles.is_none(),
            "the former opaque TileView workaround must stay removed"
        );
        let surface = model
            .result
            .render
            .glass
            .iter()
            .find(|batch| batch.layer == GlassLayer::ContextMenu)
            .and_then(|batch| batch.surfaces.first())
            .expect("context menu glass surface");
        assert_eq!(surface.tint, Some(Color::rgba(0.93, 0.94, 0.96, 0.68)));
        assert_eq!(surface.blur_radius, Some(24.0));
        assert_eq!(surface.backdrop_replacement, 1.0);

        input.content_blur = 8.0;
        let animated_model = build(&input);
        let animated_surface = animated_model
            .result
            .render
            .glass
            .iter()
            .find(|batch| batch.layer == GlassLayer::ContextMenu)
            .and_then(|batch| batch.surfaces.first())
            .expect("animated context menu glass surface");
        assert_eq!(animated_surface.blur_radius, Some(32.0));
        assert_eq!(animated_surface.backdrop_replacement, 1.0);

        input.content_opacity = 0.4;
        let closing_model = build(&input);
        let closing_surface = closing_model
            .result
            .render
            .glass
            .iter()
            .find(|batch| batch.layer == GlassLayer::ContextMenu)
            .and_then(|batch| batch.surfaces.first())
            .expect("closing context menu glass surface");
        assert_eq!(closing_surface.backdrop_replacement, 0.4);
    }

    #[test]
    fn menu_uses_near_black_label_and_system_red_destructive_colors() {
        assert_eq!(ContextMenuItem::EditHome.foreground_rgb(), MENU_LABEL_RGB);
        assert_eq!(
            ContextMenuItem::HideApp.foreground_rgb(),
            MENU_DESTRUCTIVE_RGB
        );
        assert_ne!(MENU_LABEL_RGB, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn focused_row_emits_a_translucent_pill_behind_its_content() {
        let input = ContextMenuInput {
            viewport: (1280, 800),
            scale_factor: 1.0,
            target: "app:qa-context-menu-focus",
            pos: (320.0, 180.0),
            size: (280.0, 280.0),
            open_size: (280.0, 280.0),
            radius: 28.0,
            content_scale: 1.0,
            content_opacity: 1.0,
            content_blur: 0.0,
            activation: 0.0,
            focus_amounts: [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            items: &ContextMenuItem::ALL,
            labels: &ContextMenuItem::ALL_LABELS,
        };

        let model = build(&input);
        let ink = model
            .result
            .render
            .ink
            .iter()
            .find(|batch| batch.lane == InkLane::ContextMenu)
            .expect("context-menu ink batch");
        let focus = ink
            .views
            .iter()
            .find(|view| view.kind == ControlKind::RowBackground)
            .expect("focused row background");

        assert_eq!(focus.center, model.rows[1].rect.center());
        assert_eq!(focus.opacity, FOCUS_ROW_OPACITY);
        assert_eq!(focus.corner_radius, focus.extent);
        assert_eq!(
            ink.views
                .iter()
                .filter(|view| view.kind == ControlKind::RowBackground)
                .count(),
            1
        );
    }

    #[test]
    fn menu_icons_follow_the_display_order() {
        let input = ContextMenuInput {
            viewport: (1280, 800),
            scale_factor: 1.0,
            target: "app:qa-context-menu-order",
            pos: (320.0, 180.0),
            size: (280.0, 280.0),
            open_size: (280.0, 280.0),
            radius: 28.0,
            content_scale: 1.0,
            content_opacity: 1.0,
            content_blur: 0.0,
            activation: 0.0,
            focus_amounts: [0.0; CONTEXT_MENU_ITEM_COUNT],
            items: &ContextMenuItem::ALL,
            labels: &ContextMenuItem::ALL_LABELS,
        };

        let model = build(&input);
        let icons = model
            .result
            .render
            .ink
            .iter()
            .find(|batch| batch.lane == InkLane::ContextMenu)
            .expect("context-menu icon batch")
            .views
            .iter()
            .map(|view| view.kind.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            icons,
            vec![
                ControlKind::Pencil,
                ControlKind::EyeOff,
                ControlKind::FolderIcon,
                ControlKind::ChatGptLogo,
                ControlKind::Plus,
                ControlKind::Minus,
                ControlKind::Info,
            ]
        );
    }
}

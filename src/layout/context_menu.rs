//! Pure context-menu geometry.
//!
//! Emits the renderer-neutral glass surface, item rows (icon + label), and hit
//! regions for the app-icon right-click menu. Every animated property (panel
//! position, size, corner radius, content scale/opacity/blur, glass activation)
//! is provided by the caller via [`ContextMenuInput`]; this module only lays it
//! out, mirroring the contract of [`crate::layout::folder_panel`].

use crate::layout::hit_map::{HitMap, HitRegion};
use crate::layout::LayoutResult;
use crate::ui_model::geometry::{Point, Rect};
use crate::ui_model::grid::TileAnim;
use crate::ui_model::hit::HitTarget;
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{
    Color, ControlKind, GlassBehavior, GlassLayer, GlassMaterial, GlassSurface, InkLane, InkView,
    RenderModel, TileView,
};
use crate::ui_model::text::{TextAlign, TextRole, TextStyle, TextView, TextWeight};

/// Reference demo row metrics (logical px at 1× DPI).
const ROW_HEIGHT: f32 = 40.0;
const ROW_INNER_MARGIN_X: f32 = 10.0;
const CONTENT_PAD_X: f32 = 30.0;
const CONTENT_PAD_Y: f32 = 20.0;
const ICON_SIZE: f32 = 20.0;
const ICON_GAP: f32 = 17.0;
/// Content-driven panel width is clamped to these bounds (logical px at 1× DPI)
/// so very short or very long localized labels still render comfortably. The
/// app shell measures the longest label and feeds it to
/// [`open_panel_size_logical`].
const MIN_MENU_WIDTH: f32 = 200.0;
const MAX_MENU_WIDTH: f32 = 320.0;
/// Conservative fallback for the longest label width (logical px) when the text
/// engine is unavailable at open time. Roughly the width of the longest current
/// label ("エクスプローラーで開く") at 14 px.
pub const FALLBACK_MAX_LABEL_WIDTH: f32 = 160.0;
/// Font size in logical px (1× DPI), matching the app-icon label size. The
/// renderer's `scale_factor` converts this to physical px.
const FONT_SIZE: f32 = 14.0;

/// One of the six placeholder menu actions. All items are mock actions for
/// this iteration: selecting any of them simply closes the menu.
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
    /// アイコンを大きくする
    IconLarger,
    /// アイコンを小さくする
    IconSmaller,
    /// アプリの概要
    AppInfo,
}

impl ContextMenuItem {
    /// All items in display order.
    pub const ALL: [Self; 6] = [
        Self::EditHome,
        Self::HideApp,
        Self::RevealInFinder,
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
            Self::IconLarger => "アイコンを大きくする",
            Self::IconSmaller => "アイコンを小さくする",
            Self::AppInfo => "アプリの概要",
        }
    }
}

/// Inputs resolved by the app shell from the live [`ContextMenuState`].
#[derive(Debug, Clone)]
pub struct ContextMenuInput<'a> {
    pub viewport: (u32, u32),
    pub scale_factor: f32,
    pub app_id: &'a str,
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

/// Clamp an open panel rect so it stays inside the viewport, anchored near the
/// click point. Returns the panel top-left in physical px.
pub fn open_panel_origin(anchor: (f32, f32), size: (f32, f32), viewport: (u32, u32)) -> (f32, f32) {
    let vw = viewport.0 as f32;
    let vh = viewport.1 as f32;
    // Prefer placing the panel so the anchor sits at its top-left area; shift
    // left/up if it would overflow the viewport.
    let margin = 8.0;
    let mut x = anchor.x() - size.0 * 0.25;
    let mut y = anchor.y() - size.1 * 0.25;
    if x + size.0 > vw - margin {
        x = (vw - margin - size.0).max(margin);
    }
    if y + size.1 > vh - margin {
        y = (vh - margin - size.1).max(margin);
    }
    if x < margin {
        x = margin;
    }
    if y < margin {
        y = margin;
    }
    (x, y)
}

trait TupleXy {
    fn x(&self) -> f32;
    fn y(&self) -> f32;
}

impl TupleXy for (f32, f32) {
    fn x(&self) -> f32 {
        self.0
    }
    fn y(&self) -> f32 {
        self.1
    }
}

/// Build the renderer-neutral model for the context menu.
pub fn build(input: &ContextMenuInput<'_>) -> ContextMenuModel {
    let scale = input.scale_factor.max(0.01);
    let (vw, vh) = (input.viewport.0 as f32, input.viewport.1 as f32);

    let panel_rect = Rect::new(input.pos.0, input.pos.1, input.size.0, input.size.1);

    let mut render = RenderModel::new();

    // --- Opaque background fill ---------------------------------------------
    // `GlassSurface.tint` is not wired into the glass pipeline, so an explicit
    // opaque tile is drawn beneath the glass to give the menu a solid white-ish
    // body. This also visually separates the menu from an open folder panel,
    // whose glass would otherwise smooth-union with this one.
    render.context_menu_tiles = Some(vec![TileView {
        id: UiId::context_menu_panel(),
        rect: panel_rect,
        radius: input.radius,
        color: Color::rgba(0.93, 0.94, 0.96, 1.0),
        has_icon: false,
        motion: TileAnim {
            flags: TileAnim::FLAG_FIXED,
            ..Default::default()
        },
        z: 99,
    }]);

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
            tint: None,
        }],
    );

    // --- Item rows (icon + label ink) ---------------------------------------
    // Row geometry is computed against the *fully-open* panel size and then
    // scaled by `content_scale` about the current animated panel center. Using
    // the fixed open size (not the animated one) keeps item positions stable
    // regardless of where the menu was opened.
    let content_opacity = input.content_opacity.clamp(0.0, 1.0);
    let reveal = content_opacity;

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

    for (index, (_item, label)) in input.items.iter().zip(input.labels.iter()).enumerate() {
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

        // Hit-test row rect (in current panel space).
        let (row_left, row_top) = map(ROW_INNER_MARGIN_X * scale, open_row_y);
        let (row_right, row_bottom) = map(open_w - ROW_INNER_MARGIN_X * scale, open_row_y + row_h);
        let row_rect = Rect::new(
            row_left.min(row_right),
            row_top.min(row_bottom),
            (row_right - row_left).abs(),
            (row_bottom - row_top).abs(),
        );

        ink.push(InkView {
            id: UiId::context_menu_item(input.app_id, index),
            center: Point::new(icon_cx, icon_cy),
            extent: icon_size * 0.5 * content_scale,
            opacity: reveal,
            scene_blur: 0.0,
            stroke: 1.8 * scale * content_scale,
            corner_radius: 0.0,
            color: Color::rgba(0.95, 0.96, 0.98, 1.0),
            kind: item_icon_kind(input.items[index]),
            z: 140,
            clip: None,
        });

        let label_width = (open_w - open_label_x - pad_x).max(1.0);
        text_views.push(TextView {
            id: UiId::context_menu_item(input.app_id, index),
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
                Color::rgba(0.95, 0.96, 0.98, reveal),
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
                UiId::context_menu_item(input.app_id, index),
                row.rect,
                HitTarget::context_menu_item(input.app_id, index),
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
        ContextMenuItem::IconLarger => ControlKind::Plus,
        ContextMenuItem::IconSmaller => ControlKind::Minus,
        ContextMenuItem::AppInfo => ControlKind::Info,
    }
}

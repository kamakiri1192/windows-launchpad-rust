//! Context menu app adapter. Joins the live [`ContextMenuState`] to the pure
//! [`layout::context_menu`] builder, then submits the result to the renderer
//! model on the dedicated `ContextMenu` glass/ink/glyph lanes. These lanes
//! are isolated from the folder panel's `Modal` lanes so the menu can float
//! above an open folder without their Liquid Glass smooth-unioning together.

use crate::app::event::AppCommand;
use crate::app::state::App;
use crate::domain::app_id::AppId;
use crate::domain::launcher_item::LauncherItem;
use crate::features::context_menu::MenuTarget;
use crate::layout::context_menu::{
    self, open_panel_origin, open_panel_size_logical, ContextMenuInput, ContextMenuItemRow,
    ContextMenuModel, CONTEXT_MENU_BASE_BLUR, CONTEXT_MENU_HOVER_RGB, CONTEXT_MENU_TINT_ALPHA,
    FOCUS_ROW_OPACITY, FOCUS_ROW_VERTICAL_INSET,
};
use crate::layout::hit_map::{HitMap, HitRegion};
use crate::layout::LayoutResult;
use crate::renderer::text_engine::{self, GlyphQuad, TextRenderer, UI_FONT_FAMILY};
use crate::ui_model::geometry::{Insets, Point, Rect};
use crate::ui_model::hit::HitTarget;
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{
    Color, ControlKind, GlassBehavior, GlassLayer, GlassMaterial, GlassSurface, GlyphLane, InkLane,
    InkView, RenderModel,
};

/// Menu font metrics, in logical px at 1× DPI. Match the app-icon label size
/// (`LABEL_FONT_SIZE` = 14) so the menu reads at the same scale as the grid.
/// The text renderer multiplies these by the DPI `scale_factor`.
const MENU_FONT_SIZE: f32 = 14.0;
const MENU_LINE_HEIGHT: f32 = 18.0;

/// Derive the `(is_folder, is_steam)` row-set flags the pure layout layer
/// needs from the menu's target. Every call site that picks menu rows (open,
/// release, per-frame render) funnels through here, so the rendered rows and
/// the release-time row resolution always pass the same flags. A folder is
/// never a Steam app; a Steam app is never a folder.
fn menu_target_flags(target: Option<&LauncherItem>) -> (bool, bool) {
    match target {
        Some(LauncherItem::Folder(_)) => (true, false),
        Some(LauncherItem::App(id)) => (false, id.is_steam()),
        None => (false, false),
    }
}

/// Retained context-menu layout + label shaping.
///
/// The menu animates its position, size, corner radius, content scale,
/// opacity, blur, and activation on every frame while open, but the row set,
/// fully-open panel size, DPI scale, and quantized label font size only change
/// when the target changes or the reveal animation steps to a new whole-pixel
/// font size. Re-running the pure layout build and re-shaping the labels with
/// cosmic-text on every frame is pure per-frame waste (the shaping alone is a
/// few hundred µs): this cache stores the open-panel-space row geometry — the
/// layout, independent of the animation transform — plus the shaped label
/// quads. Each frame only maps the cached geometry through the live animated
/// transform (the same math [`layout::context_menu::build`] would apply) and
/// translates/recolors the cached quads, so a steady frame performs no
/// allocation-heavy layout and no shaping at all.
#[derive(Debug)]
pub(crate) struct ContextMenuLayoutCache {
    key: ContextMenuLayoutKey,
    scale: f32,
    open_w: f32,
    open_h: f32,
    row_h: f32,
    icon_size: f32,
    /// `ROW_INNER_MARGIN_X` scaled to physical px (private to the layout
    /// module; captured here so the derivation needs no layout constants).
    row_inner_margin: f32,
    rows: Vec<ContextMenuStaticRow>,
    /// Label glyphs shaped at the cache-build anchors for the cached quantized
    /// font size. Lazily shaped: the reveal animation starts below the
    /// visibility threshold.
    glyphs: Vec<Option<CachedMenuGlyph>>,
}

/// Everything the cached layout depends on — the animated values (position,
/// size, radius, content scale/opacity/blur, activation, focus amounts) are
/// deliberately absent so they never invalidate the cache.
#[derive(Debug, Clone, PartialEq)]
struct ContextMenuLayoutKey {
    /// Stable key of the menu target (also the UiId prefix for row ids).
    target: String,
    /// Fully-open panel size in physical px.
    open_size: (f32, f32),
    /// DPI scale, bit pattern.
    scale_bits: u32,
    /// Quantized physical label font px. Steps during the reveal animation;
    /// the only font-size dimension that re-shapes the labels.
    font_px: u32,
}

/// Row geometry in fully-open panel space (physical px). The animation
/// transform maps these coordinates into the live panel each frame.
#[derive(Debug, Clone)]
struct ContextMenuStaticRow {
    open_row_y: f32,
    open_row_cy: f32,
    open_icon_x: f32,
    open_label_x: f32,
    label_width: f32,
    /// Icon control kind, matching the layout's private `item_icon_kind`
    /// mapping (verified by the round-trip test against `context_menu::build`).
    kind: ControlKind,
}

/// Shaped label glyphs plus the anchors they were shaped at; per-frame frames
/// translate them to the live label rects without re-shaping.
#[derive(Debug, Clone)]
struct CachedMenuGlyph {
    /// Label left anchor (unrounded `label_rect.x`) at shaping time.
    left: f32,
    /// Label vertical center anchor (`label_rect.y`) at shaping time.
    cy: f32,
    quads: Vec<GlyphQuad>,
}

impl ContextMenuLayoutCache {
    /// Build the open-panel-space geometry for `items`. `key` is stored
    /// verbatim; the glyphs are shaped lazily by [`Self::glyphs_for_frame`].
    fn build(
        items: &[context_menu::ContextMenuItem],
        open_size: (f32, f32),
        scale: f32,
        key: ContextMenuLayoutKey,
    ) -> Self {
        // Row metrics mirror the layout module's private constants (see
        // `layout::context_menu::build`); the round-trip test pins them.
        let open_w = open_size.0.max(1.0);
        let open_h = open_size.1.max(1.0);
        let row_h = 40.0 * scale;
        let pad_x = 30.0 * scale;
        let pad_y = 20.0 * scale;
        let icon_size = 24.0 * scale;
        let icon_gap = 17.0 * scale;
        let row_inner_margin = 10.0 * scale;
        let rows = items
            .iter()
            .enumerate()
            .map(|(index, item)| ContextMenuStaticRow {
                open_row_y: pad_y + index as f32 * row_h,
                open_row_cy: pad_y + index as f32 * row_h + row_h * 0.5,
                open_icon_x: pad_x,
                open_label_x: pad_x + icon_size + icon_gap,
                label_width: (open_w - (pad_x + icon_size + icon_gap) - pad_x).max(1.0),
                kind: context_menu_item_kind(*item),
            })
            .collect();
        Self {
            key,
            scale,
            open_w,
            open_h,
            row_h,
            icon_size,
            row_inner_margin,
            rows,
            glyphs: vec![None; items.len()],
        }
    }

    /// Derive the animated render model from the cached open-panel-space
    /// geometry, reproducing [`layout::context_menu::build`] bit for bit (the
    /// round-trip test asserts equality across the animation).
    fn derive_model(&self, input: &ContextMenuInput<'_>) -> ContextMenuModel {
        let scale = self.scale;
        let (vw, vh) = (input.viewport.0 as f32, input.viewport.1 as f32);
        let panel_rect = Rect::new(input.pos.0, input.pos.1, input.size.0, input.size.1);
        let content_opacity = input.content_opacity.clamp(0.0, 1.0);
        let reveal = content_opacity;

        let mut render = RenderModel::new();
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
                backdrop_replacement: content_opacity,
                tint: Some(Color::rgba(
                    0.93,
                    0.94,
                    0.96,
                    CONTEXT_MENU_TINT_ALPHA * content_opacity,
                )),
            }],
        );

        let open_w = self.open_w;
        let open_h = self.open_h;
        let content_scale = input.content_scale.max(0.0);
        let panel_center_x = panel_rect.x + panel_rect.width * 0.5;
        let panel_center_y = panel_rect.y + panel_rect.height * 0.5;

        let mut ink: Vec<InkView> = Vec::new();
        let mut rows: Vec<ContextMenuItemRow> = Vec::new();

        for (index, (row, item)) in self.rows.iter().zip(input.items.iter()).enumerate() {
            // Position in fully-open panel space (cached), then map into the
            // live animated panel — the exact `map` closure of the layout.
            let map = |ox: f32, oy: f32| -> (f32, f32) {
                let dx = (ox - open_w * 0.5) * content_scale;
                let dy = (oy - open_h * 0.5) * content_scale;
                (panel_center_x + dx, panel_center_y + dy)
            };
            let (icon_cx, icon_cy) = map(row.open_icon_x + self.icon_size * 0.5, row.open_row_cy);
            let (label_cx, label_cy) = map(row.open_label_x, row.open_row_cy);
            let color = item.foreground_rgb();

            // Hit-test row rect (in current panel space).
            let (row_left, row_top) = map(self.row_inner_margin, row.open_row_y);
            let (row_right, row_bottom) =
                map(open_w - self.row_inner_margin, row.open_row_y + self.row_h);
            let row_rect = Rect::new(
                row_left.min(row_right),
                row_top.min(row_bottom),
                (row_right - row_left).abs(),
                (row_bottom - row_top).abs(),
            );

            // Draw the focus treatment before the row's icon and label so the
            // foreground stays crisp above the translucent neutral material.
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
                    color: Color::rgba(
                        CONTEXT_MENU_HOVER_RGB[0],
                        CONTEXT_MENU_HOVER_RGB[1],
                        CONTEXT_MENU_HOVER_RGB[2],
                        1.0,
                    ),
                    kind: ControlKind::RowBackground,
                    z: 130,
                    clip: None,
                });
            }

            ink.push(InkView {
                id: UiId::context_menu_item(input.target, index),
                center: Point::new(icon_cx, icon_cy),
                extent: self.icon_size * 0.5 * content_scale,
                opacity: reveal,
                scene_blur: 0.0,
                stroke: 1.8 * scale * content_scale,
                corner_radius: 0.0,
                color: Color::rgba(color[0], color[1], color[2], 1.0),
                kind: row.kind.clone(),
                z: 140,
                clip: None,
            });

            rows.push(ContextMenuItemRow {
                rect: row_rect,
                icon_center: Point::new(icon_cx, icon_cy),
                label_rect: Rect::new(label_cx, label_cy, row.label_width, MENU_FONT_SIZE),
            });
        }

        render.set_ink_batch(InkLane::ContextMenu, ink);

        // Hit map: backdrop dismiss, panel, and per-row regions gated on the
        // reveal like the layout.
        let mut hits = HitMap::new();
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

    /// Shape the label glyphs on cache misses, or translate + recolor the
    /// cached quads to the live label rects on hits. Appends the frame's
    /// glyphs to `out` in row order.
    #[allow(clippy::too_many_arguments)]
    fn glyphs_for_frame(
        &mut self,
        t: &mut TextRenderer,
        items: &[context_menu::ContextMenuItem],
        rows: &[ContextMenuItemRow],
        opacity: f32,
        scale: f32,
        content_scale: f32,
        out: &mut Vec<GlyphQuad>,
    ) {
        let text_scale = quantize_menu_text_scale(content_scale, scale);
        for (index, (entry, row)) in self.glyphs.iter_mut().zip(rows.iter()).enumerate() {
            let left = row.label_rect.x;
            let center_y = row.label_rect.y;
            match entry {
                Some(cached) => {
                    // The cached quads embed the shape-time rounded anchors;
                    // translate by the exact rounded x delta (the layout's
                    // `left.round()` origin) and the fractional y delta. The
                    // stored quads are never mutated: translating them in
                    // place would compound the per-frame deltas and let the
                    // text drift away from the animated anchors.
                    let dx = left.round() - cached.left.round();
                    let dy = center_y - cached.cy;
                    let color = menu_text_color(items[index], opacity);
                    out.extend(cached.quads.iter().map(|quad| {
                        let mut translated = *quad;
                        translated.x += dx;
                        translated.y += dy;
                        translated.color = color;
                        translated
                    }));
                }
                None => {
                    let mut quads = Vec::new();
                    push_menu_text(
                        t,
                        &mut quads,
                        items[index].label(),
                        left,
                        center_y,
                        MENU_FONT_SIZE * text_scale,
                        MENU_LINE_HEIGHT * text_scale,
                        menu_text_color(items[index], opacity),
                        scale,
                    );
                    out.extend_from_slice(&quads);
                    *entry = Some(CachedMenuGlyph {
                        left,
                        cy: center_y,
                        quads,
                    });
                }
            }
        }
    }
}

/// Map a menu item to its procedural icon kind, mirroring the layout module's
/// private `item_icon_kind`. The round-trip test against
/// `layout::context_menu::build` pins this mapping to the layout.
fn context_menu_item_kind(item: context_menu::ContextMenuItem) -> ControlKind {
    match item {
        context_menu::ContextMenuItem::EditHome => ControlKind::Pencil,
        context_menu::ContextMenuItem::HideApp => ControlKind::EyeOff,
        context_menu::ContextMenuItem::RevealInFinder => ControlKind::FolderIcon,
        context_menu::ContextMenuItem::ChatGptHelp => ControlKind::ChatGptLogo,
        context_menu::ContextMenuItem::OpenInSteam => ControlKind::SteamLogo,
        context_menu::ContextMenuItem::IconLarger => ControlKind::Plus,
        context_menu::ContextMenuItem::IconSmaller => ControlKind::Minus,
        context_menu::ContextMenuItem::AppInfo => ControlKind::Info,
    }
}

impl App {
    /// Open the context menu for `target` (an app or folder), attached to the
    /// right-clicked icon's `icon_rect` (physical on-screen px). The menu
    /// attaches to the icon edge, flipping sides when there is no room
    /// (iOS-style), and the open seed blooms from the icon center. The menu
    /// uses its own `ContextMenu` glass lane, isolated from the folder/settings
    /// `Modal` lane.
    pub(crate) fn open_context_menu(&mut self, target: LauncherItem, icon_rect: Rect) {
        let target_key = target.stable_key();
        crate::debug_log!(
            "context-menu: open requested target={target_key} previous_phase={:?} active={}",
            self.context_menu.phase,
            self.context_menu.is_active(),
        );
        if self.control.wants_keyboard() {
            self.control.press_close();
        }
        self.pending_press = None;
        // A previous menu model can still be retained until its close tail
        // finishes. Do not let a pointer move re-hit-test that stale geometry
        // while this menu is opening; the new menu starts with no focused row.
        self.context_menu_layout = None;

        // Compute the stable key once here; the per-frame layout reads it back
        // from `context_menu_target_key` instead of re-running `format!`.
        self.context_menu_target_key = Some(target_key.clone());

        let scale = self.scale_factor.max(0.01);
        // Measure the longest label once at open time so the open-animation
        // target and the per-frame layout agree on the same panel width. The
        // measured set is the rows this target will display: a folder menu
        // omits four rows, a non-Steam app omits the Steam row. This is built
        // here only to size the panel; `ContextMenuState::open` rebuilds and
        // caches the same list (same target → same flags), which the render and
        // release paths then read.
        let (is_folder, is_steam) = menu_target_flags(Some(&target));
        let items = context_menu::menu_rows(is_folder, is_steam);
        let max_label_w = self.measure_menu_max_label_width_logical(scale, &items);
        self.context_menu_open_width_logical = max_label_w;
        let (lw, lh) = open_panel_size_logical(items.len(), max_label_w);
        let size_phys = (lw * scale, lh * scale);
        let ((origin_x, origin_y), seed) =
            open_panel_origin(icon_rect, size_phys, self.viewport_phys());
        let menu_target = MenuTarget {
            x: origin_x,
            y: origin_y,
            width: size_phys.0,
            height: size_phys.1,
        };
        self.context_menu.open(target, seed.0, seed.1, menu_target);
        crate::debug_log!(
            "context-menu: phase=Opening target={target_key} seed=({:.1},{:.1}) panel=({:.1},{:.1},{:.1},{:.1})",
            seed.0,
            seed.1,
            menu_target.x,
            menu_target.y,
            menu_target.width,
            menu_target.height,
        );
        self.request_redraw();
    }

    /// Begin the close animation. The menu stays visible until the close
    /// animation finishes.
    pub(crate) fn close_context_menu(&mut self) {
        if !self.context_menu.is_active() {
            return;
        }
        crate::debug_log!(
            "context-menu: close requested phase={:?} target={:?}",
            self.context_menu.phase,
            self.context_menu_target_key,
        );
        self.context_menu.close();
        self.request_redraw();
    }

    /// Press while the menu is open. Outside the panel dismisses it; a
    /// top-level backdrop click may also continue to the underlying page.
    pub(crate) fn handle_context_menu_pointer_press(&mut self, x: f32, y: f32) {
        let inside = self
            .context_menu_layout
            .as_ref()
            .map(|m| {
                m.panel_rect
                    .contains(crate::ui_model::geometry::Point::new(x, y))
            })
            .unwrap_or(false);
        if !inside {
            self.close_context_menu();

            // A click on another app/folder is a dismissal gesture, not a
            // second activation. Passing that press through used to leave a
            // pending grid click behind, so releasing outside the menu could
            // launch an unrelated app. Preserve passthrough for top-level
            // backdrop areas or continuing a page gesture, but never hand an
            // interactive tile the same pointer contact. A folder-child menu
            // keeps the folder open for every outside dismissal location.
            let over_interactive_item = if self.folders.is_active() {
                self.folder_layout
                    .as_ref()
                    .and_then(|layout| {
                        layout
                            .result
                            .hits
                            .hit_test(crate::ui_model::geometry::Point::new(x, y))
                    })
                    .is_some_and(|hit| {
                        matches!(
                            hit.target,
                            crate::ui_model::hit::HitTarget::FolderChild { .. }
                        )
                    })
            } else {
                matches!(
                    self.grid_hit_at_pointer(x, y),
                    crate::layout::grid::GridHit::App(_)
                )
            };
            if Self::should_pass_context_menu_dismissal_through(
                self.folders.is_active(),
                over_interactive_item,
            ) {
                // Closing is visual-only, so reclassifying now hands this
                // same gesture to the page or folder below. Recursion stops
                // after this one handoff because a closing menu no longer
                // accepts input.
                let action = self.classify_pointer_press(x, y);
                self.handle_pointer_press(action);
            }
        }
    }

    /// Whether a click outside the context-menu panel should continue into the
    /// underlying surface after dismissing the menu.
    ///
    /// A top-level backdrop click can still continue through to the launcher, but
    /// a context menu opened from a folder child must not turn the same click into
    /// a folder dismissal. The folder remains open; only the menu owns the
    /// dismissal gesture.
    fn should_pass_context_menu_dismissal_through(
        folder_open: bool,
        over_interactive_item: bool,
    ) -> bool {
        !folder_open && !over_interactive_item
    }

    /// Release while the menu is open. Inside a row → run the selected action;
    /// outside → already closed by the press, or close now.
    pub(crate) fn handle_context_menu_pointer_release(&mut self, x: f32, y: f32) {
        if !self.context_menu.is_active() {
            return;
        }
        // Read the cached row list (built at open time) so the release path
        // resolves the same rows the renderer laid out — never rebuild per
        // release.
        let items: &[context_menu::ContextMenuItem] = &self.context_menu.items;
        let selection = resolve_context_menu_selection(
            items,
            self.context_menu_hit_target(x, y),
            self.context_menu.active_target.as_ref(),
        );
        // Close the menu before running the action: edit mode suppresses the
        // right-click menu, and hide relayouts the grid beneath the panel.
        self.close_context_menu();
        match selection {
            ContextMenuSelection::EditHome => self.enter_edit_mode(None),
            ContextMenuSelection::HideApp(id) => self.hide_app(&id),
            ContextMenuSelection::RevealApp(id) => {
                // The reveal runs through the app command boundary like a
                // launch: the executor hides the window first, then asks the
                // OS file manager to select the app's file.
                if let Some(info) = self.registry.launch_info(&id) {
                    self.execute_command(AppCommand::RevealApp(info));
                }
            }
            ContextMenuSelection::ChatGptHelp(id) => {
                // Look up the same launch info (name + version) the reveal path
                // uses, then ask ChatGPT how to use the app. The launcher is
                // not hidden (see AppCommand::AskChatGpt).
                if let Some(info) = self.registry.launch_info(&id) {
                    self.execute_command(AppCommand::AskChatGpt(info));
                }
            }
            ContextMenuSelection::OpenInSteam(id) => {
                // The Steam row runs through the app command boundary like a
                // launch: the executor hides the window first, then opens the
                // app's Steam library page via the steam:// protocol handler.
                self.execute_command(AppCommand::OpenInSteam { app_id: id });
            }
            // Mock actions (and folder targets of hide/reveal): just close.
            ContextMenuSelection::CloseOnly => {}
        }
    }

    /// Update the row focus target from the current pointer. The feature state
    /// eases each row independently so moving between rows cross-fades the
    /// outgoing and incoming pills instead of switching them in one frame.
    pub(crate) fn update_context_menu_hover(&mut self, x: f32, y: f32) {
        if !self.context_menu.accepts_pointer_input() {
            return;
        }
        let hovered = self.context_menu_hit_target(x, y);
        if self.context_menu.set_hovered_item(hovered) {
            self.request_redraw();
        }
    }

    fn context_menu_hit_target(&self, x: f32, y: f32) -> Option<usize> {
        // While the open/close morph is still scaling the rows, each row's hit
        // rect is expanded by `content_scale` (2.0 → 1.0). Those expanded rects
        // reach the cursor that is still over the right-clicked icon and would
        // focus the middle (panel-center) row on the very first pointer move.
        // Skip the hit test until the rows have settled at their resting scale.
        if !self.context_menu.row_hit_test_enabled() {
            return None;
        }
        let model = self.context_menu_layout.as_ref()?;
        let p = crate::ui_model::geometry::Point::new(x, y);
        for (index, row) in model.rows.iter().enumerate() {
            if row.rect.contains(p) {
                return Some(index);
            }
        }
        None
    }

    /// Build the context-menu render model from the live animation state and
    /// submit it to the Modal lanes. Called from the frame loop while the menu
    /// is active.
    pub(crate) fn render_context_menu(&mut self) {
        if self.context_menu.active_target.is_none() {
            self.clear_context_menu_presentation();
            return;
        }

        let scale = self.scale_factor.max(0.01);
        // The row set is cached on the state at open time; read it directly so
        // the per-frame render path never calls `menu_rows` (zero allocation)
        // and the laid-out rows always match the rows the release path resolves
        // against.
        let items: &[context_menu::ContextMenuItem] = &self.context_menu.items;

        // The fully-open panel size is fixed at open time and stays constant
        // through the animation; the live (animated) size is separate. We reuse
        // the label width measured in `open_context_menu` so the laid-out rows
        // match the animated panel exactly.
        let (open_lw, open_lh) =
            open_panel_size_logical(items.len(), self.context_menu_open_width_logical);
        let open_size = (open_lw * scale, open_lh * scale);

        // `stable_key` was computed once at open time; borrow it instead of
        // re-running `format!` (and cloning the target) every frame.
        let target_key = self.context_menu_target_key.as_deref().unwrap_or_default();
        let content_scale = self.context_menu.content_scale().max(0.0);
        let opacity = self.context_menu.content_opacity();
        // cosmic-text includes the exact f32 font size in its cache key. The
        // menu animates `content_scale` every frame, so pass a
        // physical-pixel-quantized scale to keep repeated open/close
        // animations from manufacturing unbounded glyph variants — and to key
        // the retained shaping on a bounded set of font sizes.
        let text_scale = quantize_menu_text_scale(content_scale, scale);
        let font_px = (MENU_FONT_SIZE * text_scale * scale).round() as u32;

        let input = ContextMenuInput {
            viewport: self.viewport_phys(),
            scale_factor: scale,
            target: target_key,
            pos: (self.context_menu.pos_x(), self.context_menu.pos_y()),
            size: (self.context_menu.width(), self.context_menu.height()),
            open_size,
            radius: self.context_menu.radius(),
            content_scale,
            content_opacity: opacity,
            content_blur: self.context_menu.content_blur(),
            activation: self.context_menu.activation(),
            focus_amounts: self.context_menu.focus_amounts(),
            items,
        };

        // Retained layout + shaping, keyed on everything except the animated
        // values (target/items, panel width, quantized font size, DPI). While
        // the reveal animation runs, only the animation parameters change per
        // frame — the open-panel-space geometry is re-mapped through them and
        // the cached label quads are translated/recolored, so a steady frame
        // performs no layout build and no shaping.
        let cache_miss = self.context_menu_layout_cache.as_ref().is_none_or(|cache| {
            cache.key.target != target_key
                || cache.key.open_size != open_size
                || cache.key.scale_bits != scale.to_bits()
                || cache.key.font_px != font_px
        });
        if cache_miss {
            self.context_menu_layout_cache = Some(ContextMenuLayoutCache::build(
                items,
                open_size,
                scale,
                ContextMenuLayoutKey {
                    target: target_key.to_owned(),
                    open_size,
                    scale_bits: scale.to_bits(),
                    font_px,
                },
            ));
        }
        let cache = self
            .context_menu_layout_cache
            .as_mut()
            .expect("context menu cache must exist after build");
        let model = cache.derive_model(&input);

        // Promote the layout's ink/glass into the shared Modal lanes.
        let modal = model
            .result
            .render
            .glass
            .iter()
            .find(|batch| batch.layer == GlassLayer::ContextMenu)
            .map(|batch| batch.surfaces.clone())
            .unwrap_or_default();
        let ink = model
            .result
            .render
            .ink
            .iter()
            .find(|batch| batch.lane == InkLane::ContextMenu)
            .map(|batch| batch.views.clone())
            .unwrap_or_default();

        // Shape label text into glyph quads. We render only when the content
        // has meaningfully revealed to avoid wasted raster work mid-collapse.
        // Pre-size for the menu's ~6 short CJK labels (a handful of glyphs each)
        // so the loop doesn't trigger reallocations.
        let mut glyphs: Vec<GlyphQuad> = Vec::with_capacity(items.len() * 8);
        if opacity > crate::features::context_menu::CONTENT_VISIBILITY_THRESHOLD {
            if let Some(text) = self.text.as_mut() {
                cache.glyphs_for_frame(
                    text,
                    items,
                    &model.rows,
                    opacity,
                    scale,
                    content_scale,
                    &mut glyphs,
                );
            }
        }

        // The menu owns the Modal lane exclusively (the folder/settings panels
        // are dismissed on open), so a plain replace is correct and keeps the
        // Liquid Glass modal pass on a single surface.
        //
        // Glass has no opacity field, so once the content has faded below the
        // reveal threshold we drop the glass disc entirely — otherwise the
        // collapsed seed (40×40, radius 130 = full disc) lingers until the slow
        // close position spring settles. We still keep the layout so hit/dismiss
        // logic stays valid during the close tail.
        if opacity > crate::features::context_menu::CONTENT_VISIBILITY_THRESHOLD {
            self.render_model
                .set_glass_batch(GlassLayer::ContextMenu, modal);
            self.render_model.set_ink_batch(InkLane::ContextMenu, ink);
            self.render_model
                .set_glyph_batch(GlyphLane::ContextMenu, glyph_views(&glyphs));
            self.render_model.context_menu_tiles = model.result.render.context_menu_tiles.clone();
            self.render_model.context_menu_icons = model.result.render.context_menu_icons.clone();
        } else {
            self.render_model
                .set_glass_batch(GlassLayer::ContextMenu, Vec::new());
            self.render_model
                .set_ink_batch(InkLane::ContextMenu, Vec::new());
            self.render_model
                .set_glyph_batch(GlyphLane::ContextMenu, Vec::new());
            self.render_model.context_menu_tiles = Some(Vec::new());
            self.render_model.context_menu_icons = Some(Vec::new());
        }

        // Keep the text atlas current so the renderer uploads any newly
        // rasterized menu glyphs. Every other text adapter does this;
        // omitting it left the menu's new glyphs missing from the GPU
        // texture, so the menu text vanished (and, once the base atlas
        // filled, every other lane's text too).
        if let (Some(renderer), Some(text)) = (self.renderer.as_mut(), self.text.as_ref()) {
            if text.atlas_dirty {
                let (aw, ah) = text.atlas_dimensions();
                renderer.upload_atlas(text.atlas_rgba(), aw, ah);
            }
        }
        if let Some(text) = self.text.as_mut() {
            text.atlas_dirty = false;
        }

        self.context_menu_layout = Some(model);
    }

    /// Drop the context menu's Modal-lane content (called when the menu is
    /// fully closed). The Modal lane is exclusive — the folder/settings panels
    /// are already dismissed — so a full clear is correct.
    pub(crate) fn clear_context_menu_presentation(&mut self) {
        self.render_model
            .set_glass_batch(GlassLayer::ContextMenu, Vec::new());
        self.render_model
            .set_ink_batch(InkLane::ContextMenu, Vec::new());
        self.render_model
            .set_glyph_batch(GlyphLane::ContextMenu, Vec::new());
        self.render_model.context_menu_tiles = Some(Vec::new());
        self.render_model.context_menu_icons = Some(Vec::new());
        self.context_menu_layout = None;
        self.context_menu_layout_cache = None;
        self.context_menu_target_key = None;
    }

    /// Measure the widest label among `items` and return its width in logical
    /// px at 1× DPI (i.e. the physical measurement divided by `scale`). Used
    /// once at open time to size the panel to its content. Falls back to the
    /// layout layer's [`FALLBACK_MAX_LABEL_WIDTH`] when the text engine is
    /// absent.
    fn measure_menu_max_label_width_logical(
        &mut self,
        scale: f32,
        items: &[context_menu::ContextMenuItem],
    ) -> f32 {
        let Some(t) = self.text.as_mut() else {
            return context_menu::FALLBACK_MAX_LABEL_WIDTH;
        };
        let mut widest_phys = 0.0f32;
        for item in items {
            let w = t.measure_text(&text_engine::CenteredLineSpec {
                text: item.label(),
                font_size: MENU_FONT_SIZE,
                line_height: MENU_LINE_HEIGHT,
                family: UI_FONT_FAMILY,
                color: [1.0; 4],
                center: (0.0, 0.0),
                scale_factor: scale,
            });
            widest_phys = widest_phys.max(w);
        }
        (widest_phys / scale.max(0.01)).max(0.0)
    }
}

/// Snap an animated scale to whole physical font pixels. This preserves the
/// 1×→2× menu motion while restricting the atlas to a small, repeatable set
/// of glyph cache keys at every DPI scale.
fn quantize_menu_text_scale(content_scale: f32, dpi_scale: f32) -> f32 {
    let dpi_scale = dpi_scale.max(0.01);
    let physical_font_px = MENU_FONT_SIZE * content_scale.max(0.0) * dpi_scale;
    physical_font_px.round() / (MENU_FONT_SIZE * dpi_scale)
}

#[allow(clippy::too_many_arguments)]
fn push_menu_text(
    t: &mut text_engine::TextRenderer,
    quads: &mut Vec<GlyphQuad>,
    value: &str,
    left: f32,
    center_y: f32,
    font_size: f32,
    line_height: f32,
    color: [f32; 4],
    scale: f32,
) {
    // Single shaping pass: the menu left-aligns each label inside its row, so
    // the left-anchored path returns both the quads and the width without the
    // separate `measure_text` the old centered approach needed. The horizontal
    // origin (`left.round()`) matches what the old path produced after
    // measure→center→recenter, so glyph subpixel bins are unchanged.
    let (row_quads, _width) = t.layout_left_anchored_line_with_width(
        value,
        left,
        center_y,
        font_size,
        line_height,
        UI_FONT_FAMILY,
        color,
        scale,
    );
    quads.extend(row_quads);
}

fn menu_text_color(item: context_menu::ContextMenuItem, opacity: f32) -> [f32; 4] {
    let rgb = item.foreground_rgb();
    [rgb[0], rgb[1], rgb[2], opacity.clamp(0.0, 1.0)]
}

/// What a released menu row should do. Resolved from the row hit and the
/// menu's target; the app shell runs the side effects after closing the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ContextMenuSelection {
    /// ホーム画面を編集: enter edit mode on the home grid (✕ badges appear).
    EditHome,
    /// アプリを非表示: hide the target app, mirroring the ✕ badge path.
    HideApp(AppId),
    /// Finderで開く / エクスプローラーで開く: reveal the target app in the OS
    /// file manager, mirroring the launch path (via `AppCommand::RevealApp`).
    RevealApp(AppId),
    /// ChatGPTで使い方を調べる: open ChatGPT with a how-to-use prompt built
    /// from the target app's name + version (via `AppCommand::AskChatGpt`).
    ChatGptHelp(AppId),
    /// Steamで開く: open the target app's Steam library page via the
    /// `steam://` protocol handler (via `AppCommand::OpenInSteam`). Only the
    /// Steam-app menu shows this row, and it only resolves when the target is
    /// a Steam app.
    OpenInSteam(AppId),
    /// Mock action (or a folder target of hide/reveal, which have no file on
    /// disk to act on): just close the menu.
    CloseOnly,
}

/// Resolve a release inside the menu into the action to run. `items` is the
/// row set the menu is actually displaying (folder menus omit the hide-app
/// row, so its row indices never land on [`ContextMenuSelection::HideApp`]).
/// An outside release (no row hit) and every mock row resolve to
/// [`ContextMenuSelection::CloseOnly`]. The Steam row resolves to
/// [`ContextMenuSelection::OpenInSteam`] only when the target is a Steam app;
/// on any other target (or no target) it falls back to close-only.
fn resolve_context_menu_selection(
    items: &[context_menu::ContextMenuItem],
    row: Option<usize>,
    target: Option<&LauncherItem>,
) -> ContextMenuSelection {
    let Some(index) = row else {
        return ContextMenuSelection::CloseOnly;
    };
    match items.get(index) {
        Some(context_menu::ContextMenuItem::EditHome) => ContextMenuSelection::EditHome,
        Some(context_menu::ContextMenuItem::HideApp) => match target {
            Some(LauncherItem::App(id)) => ContextMenuSelection::HideApp(id.clone()),
            _ => ContextMenuSelection::CloseOnly,
        },
        Some(context_menu::ContextMenuItem::RevealInFinder) => match target {
            Some(LauncherItem::App(id)) => ContextMenuSelection::RevealApp(id.clone()),
            _ => ContextMenuSelection::CloseOnly,
        },
        Some(context_menu::ContextMenuItem::ChatGptHelp) => match target {
            Some(LauncherItem::App(id)) => ContextMenuSelection::ChatGptHelp(id.clone()),
            _ => ContextMenuSelection::CloseOnly,
        },
        Some(context_menu::ContextMenuItem::OpenInSteam) => match target {
            Some(LauncherItem::App(id)) if id.is_steam() => {
                ContextMenuSelection::OpenInSteam(id.clone())
            }
            _ => ContextMenuSelection::CloseOnly,
        },
        _ => ContextMenuSelection::CloseOnly,
    }
}

fn glyph_views(quads: &[GlyphQuad]) -> Vec<crate::ui_model::render_model::GlyphView> {
    quads
        .iter()
        .map(|q| crate::ui_model::render_model::GlyphView {
            presentation_pivot: None,
            id: crate::ui_model::ids::UiId::named("context-menu-glyph"),
            rect: crate::ui_model::geometry::Rect::new(q.x, q.y, q.w, q.h),
            uv: crate::ui_model::geometry::UvRect {
                u0: q.u0,
                v0: q.v0,
                u1: q.u1,
                v1: q.v1,
            },
            color: crate::ui_model::render_model::Color::rgba(
                q.color[0], q.color[1], q.color[2], q.color[3],
            ),
            z: 141,
            clip: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn animated_menu_scales_reuse_a_bounded_set_of_physical_font_sizes() {
        let dpi_scale = 1.25;
        let sampled: BTreeSet<_> = (0..=1_000)
            .map(|step| 1.0 + step as f32 / 1_000.0)
            .map(|scale| quantize_menu_text_scale(scale, dpi_scale))
            .map(|scale| (MENU_FONT_SIZE * scale * dpi_scale).round() as u32)
            .collect();

        // The 1×→2× transition at 125% DPI spans only 18 whole-pixel font
        // sizes. Replaying arbitrary frame timings cannot add more variants.
        assert_eq!(sampled.len(), 18);
        assert_eq!(sampled.first(), Some(&18));
        assert_eq!(sampled.last(), Some(&35));
    }

    #[test]
    fn menu_text_color_marks_only_hide_as_destructive() {
        assert_eq!(
            menu_text_color(context_menu::ContextMenuItem::EditHome, 0.75),
            [0.11, 0.11, 0.118, 0.75]
        );
        assert_eq!(
            menu_text_color(context_menu::ContextMenuItem::HideApp, 0.75),
            [1.0, 0.231, 0.188, 0.75]
        );
    }

    #[test]
    fn edit_home_row_resolves_to_edit_home_for_an_app_target() {
        let target = LauncherItem::app(AppId::from_normalized("calc"));
        assert_eq!(
            resolve_context_menu_selection(
                &context_menu::ContextMenuItem::ALL,
                Some(0),
                Some(&target)
            ),
            ContextMenuSelection::EditHome
        );
    }

    #[test]
    fn hide_app_row_resolves_to_hiding_the_target_app() {
        let target = LauncherItem::app(AppId::from_normalized("calc"));
        assert_eq!(
            resolve_context_menu_selection(
                &context_menu::ContextMenuItem::ALL,
                Some(1),
                Some(&target)
            ),
            ContextMenuSelection::HideApp(AppId::from_normalized("calc"))
        );
    }

    #[test]
    fn folder_menu_rows_omit_hide_app_reveal_chatgpt_and_steam_rows() {
        let items = context_menu::menu_rows(true, false);
        assert_eq!(items, context_menu::ContextMenuItem::FOLDER_ITEMS);
        // A folder has no hide action, no file to reveal, no app metadata for a
        // ChatGPT lookup, and is never a Steam app, so the folder menu omits all
        // four rows — four shorter than the Steam-app superset menu.
        assert_eq!(items.len(), context_menu::ContextMenuItem::ALL.len() - 4);
        assert!(!items.contains(&context_menu::ContextMenuItem::HideApp));
        assert!(!items.contains(&context_menu::ContextMenuItem::RevealInFinder));
        assert!(!items.contains(&context_menu::ContextMenuItem::ChatGptHelp));
        assert!(!items.contains(&context_menu::ContextMenuItem::OpenInSteam));
    }

    #[test]
    fn non_steam_app_menu_rows_show_seven_items_without_the_steam_row() {
        let items = context_menu::menu_rows(false, false);
        assert_eq!(items, context_menu::ContextMenuItem::APP_ITEMS);
        assert!(!items.contains(&context_menu::ContextMenuItem::OpenInSteam));
    }

    #[test]
    fn steam_app_menu_rows_show_all_eight_items() {
        let items = context_menu::menu_rows(false, true);
        assert_eq!(items, context_menu::ContextMenuItem::ALL);
        assert!(items.contains(&context_menu::ContextMenuItem::OpenInSteam));
    }

    #[test]
    fn folder_menu_row_indices_never_resolve_to_hide_reveal_or_chatgpt() {
        let target = LauncherItem::folder(crate::domain::folders::FolderId::from_normalized(
            "folder-a",
        ));
        let items = context_menu::menu_rows(true, false);
        // A folder menu has no hide-app, reveal, ChatGPT-help, or Steam row;
        // every row resolves to close-only (or edit-home for row 0). Walk every
        // row.
        for row in 0..items.len() {
            let expected = if row == 0 {
                ContextMenuSelection::EditHome
            } else {
                ContextMenuSelection::CloseOnly
            };
            assert_eq!(
                resolve_context_menu_selection(&items, Some(row), Some(&target)),
                expected
            );
        }
    }

    #[test]
    fn hide_app_row_with_no_target_is_close_only() {
        assert_eq!(
            resolve_context_menu_selection(&context_menu::ContextMenuItem::ALL, Some(1), None),
            ContextMenuSelection::CloseOnly
        );
    }

    #[test]
    fn reveal_row_resolves_to_reveal_for_an_app_target() {
        let target = LauncherItem::app(AppId::from_normalized("calc"));
        // RevealInFinder is row 2 in the app menu (EditHome, HideApp, …).
        assert_eq!(
            resolve_context_menu_selection(
                &context_menu::ContextMenuItem::ALL,
                Some(2),
                Some(&target)
            ),
            ContextMenuSelection::RevealApp(AppId::from_normalized("calc"))
        );
    }

    #[test]
    fn reveal_row_with_no_target_is_close_only() {
        assert_eq!(
            resolve_context_menu_selection(&context_menu::ContextMenuItem::ALL, Some(2), None),
            ContextMenuSelection::CloseOnly
        );
    }

    #[test]
    fn chatgpt_row_resolves_to_chatgpt_for_an_app_target() {
        let target = LauncherItem::app(AppId::from_normalized("calc"));
        // ChatGptHelp is row 3 in the app menu (EditHome, HideApp, RevealInFinder, …).
        assert_eq!(
            resolve_context_menu_selection(
                &context_menu::ContextMenuItem::ALL,
                Some(3),
                Some(&target)
            ),
            ContextMenuSelection::ChatGptHelp(AppId::from_normalized("calc"))
        );
    }

    #[test]
    fn chatgpt_row_with_no_target_is_close_only() {
        assert_eq!(
            resolve_context_menu_selection(&context_menu::ContextMenuItem::ALL, Some(3), None),
            ContextMenuSelection::CloseOnly
        );
    }

    #[test]
    fn outside_release_and_mock_rows_are_close_only() {
        let target = LauncherItem::app(AppId::from_normalized("calc"));
        // Outside the panel: no row hit.
        assert_eq!(
            resolve_context_menu_selection(
                &context_menu::ContextMenuItem::ALL,
                None,
                Some(&target)
            ),
            ContextMenuSelection::CloseOnly
        );
        // OpenInSteam (row 4) on a *non-Steam* target resolves to close-only,
        // and the trailing mock rows (IconLarger, IconSmaller, AppInfo) at rows
        // 5..8 are always close-only.
        for row in 4..context_menu::ContextMenuItem::ALL.len() {
            assert_eq!(
                resolve_context_menu_selection(
                    &context_menu::ContextMenuItem::ALL,
                    Some(row),
                    Some(&target)
                ),
                ContextMenuSelection::CloseOnly
            );
        }
    }

    #[test]
    fn steam_row_resolves_to_open_in_steam_for_a_steam_app_target() {
        let target = LauncherItem::app(AppId::from_normalized("steam:620"));
        // OpenInSteam is row 4 in the Steam-app menu (EditHome, HideApp,
        // RevealInFinder, ChatGptHelp, …).
        assert_eq!(
            resolve_context_menu_selection(
                &context_menu::ContextMenuItem::ALL,
                Some(4),
                Some(&target)
            ),
            ContextMenuSelection::OpenInSteam(AppId::from_normalized("steam:620"))
        );
    }

    #[test]
    fn steam_row_is_close_only_for_a_non_steam_target() {
        let target = LauncherItem::app(AppId::from_normalized("calc"));
        assert_eq!(
            resolve_context_menu_selection(
                &context_menu::ContextMenuItem::ALL,
                Some(4),
                Some(&target)
            ),
            ContextMenuSelection::CloseOnly
        );
    }

    #[test]
    fn steam_row_with_no_target_is_close_only() {
        assert_eq!(
            resolve_context_menu_selection(&context_menu::ContextMenuItem::ALL, Some(4), None),
            ContextMenuSelection::CloseOnly
        );
    }

    #[test]
    fn menu_target_flags_distinguish_folder_steam_and_plain_app() {
        let folder = LauncherItem::folder(crate::domain::folders::FolderId::from_normalized("f"));
        let steam = LauncherItem::app(AppId::from_normalized("steam:620"));
        let app = LauncherItem::app(AppId::from_normalized("calc"));
        assert_eq!(menu_target_flags(Some(&folder)), (true, false));
        assert_eq!(menu_target_flags(Some(&steam)), (false, true));
        assert_eq!(menu_target_flags(Some(&app)), (false, false));
        assert_eq!(menu_target_flags(None), (false, false));
    }

    #[test]
    fn folder_context_menu_dismissal_does_not_pass_through() {
        assert!(!App::should_pass_context_menu_dismissal_through(
            true, false
        ));
        assert!(!App::should_pass_context_menu_dismissal_through(true, true));
    }

    #[test]
    fn top_level_context_menu_keeps_existing_passthrough_rules() {
        assert!(App::should_pass_context_menu_dismissal_through(
            false, false
        ));
        assert!(!App::should_pass_context_menu_dismissal_through(
            false, true
        ));
    }

    #[allow(clippy::too_many_arguments)]
    fn roundtrip_input(
        pos: (f32, f32),
        size: (f32, f32),
        radius: f32,
        content_scale: f32,
        content_opacity: f32,
        content_blur: f32,
        activation: f32,
        focus: [f32; 8],
        items: &[context_menu::ContextMenuItem],
    ) -> ContextMenuInput<'_> {
        ContextMenuInput {
            viewport: (1280, 800),
            scale_factor: 1.25,
            target: "app:qa-roundtrip",
            pos,
            size,
            open_size: (325.0, 450.0),
            radius,
            content_scale,
            content_opacity,
            content_blur,
            activation,
            focus_amounts: focus,
            items,
        }
    }

    /// The cached open-panel-space derivation must reproduce the layout's own
    /// `build` bit for bit across the whole animation (seed, mid-reveal, rest,
    /// close, focus cross-fade).
    #[test]
    fn derived_model_matches_layout_build_across_the_animation() {
        let items = context_menu::menu_rows(false, false);
        let cache = ContextMenuLayoutCache::build(
            &items,
            (325.0, 450.0),
            1.25,
            ContextMenuLayoutKey {
                target: "app:qa-roundtrip".to_string(),
                open_size: (325.0, 450.0),
                scale_bits: 1.25f32.to_bits(),
                font_px: 18,
            },
        );
        let mut focus = [0.0f32; 8];
        focus[1] = 1.0;
        let mut partial_focus = [0.0f32; 8];
        partial_focus[0] = 0.5;
        partial_focus[2] = 0.5;
        let states: Vec<ContextMenuInput<'_>> = vec![
            // Seed: collapsed disc, content scaled up, blurred, opaque body.
            roundtrip_input(
                (1000.0, 500.0),
                (50.0, 50.0),
                130.0,
                2.0,
                0.0,
                8.0,
                1.0,
                [0.0; 8],
                &items,
            ),
            // Mid-reveal.
            roundtrip_input(
                (1030.0, 510.0),
                (180.0, 240.0),
                88.0,
                1.6,
                0.3,
                4.0,
                0.6,
                [0.0; 8],
                &items,
            ),
            // Rest with the hover pill on row 1.
            roundtrip_input(
                (1060.0, 520.0),
                (325.0, 450.0),
                70.0,
                1.0,
                1.0,
                0.0,
                0.0,
                focus,
                &items,
            ),
            // Mid-close with partial focus cross-fade.
            roundtrip_input(
                (1050.0, 515.0),
                (300.0, 420.0),
                80.0,
                1.2,
                0.8,
                2.0,
                0.3,
                partial_focus,
                &items,
            ),
            // Fully open at rest.
            roundtrip_input(
                (1060.0, 520.0),
                (325.0, 450.0),
                70.0,
                1.0,
                1.0,
                0.0,
                0.0,
                [0.0; 8],
                &items,
            ),
        ];
        for input in states {
            let reference = context_menu::build(&input);
            let derived = cache.derive_model(&input);
            assert_eq!(derived.panel_rect, reference.panel_rect);
            assert_eq!(derived.rows.len(), reference.rows.len());
            for (a, b) in derived.rows.iter().zip(reference.rows.iter()) {
                assert_eq!(a.rect, b.rect);
                assert_eq!(a.icon_center, b.icon_center);
                assert_eq!(a.label_rect, b.label_rect);
            }
            assert_eq!(derived.result.render, reference.result.render);
            assert_eq!(derived.result.hits, reference.result.hits);
            assert_eq!(derived.scale, reference.scale);
        }
    }

    /// Steady frames translate the cached label quads instead of re-shaping:
    /// the atlas gains no new glyphs between two frames in the same font
    /// bucket, and the x anchor stays exact (the layout rounds it).
    #[test]
    fn context_menu_glyphs_are_shaped_once_and_translated_on_hits() {
        let items = context_menu::menu_rows(false, false);
        let build_cache = |target: &str| {
            ContextMenuLayoutCache::build(
                &items,
                (325.0, 450.0),
                1.25,
                ContextMenuLayoutKey {
                    target: target.to_string(),
                    open_size: (325.0, 450.0),
                    scale_bits: 1.25f32.to_bits(),
                    font_px: 18,
                },
            )
        };
        let mut text = TextRenderer::new();
        let mut cache = build_cache("app:qa-glyph-hit");
        let input1 = roundtrip_input(
            (1060.0, 520.0),
            (325.0, 450.0),
            70.0,
            1.0,
            1.0,
            0.0,
            0.0,
            [0.0; 8],
            &items,
        );
        let model1 = cache.derive_model(&input1);
        let mut first = Vec::new();
        cache.glyphs_for_frame(&mut text, &items, &model1.rows, 1.0, 1.25, 1.0, &mut first);
        assert!(!first.is_empty());
        let misses_after_shape = text.atlas_stats().cache_misses;

        // Same font bucket, animation moved the panel: translate, don't shape.
        let input2 = roundtrip_input(
            (1062.5, 521.25),
            (325.0, 450.0),
            70.0,
            1.0,
            1.0,
            0.0,
            0.0,
            [0.0; 8],
            &items,
        );
        let model2 = cache.derive_model(&input2);
        let mut second = Vec::new();
        cache.glyphs_for_frame(&mut text, &items, &model2.rows, 1.0, 1.25, 1.0, &mut second);
        assert_eq!(
            text.atlas_stats().cache_misses,
            misses_after_shape,
            "hit frames must not rasterize new glyphs"
        );
        assert_eq!(second.len(), first.len());
        // The x translation is exact: the layout rounds the label anchor, and
        // the cached quads embed the shape-time rounded anchor.
        let dx = model2.rows[0].label_rect.x.round() - model1.rows[0].label_rect.x.round();
        for (a, b) in first.iter().zip(second.iter()) {
            assert_eq!(a.x + dx, b.x, "x anchor translation must be exact");
        }

        // A third, further-animated frame must translate from the *stored*
        // (shape-time) anchors, not from the previous frame's position:
        // mutating the cache in place would compound the deltas and drift the
        // text away from the live anchors.
        let input3 = roundtrip_input(
            (1070.0, 525.0),
            (325.0, 450.0),
            70.0,
            1.0,
            1.0,
            0.0,
            0.0,
            [0.0; 8],
            &items,
        );
        let model3 = cache.derive_model(&input3);
        let mut third = Vec::new();
        cache.glyphs_for_frame(&mut text, &items, &model3.rows, 1.0, 1.25, 1.0, &mut third);
        assert_eq!(third.len(), first.len());
        let dx2 = model3.rows[0].label_rect.x.round() - model1.rows[0].label_rect.x.round();
        for (a, b) in first.iter().zip(third.iter()) {
            assert_eq!(
                a.x + dx2,
                b.x,
                "translation must anchor on the stored quads"
            );
        }

        // A new font bucket (the reveal animation stepped) re-shapes: the
        // cache key includes the quantized physical font px, and a bucket
        // change re-runs the shaping at the new size.
        let mut stepped = build_cache("app:qa-glyph-hit");
        stepped.key.font_px = 19;
        let step_scale = 19.0 / 17.5;
        let input3 = roundtrip_input(
            (1062.5, 521.25),
            (325.0, 450.0),
            70.0,
            step_scale,
            1.0,
            0.0,
            0.0,
            [0.0; 8],
            &items,
        );
        let model3 = stepped.derive_model(&input3);
        let mut third = Vec::new();
        stepped.glyphs_for_frame(
            &mut text,
            &items,
            &model3.rows,
            1.0,
            1.25,
            step_scale,
            &mut third,
        );
        assert!(
            text.atlas_stats().cache_misses > misses_after_shape,
            "a new quantized font size must re-shape the labels"
        );
    }
}

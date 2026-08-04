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
    self, open_panel_origin, open_panel_size_logical, ContextMenuInput,
};
use crate::renderer::text_engine::{self, GlyphQuad, UI_FONT_FAMILY};
use crate::ui_model::geometry::Rect;
use crate::ui_model::render_model::{GlassLayer, GlyphLane, InkLane};

/// Menu font metrics, in logical px at 1× DPI. Match the app-icon label size
/// (`LABEL_FONT_SIZE` = 14) so the menu reads at the same scale as the grid.
/// The text renderer multiplies these by the DPI `scale_factor`.
const MENU_FONT_SIZE: f32 = 14.0;
const MENU_LINE_HEIGHT: f32 = 18.0;

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
        // omits "アプリを非表示", so its panel is one row shorter.
        let (items, _labels) = context_menu::menu_rows(matches!(target, LauncherItem::Folder(_)));
        let max_label_w = self.measure_menu_max_label_width_logical(scale, items);
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
        let (items, _labels) = context_menu::menu_rows(matches!(
            self.context_menu.active_target,
            Some(LauncherItem::Folder(_))
        ));
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
        // The row set is fixed at open time by the target kind (folder menus
        // omit "アプリを非表示"), so the laid-out rows always match the rows
        // the release path resolves against.
        let (items, labels) = context_menu::menu_rows(matches!(
            self.context_menu.active_target,
            Some(LauncherItem::Folder(_))
        ));

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
        let input = ContextMenuInput {
            viewport: self.viewport_phys(),
            scale_factor: scale,
            target: target_key,
            pos: (self.context_menu.pos_x(), self.context_menu.pos_y()),
            size: (self.context_menu.width(), self.context_menu.height()),
            open_size,
            radius: self.context_menu.radius(),
            content_scale: self.context_menu.content_scale(),
            content_opacity: self.context_menu.content_opacity(),
            content_blur: self.context_menu.content_blur(),
            activation: self.context_menu.activation(),
            focus_amounts: self.context_menu.focus_amounts(),
            items,
            labels,
        };
        let model = context_menu::build(&input);

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
        let mut glyphs: Vec<GlyphQuad> = Vec::with_capacity(labels.len() * 8);
        let opacity = self.context_menu.content_opacity();
        let content_scale = self.context_menu.content_scale().max(0.0);
        if opacity > crate::features::context_menu::CONTENT_VISIBILITY_THRESHOLD {
            if let Some(text) = self.text.as_mut() {
                // cosmic-text includes the exact f32 font size in its cache
                // key. The menu animates `content_scale` every frame, so pass
                // a physical-pixel-quantized scale to keep repeated open/close
                // animations from manufacturing unbounded glyph variants.
                let text_scale = quantize_menu_text_scale(content_scale, scale);
                for ((item, row), label) in items.iter().zip(model.rows.iter()).zip(labels.iter()) {
                    let left = row.label_rect.x;
                    let center_y = row.label_rect.y;
                    let color = menu_text_color(*item, opacity);
                    // Scale the font with content_scale so the text shrinks/grows
                    // in sync with the glass + ink during open/close morph.
                    push_menu_text(
                        text,
                        &mut glyphs,
                        label,
                        left,
                        center_y,
                        MENU_FONT_SIZE * text_scale,
                        MENU_LINE_HEIGHT * text_scale,
                        color,
                        scale,
                    );
                }
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
    /// Mock action (or a folder target of hide/reveal, which have no file on
    /// disk to act on): just close the menu.
    CloseOnly,
}

/// Resolve a release inside the menu into the action to run. `items` is the
/// row set the menu is actually displaying (folder menus omit the hide-app
/// row, so its row indices never land on [`ContextMenuSelection::HideApp`]).
/// An outside release (no row hit) and every mock row resolve to
/// [`ContextMenuSelection::CloseOnly`].
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
        _ => ContextMenuSelection::CloseOnly,
    }
}

fn glyph_views(quads: &[GlyphQuad]) -> Vec<crate::ui_model::render_model::GlyphView> {
    quads
        .iter()
        .map(|q| crate::ui_model::render_model::GlyphView {
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
    fn folder_menu_rows_omit_hide_app_and_reveal_rows() {
        let (items, labels) = context_menu::menu_rows(true);
        assert_eq!(items, &context_menu::ContextMenuItem::FOLDER_ITEMS[..]);
        assert_eq!(
            labels,
            &context_menu::ContextMenuItem::FOLDER_ITEMS_LABELS[..]
        );
        // A folder has neither a hide action nor a file to reveal, so the
        // folder menu omits both rows — two shorter than the app menu.
        assert_eq!(items.len(), context_menu::ContextMenuItem::ALL.len() - 2);
        assert!(!items.contains(&context_menu::ContextMenuItem::HideApp));
        assert!(!items.contains(&context_menu::ContextMenuItem::RevealInFinder));
    }

    #[test]
    fn app_menu_rows_show_all_six_items() {
        let (items, labels) = context_menu::menu_rows(false);
        assert_eq!(items, &context_menu::ContextMenuItem::ALL[..]);
        assert_eq!(labels, &context_menu::ContextMenuItem::ALL_LABELS[..]);
    }

    #[test]
    fn folder_menu_row_indices_never_resolve_to_hide_app_or_reveal() {
        let target = LauncherItem::folder(crate::domain::folders::FolderId::from_normalized(
            "folder-a",
        ));
        let (items, _labels) = context_menu::menu_rows(true);
        // A folder menu has neither a hide-app nor a reveal row; every row
        // resolves to close-only (or edit-home for row 0). Walk every row.
        for row in 0..items.len() {
            let expected = if row == 0 {
                ContextMenuSelection::EditHome
            } else {
                ContextMenuSelection::CloseOnly
            };
            assert_eq!(
                resolve_context_menu_selection(items, Some(row), Some(&target)),
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
        // The remaining mock rows (IconLarger, IconSmaller, AppInfo).
        for row in 3..context_menu::ContextMenuItem::ALL.len() {
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
}

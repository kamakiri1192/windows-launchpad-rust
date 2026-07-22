//! App state transitions: the `&mut self` methods that mutate `App` state in
//! response to routed input.

use std::time::Instant;

use crate::debug_log;
use crate::domain::app_id::AppId;
use crate::domain::app_registry::AppLaunchInfo;
use crate::domain::launcher_item::LauncherItem;
use crate::domain::settings::{Settings, SortOrder};
use crate::scroll::Phase;
use crate::workers::icon_worker::IconResult;
use crate::workers::refresh_watcher::RefreshMessage;

use crate::app::render::{settings_category_id, settings_press_target_from_layout_hit};
use crate::app::state::{App, PendingPress, SettingsPressTarget, WorkerMessage, CLICK_SLOP_PHYS};

impl App {
    pub(crate) fn settings_hit_target(&self, x: f32, y: f32) -> SettingsPressTarget {
        let layout = self.settings_panel_layout();
        let hit = crate::layout::settings_panel::hit_test(
            &layout,
            self.scale_factor,
            settings_category_id(self.settings_category),
            crate::ui_model::geometry::Point::new(x, y),
        );
        settings_press_target_from_layout_hit(hit)
    }

    pub(crate) fn handle_settings_click(&mut self, target: SettingsPressTarget) {
        match target {
            SettingsPressTarget::Close => self.close_settings(),
            SettingsPressTarget::Category(category) => {
                self.settings_category = category;
                self.request_redraw();
            }
            SettingsPressTarget::Sort(order) => {
                self.settings.sort_order = order;
                if order == SortOrder::Name {
                    // Reset the user layout to a name-sorted arrangement. Phase 7
                    // keeps folder/hidden intents but drops the customized flag so
                    // the next discovered-app integration rebuilds the item list
                    // from a display-name sort (legacy "名前順" behavior).
                    self.launcher_state.customized = false;
                    self.sync_launcher_layout_with_registry();
                    self.persist_launcher_state();
                    self.relayout();
                }
                self.persist_settings();
                self.request_redraw();
            }
            SettingsPressTarget::FrequentToggle => {
                self.settings.frequent_apps_enabled = !self.settings.frequent_apps_enabled;
                self.persist_settings();
                self.request_redraw();
            }
            SettingsPressTarget::SteamToggle => {
                self.settings.show_steam_apps = !self.settings.show_steam_apps;
                self.persist_settings();
                self.close_folder();
                self.relayout();
                self.request_redraw();
            }
            SettingsPressTarget::SearchHiddenToggle => {
                self.settings.search_includes_hidden = !self.settings.search_includes_hidden;
                self.persist_settings();
                self.search_input_changed();
            }
            SettingsPressTarget::ResetCache => {
                self.reset_icons();
                self.request_redraw();
            }
            SettingsPressTarget::ResetSettings => {
                self.settings = Settings::default();
                // Reset the user layout entirely (order, folders, hidden).
                self.launcher_state = crate::domain::launcher_state::LauncherState::new();
                self.sync_launcher_layout_with_registry();
                self.persist_settings();
                self.persist_launcher_state();
                self.relayout();
                self.request_redraw();
            }
            SettingsPressTarget::Inside | SettingsPressTarget::Outside => {}
        }
    }

    /// Drain the shared inbox and dispatch each message.
    pub(crate) fn drain_inbox(&mut self) {
        let messages: Vec<WorkerMessage> = {
            let mut g = self.inbox.lock().expect("inbox poisoned");
            std::mem::take(&mut *g)
        };
        for msg in messages {
            match msg {
                WorkerMessage::Icon(IconResult::Loaded { app_id, image }) => {
                    self.apply_icon(&app_id, image, false);
                }
                WorkerMessage::Icon(IconResult::Failed { app_id, error }) => {
                    self.fail_icon(&app_id, error);
                }
                WorkerMessage::Refresh(RefreshMessage::Initial(snap)) => {
                    self.ingest_snapshot(snap, true);
                }
                WorkerMessage::Refresh(RefreshMessage::Diff(diff)) => {
                    self.apply_diff(diff);
                }
            }
        }
    }

    pub(crate) fn handle_drag_start(&mut self, x_phys: f32, y_phys: f32) {
        self.drag_start_x = x_phys;
        self.drag_start_y = y_phys;
        if let Some(s) = self.scroller.as_mut() {
            s.drag_start(x_phys);
        }
        self.request_redraw();
    }

    pub(crate) fn handle_drag_move(&mut self, x_phys: f32) {
        if let Some(s) = self.scroller.as_mut() {
            s.drag_move(x_phys);
        }
        self.request_redraw();
    }

    pub(crate) fn handle_drag_end(&mut self) {
        if let Some(s) = self.scroller.as_mut() {
            s.drag_end();
        }
        self.request_redraw();
    }

    /// Resolve the scroller-drag release into an optional app launch, then end
    /// the drag. Returns the launch info if the release was a stationary click
    /// over a visible app.
    pub(crate) fn handle_pointer_release_launch(&mut self) -> Option<AppLaunchInfo> {
        let x = self.pointer_phys_x;
        let y = self.pointer_phys_y;
        let dx = x - self.drag_start_x;
        let dy = y - self.drag_start_y;
        let is_click = dx * dx + dy * dy <= CLICK_SLOP_PHYS * CLICK_SLOP_PHYS;

        let launch = is_click.then(|| self.resolve_clicked_app(x, y)).flatten();
        self.handle_drag_end();
        launch
    }

    // ---- edit mode (iOS-style reorder) -----------------------------------

    /// Begin a grid press. Instead of immediately starting a scroll drag (the
    /// old behavior), we record the press and wait to see whether it becomes a
    /// drag (→ scroll), a quick release (→ click/launch), or a long-press
    /// (→ enter edit mode). The scroller stays `Idle` until intent is clear.
    ///
    /// Press classification goes through the layout layer's `GridHit` so the
    /// app / empty-in-frame / outside-frame decision comes from one calculation
    /// instead of separate `hit_test_app` + `frame_contains_point` calls. This
    /// preserves the previous behavior exactly: the visible-stream app index
    /// and the `outside_glass` flag are derived from the same geometry.
    pub(crate) fn begin_grid_press(&mut self, now: Instant) {
        let x = self.pointer_phys_x;
        let y = self.pointer_phys_y;
        let hit = self.grid_hit_at_pointer(x, y);
        let item_index = hit.app_index();
        let item = item_index.and_then(|idx| self.visible_launcher_items().get(idx).cloned());
        let outside_glass = hit.is_outside_frame();
        debug_log!(
            "edit-press: pending x={x:.1} y={y:.1} item_index={item_index:?} outside_glass={outside_glass}"
        );
        self.pending_press = Some(PendingPress {
            start: now,
            x,
            y,
            item_index,
            item,
            outside_glass,
        });
        self.request_redraw();
    }

    /// Hide an app from the launcher (the ✕ badge action): removes it from the
    /// visible stream, persists the launcher layout, and relayouts. Reversible
    /// by clearing the hidden list later. Stays a no-op if already hidden.
    ///
    /// Phase 7 routes this through `LauncherState::hide_app`, which removes the
    /// app from the top-level item list and records it as hidden. Persistence
    /// writes the unified launcher state.
    pub(crate) fn hide_app(&mut self, id: &AppId) {
        if !self.launcher_state.hide_app(id) {
            return;
        }
        self.persist_launcher_state();
        // Drop any in-flight drag of the just-hidden app.
        if self.drag_item.as_ref()
            == Some(&crate::domain::launcher_item::LauncherItem::App(id.clone()))
        {
            self.drag_item = None;
        }
        self.relayout();
        self.request_redraw();
    }

    /// A press is pending; check whether movement past `CLICK_SLOP_PHYS`
    /// promotes it to a real scroll drag. Returns true if it was promoted.
    pub(crate) fn maybe_promote_press_to_drag(&mut self) -> bool {
        let Some(p) = self.pending_press.as_ref() else {
            return false;
        };
        let dx = self.pointer_phys_x - p.x;
        let dy = self.pointer_phys_y - p.y;
        if dx * dx + dy * dy <= CLICK_SLOP_PHYS * CLICK_SLOP_PHYS {
            return false;
        }
        let start_x = p.x;
        let start_y = p.y;
        // Promote: start the scroll drag from the original anchor, then apply
        // the current pointer so the page follows the gesture from here.
        self.pending_press = None;
        self.handle_drag_start(start_x, start_y);
        if self.scroller.as_ref().map(|s| s.phase) == Some(Phase::Dragging) {
            self.handle_drag_move(self.pointer_phys_x);
        }
        true
    }

    /// Check whether the pending press has been held long enough to enter edit
    /// mode. Called from `about_to_wait`. Returns true if edit mode was entered.
    ///
    /// The long-press decision (outside-glass rejects, slop rejects, threshold)
    /// comes from [`features::edit_mode::should_enter_from_long_press`], keeping
    /// the pure intent in the feature module while `PendingPress` stays in
    /// `main.rs` (it also drives launch/passthrough/scroll-drag; Phase 5 moves
    /// it to the app shell).
    pub(crate) fn maybe_long_press_into_edit(&mut self, now: Instant) -> bool {
        if !self.visible_search_query().trim().is_empty() || self.folders.is_active() {
            return false;
        }
        let Some(p) = self.pending_press.as_ref() else {
            return false;
        };
        let snapshot = crate::features::edit_mode::PressSnapshot {
            start: p.start,
            x: p.x,
            y: p.y,
            outside_glass: p.outside_glass,
            pointer: crate::features::edit_mode::PointerSnapshot::new(
                self.pointer_phys_x,
                self.pointer_phys_y,
            ),
        };
        if !crate::features::edit_mode::should_enter_from_long_press(&snapshot, now) {
            return false;
        }
        // Enter edit mode and immediately lift the pressed app into a drag.
        let item_index = p.item_index;
        self.enter_edit_mode(item_index);
        true
    }

    /// Enter edit mode, optionally lifting `app_index` straight into a drag
    /// (the long-press path). Edit mode is idempotent.
    ///
    /// Phase 5 consolidation: the state transitions and side effects come from
    /// [`features::edit_mode::enter`], which returns a `Vec<EditModeCommand>`.
    /// The app shell runs them through [`Self::execute_edit_mode_commands`], the
    /// single command boundary shared with settings/search/grid. This preserves
    /// the historical inline `enter_edit_mode` behavior: `editing = true`,
    /// cancel pending press + wiggle reset + scroll cancel, optional app lift,
    /// relayout + redraw (and the log-on-first-transition via `SetEditing`).
    pub(crate) fn enter_edit_mode(&mut self, app_index: Option<usize>) {
        let mut state = self.edit_mode_state();
        let pointer = crate::features::edit_mode::PointerSnapshot::new(
            self.pointer_phys_x,
            self.pointer_phys_y,
        );
        let visible = self.visible_launcher_items();
        let commands = crate::features::edit_mode::enter(&mut state, app_index, &visible, pointer);
        self.sync_edit_mode_state(&state);
        self.execute_edit_mode_commands(commands);
    }

    /// Exit edit mode. Commits any in-progress drag (if the lifted app was
    /// dropped on a valid cell) and persists the resulting order. Safe to call
    /// when not editing.
    ///
    /// Phase 5 consolidation: the state transitions and side effects come from
    /// [`features::edit_mode::exit`] (fed the [`features::edit_mode::commit_drag`]
    /// commands when a drag was in flight), run through the shared command
    /// boundary. The `if !self.editing` early return preserves the historical
    /// no-op-when-not-editing behavior (the feature `exit` would otherwise emit
    /// transitions even when already exited).
    pub(crate) fn exit_edit_mode(&mut self) {
        if !self.editing {
            return;
        }
        if self.cancel_folder_child_exit_preview() {
            self.drag_item = None;
        }
        let mut state = self.edit_mode_state();
        // If a drag was in flight, finalize it as a drop at the current cell.
        let commit_commands = if state.drag_item.is_some() {
            self.live_reorder();
            crate::features::edit_mode::commit_drag(&state)
        } else {
            Vec::new()
        };
        let commands = crate::features::edit_mode::exit(&mut state, commit_commands);
        self.sync_edit_mode_state(&state);
        self.execute_edit_mode_commands(commands);
    }

    /// Open the settings overlay. Dismisses edit mode and the search field so
    /// they cannot be interacted with underneath the panel.
    pub(crate) fn open_settings(&mut self) {
        if self.folders.is_active() {
            self.folders = crate::features::folders::FolderFeatureState::default();
            self.folder_scroller = None;
            self.folder_layout = None;
        }
        if self.editing {
            self.exit_edit_mode();
        }
        if self.control.wants_keyboard() {
            self.control.press_close();
            self.search_input_changed();
        }
        self.pending_press = None;
        self.pressed_on_control = false;
        self.settings_open = true;
        debug_log!("settings: opened");
        self.request_redraw();
    }

    /// Close the settings overlay if it is open. Safe to call when closed.
    pub(crate) fn close_settings(&mut self) {
        if !self.settings_open {
            return;
        }
        self.settings_open = false;
        debug_log!("settings: closed");
        self.request_redraw();
    }

    /// Toggle the settings overlay.
    pub(crate) fn toggle_settings(&mut self) {
        if self.settings_open {
            self.close_settings();
        } else {
            self.open_settings();
        }
    }

    /// Update the dragged tile's follow position during an edit-mode move.
    pub(crate) fn handle_edit_drag_move(&mut self) {
        if self.drag_item.is_some() {
            self.drag_x = self.pointer_phys_x;
            self.drag_y = self.pointer_phys_y;
            self.refresh_dragged_folder_glass_position();
            let hover_candidate = self.folder_hover_candidate_at_pointer();
            let hover_ready = self.folders.hover.as_ref().is_some_and(|hover| {
                hover_candidate.as_ref() == Some(&hover.target) && hover.ready()
            });
            if hover_candidate.is_none() {
                self.folders.update_hover(None, 0.0);
            }
            if crate::features::folders::top_level_reorder_allowed(
                hover_candidate.as_ref(),
                hover_ready,
            ) {
                self.live_reorder();
            }
            self.maybe_autoscroll_edit_drag();
            self.request_redraw();
        }
    }

    /// Move `drag_app` to the visible cell currently under the pointer (if it's
    /// a different cell). No-op when the pointer is off the grid.
    ///
    /// The insert-index decision comes from
    /// [`layout::edit_mode::reorder_insert_index`] and the new order is computed
    /// by [`features::edit_mode::apply_reorder`]; this function runs the
    /// `registry.set_order` + relayout side effect. Reorder is keyed by stable
    /// `AppId`, not positional index.
    pub(crate) fn live_reorder(&mut self) {
        let Some(drag_item) = self.drag_item.clone() else {
            return;
        };
        let Some(target_idx) = self.edit_drop_index_at_pointer(self.drag_x, self.drag_y) else {
            return;
        };
        let visible = self.visible_launcher_items();
        let Some(drag_pos) = visible.iter().position(|item| item == &drag_item) else {
            return;
        };
        if let Some(target) = visible.get(target_idx) {
            if target != &drag_item {
                // Reorder intent is based on stable grid slots, not the
                // in-flight spring positions. A spring can sit between rows
                // after the previous swap; using it as the source made the
                // dominant-axis test change frame-to-frame and caused vertical
                // moves to intermittently stop responding.
                let viewport_w = self.viewport_phys().0 as f32;
                let scroll_x = self.scroller.as_ref().map_or(0.0, |s| s.position);
                let slot_rect = |index: usize| {
                    let (x, y) = self.layout.tile_position(viewport_w, index);
                    crate::ui_model::geometry::Rect::new(
                        x + scroll_x,
                        y,
                        self.layout.tile_size,
                        self.layout.tile_size,
                    )
                };
                let crossed = crate::layout::edit_mode::reorder_crossed_target(
                    slot_rect(drag_pos),
                    slot_rect(target_idx),
                    crate::ui_model::geometry::Point::new(self.drag_x, self.drag_y),
                );
                if !crossed {
                    return;
                }
            }
        }
        let Some(insert_idx) =
            crate::layout::edit_mode::reorder_insert_index(visible.len(), drag_pos, target_idx)
        else {
            return;
        };
        debug_log!(
            "edit-reorder: moving drag_pos={drag_pos} target_idx={target_idx} insert_idx={insert_idx}"
        );
        self.reorder_by_index(&drag_item, insert_idx);
    }

    /// Start a one-page autoscroll if the lifted edit-mode icon is held near a
    /// page-frame edge. Returns true when a new page glide was started.
    ///
    /// The zone width and the gutter clamp come from
    /// [`layout::edit_mode::configured_edge_zone`] /
    /// [`layout::edit_mode::edge_autoscroll_zones`], and the target page
    /// decision comes from [`layout::edit_mode::edge_autoscroll_target`]. This
    /// function only runs the side effect: it checks the scroller is `Idle` and
    /// calls `settle_to_page`. The gutter clamp keeps the rightmost tile
    /// columns reachable as drop targets while the icon is held in the gutter.
    pub(crate) fn maybe_autoscroll_edit_drag(&mut self) -> bool {
        if !self.editing || self.drag_item.is_none() {
            return false;
        }

        let (w, _h) = self.viewport_phys();
        let (cx, cy, panel_w, panel_h) = self.layout.frame_panel_rect(w.max(1) as f32);
        let panel_left = cx - panel_w * 0.5;
        let panel_right = cx + panel_w * 0.5;
        let panel_top = cy - panel_h * 0.5;
        let panel_bottom = cy + panel_h * 0.5;
        let grid_left = self.layout.margin_left;
        let grid_right = self.layout.margin_left + self.layout.grid_w();

        let zone = crate::layout::edit_mode::configured_edge_zone(&self.layout, panel_w);
        let (left_zone, right_zone) = crate::layout::edit_mode::edge_autoscroll_zones(
            zone,
            panel_left,
            panel_right,
            grid_left,
            grid_right,
        );
        let current = self.current_page();
        let Some(target) = crate::layout::edit_mode::edge_autoscroll_target(
            &crate::layout::edit_mode::EdgeAutoscrollInput {
                drag: (self.drag_x, self.drag_y),
                panel: (panel_left, panel_right, panel_top, panel_bottom),
                zones: (left_zone, right_zone),
                current_page: current,
                page_count: self.layout.page_count,
            },
        ) else {
            return false;
        };

        let Some(scroller) = self.scroller.as_mut() else {
            return false;
        };
        if scroller.phase != Phase::Idle {
            return false;
        }
        if scroller.settle_to_page(target) {
            self.request_redraw();
            true
        } else {
            false
        }
    }

    /// Finalize the in-flight drag: drop the dragged app onto the current cell
    /// and persist the new order. The sort order is set to `Manual` and the
    /// settings + user order are persisted.
    ///
    /// Phase 5 consolidation: the persist-intent commands come from
    /// [`features::edit_mode::commit_drag`] and run through the shared command
    /// boundary. The order is `SetSortManual` → `PersistSettings` →
    /// `PersistUserOrder` (the feature emits them in that order), matching the
    /// historical inline sequence. `live_reorder` still runs first so the drop
    /// lands at the current cell before persist.
    pub(crate) fn commit_reorder(&mut self) {
        self.live_reorder();
        let state = self.edit_mode_state();
        let commands = crate::features::edit_mode::commit_drag(&state);
        self.execute_edit_mode_commands(commands);
    }

    /// Reorder the launcher layout so that `drag_id` moves to `insert_idx` in
    /// the visible order, shifting the apps between them. Hidden apps are
    /// preserved in the hidden set (they are not in the visible stream).
    ///
    /// The new order is computed by [`features::edit_mode::apply_reorder`]
    /// (pure); this function applies it to `launcher_state` + relayout. Phase 7
    /// keeps the historical concatenated visible-then-hidden semantics by
    /// feeding the current hidden ids as the hidden tail, then applying the
    /// result to the launcher state's app items.
    pub(crate) fn reorder_by_index(
        &mut self,
        drag_item: &crate::domain::launcher_item::LauncherItem,
        insert_idx: usize,
    ) {
        let visible = self.visible_launcher_items();
        let Some(order) =
            crate::features::edit_mode::apply_item_reorder(&visible, drag_item, insert_idx)
        else {
            return;
        };
        if let Some(preview) = self.folders.child_exit_preview.as_mut() {
            preview
                .launcher_state_mut()
                .reorder_visible_items(&visible, order);
        } else {
            self.launcher_state.reorder_visible_items(&visible, order);
        }
        self.relayout();
    }

    pub(crate) fn request_redraw(&self) {
        if let Some(r) = self.renderer.as_ref() {
            r.window.request_redraw();
        }
    }

    /// Integrate the registry's current discovered-app set into the launcher
    /// state, then normalize invariants. Used after discovery refresh, sort
    /// reset, and reset-settings so the user-owned layout reflects what the OS
    /// currently reports without losing the user's arrangement.
    ///
    /// `prune_missing` is false in production: undiscovered apps are retained as
    /// placeholders so a later re-detection restores them exactly where they
    /// were. A future "compact layout" action could pass true.
    pub(crate) fn sync_launcher_layout_with_registry(&mut self) {
        let discovered = self.registry.discovered_id_set();
        let name_of = |id: &AppId| self.registry.lowercased_name_of(id);
        self.launcher_state
            .integrate_discovered_apps(&discovered, name_of);
        self.launcher_state.normalize(&discovered, false);
    }

    /// Build a feature-side [`EditModeState`] mirror from the app boundary's
    /// source-of-truth fields. The feature module's decision functions operate
    /// on this mirror; [`Self::sync_edit_mode_state`] writes the result back.
    fn edit_mode_state(&self) -> crate::features::edit_mode::EditModeState {
        crate::features::edit_mode::EditModeState {
            editing: self.editing,
            drag_item: self.drag_item.clone(),
            drag_x: self.drag_x,
            drag_y: self.drag_y,
            wiggle_phase: self.wiggle_phase,
            pending_press: self.pending_press.is_some(),
        }
    }

    /// Write the feature-side [`EditModeState`] mirror back to the app
    /// boundary's source-of-truth fields.
    ///
    /// Only the fields the feature module mutates in-place during `enter`/`exit`
    /// that are NOT also carried as commands are copied back (`drag_app`,
    /// `wiggle_phase`). `editing` is intentionally NOT synced here: it is owned
    /// by the `SetEditing` command, which logs the first-transition via
    /// [`App::set_editing`]. Syncing it here would pre-mutate the field before
    /// the command runs and silence that log. `pending_press` is likewise
    /// command-owned (`ClearPendingPress`), and `drag_x`/`drag_y` are owned by
    /// the pointer-follow path (`SetDragPos`) and are not mutated by
    /// `enter`/`exit`.
    fn sync_edit_mode_state(&mut self, state: &crate::features::edit_mode::EditModeState) {
        self.drag_item = state.drag_item.clone();
        self.wiggle_phase = state.wiggle_phase;
    }

    /// Handle a click (press + release inside the capsule with no drag) on the
    /// bottom control. Decides whether it hit the close (×) button, the
    /// edit-mode settings gear, the Done capsule, or the search pill/field,
    /// using the layout layer's hit map so render geometry and pointer targets
    /// share one calculation.
    pub(crate) fn handle_control_click(&mut self, x: f32, y: f32) {
        let intent = self.bottom_control_intent(x, y);
        // In edit mode the bottom control shows two capsules: [完了] on the
        // left and a settings gear [⚙] on the right. The previous code handled
        // edit mode in a single early branch that returned before any
        // close-button logic, so an invisible close hotspot from a previously
        // open search field could never intercept a click while editing. Keep
        // that precedence: while editing, only the gear and Done capsules are
        // reachable; the close-button intent is ignored.
        if self.editing {
            match intent {
                crate::layout::bottom_control::BottomControlPointerIntent::EditGear => {
                    self.open_settings();
                }
                // Done capsule (or anywhere else on the capsule body).
                _ => self.exit_edit_mode(),
            }
            return;
        }
        match intent {
            crate::layout::bottom_control::BottomControlPointerIntent::CloseButton => {
                self.control.press_close();
                self.search_input_changed();
            }
            crate::layout::bottom_control::BottomControlPointerIntent::Capsule => {
                match self.control.mode {
                    crate::features::bottom_control::Mode::Pill
                    | crate::features::bottom_control::Mode::Indicator
                    | crate::features::bottom_control::Mode::Collapsing => {
                        self.control.open_search();
                    }
                    crate::features::bottom_control::Mode::Expanding
                    | crate::features::bottom_control::Mode::Field => {
                        // Clicking inside an open field does nothing (keep
                        // focus). A click outside the field's text area
                        // could move the caret; the MVP leaves the caret at
                        // the end.
                    }
                }
                self.request_redraw();
            }
            crate::layout::bottom_control::BottomControlPointerIntent::EditGear
            | crate::layout::bottom_control::BottomControlPointerIntent::None => {
                // Gear is only reachable in edit mode (handled above); None
                // should not happen because the caller only invokes us when
                // the release stayed on the capsule.
            }
        }
    }
}

impl App {
    pub(crate) fn folder_hover_candidate_at_pointer(
        &self,
    ) -> Option<crate::domain::launcher_item::LauncherItem> {
        let drag = self.drag_item.as_ref()?;
        if !matches!(drag, crate::domain::launcher_item::LauncherItem::App(_)) {
            return None;
        }
        if let (Some(folder_id), Some(layout)) = (
            self.folders.hover_opened.as_ref(),
            self.folder_layout.as_ref(),
        ) {
            let pointer = crate::ui_model::geometry::Point::new(self.drag_x, self.drag_y);
            if layout.current_panel_rect.contains(pointer) {
                return Some(crate::domain::launcher_item::LauncherItem::Folder(
                    folder_id.clone(),
                ));
            }
        }
        let index = self
            .grid_hit_at_pointer(self.drag_x, self.drag_y)
            .app_index()?;
        let target = self.visible_launcher_items().get(index)?.clone();
        if target == *drag {
            return None;
        }
        let tile = self.launcher_item_rect(&target)?;
        crate::layout::edit_mode::folder_merge_intent(
            tile,
            crate::ui_model::geometry::Point::new(self.drag_x, self.drag_y),
        )
        .then_some(target)
    }

    pub(crate) fn commit_edit_drop(&mut self) {
        if self.folders.child_exit_preview.is_some() && !self.commit_folder_child_exit_preview() {
            self.drag_item = None;
            self.folders.hover = None;
            self.relayout();
            return;
        }
        let drag = self.drag_item.clone();
        let hover = self.folders.hover.clone();
        let current_hover_target = self.folder_hover_candidate_at_pointer();
        let mut changed = false;
        let mut opened = None;
        if let (Some(crate::domain::launcher_item::LauncherItem::App(dragged)), Some(hover)) =
            (drag.as_ref(), hover.as_ref())
        {
            if hover.ready() && current_hover_target.as_ref() == Some(&hover.target) {
                match &hover.target {
                    crate::domain::launcher_item::LauncherItem::App(target) => {
                        if let Some(id) =
                            self.launcher_state
                                .create_folder_from_apps(target, dragged, "フォルダ")
                        {
                            changed = true;
                            opened = Some((id, Some(hover.panel_progress())));
                        }
                    }
                    crate::domain::launcher_item::LauncherItem::Folder(folder_id) => {
                        changed = self
                            .launcher_state
                            .move_top_level_app_into_folder(dragged, folder_id);
                        if changed {
                            opened = Some((folder_id.clone(), None));
                        }
                    }
                }
            }
        }
        if changed {
            self.settings.sort_order = crate::domain::settings::SortOrder::Manual;
            self.persist_settings();
            self.persist_launcher_state();
            self.folders.hover = None;
            self.folders.hover_opened = None;
            if let Some((id, inherited_progress)) = opened {
                self.folders.open(id);
                if let Some(progress) = inherited_progress {
                    self.folders.motion.progress = progress;
                }
            }
        } else {
            if self.folders.hover_opened.is_some() {
                self.folders.close();
            }
            self.folders.hover = None;
            self.commit_reorder();
        }
    }

    pub(crate) fn commit_folder_rename(&mut self) {
        let Some(folder_id) = self.folders.active.clone() else {
            self.folders.cancel_rename();
            return;
        };
        let Some(name) = self.folders.finish_rename() else {
            return;
        };
        if let Some(folder) = self.launcher_state.folders.get_mut(&folder_id) {
            if folder.name != name {
                folder.name = name;
                self.launcher_state.customized = true;
                self.persist_launcher_state();
                self.relayout();
            }
        }
        self.request_redraw();
    }

    fn folder_hit_target(&self, x: f32, y: f32) -> Option<crate::ui_model::hit::HitTarget> {
        self.folder_layout
            .as_ref()?
            .result
            .hits
            .hit_test(crate::ui_model::geometry::Point::new(x, y))
            .map(|hit| hit.target.clone())
    }

    pub(crate) fn handle_folder_pointer_press(&mut self, x: f32, y: f32) {
        use crate::ui_model::hit::HitTarget;
        let target = self.folder_hit_target(x, y);
        if self.folders.rename.is_some() && !matches!(target, Some(HitTarget::FolderTitle { .. })) {
            self.commit_folder_rename();
        }
        match target {
            Some(HitTarget::FolderChildBadge { child, .. }) if self.editing => {
                let app_id = crate::domain::app_id::AppId::from_normalized(child);
                self.folders.clear_child_pointer();
                self.hide_app(&app_id);
            }
            Some(HitTarget::FolderTitle { .. }) => {
                if self.editing {
                    let id = self.folders.active.clone();
                    if let Some(id) = id.as_ref() {
                        if let Some(folder) = self.launcher_state.folders.get(id) {
                            self.folders.begin_rename(folder.name.clone());
                        }
                    }
                }
            }
            Some(HitTarget::FolderChild { child, index, .. }) => {
                let app_id = crate::domain::app_id::AppId::from_normalized(child);
                let domain_index = self
                    .folders
                    .active
                    .as_ref()
                    .and_then(|folder_id| self.launcher_state.folders.get(folder_id))
                    .and_then(|folder| folder.children.iter().position(|id| id == &app_id))
                    .unwrap_or(index);
                self.folders
                    .begin_child_press(app_id, domain_index, Instant::now(), x, y);
                if !self.editing {
                    self.folders.begin_page_press(x, y);
                }
            }
            Some(HitTarget::FolderPagePrevious { .. }) => {
                self.settle_folder_page(self.folders.page.saturating_sub(1));
            }
            Some(HitTarget::FolderPageNext { .. }) => {
                if let Some(layout) = &self.folder_layout {
                    self.settle_folder_page((self.folders.page + 1).min(layout.page_count - 1));
                }
            }
            Some(HitTarget::FolderPanel { .. }) => self.folders.begin_page_press(x, y),
            Some(HitTarget::Backdrop { .. }) | None => self.close_folder(),
            _ => {}
        }
        self.request_redraw();
    }

    /// Enter folder edit mode and lift the child held by the current press in
    /// one transition. This mirrors the top-level long-press path and prevents
    /// the same press from being reclassified as a folder page swipe.
    pub(crate) fn begin_folder_child_edit_drag_if_ready(&mut self, now: Instant) -> bool {
        if self.editing
            || !self
                .folders
                .pressed_child
                .as_ref()
                .is_some_and(|press| press.held_long_enough(now))
        {
            return false;
        }
        let Some(folder_id) = self.folders.active.clone() else {
            return false;
        };
        let children = self
            .launcher_state
            .folders
            .get(&folder_id)
            .map(|folder| folder.children.clone())
            .unwrap_or_default();

        self.enter_edit_mode(None);
        if !self.folders.begin_child_drag_from_press(&children) {
            return false;
        }
        self.drag_x = self.pointer_phys_x;
        self.drag_y = self.pointer_phys_y;
        self.relayout();
        self.request_redraw();
        true
    }

    pub(crate) fn handle_folder_pointer_move(&mut self, x: f32, y: f32) {
        self.folder_pointer_move_serial = self.folder_pointer_move_serial.wrapping_add(1);
        self.begin_folder_child_edit_drag_if_ready(Instant::now());
        let Some(folder_id) = self.folders.active.clone() else {
            return;
        };
        let children = self
            .launcher_state
            .folders
            .get(&folder_id)
            .map(|folder| folder.children.clone())
            .unwrap_or_default();
        if self
            .folder_scroller
            .as_ref()
            .is_some_and(|scroller| scroller.phase == Phase::Dragging)
        {
            if let Some(scroller) = self.folder_scroller.as_mut() {
                scroller.drag_move(x);
            }
            self.update_folder_page_from_scroll();
            self.relayout();
            self.request_redraw();
            return;
        }
        let page_drag_start = self
            .folders
            .page_press
            .as_ref()
            .filter(|press| press.moved_past_slop(x, y))
            .filter(|_| !self.editing || self.folders.pressed_child.is_none())
            .map(|press| press.start_x);
        if let (Some(start_x), Some(scroller)) = (page_drag_start, self.folder_scroller.as_mut()) {
            scroller.drag_start(start_x);
            scroller.drag_move(x);
            self.folders.pressed_child = None;
            self.folders.page_press = None;
            self.update_folder_page_from_scroll();
            self.relayout();
            self.request_redraw();
            return;
        }
        if self.editing {
            self.folders.maybe_begin_child_drag(&children, x, y);
        }
        if self.folders.child_drag.is_some() {
            self.drag_x = x;
            self.drag_y = y;
            let boundary_intent = self.folder_child_boundary_intent(x, y);
            if boundary_intent == crate::features::folders::ChildDragBoundaryIntent::Exit
                && self.promote_folder_child_drag_to_top_level()
            {
                return;
            }
            let (_, reordered) = self.reorder_folder_child_drag_at(x, y);
            if reordered {
                self.relayout();
            }
            self.request_redraw();
        }
    }

    fn folder_child_boundary_intent(
        &self,
        x: f32,
        y: f32,
    ) -> crate::features::folders::ChildDragBoundaryIntent {
        let Some(layout) = self.folder_layout.as_ref() else {
            return crate::features::folders::ChildDragBoundaryIntent::Stay;
        };
        crate::features::folders::child_drag_boundary_intent(
            layout.current_panel_rect,
            crate::ui_model::geometry::Point::new(x, y),
            self.folders.page,
            layout.page_count,
            self.scale_factor,
        )
    }

    /// Update the held child's preview order for either an occupied child cell
    /// or an empty 3x3 cell on the current page. Returns `(valid, changed)` so
    /// release can distinguish a real empty-cell drop from panel chrome.
    fn reorder_folder_child_drag_at(&mut self, x: f32, y: f32) -> (bool, bool) {
        let target_child = match self.folder_hit_target(x, y) {
            Some(crate::ui_model::hit::HitTarget::FolderChild { child, .. }) => {
                Some(crate::domain::app_id::AppId::from_normalized(child))
            }
            _ => None,
        };
        let empty_index = target_child
            .is_none()
            .then(|| self.folder_child_empty_drop_index(x, y))
            .flatten();
        let Some(drag) = self.folders.child_drag.as_mut() else {
            return (false, false);
        };
        if let Some(target) = target_child {
            (true, drag.preview_reorder_to(&target))
        } else if let Some(index) = empty_index {
            (true, drag.preview_reorder(index))
        } else {
            (false, false)
        }
    }

    fn folder_child_empty_drop_index(&self, x: f32, y: f32) -> Option<usize> {
        let layout = self.folder_layout.as_ref()?;
        let child_count = self
            .folders
            .child_drag
            .as_ref()
            .map(|drag| drag.preview_order.len())?;
        crate::layout::folder_panel::child_drop_index(
            layout.target_panel_rect,
            crate::ui_model::geometry::Point::new(x, y),
            self.folders.page,
            child_count,
            self.scale_factor,
        )
    }

    /// Advance the held-child side-edge dwell. A completed dwell glides one
    /// folder page and latches until the pointer returns to the panel center.
    /// Top/bottom exits are handled immediately by the pointer-move path.
    pub(crate) fn tick_folder_child_page_hover(&mut self, dt: f32) -> bool {
        if self.folders.child_drag.is_none() {
            self.folders.child_page_hover = None;
            self.folders.child_page_latched = false;
            return false;
        }
        let Some(layout) = self.folder_layout.as_ref() else {
            return false;
        };
        let panel = layout.current_panel_rect;
        let pointer = crate::ui_model::geometry::Point::new(self.drag_x, self.drag_y);
        let in_page_edge =
            crate::features::folders::child_drag_in_page_edge(panel, pointer, self.scale_factor);
        match self.folder_child_boundary_intent(self.drag_x, self.drag_y) {
            crate::features::folders::ChildDragBoundaryIntent::Page(target)
                if !self.folders.child_page_latched =>
            {
                match self.folders.child_page_hover.as_mut() {
                    Some(hover) if hover.target == target => {
                        hover.elapsed += dt.max(0.0);
                    }
                    _ => {
                        self.folders.child_page_hover =
                            Some(crate::features::folders::ChildPageHover {
                                target,
                                elapsed: dt.max(0.0),
                            });
                    }
                }
                let ready = self.folders.child_page_hover.as_ref().is_some_and(|hover| {
                    hover.elapsed >= crate::features::folders::CHILD_PAGE_EDGE_DWELL
                });
                let can_settle = self
                    .folder_scroller
                    .as_ref()
                    .is_some_and(|scroller| scroller.phase == Phase::Idle);
                if ready && can_settle {
                    self.folders.child_page_hover = None;
                    self.folders.child_page_latched = true;
                    self.settle_folder_page(target);
                    return true;
                }
            }
            _ => {
                self.folders.child_page_hover = None;
                if !in_page_edge {
                    self.folders.child_page_latched = false;
                }
            }
        }
        false
    }

    /// Move a lifted folder child into the top-level model as soon as the
    /// pointer leaves the folder glass, then continue the same held gesture as
    /// the ordinary main-grid edit drag. The initial insertion is the source
    /// folder's slot; subsequent pointer moves can live-reorder it normally.
    fn promote_folder_child_drag_to_top_level(&mut self) -> bool {
        let Some(drag) = self.folders.child_drag.clone() else {
            return false;
        };
        let source_item = LauncherItem::Folder(drag.folder_id.clone());
        let insert_index = self
            .launcher_state
            .items
            .iter()
            .position(|item| item == &source_item)
            .unwrap_or(self.launcher_state.items.len());
        let Some(preview) = crate::features::folders::ChildExitPreview::begin(
            &self.launcher_state,
            &drag,
            insert_index,
        ) else {
            return false;
        };
        self.folders.child_exit_preview = Some(preview);

        self.drag_item = Some(LauncherItem::App(drag.app_id));
        self.drag_x = self.pointer_phys_x;
        self.drag_y = self.pointer_phys_y;
        self.folders.close();
        self.folders.hover = None;
        self.relayout();
        self.request_redraw();
        true
    }

    fn commit_folder_child_exit_preview(&mut self) -> bool {
        let Some(preview) = self.folders.take_child_exit_preview() else {
            return false;
        };
        let source_folder = preview.source_folder.clone();
        if preview.commit_into(&mut self.launcher_state) {
            true
        } else {
            self.folders.open(source_folder);
            false
        }
    }

    pub(crate) fn cancel_folder_child_exit_preview(&mut self) -> bool {
        let Some(preview) = self.folders.take_child_exit_preview() else {
            return false;
        };
        self.folders.open(preview.source_folder);
        true
    }

    pub(crate) fn handle_folder_pointer_release(&mut self, x: f32, y: f32) {
        use crate::ui_model::hit::HitTarget;
        if self
            .folder_scroller
            .as_ref()
            .is_some_and(|scroller| scroller.phase == Phase::Dragging)
        {
            if let Some(scroller) = self.folder_scroller.as_mut() {
                scroller.drag_end();
            }
            self.folders.clear_child_pointer();
            self.request_redraw();
            return;
        }
        if self.folders.child_drag.is_some() {
            let (valid_folder_drop, reordered) = self.reorder_folder_child_drag_at(x, y);
            let drag = self
                .folders
                .child_drag
                .clone()
                .expect("child drag was checked above");
            let mut changed = false;
            if valid_folder_drop {
                if let Some(folder) = self.launcher_state.folders.get_mut(&drag.folder_id) {
                    if folder.children != drag.preview_order {
                        folder.children = drag.preview_order;
                        changed = true;
                    }
                }
            } else if self.folder_child_boundary_intent(x, y)
                == crate::features::folders::ChildDragBoundaryIntent::Exit
            {
                self.pointer_phys_x = x;
                self.pointer_phys_y = y;
                if self.promote_folder_child_drag_to_top_level() {
                    self.commit_edit_drop();
                    self.drag_item = None;
                    self.relayout();
                    self.request_redraw();
                    return;
                }
            }
            if changed {
                self.persist_launcher_state();
                if !self.launcher_state.folders.contains_key(&drag.folder_id) {
                    self.folders.close();
                }
                self.relayout();
            } else if reordered {
                self.relayout();
            }
            self.folders.clear_child_pointer();
            self.request_redraw();
            return;
        }

        let pressed = self.folders.pressed_child.take();
        if let (Some(pressed), Some(HitTarget::FolderChild { child, .. })) =
            (pressed, self.folder_hit_target(x, y))
        {
            if !self.editing && pressed.is_click(x, y) && pressed.app_id.as_str() == child {
                if let Some(info) = self.registry.launch_info(&pressed.app_id) {
                    self.execute_command(crate::app::event::AppCommand::LaunchApp(info));
                }
            }
        }
        let page_press = self.folders.page_press.take();
        if self.editing
            && page_press
                .as_ref()
                .is_some_and(|press| !press.moved_past_slop(x, y))
            && matches!(
                self.folder_hit_target(x, y),
                Some(HitTarget::FolderPanel { .. })
            )
        {
            self.exit_edit_mode();
        }
    }

    fn settle_folder_page(&mut self, page: usize) {
        if let Some(scroller) = self.folder_scroller.as_mut() {
            scroller.settle_to_page(page);
        } else {
            self.folders.page = page;
        }
        self.request_redraw();
    }

    pub(crate) fn update_folder_page_from_scroll(&mut self) {
        let Some(layout) = self.folder_layout.as_ref() else {
            return;
        };
        let Some(scroller) = self.folder_scroller.as_ref() else {
            return;
        };
        let extent = layout.target_panel_rect.width.max(1.0);
        self.folders.page = ((-scroller.position / extent).round() as isize)
            .clamp(0, layout.page_count.saturating_sub(1) as isize)
            as usize;
    }
}

//! App state transitions: the `&mut self` methods that mutate `App` state in
//! response to routed input.

use std::time::Instant;

use crate::app_id::AppId;
use crate::app_registry::AppLaunchInfo;
use crate::debug_log;
use crate::icon_worker::IconResult;
use crate::refresh_watcher::RefreshMessage;
use crate::scroll::Phase;
use crate::settings::{Settings, SortOrder};

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
                    self.registry.set_order(Vec::new());
                    self.persist_user_order();
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
                self.registry.set_order(Vec::new());
                self.registry.set_hidden(Vec::new());
                self.persist_settings();
                self.persist_user_order();
                self.persist_hidden();
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

    pub(crate) fn handle_pointer_release(&mut self) -> Option<AppLaunchInfo> {
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
        let app_index = hit.app_index();
        let app_id = app_index.and_then(|idx| self.visible_app_ids().get(idx).cloned());
        let outside_glass = hit.is_outside_frame();
        debug_log!(
            "edit-press: pending x={x:.1} y={y:.1} app_index={app_index:?} outside_glass={outside_glass}"
        );
        self.pending_press = Some(PendingPress {
            start: now,
            x,
            y,
            app_index,
            app_id,
            outside_glass,
        });
        self.request_redraw();
    }

    /// Hide an app from the launcher (the ✕ badge action): removes it from the
    /// visible stream, persists the hidden list, and relayouts. Reversible by
    /// clearing the hidden list later. Stays a no-op if already hidden.
    ///
    /// The new order (hidden id moved to the tail so it does not linger
    /// invisibly mid-grid) is computed by
    /// [`features::edit_mode::hidden_order_after_hide`]; this function runs the
    /// registry mutation + persist side effects.
    pub(crate) fn hide_app(&mut self, id: &AppId) {
        if self.registry.is_hidden(id) {
            return;
        }
        self.registry.hide(id);
        // Drop the app from the user order too so it doesn't linger invisibly.
        let order = crate::features::edit_mode::hidden_order_after_hide(self.registry.order(), id);
        self.registry.set_order(order);
        self.persist_hidden();
        self.persist_user_order();
        // Drop any in-flight drag of the just-hidden app.
        if self.drag_app.as_ref() == Some(id) {
            self.drag_app = None;
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
        let app_index = p.app_index;
        self.enter_edit_mode(app_index);
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
        let visible = self.visible_app_ids();
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
        let mut state = self.edit_mode_state();
        // If a drag was in flight, finalize it as a drop at the current cell.
        let commit_commands = if state.drag_app.is_some() {
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
        if self.drag_app.is_some() {
            self.drag_x = self.pointer_phys_x;
            self.drag_y = self.pointer_phys_y;
            // Live reorder: move the dragged app to the cell under the pointer
            // so other icons shift around it.
            self.live_reorder();
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
        let Some(drag_id) = self.drag_app.clone() else {
            return;
        };
        let Some(target_idx) = self.edit_drop_index_at_pointer(self.drag_x, self.drag_y) else {
            return;
        };
        let visible = self.visible_app_ids();
        let Some(drag_pos) = visible.iter().position(|id| id == &drag_id) else {
            return;
        };
        let Some(insert_idx) =
            crate::layout::edit_mode::reorder_insert_index(visible.len(), drag_pos, target_idx)
        else {
            return;
        };
        debug_log!(
            "edit-reorder: moving drag_pos={drag_pos} target_idx={target_idx} insert_idx={insert_idx}"
        );
        self.reorder_by_index(&drag_id, insert_idx);
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
        if !self.editing || self.drag_app.is_none() {
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

    /// Reorder the registry so that `drag_id` moves to `insert_idx` in the
    /// visible order, shifting the apps between them. Hidden apps are preserved
    /// after the visible stream.
    ///
    /// The new order is computed by [`features::edit_mode::apply_reorder`]
    /// (pure); this function runs the `registry.set_order` + relayout side
    /// effect.
    pub(crate) fn reorder_by_index(&mut self, drag_id: &AppId, insert_idx: usize) {
        let visible = self.visible_app_ids();
        let hidden: Vec<AppId> = self.registry.hidden().iter().cloned().collect();
        let Some(order) =
            crate::features::edit_mode::apply_reorder(&visible, &hidden, drag_id, insert_idx)
        else {
            return;
        };
        self.registry.set_order(order);
        self.relayout();
    }

    pub(crate) fn request_redraw(&self) {
        if let Some(r) = self.renderer.as_ref() {
            r.window.request_redraw();
        }
    }

    /// Build a feature-side [`EditModeState`] mirror from the app boundary's
    /// source-of-truth fields. The feature module's decision functions operate
    /// on this mirror; [`Self::sync_edit_mode_state`] writes the result back.
    fn edit_mode_state(&self) -> crate::features::edit_mode::EditModeState {
        crate::features::edit_mode::EditModeState {
            editing: self.editing,
            drag_app: self.drag_app.clone(),
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
        self.drag_app = state.drag_app.clone();
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
                    crate::bottom_control::Mode::Pill
                    | crate::bottom_control::Mode::Indicator
                    | crate::bottom_control::Mode::Collapsing => {
                        self.control.open_search();
                    }
                    crate::bottom_control::Mode::Expanding | crate::bottom_control::Mode::Field => {
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

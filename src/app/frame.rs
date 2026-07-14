//! Per-frame tick and redraw orchestration.
//!
//! [`App::tick_frame`] is the body of the historical
//! `WindowEvent::RedrawRequested` handler: it advances the scroller, edit-mode
//! wiggle, tile springs, page indicator, bottom-control and settings-panel
//! animations, uploads the control/gear/settings geometry, syncs the OS IME,
//! and submits the render. It is extracted verbatim so the handler module can
//! stay a thin dispatcher.
//!
//! Behavior preservation: the ordering of every step (scroller tick →
//! autoscroll → live reorder → wiggle → springs → page indicator → control
//! tick → control/gear/settings upload → IME sync → render → animation-gated
//! redraw) is unchanged.

use std::time::Instant;

use crate::renderer::DrawArgs;
use crate::scroll::Phase;
use crate::startup_timer::prefix;

use super::state::App;

impl App {
    /// Advance one frame and submit the render. Returns early if the scroller
    /// is not yet initialized (mirrors the historical early `return`).
    pub(crate) fn tick_frame(&mut self) {
        let now = Instant::now();
        let vp = self.viewport_phys();
        let scroll_x;
        let dragging;
        if let Some(s) = self.scroller.as_mut() {
            dragging = s.phase == Phase::Dragging;
            s.tick(now);
            scroll_x = s.position;
        } else {
            return;
        }
        let scroller_animating = self
            .scroller
            .as_ref()
            .map(|s| s.is_animating())
            .unwrap_or(false);
        let folder_scroller_animating = if let Some(scroller) = self.folder_scroller.as_mut() {
            scroller.tick(now);
            scroller.is_animating()
        } else {
            false
        };
        if folder_scroller_animating {
            self.update_folder_page_from_scroll();
            self.relayout();
        }
        let auto_scroll_started = self.maybe_autoscroll_edit_drag();
        // Resolve the stable hover identity before any live reorder. Moving
        // the dragged item into the target cell first would make the next hit
        // resolve to the dragged item itself and continuously reset the
        // app-on-app / app-on-folder hover timer.
        let hover_candidate = self.folder_hover_candidate_at_pointer();
        let hover_ready =
            self.folders.hover.as_ref().is_some_and(|hover| {
                hover_candidate.as_ref() == Some(&hover.target) && hover.ready()
            });
        if self.editing
            && self.drag_item.is_some()
            && crate::features::folders::top_level_reorder_allowed(
                hover_candidate.as_ref(),
                hover_ready,
            )
        {
            self.live_reorder();
        }

        // Advance the wiggle animation phase while editing. dt is taken
        // from the redraw cadence (clamped like the control's).
        let anim_dt = match self.last_redraw {
            Some(prev) => now.duration_since(prev).as_secs_f32().min(0.1),
            None => 1.0 / 60.0,
        };
        if self.editing {
            self.wiggle_phase += anim_dt;
        }
        let candidate_folder = hover_candidate.as_ref().and_then(|item| match item {
            crate::domain::launcher_item::LauncherItem::Folder(id) => Some(id.clone()),
            crate::domain::launcher_item::LauncherItem::App(_) => None,
        });
        if self.folders.hover_opened.is_some()
            && self.folders.hover_opened.as_ref() != candidate_folder.as_ref()
        {
            self.folders.close();
        }
        let hover_changed = self.folders.update_hover(hover_candidate, anim_dt);
        if let (Some(folder_id), Some(hover)) = (candidate_folder, self.folders.hover.as_ref()) {
            if hover.ready() && self.folders.hover_opened.as_ref() != Some(&folder_id) {
                self.folders.open(folder_id.clone());
                self.folders.hover_opened = Some(folder_id);
            }
        }
        let folder_was_active = self.folders.is_active();
        let folder_animating = self.folders.tick(anim_dt);
        if folder_was_active && !self.folders.is_active() {
            self.folder_scroller = None;
        }
        if folder_animating || hover_changed || (folder_was_active && !self.folders.is_active()) {
            self.relayout();
        }

        // Advance the per-tile position springs and re-push the
        // instance buffers if any are still sliding (reorder animation).
        let springs_animating = self.step_tile_springs(anim_dt);
        if springs_animating {
            self.refresh_spring_instances();
        }
        let folder_child_springs_animating = self.step_folder_child_springs(anim_dt);
        if folder_child_springs_animating {
            self.relayout();
        }

        // Detect a page change (settled page differs from the last
        // tracked one) and arm the transient page indicator.
        let page = self.current_page() as i32;
        if page != self.last_page && !scroller_animating {
            self.last_page = page;
            self.control.on_page_change(now);
        }

        // Advance the bottom-control's animations + timers. Use the
        // real elapsed dt (not a fixed 1/60) so the caret blink and
        // morph speeds are correct even when redraws fire faster than
        // 60 Hz (e.g. on backdrop-frame arrivals).
        let control_dt = match self.last_redraw {
            Some(prev) => now.duration_since(prev).as_secs_f32().min(0.1),
            None => 1.0 / 60.0,
        };
        self.last_redraw = Some(now);
        let control_animating = self.control.tick(now, control_dt);
        let edit_control_animating = self.step_edit_control_width(control_dt);
        let settings_animating = self.step_settings_panel(control_dt);

        // Upload the control's capsule + overlays before the render.
        // This also measures query + preedit width for the IME cursor.
        let control_shape = self.render_bottom_control();
        self.refresh_interaction_glass();
        // Upload the corner gear capsule + glyph (if shown).
        self.render_gear(control_shape);
        // Upload the settings overlay panel (if open).
        self.render_settings_panel();
        self.render_folder_panel();

        // Sync the OS IME with the search field (on while focused,
        // parked at the caret) so Japanese / other IME input works.
        self.update_ime_state();

        // Submit one complete renderer-neutral frame model. Renderer-side
        // dirty tracking updates only lanes whose model data changed.
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.prepare(&self.render_model);
        }

        // Render the frame (consumes the uploaded buffers).
        let qa_capture_path = self.qa_capture_path(now);
        let qa_enabled = self.qa_enabled();
        if let Some(r) = self.renderer.as_mut() {
            if let Some(path) = qa_capture_path {
                r.qa_shot = Some(path);
            }
            // QA self-capture trigger: if LAUNCHPAD_QA_SHOT_FILE points
            // to a file whose contents name a path, the next rendered
            // frame is saved there as a PNG (see docs/EDIT_MODE_VISUAL_QA.md).
            // The harness writes the path, waits one frame, then reads
            // the PNG — letting CI / sandboxes capture arbitrary states
            // without foreground access.
            if r.qa_shot.is_none() {
                if let Some(trigger) = std::env::var_os("LAUNCHPAD_QA_SHOT_FILE") {
                    if let Ok(path_str) = std::fs::read_to_string(&trigger) {
                        let path_str = path_str.trim();
                        if !path_str.is_empty() {
                            r.qa_shot = Some(std::path::PathBuf::from(path_str));
                            // Clear the trigger so we only capture once per write.
                            let _ = std::fs::write(&trigger, "");
                        }
                    }
                }
            }
            r.render(&DrawArgs {
                scroll_x,
                viewport: vp,
                defer_backdrop_capture: dragging || qa_enabled,
                time: self.wiggle_phase,
                drag_active: if self.drag_item.is_some() || self.folders.child_drag.is_some() {
                    1.0
                } else {
                    0.0
                },
                drag_pos: (self.drag_x, self.drag_y),
            });
        }

        if !self.first_frame_rendered {
            self.first_frame_rendered = true;
            self.timer.mark(prefix::STARTUP, "first frame rendered");
        }
        if scroller_animating
            || auto_scroll_started
            || control_animating
            || edit_control_animating
            || settings_animating
            || folder_animating
            || hover_changed
            || springs_animating
            || folder_child_springs_animating
            || folder_scroller_animating
            || self.editing
        {
            self.request_redraw();
        }
    }
}

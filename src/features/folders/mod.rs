//! Folder interaction state: reversible open/close motion, rename editing,
//! drag hover previews, and child-order presentation. Durable mutations stay
//! in `domain::LauncherState`; this module never performs persistence or GPU
//! work.

use crate::domain::app_id::AppId;
use crate::domain::folders::FolderId;
use crate::domain::launcher_item::LauncherItem;
use std::time::Instant;

pub const HOVER_OPEN_DELAY: f32 = 0.38;
const HOVER_PREVIEW_DURATION: f32 = 0.42;
const MOTION_OMEGA: f32 = 17.0;
const MOTION_ZETA: f32 = 0.9;
const MOTION_EPS: f32 = 0.001;
const MAX_STEP: f32 = 1.0 / 120.0;
const CHILD_DRAG_SLOP: f32 = 8.0;

/// A completed folder dwell owns the drop. Before the dwell completes, normal
/// reordering may still win after the pointer crosses the target's far-side
/// threshold; the layout layer enforces that geometric threshold.
pub fn top_level_reorder_allowed(
    hover_candidate: Option<&LauncherItem>,
    hover_ready: bool,
) -> bool {
    hover_candidate.is_none() || !hover_ready
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderPhase {
    Closed,
    Opening,
    Open,
    Closing,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FolderMotion {
    pub progress: f32,
    pub velocity: f32,
    pub target: f32,
}

impl Default for FolderMotion {
    fn default() -> Self {
        Self {
            progress: 0.0,
            velocity: 0.0,
            target: 0.0,
        }
    }
}

impl FolderMotion {
    pub fn step(&mut self, dt: f32) -> bool {
        let mut remaining = dt.clamp(0.0, 0.1);
        while remaining > 0.0 {
            let step = remaining.min(MAX_STEP);
            remaining -= step;
            let displacement = self.progress - self.target;
            let acceleration = -MOTION_OMEGA * MOTION_OMEGA * displacement
                - 2.0 * MOTION_ZETA * MOTION_OMEGA * self.velocity;
            self.velocity += acceleration * step;
            self.progress += self.velocity * step;
        }
        if (self.progress - self.target).abs() < MOTION_EPS && self.velocity.abs() < MOTION_EPS {
            self.progress = self.target;
            self.velocity = 0.0;
            false
        } else {
            true
        }
    }

    pub fn visual_progress(self) -> f32 {
        self.progress.clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameEditor {
    original: String,
    pub text: String,
    pub preedit: String,
    pub cursor: usize,
}

impl RenameEditor {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        Self {
            original: value.clone(),
            cursor: value.len(),
            text: value,
            preedit: String::new(),
        }
    }

    pub fn visible_text(&self) -> String {
        let mut value = self.text.clone();
        value.insert_str(self.cursor, &self.preedit);
        value
    }

    pub fn set_preedit(&mut self, value: String) {
        self.preedit = value;
    }

    pub fn commit_text(&mut self, value: &str) {
        self.preedit.clear();
        self.text.insert_str(self.cursor, value);
        self.cursor += value.len();
    }

    pub fn backspace(&mut self) {
        if !self.preedit.is_empty() || self.cursor == 0 {
            return;
        }
        let previous = self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap_or(0);
        self.text.drain(previous..self.cursor);
        self.cursor = previous;
    }

    pub fn move_left(&mut self) {
        if self.preedit.is_empty() && self.cursor > 0 {
            self.cursor = self.text[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(index, _)| index)
                .unwrap_or(0);
        }
    }

    pub fn move_right(&mut self) {
        if self.preedit.is_empty() && self.cursor < self.text.len() {
            self.cursor += self.text[self.cursor..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(0);
        }
    }

    pub fn committed_name(&self) -> String {
        let value = self.text.trim();
        if value.is_empty() {
            "フォルダ".to_owned()
        } else {
            value.to_owned()
        }
    }

    pub fn original(&self) -> &str {
        &self.original
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FolderHover {
    pub target: LauncherItem,
    pub elapsed: f32,
}

impl FolderHover {
    pub fn progress(&self) -> f32 {
        (self.elapsed / HOVER_OPEN_DELAY).clamp(0.0, 1.0)
    }

    pub fn ready(&self) -> bool {
        self.elapsed >= HOVER_OPEN_DELAY
    }

    /// Presentation-only panel progress after the formation threshold. Domain
    /// mutation still waits for drop.
    pub fn panel_progress(&self) -> f32 {
        ((self.elapsed - HOVER_OPEN_DELAY) / HOVER_PREVIEW_DURATION).clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PressedChild {
    pub app_id: AppId,
    pub index: usize,
    pub start: Instant,
    pub start_x: f32,
    pub start_y: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PagePress {
    pub start_x: f32,
    pub start_y: f32,
}

impl PagePress {
    pub fn moved_past_slop(&self, x: f32, y: f32) -> bool {
        let dx = x - self.start_x;
        let dy = y - self.start_y;
        dx * dx + dy * dy > CHILD_DRAG_SLOP * CHILD_DRAG_SLOP
    }
}

impl PressedChild {
    pub fn moved_past_slop(&self, x: f32, y: f32) -> bool {
        let dx = x - self.start_x;
        let dy = y - self.start_y;
        dx * dx + dy * dy > CHILD_DRAG_SLOP * CHILD_DRAG_SLOP
    }

    pub fn is_click(&self, x: f32, y: f32) -> bool {
        !self.moved_past_slop(x, y)
    }

    pub fn long_press_ready(&self, now: Instant, x: f32, y: f32) -> bool {
        !self.moved_past_slop(x, y)
            && now.duration_since(self.start) >= crate::features::edit_mode::LONG_PRESS_THRESHOLD
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildDrag {
    pub folder_id: FolderId,
    pub app_id: AppId,
    pub origin_index: usize,
    pub preview_order: Vec<AppId>,
}

impl ChildDrag {
    pub fn preview_reorder(&mut self, index: usize) -> bool {
        let Some(old) = self.preview_order.iter().position(|id| id == &self.app_id) else {
            return false;
        };
        let id = self.preview_order.remove(old);
        let new = index.min(self.preview_order.len());
        self.preview_order.insert(new, id);
        old != new
    }

    /// Reorder against a stable child identity rather than a visible-only
    /// cell index. This keeps undiscovered placeholder children in the domain
    /// order while the visible children still move to the cell under the
    /// pointer.
    pub fn preview_reorder_to(&mut self, target: &AppId) -> bool {
        let Some(index) = self.preview_order.iter().position(|id| id == target) else {
            return false;
        };
        self.preview_reorder(index)
    }
}

#[derive(Debug, Clone, Default)]
pub struct FolderFeatureState {
    pub active: Option<FolderId>,
    pub phase: FolderPhase,
    pub motion: FolderMotion,
    pub page: usize,
    pub rename: Option<RenameEditor>,
    pub hover: Option<FolderHover>,
    /// Existing folder opened as a reversible drag-hover preview. This is
    /// distinct from a folder the user opened normally so leaving the hover
    /// target can close only the preview.
    pub hover_opened: Option<FolderId>,
    pub pressed_child: Option<PressedChild>,
    pub page_press: Option<PagePress>,
    pub child_drag: Option<ChildDrag>,
}

impl Default for FolderPhase {
    fn default() -> Self {
        Self::Closed
    }
}

impl FolderFeatureState {
    pub fn open(&mut self, id: FolderId) {
        if self.active.as_ref() != Some(&id) {
            self.active = Some(id);
            self.motion.progress = 0.0;
            self.motion.velocity = 0.0;
            self.page = 0;
        }
        self.motion.target = 1.0;
        self.phase = FolderPhase::Opening;
    }

    pub fn close(&mut self) {
        if self.active.is_none() {
            return;
        }
        self.rename = None;
        self.pressed_child = None;
        self.page_press = None;
        self.child_drag = None;
        self.hover_opened = None;
        self.motion.target = 0.0;
        self.phase = FolderPhase::Closing;
    }

    pub fn tick(&mut self, dt: f32) -> bool {
        if self.active.is_none() {
            return false;
        }
        let animating = self.motion.step(dt);
        if !animating {
            if self.motion.target >= 1.0 {
                self.phase = FolderPhase::Open;
            } else {
                self.phase = FolderPhase::Closed;
                self.active = None;
                self.page = 0;
            }
        }
        animating
    }

    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub fn begin_rename(&mut self, value: impl Into<String>) {
        self.rename = Some(RenameEditor::new(value));
    }

    pub fn cancel_rename(&mut self) -> bool {
        self.rename.take().is_some()
    }

    pub fn finish_rename(&mut self) -> Option<String> {
        self.rename.take().map(|editor| editor.committed_name())
    }

    pub fn update_hover(&mut self, candidate: Option<LauncherItem>, dt: f32) -> bool {
        match candidate {
            Some(target) => match self.hover.as_mut() {
                Some(hover) if hover.target == target => {
                    let before = hover.elapsed;
                    hover.elapsed = (hover.elapsed + dt.max(0.0))
                        .min(HOVER_OPEN_DELAY + HOVER_PREVIEW_DURATION);
                    (hover.elapsed - before).abs() > f32::EPSILON
                }
                _ => {
                    self.hover = Some(FolderHover {
                        target,
                        elapsed: dt.clamp(0.0, HOVER_OPEN_DELAY + HOVER_PREVIEW_DURATION),
                    });
                    true
                }
            },
            None => self.hover.take().is_some(),
        }
    }

    pub fn begin_child_press(
        &mut self,
        app_id: AppId,
        index: usize,
        start: Instant,
        x: f32,
        y: f32,
    ) {
        self.pressed_child = Some(PressedChild {
            app_id,
            index,
            start,
            start_x: x,
            start_y: y,
        });
    }

    pub fn begin_page_press(&mut self, x: f32, y: f32) {
        self.page_press = Some(PagePress {
            start_x: x,
            start_y: y,
        });
    }

    pub fn child_long_press_ready(&self, now: Instant, x: f32, y: f32) -> bool {
        self.pressed_child
            .as_ref()
            .is_some_and(|press| press.long_press_ready(now, x, y))
    }

    pub fn maybe_begin_child_drag(&mut self, children: &[AppId], x: f32, y: f32) -> bool {
        let Some(press) = self.pressed_child.as_ref() else {
            return false;
        };
        if !press.moved_past_slop(x, y) {
            return false;
        }
        let Some(folder_id) = self.active.clone() else {
            return false;
        };
        self.child_drag = Some(ChildDrag {
            folder_id,
            app_id: press.app_id.clone(),
            origin_index: press.index,
            preview_order: children.to_vec(),
        });
        self.pressed_child = None;
        true
    }

    pub fn clear_child_pointer(&mut self) {
        self.pressed_child = None;
        self.page_press = None;
        self.child_drag = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn app(value: &str) -> AppId {
        AppId::from_normalized(value.to_owned())
    }

    #[test]
    fn motion_reverses_without_snapping() {
        let mut state = FolderFeatureState::default();
        state.open(FolderId::generate(0));
        for _ in 0..8 {
            state.tick(1.0 / 60.0);
        }
        let mid = state.motion.progress;
        assert!(mid > 0.0 && mid < 1.0);
        state.close();
        state.tick(1.0 / 120.0);
        assert!(state.motion.progress > 0.0);
    }

    #[test]
    fn motion_endpoints_match_at_60_and_120_hz() {
        for hz in [15.0, 60.0, 120.0] {
            let mut state = FolderFeatureState::default();
            state.open(FolderId::generate(0));
            for _ in 0..(hz as usize * 2) {
                state.tick(1.0 / hz);
            }
            assert_eq!(state.phase, FolderPhase::Open);
            assert_eq!(state.motion.progress, 1.0);
        }
    }

    #[test]
    fn dropped_frame_uses_the_same_adaptive_substeps() {
        let mut dropped = FolderMotion {
            target: 1.0,
            ..FolderMotion::default()
        };
        let mut regular = dropped;
        dropped.step(0.1);
        for _ in 0..12 {
            regular.step(1.0 / 120.0);
        }
        assert!((dropped.progress - regular.progress).abs() < 1e-5);
        assert!((dropped.velocity - regular.velocity).abs() < 1e-5);
    }

    #[test]
    fn open_and_close_reach_exact_state_endpoints() {
        let id = FolderId::generate(3);
        let mut state = FolderFeatureState::default();
        state.open(id.clone());
        assert_eq!(state.active, Some(id));
        assert_eq!(state.phase, FolderPhase::Opening);
        for _ in 0..240 {
            state.tick(1.0 / 120.0);
        }
        assert_eq!(state.phase, FolderPhase::Open);
        assert_eq!(state.motion.progress, 1.0);
        state.close();
        for _ in 0..240 {
            state.tick(1.0 / 120.0);
        }
        assert_eq!(state.phase, FolderPhase::Closed);
        assert!(state.active.is_none());
        assert_eq!(state.motion.progress, 0.0);
    }

    #[test]
    fn rename_is_utf8_safe_and_normalizes_blank() {
        let mut editor = RenameEditor::new("仕事");
        editor.move_left();
        editor.backspace();
        editor.commit_text("予定");
        assert_eq!(editor.text, "予定事");
        assert_eq!(RenameEditor::new("  ").committed_name(), "フォルダ");
    }

    #[test]
    fn hover_does_not_commit_before_threshold() {
        let mut state = FolderFeatureState::default();
        let target = LauncherItem::App(app("target"));
        state.update_hover(Some(target.clone()), HOVER_OPEN_DELAY * 0.5);
        assert!(!state.hover.as_ref().unwrap().ready());
        state.update_hover(None, 0.0);
        assert!(state.hover.is_none());
    }

    #[test]
    fn hover_preview_starts_only_after_threshold_without_domain_mutation() {
        let mut state = FolderFeatureState::default();
        let target = LauncherItem::App(app("target"));
        state.update_hover(Some(target.clone()), HOVER_OPEN_DELAY);
        let hover = state.hover.as_ref().unwrap();
        assert!(hover.ready());
        assert_eq!(hover.panel_progress(), 0.0);
        assert!(state.active.is_none());
        state.update_hover(Some(target), HOVER_PREVIEW_DURATION * 0.5);
        assert!((state.hover.as_ref().unwrap().panel_progress() - 0.5).abs() < 1e-5);
        assert!(state.active.is_none());
    }

    #[test]
    fn stable_folder_hover_suspends_normal_top_level_reorder() {
        let app_target = LauncherItem::App(app("target"));
        let folder_target = LauncherItem::Folder(FolderId::generate(1));
        assert!(top_level_reorder_allowed(Some(&app_target), false));
        assert!(top_level_reorder_allowed(Some(&folder_target), false));
        assert!(!top_level_reorder_allowed(Some(&app_target), true));
        assert!(top_level_reorder_allowed(None, false));
    }

    #[test]
    fn child_reorder_is_presentation_only_until_caller_commits() {
        let original = vec![app("a"), app("b"), app("c")];
        let mut drag = ChildDrag {
            folder_id: FolderId::generate(0),
            app_id: app("a"),
            origin_index: 0,
            preview_order: original.clone(),
        };
        assert!(drag.preview_reorder(2));
        assert_eq!(drag.preview_order, vec![app("b"), app("c"), app("a")]);
        assert_eq!(original, vec![app("a"), app("b"), app("c")]);
    }

    #[test]
    fn child_reorder_uses_stable_target_with_undiscovered_placeholder() {
        let mut drag = ChildDrag {
            folder_id: FolderId::generate(0),
            app_id: app("a"),
            origin_index: 0,
            preview_order: vec![app("a"), app("undiscovered"), app("b")],
        };

        assert!(drag.preview_reorder_to(&app("b")));
        assert_eq!(
            drag.preview_order,
            vec![app("undiscovered"), app("b"), app("a")]
        );
    }

    #[test]
    fn child_long_press_uses_the_shared_edit_threshold_and_slop() {
        let start = Instant::now();
        let mut state = FolderFeatureState::default();
        state.begin_child_press(app("a"), 0, start, 100.0, 100.0);

        assert!(!state.child_long_press_ready(
            start + crate::features::edit_mode::LONG_PRESS_THRESHOLD - Duration::from_millis(1),
            100.0,
            100.0,
        ));
        assert!(state.child_long_press_ready(
            start + crate::features::edit_mode::LONG_PRESS_THRESHOLD,
            100.0,
            100.0,
        ));
        assert!(!state.child_long_press_ready(
            start + crate::features::edit_mode::LONG_PRESS_THRESHOLD,
            109.0,
            100.0,
        ));
    }

    #[test]
    fn folder_child_click_rejects_a_drag_distance() {
        let press = PressedChild {
            app_id: app("a"),
            index: 0,
            start: Instant::now(),
            start_x: 20.0,
            start_y: 30.0,
        };
        assert!(press.is_click(27.0, 30.0));
        assert!(!press.is_click(29.0, 30.0));
    }

    #[test]
    fn page_press_uses_the_same_movement_slop_as_child_gestures() {
        let press = PagePress {
            start_x: 40.0,
            start_y: 50.0,
        };
        assert!(!press.moved_past_slop(47.0, 50.0));
        assert!(press.moved_past_slop(49.0, 50.0));
    }
}

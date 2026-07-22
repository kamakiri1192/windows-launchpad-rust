//! Tests for the edit-mode feature module.

use super::*;
use crate::domain::app_id::AppId;
use crate::domain::folders::FolderId;
use crate::domain::launcher_item::LauncherItem;
use crate::layout::grid::GridHit;
use std::time::{Duration, Instant};

fn app(id: &str) -> AppId {
    AppId::from_normalized(id.to_string())
}

fn item(id: &str) -> LauncherItem {
    LauncherItem::App(app(id))
}

fn ptr(x: f32, y: f32) -> PointerSnapshot {
    PointerSnapshot::new(x, y)
}

fn press(start: Instant, x: f32, y: f32, outside: bool, pointer: PointerSnapshot) -> PressSnapshot {
    PressSnapshot {
        start,
        x,
        y,
        outside_glass: outside,
        pointer,
    }
}

// ---- long-press entry -----------------------------------------------------

#[test]
fn long_press_enters_edit_mode_after_threshold() {
    let start = Instant::now();
    let p = press(start, 100.0, 100.0, false, ptr(100.0, 100.0));
    let now = start + LONG_PRESS_THRESHOLD;
    assert!(should_enter_from_long_press(&p, now));
}

#[test]
fn long_press_does_not_enter_before_threshold() {
    let start = Instant::now();
    let p = press(start, 100.0, 100.0, false, ptr(100.0, 100.0));
    // Just under the threshold.
    let now = start + LONG_PRESS_THRESHOLD - Duration::from_millis(1);
    assert!(!should_enter_from_long_press(&p, now));
}

#[test]
fn long_press_does_not_enter_outside_glass() {
    let start = Instant::now();
    // outside_glass = true → passthrough path, never edit mode.
    let p = press(start, 5.0, 5.0, true, ptr(5.0, 5.0));
    let now = start + LONG_PRESS_THRESHOLD;
    assert!(!should_enter_from_long_press(&p, now));
}

#[test]
fn long_press_does_not_enter_after_slop_exceeded() {
    let start = Instant::now();
    // Press started at (100,100) but the pointer has since moved past slop.
    // (At exactly CLICK_SLOP_PHYS the <= check still allows the long-press, so
    // the test moves one px past the boundary.)
    let p = press(
        start,
        100.0,
        100.0,
        false,
        ptr(100.0 + CLICK_SLOP_PHYS + 1.0, 100.0),
    );
    let now = start + LONG_PRESS_THRESHOLD;
    assert!(!should_enter_from_long_press(&p, now));
}

#[test]
fn long_press_tolerates_movement_within_slop() {
    let start = Instant::now();
    // Move a little, but stay within slop → still a long-press candidate.
    let p = press(
        start,
        100.0,
        100.0,
        false,
        ptr(100.0 + CLICK_SLOP_PHYS * 0.5, 100.0 + CLICK_SLOP_PHYS * 0.5),
    );
    let now = start + LONG_PRESS_THRESHOLD;
    assert!(should_enter_from_long_press(&p, now));
}

#[test]
fn within_click_slop_boundary() {
    // Exactly at slop radius: dx^2+dy^2 == slop^2 → within (<=).
    assert!(within_click_slop(CLICK_SLOP_PHYS, 0.0));
    // Just over slop on both axes → outside.
    assert!(!within_click_slop(CLICK_SLOP_PHYS, CLICK_SLOP_PHYS));
}

// ---- edit press classification -------------------------------------------

#[test]
fn edit_press_badge_hit_classifies_as_hide() {
    let intent = edit_press_classify(GridHit::App(3), true);
    assert_eq!(intent, EditPressIntent::HideApp { visible_index: 3 });
}

#[test]
fn edit_press_app_without_badge_classifies_as_drag() {
    let intent = edit_press_classify(GridHit::App(3), false);
    assert_eq!(intent, EditPressIntent::DragApp { visible_index: 3 });
}

#[test]
fn edit_press_badge_takes_precedence_over_drag() {
    // The same app hit, but with badge_hit=true → HideApp, not DragApp.
    assert_eq!(
        edit_press_classify(GridHit::App(3), true),
        EditPressIntent::HideApp { visible_index: 3 }
    );
    assert_ne!(
        edit_press_classify(GridHit::App(3), true),
        EditPressIntent::DragApp { visible_index: 3 }
    );
}

#[test]
fn edit_press_empty_in_frame_exits_edit_mode() {
    let intent = edit_press_classify(GridHit::EmptyInFrame, false);
    assert_eq!(intent, EditPressIntent::EmptyExit);
}

#[test]
fn edit_press_outside_frame_is_noop_for_edit_branch() {
    let intent = edit_press_classify(GridHit::OutsideFrame, false);
    assert_eq!(intent, EditPressIntent::Noop);
}

// ---- enter / exit / drag lifecycle ---------------------------------------

#[test]
fn enter_with_app_lifts_into_drag_and_resets_state() {
    let mut state = EditModeState::new();
    state.pending_press = true;
    state.wiggle_phase = 5.0;
    let visible = vec![app("a"), app("b"), app("c")];
    let cmds = enter(&mut state, Some(1), &visible, ptr(200.0, 300.0));
    assert!(state.editing);
    assert!(!state.pending_press);
    assert!((state.wiggle_phase - 0.0).abs() < 1e-6);
    assert_eq!(state.drag_item, Some(item("b")));
    assert!((state.drag_x - 200.0).abs() < 1e-6);
    assert!((state.drag_y - 300.0).abs() < 1e-6);
    // Must cancel scroll, clear pending press, reset wiggle, lift, relayout, redraw.
    assert!(cmds.contains(&EditModeCommand::CancelScroll));
    assert!(cmds.contains(&EditModeCommand::ClearPendingPress));
    assert!(cmds.contains(&EditModeCommand::ResetWigglePhase));
    assert!(cmds.contains(&EditModeCommand::SetEditing(true)));
    assert!(cmds.contains(&EditModeCommand::SetDragItem(Some(item("b")))));
    assert!(cmds.contains(&EditModeCommand::Relayout));
    assert!(cmds.contains(&EditModeCommand::RequestRedraw));
}

#[test]
fn enter_with_empty_long_press_does_not_lift() {
    let mut state = EditModeState::new();
    let visible = vec![app("a"), app("b")];
    let cmds = enter(&mut state, None, &visible, ptr(50.0, 50.0));
    assert!(state.editing);
    assert_eq!(state.drag_item, None);
    // No SetDragApp command.
    assert!(!cmds
        .iter()
        .any(|c| matches!(c, EditModeCommand::SetDragItem(Some(_)))));
}

#[test]
fn enter_with_out_of_range_index_does_not_lift() {
    let mut state = EditModeState::new();
    let visible = vec![app("a")];
    let cmds = enter(&mut state, Some(5), &visible, ptr(50.0, 50.0));
    assert!(state.editing);
    assert_eq!(state.drag_item, None);
    assert!(!cmds
        .iter()
        .any(|c| matches!(c, EditModeCommand::SetDragItem(Some(_)))));
}

#[test]
fn exit_commits_drag_then_clears_state() {
    let mut state = EditModeState {
        editing: true,
        drag_item: Some(item("b")),
        drag_x: 1.0,
        drag_y: 2.0,
        wiggle_phase: 0.5,
        pending_press: true,
    };
    let commit = commit_drag(&state);
    let cmds = exit(&mut state, commit);
    assert!(!state.editing);
    assert_eq!(state.drag_item, None);
    assert!(!state.pending_press);
    // Commit commands ran first (SetSortManual + persist).
    let commit_idx = cmds
        .iter()
        .position(|c| matches!(c, EditModeCommand::SetSortManual))
        .unwrap();
    let exit_idx = cmds
        .iter()
        .position(|c| matches!(c, EditModeCommand::SetEditing(false)))
        .unwrap();
    assert!(
        commit_idx < exit_idx,
        "commit must run before the exit clear"
    );
    assert!(cmds.contains(&EditModeCommand::SetDragItem(None)));
    assert!(cmds.contains(&EditModeCommand::PersistUserOrder));
    assert!(cmds.contains(&EditModeCommand::PersistSettings));
    assert!(cmds.contains(&EditModeCommand::Relayout));
}

#[test]
fn exit_without_drag_skips_commit() {
    let mut state = EditModeState {
        editing: true,
        drag_item: None,
        ..EditModeState::default()
    };
    let cmds = exit(&mut state, Vec::new());
    assert!(!cmds
        .iter()
        .any(|c| matches!(c, EditModeCommand::SetSortManual)));
    assert!(cmds.contains(&EditModeCommand::SetEditing(false)));
}

#[test]
fn start_drag_lifts_visible_app() {
    let mut state = EditModeState::new();
    let visible = vec![app("a"), app("b"), app("c")];
    let cmds = start_drag(&mut state, &visible, 2, ptr(400.0, 500.0));
    assert_eq!(state.drag_item, Some(item("c")));
    assert!((state.drag_x - 400.0).abs() < 1e-6);
    assert!(cmds.contains(&EditModeCommand::SetDragItem(Some(item("c")))));
}

#[test]
fn start_drag_out_of_range_is_noop() {
    let mut state = EditModeState::new();
    let visible = vec![app("a")];
    let cmds = start_drag(&mut state, &visible, 9, ptr(0.0, 0.0));
    assert!(cmds.is_empty());
    assert_eq!(state.drag_item, None);
}

#[test]
fn drag_move_updates_follow_position() {
    let mut state = EditModeState {
        editing: true,
        drag_item: Some(item("a")),
        ..EditModeState::default()
    };
    let cmds = drag_move(&mut state, ptr(123.0, 456.0));
    assert!((state.drag_x - 123.0).abs() < 1e-6);
    assert!((state.drag_y - 456.0).abs() < 1e-6);
    assert!(cmds.contains(&EditModeCommand::SetDragPos(123.0, 456.0)));
    assert!(cmds.contains(&EditModeCommand::RequestRedraw));
}

#[test]
fn commit_drag_only_persists_when_drag_in_flight() {
    let with_drag = EditModeState {
        drag_item: Some(item("a")),
        ..EditModeState::default()
    };
    let without_drag = EditModeState::default();
    assert!(!commit_drag(&with_drag).is_empty());
    assert!(commit_drag(&without_drag).is_empty());
}

// ---- reorder order computation -------------------------------------------

#[test]
fn apply_reorder_moves_drag_id_to_insert_index() {
    let visible = vec![app("a"), app("b"), app("c"), app("d")];
    let order = apply_reorder(&visible, &[], &app("a"), 3).unwrap();
    assert_eq!(order, vec![app("b"), app("c"), app("d"), app("a")]);
}

#[test]
fn apply_reorder_clamps_insert_index_to_len() {
    let visible = vec![app("a"), app("b")];
    // insert_idx way past the end clamps to len.
    let order = apply_reorder(&visible, &[], &app("a"), 99).unwrap();
    assert_eq!(order, vec![app("b"), app("a")]);
}

#[test]
fn apply_reorder_preserves_hidden_after_visible() {
    let visible = vec![app("a"), app("b"), app("c")];
    let hidden = vec![app("h1"), app("h2")];
    // Drag 'a' to index 2 in the visible stream; hidden apps stay at the tail.
    let order = apply_reorder(&visible, &hidden, &app("a"), 2).unwrap();
    assert_eq!(
        order,
        vec![app("b"), app("c"), app("a"), app("h1"), app("h2")]
    );
}

#[test]
fn apply_reorder_drag_id_not_present_returns_none() {
    let visible = vec![app("a"), app("b")];
    assert!(apply_reorder(&visible, &[], &app("zzz"), 0).is_none());
}

#[test]
fn apply_item_reorder_inserts_app_between_adjacent_folders() {
    let left = LauncherItem::Folder(FolderId::generate(1));
    let right = LauncherItem::Folder(FolderId::generate(2));
    let dragged = item("dragged");
    let visible = vec![left.clone(), right.clone(), dragged.clone()];

    let order = apply_item_reorder(&visible, &dragged, 1).unwrap();

    assert_eq!(order, vec![left, dragged, right]);
}

#[test]
fn hidden_order_after_hide_moves_id_to_tail() {
    let order = vec![app("a"), app("b"), app("c")];
    let new_order = hidden_order_after_hide(&order, &app("b"));
    assert_eq!(new_order, vec![app("a"), app("c"), app("b")]);
}

#[test]
fn hidden_order_after_hide_noop_for_missing_id() {
    let order = vec![app("a"), app("b")];
    let new_order = hidden_order_after_hide(&order, &app("zzz"));
    // The id is appended at the tail (matches the historical behavior).
    assert_eq!(new_order, vec![app("a"), app("b"), app("zzz")]);
}

// ---- commit reorder → SortOrder::Manual + persistence --------------------

#[test]
fn commit_drag_emits_set_sort_manual_before_persist() {
    // The historical commit_reorder sets SortOrder::Manual and then persists
    // both settings and user order. The feature ordering keeps SetSortManual
    // first so the app boundary can apply it before persisting settings.
    let state = EditModeState {
        drag_item: Some(item("a")),
        ..EditModeState::default()
    };
    let cmds = commit_drag(&state);
    let sort_idx = cmds
        .iter()
        .position(|c| matches!(c, EditModeCommand::SetSortManual))
        .unwrap();
    let persist_settings_idx = cmds
        .iter()
        .position(|c| matches!(c, EditModeCommand::PersistSettings))
        .unwrap();
    let persist_order_idx = cmds
        .iter()
        .position(|c| matches!(c, EditModeCommand::PersistUserOrder))
        .unwrap();
    assert!(sort_idx < persist_settings_idx);
    assert!(sort_idx < persist_order_idx);
}

#[test]
fn commit_drag_emits_both_persist_commands() {
    let state = EditModeState {
        drag_item: Some(item("a")),
        ..EditModeState::default()
    };
    let cmds = commit_drag(&state);
    assert!(cmds.contains(&EditModeCommand::PersistSettings));
    assert!(cmds.contains(&EditModeCommand::PersistUserOrder));
}

// ---- press classification edge cases -------------------------------------

#[test]
fn edit_press_drag_and_hide_share_visible_index() {
    // The same `GridHit::App(2)` produces HideApp with a badge hit and DragApp
    // without; the visible_index is preserved in both, so the app boundary can
    // resolve the same app either way.
    let drag = edit_press_classify(GridHit::App(2), false);
    let hide = edit_press_classify(GridHit::App(2), true);
    match (drag, hide) {
        (
            EditPressIntent::DragApp { visible_index: d },
            EditPressIntent::HideApp { visible_index: h },
        ) => {
            assert_eq!(d, 2);
            assert_eq!(h, 2);
        }
        _ => panic!("expected DragApp then HideApp"),
    }
}

#[test]
fn edit_press_outside_frame_badge_does_not_classify_as_hide() {
    // A badge hit test only runs when the grid classifier says App; an
    // outside-frame hit never reaches HideApp even if the caller passed
    // badge_hit=true (which it would not, but the classifier must be robust).
    assert_eq!(
        edit_press_classify(GridHit::OutsideFrame, true),
        EditPressIntent::Noop
    );
}

// ---- reorder order preservation of hidden apps ---------------------------

#[test]
fn apply_reorder_preserves_historical_concatenated_insert_behavior() {
    // The historical `reorder_by_index` operated on the visible-stream-then-
    // hidden concatenated list, with insert_idx clamped to visible.len() by the
    // caller (`live_reorder`). This test pins that exact behavior so the
    // behavior-preserving refactor does not silently change where the dragged
    // id lands in the persisted order relative to hidden apps.
    let visible = vec![app("a"), app("b")];
    let hidden = vec![app("h1"), app("h2"), app("h3")];
    // Drag 'a' to insert_idx = visible.len() (== 2). Concatenated list is
    // [a,b,h1,h2,h3]; removing 'a' (pos 0) yields [b,h1,h2,h3]; inserting at
    // index 2 yields [b,h1,a,h2,h3]. The *visible* result (after the registry
    // filters hidden) is still [b,a] — the drag visibly landed at the tail.
    let order = apply_reorder(&visible, &hidden, &app("a"), 2).unwrap();
    assert_eq!(
        order,
        vec![app("b"), app("h1"), app("a"), app("h2"), app("h3")]
    );
    // Dragging 'b' to the first cell (insert_idx = 0) moves it before 'a';
    // hidden apps stay in their original relative order.
    let order = apply_reorder(&visible, &hidden, &app("b"), 0).unwrap();
    assert_eq!(
        order,
        vec![app("b"), app("a"), app("h1"), app("h2"), app("h3")]
    );
}

#[test]
fn apply_reorder_dragging_forward_then_backward_is_invertible() {
    let visible = vec![app("a"), app("b"), app("c"), app("d")];
    // Move 'a' forward to index 3.
    let forward = apply_reorder(&visible, &[], &app("a"), 3).unwrap();
    assert_eq!(forward, vec![app("b"), app("c"), app("d"), app("a")]);
    // Move 'a' back to index 0.
    let back = apply_reorder(&forward, &[], &app("a"), 0).unwrap();
    assert_eq!(back, vec![app("a"), app("b"), app("c"), app("d")]);
}

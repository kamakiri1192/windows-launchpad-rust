# DF Current Behavior Inventory

Status: migration support document.

This document records current user-visible behavior that rearchitecture slices
must preserve. It is intentionally concrete. Each vertical slice should update
or extend this inventory before moving behavior behind a new boundary.

## Settings Overlay

Source before extraction:

- `src/main.rs`
- `src/settings.rs`
- `src/bottom_control.rs` control overlay instance kinds
- `src/text.rs` centered line text renderer
- `src/renderer.rs` settings glass, control, and text upload methods

Current behavior to preserve:

- The overlay opens from the edit-mode settings gear and the tray Settings
  command.
- Opening settings exits edit mode, closes the search field, clears pending
  grid/control presses, and requests redraw.
- Closing settings sets `settings_open = false` and lets the close animation
  finish through `settings_panel_progress`.
- Hiding the launcher closes the overlay and resets `settings_panel_progress`
  to `0.0`.
- While settings is open, pointer presses are consumed by the overlay. No grid,
  search, edit-mode, or app-launch interaction is reachable underneath.
- Press and release must match for most settings controls. A press outside and
  release outside closes the overlay.
- Outside settings clicks are modal dismiss clicks. They must not replay a
  click to the underlying Windows app.
- The close button closes the overlay.
- Sidebar category rows switch the active settings category and request redraw.
- Apps category:
  - sort segment selects `SortOrder`;
  - selecting name sort clears manual app order, persists user order, relayouts,
    persists settings, and requests redraw;
  - frequent-apps row toggles `frequent_apps_enabled`, persists settings, and
    requests redraw;
  - hidden-apps row is currently informational and does not toggle.
- Search category:
  - search-hidden row toggles `search_includes_hidden`, persists settings, and
    re-runs search filtering.
- System category:
  - reset-cache row clears icon cache, clears atlas slots, resets icon states,
    re-queues extraction, and requests redraw;
  - reset-settings row restores default settings, clears manual order and
    hidden apps, persists all three stores, relayouts, and requests redraw.
- About category shows version information and has no interactive row.
- The panel is centered in the physical-pixel viewport.
- Geometry scales by the window scale factor.
- The visual panel animates with alpha and pop scale. Hit testing uses the
  unscaled panel geometry, matching the current implementation.
- Settings text uses the current centered-line text path and `Yu Gothic UI`.
- The renderer still receives existing settings glass, control instances, and
  glyph quads until the renderer facade phase.

Current Phase 1 boundary:

- `layout/settings_panel.rs` owns settings panel geometry, hit classification,
  animation helper values, text placement, settings `LayoutResult`, and
  deterministic tests.
- `main.rs` converts current domain settings enums into layout IDs and adapts
  the layout result back into the existing renderer upload path.
- Text strings and GPU-specific `ControlInstance`/`GlyphQuad` construction
  still pass through `main.rs` as adapter inputs/outputs to avoid changing
  visible rendering in this slice.

Screen verification required for this slice:

- Launcher opens and first frame is non-blank.
- Settings opens from edit-mode gear.
- Settings closes from close button.
- Settings closes from outside modal click without click passthrough.
- Category switching works.
- Sort segment works.
- Frequent-apps toggle works.
- Search-hidden toggle works.
- Reset-cache and reset-settings rows remain clickable.

## Bottom Control and Search Field

Source before extraction:

- `src/main.rs` (`handle_control_click`, press/release hit-test branches,
  `render_bottom_control`, `render_gear`, `self_layout_control_text`,
  `update_ime_state`, `control_caret_screen_x`, `frame_control_cy`,
  `resolve_control`, keyboard/IME routing, `search_input_changed` and the
  filter fns)
- `src/bottom_control.rs` (state machine `BottomControl`, capsule geometry
  `resolve_scaled_with_edit_width`, capsule hit-test `hit_test_scaled`,
  close-button X `close_button_x_scaled`, edit gear geometry/hit
  `edit_gear_geometry`/`edit_gear_hit`, procedural overlay builder
  `build_overlay_instances`, `ControlInstance` + KIND constants)
- `src/text.rs` centered line text renderer (query/preedit/placeholder/Done)

Current behavior to preserve:

- The control is a single morphing capsule centered horizontally at the bottom
  of the window, sitting a fixed margin below the fixed page frame bottom edge,
  clamped into the viewport.
- `Mode::Pill` is the default: a compact "🔍 検索" Liquid Glass pill. A click on
  the pill opens the search field (`open_search` → `Expanding`).
- `Mode::Indicator` is transient: shown for `INDICATOR_HOLD` (1.8s) after a page
  change, then retires back to `Pill`. A page change is ignored while the field
  is open or opening (`Field`/`Expanding`) so focus is not yanked.
- `Mode::Expanding`/`Mode::Collapsing` animate the capsule half-width between
  the compact pill and `FIELD_HALF_WIDTH` over `EXPAND_DURATION`/`COLLAPSE_DURATION`.
  Content layers cross-fade on top (`SearchPill` ↔ `SearchField`).
- `Mode::Field` holds the capsule fully open with the caret blinking.
- Close button (×): visible only when `mode` is `Field`/`Expanding`/`Collapsing`
  and `expand >= 0.5`. Its hit region is an axis-aligned square of half-size
  `12.0 * scale.max(1.0)` centered at `(close_button_x, capsule_center_y)` where
  `close_button_x = capsule_center_x + half_width - 20.0 * scale`. A click on it
  calls `press_close` (clears query/caret/preedit, then collapses).
- Edit mode swaps the capsule contents to a "完了" (Done) label whose width
  morphs via `edit_control_progress`, and adds a settings gear capsule to its
  right. The gear is visible only when `editing`, `edit_visual_progress > 0`,
  and the settings overlay is not active.
- Gear hit is a true circle of radius `CAPSULE_HEIGHT * scale * 0.5`; a click on
  it opens settings. A click on the Done capsule exits edit mode.
- Pointer press records `pressed_on_control` when the press starts on the
  capsule or (in edit mode) the gear; a scroll drag must not begin in that case.
  Release re-tests the capsule and dispatches the click only if still on the
  capsule. The release re-hit-tests the main capsule shape only; the gear is
  re-resolved inside `handle_control_click`.
- Keyboard input is routed to the control only while `wants_keyboard()`
  (`Field`/`Expanding`/`Collapsing`):
  - Esc while the field is open calls `press_close` (clears query + collapses)
    rather than hiding the launcher; Esc with the field closed hides the
    launcher.
  - Backspace deletes one unicode scalar before the caret, but only when
    `preedit` is empty (the OS IME owns backspace inside a composition).
  - Left/Right move the caret one char, only when `preedit` is empty.
  - Printable chars are appended at the caret; direct char input is blocked
    while `preedit` is non-empty.
  - Enter is not handled by the search field (no keyboard launch).
  - Edit-mode Esc and settings-open Esc take precedence over the search field.
- IME: enabled while `wants_keyboard()`, with the composition window parked at
  the caret `(field_text_origin_x + query_width, capsule_center_y)`.
  `Ime::Preedit` stores the composition string; `Ime::Commit` clears preedit
  then appends committed chars; `Ime::Disabled` drops preedit. Each mutation
  calls `search_input_changed()`.
- Search filtering uses `visible_search_query()` = committed `query` +
  in-flight `preedit` (so Japanese composition narrows the grid live).
  `matches_search` is case-insensitive substring, AND across whitespace-split
  words, with the query trimmed; an empty query matches everything. Hidden apps
  surface only when `search_includes_hidden` is true and the trimmed query is
  non-empty.
- `search_input_changed` relayouts, re-snaps scroll to the nearest page, resets
  scroll velocity/phase, updates `last_page`, and requests redraw.
- The caret X is computed each frame from the cosmic-text-measured width of
  `query + preedit` (cached during `render_bottom_control`). The caret blinks
  on for the first 0.6 of a ~1.06s cycle while in `Field` mode.
- The capsule hit shape ignores the edit Done-width morph
  (`hit_test_scaled` uses `resolve_scaled`, not `resolve_scaled_with_edit_width`).

Current Phase 2 boundary (target):

- `layout/bottom_control.rs` owns bottom-control/search/gear hit regions and
  the layout rects (capsule geometry, layers, gear geometry, close-button X)
  produced from one layout pass, plus a pointer-intent helper.
- `main.rs` adapts the layout result into the existing renderer upload path and
  routes pointer press/release/click through the narrow intent boundary.
- The state machine, IME, caret blink, page indicator timing, search matching,
  and `ControlInstance`/`build_overlay_instances` generation remain in
  `src/bottom_control.rs` / `src/main.rs` unchanged.

Screen verification required for this slice:

- Launcher opens and first frame is non-blank.
- Search opens from a pill click and closes from the × button and from Esc.
- Search text entry (ASCII and Japanese) updates the field and filters the grid.
- IME commit and preedit behave as before (if observable).
- Page indicator appears transiently after a page change and retires to the pill.
- Edit-mode Done capsule and settings gear hit behavior is unchanged.
- Resize / DPI-sensitive layout keeps the capsule geometry correct.

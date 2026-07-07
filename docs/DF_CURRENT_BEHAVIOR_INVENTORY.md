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

## Launcher Grid and Click Passthrough

Source before extraction:

- `src/main.rs` (`relayout`, `begin_grid_press`, `pending_press_*` helpers,
  `pointer_over_page_glass`, `app_index_at_pointer`, `edit_drop_index_at_pointer`,
  `badge_hit`, `maybe_promote_press_to_drag`, `maybe_long_press_into_edit`,
  `resolve_clicked_app`, `handle_pointer_release`, `hide_with_click_passthrough`,
  `grid_apps_owned`, `visible_app_ids`, `matches_search`, the `MouseInput`
  press/release routing, edge autoscroll zone math)
- `src/grid.rs` (`GridLayout`, `GridApp`, `TileAnim`, `TileInstance`, the
  `FRAME_*` / `BASE_TILE_SIZE` / `LABEL_CLICK_EXTRA_*` constants,
  `frame_panel_rect`, `frame_contains_point`, `hit_test_app`,
  `hit_test_tile_cell`, `tile_position`, `build_instances`,
  `build_icon_instances`, `build_labels`, `edit_badge_radius_for_tile_size`)
- `src/renderer.rs` (`rebuild_instances`, `frame_clip`,
  `edit_badge_sources`/`animated_badge_center`)
- `src/liquid_glass/geometry.rs` + `src/liquid_glass/renderer.rs` (page-frame
  glass shape build from `GridLayout` + `GridApp`)
- `src/scroll.rs` (`ScrollBounds` — page extent + snap/paging/rubber-band
  physics)

Current behavior to preserve:

- **Page frame geometry and clipping.** The fixed Liquid Glass page-frame panel
  is the single source of truth for the rounded-rect clip applied to tiles,
  icons, and labels. `frame_panel_rect` returns `(center_x, center_y, panel_w,
  panel_h)` where `panel_w == page_width(viewport_w)` and `panel_h = grid_h() +
  scaled(FRAME_PADDING_HEIGHT)`. The corner radius is `scaled(FRAME_CORNER_RADIUS)`
  clamped to `min(half_w, half_h)`.
- **Page width, scroll bounds, resize, DPI scaling.** `page_width(viewport_w)`
  is `grid_w() + scaled(FRAME_PADDING_WIDTH)` clamped to
  `[grid_w(), viewport_w - scaled(FRAME_VIEWPORT_GUTTER)]`, so the page can be
  narrower than the viewport (pages slide adjacent with a gutter) but never
  narrower than the grid. `GridLayout::bounds(viewport_w)` produces
  `ScrollBounds { page_extent: page_width(viewport_w), page_count }`.
  `with_scale_factor` scales tile_size/gap/row_gap/margin_top/margin_left
  proportionally (ratio-based, not accumulating).
- **Tile / icon / label / placeholder visual geometry.** Each cell sits at
  `x = page * page_w + margin_left + col * (tile_size + gap)`,
  `y = margin_top + row * (tile_size + row_gap)`. Labels sit below their tile,
  centered, with `max_width = tile_size + scaled(20.0)` and
  `y = tile_y + tile_size + scaled(8.0)`. Tiles without an icon UV get a stable
  per-index HSL color and `icon_index = -1.0` (the shader renders the color
  fallback). The icon instance list only carries tiles whose app has a UV.
- **App launch hit regions.** `hit_test_app` returns the visible-stream index
  under a screen-space pointer. The clickable region **includes the label area**
  (`LABEL_CLICK_EXTRA_X` / `LABEL_CLICK_EXTRA_Y` slop) because this is an app
  launcher. The frame rounded-rect clip is applied first: a point outside
  `frame_contains_point` returns `None` even if it is geometrically over a tile.
- **Label area click behavior.** A click in the label slop band below a tile
  resolves to that tile's app (the slop widens the cell rectangle, not a
  separate target).
- **Gaps and empty slots.** Points between cells (gaps, inter-page gutters)
  return `None`. Empty slots past the last visible app return `None` for
  `hit_test_app` (app-count-bounded) but `Some` for `hit_test_tile_cell`
  (cell-count-bounded, used by edit-mode drop).
- **Press-time stable `AppId` launch.** On press, `PendingPress` records the
  app id under the press (`app_index_at_pointer` → `visible_app_ids()[idx]`).
  On a stationary release, `pending_press_launch_id` returns that press-time id,
  not whatever moved under the release point. Launch resolves through
  `registry.launch_info(id)` which clones `AppLaunchInfo` before dismiss, so a
  concurrent rescan cannot launch the wrong app. The launcher hides first, then
  opens the shortcut.
- **Press target drift does not launch a different app.** Because launch uses
  the press-time `app_id`, releasing over a different app (or empty space, as
  long as the gesture stayed within slop) still launches the originally pressed
  app.
- **Drag beyond slop becomes scroll, not launch.** A pending press that moves
  past `CLICK_SLOP_PHYS` is promoted to a scroll drag
  (`maybe_promote_press_to_drag` → `handle_drag_start`). The scroller owns
  drag/inertia/snap/rubber-band from there. A press that turned into a drag
  cannot launch on release.
- **Transparent-area stationary click → hide + click passthrough.** A press that
  starts outside the page-frame glass (`outside_glass = !frame_contains_point`)
  and releases within slop calls `hide_with_click_passthrough`, which hides the
  launcher and then `platform_windows::replay_left_click_at_cursor()` so the
  click reaches the underlying window.
- **Page-frame empty click does NOT passthrough.** A press that starts *inside*
  the frame but over empty space (gap, past the last app, page gutter) has
  `outside_glass = false`. A stationary release there neither launches an app
  nor triggers passthrough — it is swallowed by the launcher.
- **Pointer precedence: settings > bottom control > grid.** `MouseInput`
  press/release routing checks the settings overlay first, then the bottom
  control (`pressed_on_control`), then the grid (`begin_grid_press` /
  `pending_press`). A press on the bottom control never becomes a scroll drag.
- **Hidden apps / search results / icon placeholder effect on grid hit.**
  `visible_app_ids()` filters `registry.apps()` by
  `(search_includes_hidden && query non-empty) || !is_hidden(id)` and then by
  `matches_search(name, query)` (case-insensitive AND of whitespace-split
  substrings; empty query matches all). The hit-test `app_count` is
  `visible_ids.len()`, so hidden apps and filtered-out apps are unreachable, and
  pages shrink to the filtered count. Apps whose icon is still loading stay
  launchable (the color placeholder is shown and the hit region is unchanged).

Current Phase 3 boundary (target):

- `layout/grid.rs` owns the pure grid geometry and hit classification:
  `GridLayout`, the `FRAME_*` / `BASE_TILE_SIZE` constants, `frame_panel_rect`,
  `frame_contains_point`, `hit_test_app`, `hit_test_tile_cell`, `tile_position`,
  `page_extent`, and a `GridHit` classifier that distinguishes app / empty-in-frame
  / outside-frame in one calculation.
- `src/grid.rs` stays as a binary adapter: it re-exports `GridLayout`, provides
  the `ScrollBounds`-returning `bounds()` adapter, and keeps the GPU-facing
  `TileInstance` / `GridApp` / `TileAnim` and the `build_instances` /
  `build_icon_instances` / `build_labels` instance builders.
- `main.rs` routes press/release through the `layout::grid` classifier for
  `outside_glass` / `app_index` (used to build `PendingPress`), while preserving
  the press-time `AppId`, slop, launch, and passthrough behavior exactly.
- The scroller physics (`scroll.rs`), search filtering, bottom control, and
  settings overlay remain unchanged.

Screen verification required for this slice:

- Launcher opens and first frame is non-blank.
- Page frame / tiles / icons / labels / placeholders render correctly.
- An app launch hit target launches the expected app (for a safe target).
- Transparent-area stationary click hides the launcher and passes the click
  through to the underlying window.
- A click inside the page frame but on empty space does NOT passthrough.
- Horizontal drag / inertia / snap / rubber-band behave as before.
- A drag beyond slop does not launch.
- Resize / DPI-sensitive layout keeps grid geometry correct.
- Search filtering keeps grid hit targets aligned with the filtered set.
- Settings overlay and bottom control pointer precedence is unchanged.

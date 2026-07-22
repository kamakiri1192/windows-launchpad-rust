# DF Current Behavior Inventory

Status: migration support document.

This document records current user-visible behavior that rearchitecture slices
must preserve. It is intentionally concrete. Each vertical slice should update
or extend this inventory before moving behavior behind a new boundary.

## Folder Feature

Source after the Phase 8 vertical slice:

- `src/domain/launcher_state.rs` owns item order, folder membership, moves, and
  automatic folder dissolution.
- `src/features/folders/mod.rs` owns open/close motion, hover intent, rename,
  and child-drag preview state.
- `src/layout/folder_panel.rs` owns the dynamic panel, pagination, child
  trajectories, and matching hit regions.
- `src/app/render/folders.rs` adapts domain records to generic UI primitives.

Current behavior to preserve:

- The normal grid interleaves app and folder items in persisted
  `LauncherState.items` order. Search stays a flat app-only result set.
- A closed folder uses only its Liquid Glass container and shows at most the
  first nine visible child icons in ordered 3x3 mini slots. It does not render
  the opaque colored fallback used behind a normal app icon. Hidden or
  currently undiscovered children are not rendered. The folder container uses
  a nested grid-glass compositing lane so its rounded boundary remains visible
  inside the larger page-frame glass, with mini icons drawn above it. Its outer
  bounds and corner radius match a normal app tile; it does not use the larger
  tile-halo dimensions.
- Clicking a folder opens a centered Liquid Glass modal. The panel size is
  derived from the visible child count, incomplete rows are centered, and more
  than nine children are split into pages with a page indicator.
- Opening morphs the current folder tile rectangle into the panel while child
  icons travel from their mini slots to panel cells. Closing reverses the same
  spring without restarting, and resolves the source rectangle from the latest
  grid layout on every frame. The morph also receives the current closed-folder
  corner radius, so its final frame exactly matches the restored grid glass.
  During the closed-end portion of the morph, each child's colored tile fill
  collapses into its own center while the icon continues to its mini slot. The
  fill is already gone before the closed-folder preview takes over.
- While a folder is active, its source tile's grid-preview mini icons are
  suppressed because the modal lane owns the moving child icons. They return
  only after the close morph reaches its endpoint.
- Opening a folder fades in the **Glass Focus Veil**. Before the modal is drawn,
  the completed lower scene (page glass, app fills, icons, closed folders, and
  labels) is rendered to an intermediate texture and passed through a
  three-level Dual-Kawase blur. The blurred scene plus a restrained cool-neutral
  tint is recomposited only inside the fixed page-frame rounded rectangle. A
  12 px inner transition preserves the crisp Liquid Glass rim and prevents the
  transparent window surround from bleeding into it; the surround itself is
  unaffected. The folder panel glass and its children are drawn afterward and
  remain sharp.
- The modal owns input while visible. Clicking the modal backdrop closes it
  without replaying the click underneath. `Esc` first cancels an active rename;
  otherwise it closes the folder.
- Clicking the title starts rename. IME preedit is shown without committing,
  Enter commits, and blank or whitespace-only names become `フォルダ`. Cursor
  movement and deletion use UTF-8 character boundaries.
- Child labels and the panel title are truncated at character boundaries with
  an ellipsis when they do not fit.
- Dragging one top-level app over another past the hover threshold previews the
  merge with two Liquid Glass overlays. The new folder and its ordered children
  (`target`, then `dragged`) are committed only on drop.
- Hovering a dragged top-level app over an existing folder past the same
  threshold spring-opens it. Dropping then moves the app into that folder.
- Folder children can be reordered with a live preview, moved to another
  folder, or dragged back to the top-level grid.
- A folder with one child after a move or hide is dissolved in place and the
  remaining child is promoted. An empty folder is removed.
- Opening settings, resetting settings, hiding the launcher, or losing the
  referenced folder clears folder-modal state so stale ids cannot receive
  input.
- Folder open/close redraws only while its spring is moving. A fully open idle
  panel does not create a continuous redraw loop.
- Renderer submission remains semantic-free: modal surfaces, icons, text, and
  backdrop are generic render-model lanes; renderer modules do not import
  `LauncherItem`, `Folder`, `FolderId`, or `LauncherState`.

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
- The panel's open/close progress drives the same Glass Focus Veil used by the
  folder modal, clipped to the fixed page-frame glass.
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

## Edit Mode (iOS-style long-press / drag-to-reorder / hide)

Source before extraction:

- `src/main.rs` (`begin_grid_press`, `maybe_long_press_into_edit`,
  `maybe_promote_press_to_drag`, `enter_edit_mode`, `exit_edit_mode`,
  `handle_edit_drag_move`, `live_reorder`, `maybe_autoscroll_edit_drag`,
  `commit_reorder`, `reorder_by_index`, `hide_app`, `edit_drop_index_at_pointer`,
  `badge_hit`, `edit_anim`, `lift_dragged_instances`, `update_tile_springs`,
  `apply_spring_positions`, `step_tile_springs`, `refresh_spring_instances`,
  `step_edit_control_width`, `edit_visual_progress`, the `MouseInput` /
  `CursorMoved` / `CursorLeft` / `Focused` edit-mode branches, the `LONG_PRESS_THRESHOLD`
  / `CLICK_SLOP_PHYS` / `EDIT_EDGE_SCROLL_ZONE` constants, the `PendingPress` type)
- `src/grid.rs` / `src/layout/grid.rs` (`GridLayout::hit_test_tile_cell`,
  `GridLayout::tile_position`, `GridLayout::edit_badge_radius`,
  `GridLayout::edit_badge_hit_slop`, `edit_badge_radius_for_tile_size`,
  `TileAnim` with `FLAG_WIGGLE` / `FLAG_DRAG`)
- `src/renderer.rs` (badge source geometry, `edit_badge_sources` /
  `animated_badge_center`, `DrawArgs.drag_active` / `drag_pos` /
  `time` used by the shader for wiggle)
- `src/bottom_control.rs` / `src/layout/bottom_control.rs` (edit-mode Done
  capsule + settings gear, `BottomControlPointerIntent::EditGear`)
- `src/app_registry.rs` (`set_order`, `hide`, `is_hidden`, `hidden()`, `order()`)
- `src/settings.rs` (`SortOrder::Manual`)

Current behavior to preserve:

- **Long-press entry.** A grid press that starts *inside* the page-frame Liquid
  Glass (`outside_glass = false`) and is held for `LONG_PRESS_THRESHOLD`
  (`Duration::from_millis(500)`) without moving past `CLICK_SLOP_PHYS` (`8.0` px)
  enters edit mode. A press that starts outside the page glass never long-
  presses into edit mode (it is the click-passthrough path instead).
- **Pending press and gesture resolution.** While a press is pending
  (`PendingPress = Some`), the scroller stays `Idle` — we do not start a scroll
  drag until the gesture reveals its intent. Resolution order:
  1. pointer moves past `CLICK_SLOP_PHYS` → promote to a scroll drag
     (`maybe_promote_press_to_drag` → `handle_drag_start`); the press is dropped
     and can no longer long-press or launch;
  2. quick release within slop over a visible app → click → launch the
     press-time `AppId` (`pending_press_launch_id`);
  3. quick release within slop over `outside_glass` → hide + click passthrough
     (`pending_press_is_outside_glass_click` → `hide_with_click_passthrough`);
  4. held for `LONG_PRESS_THRESHOLD` without moving past slop → enter edit mode.
- **Edit entry side effects.** On `enter_edit_mode`:
  - `editing = true` (idempotent — re-entry while already editing only logs on
    the first transition);
  - `pending_press = None` (the long-press press is consumed);
  - `wiggle_phase = 0.0`;
  - any in-flight scroll is cancelled (`phase = Idle`, `velocity = 0.0`) so the
    page sits still while editing;
  - the long-pressed app (if the press was over one) is lifted straight into a
    drag: `drag_app = visible_ids[app_index]`, `drag_x`/`drag_y` = current
    pointer;
  - relayout + redraw requested.
- **Icon wiggle / dragged icon visuals.** While `editing`, every visible app
  gets a `TileAnim` with `FLAG_WIGGLE` and a per-app phase offset
  (`wiggle_phase + i * 0.37`) so icons wobble out of sync. The dragged app (if
  any) additionally gets `FLAG_DRAG`, `lift = 24.0 * scale.max(1.0)`,
  `scale = 1.15`. The shader uses `FLAG_DRAG` to bypass the page-frame clip and
  to follow `drag_pos` instead of the tile's home cell. The dragged tile/icon
  instance is moved to the end of the GPU instance lists so it draws on top.
- **Edit badge hide (✕).** Each tile shows a delete badge at its top-left
  corner (rendered by the shader at radius `edit_badge_radius_for_tile_size`,
  ~13% of tile size clamped to `[9*scale, 13.5*scale]`). The hit region is a
  circle of radius `edit_badge_radius + edit_badge_hit_slop` (`6.0 * scale`),
  centered at `(tile_x + scroll_x + radius*0.45, tile_y + radius*0.45)`. The
  badge hit takes **precedence over a drag** at edit-mode press time: if the
  pointer is over a tile's badge, `hide_app` runs and the press never becomes a
  drag.
- **`hide_app` (✕ action).** Hides the app from the visible stream:
  `registry.hide(id)`; moves the id to the tail of the user order (so it does
  not linger invisibly mid-grid); `set_order` (relayout) ; `persist_hidden`;
  `persist_user_order`; drop any in-flight drag of that id; relayout + redraw.
  No-op if already hidden.
- **Edit-mode press behavior.** When `editing` and a left press lands on the
  grid (settings/capsule precedence already passed):
  - over a visible app and its ✕ badge → `hide_app`;
  - over a visible app but not its badge → start a drag (`drag_app = id`,
    `drag_x`/`drag_y` = pointer; relayout + redraw);
  - over empty space inside the frame → `exit_edit_mode` (empty-click exit).
  No pending press is recorded while editing (the press does not turn into a
  scroll drag or a launch).
- **Edit-mode release behavior.** When `editing` and `drag_app.is_some()` on
  release: `commit_reorder` (drop at the current cell + persist), then
  `drag_app = None`, relayout + redraw.
- **CursorLeft while editing.** If a drag is in flight when the pointer leaves
  the window, it is finalized in place (`commit_reorder`, `drag_app = None`,
  relayout). A pending long-press is cancelled (`pending_press = None`). A
  scroller drag is ended (`handle_drag_end`).
- **Exit paths.** Edit mode is exited (committing any in-flight drag first)
  via: Esc key (takes precedence over the search field's Esc), the Done capsule
  click (`handle_control_click` edit branch → `exit_edit_mode`), an empty-space
  click inside the frame, opening settings, or `hide()`. While `editing`, a
  `Focused(false)` event does **not** auto-hide the launcher (so clicking
  outside to dismiss edit mode does not race a focus-loss vanish); the settings
  overlay gets the same treatment.
- **Settings gear in edit mode.** Clicking the edit settings gear (intent
  `EditGear`, from `layout::bottom_control`) opens settings, which first calls
  `exit_edit_mode` and closes the search field. The Done capsule body (any
  non-gear capsule hit) exits edit mode.
- **Live reorder.** While a drag is in flight, each pointer move calls
  `live_reorder`: resolve the tile cell under `drag_x`/`drag_y` via
  `edit_drop_index_at_pointer` (`hit_test_tile_cell` with `total_tiles`, label
  area **excluded**), compute `insert_idx = target_idx.min(visible.len())`, and
  if it differs from the dragged app's current visible position, `reorder_by_index`
  moves the app there. Reorder is keyed by stable `AppId`, not positional index.
- **Empty-cell drop.** `edit_drop_index_at_pointer` uses `hit_test_tile_cell`
  (cell-count-bounded by `total_tiles`), so the empty slot immediately after
  the last visible app on the current page is a valid drop target. App-hit
  resolution (`hit_test_app`) is app-count-bounded and would return `None`
  there, which is why drop uses the cell variant.
- **Rightmost columns.** Pages are spaced by the (narrower) content page width
  while the grid is centered in the viewport. The drop hit-test mirrors the
  exact tile-placement formula (`page * page_w + margin_left + col * step_x`),
  so the rightmost one or two columns are reachable and are not misclassified
  as the next page. (Regression: see `tile_cell_hit_test_allows_rightmost_columns`.)
- **Label area is NOT a drop target.** `hit_test_tile_cell` is called with
  `include_label = false`, so a drop in the label band below a tile does not
  reorder. App *launch* uses `include_label = true` (label slop widens the
  clickable cell); edit drop does not.
- **Edge autoscroll.** While dragging, holding the lifted icon near a page-frame
  edge starts a one-page settle toward that edge
  (`maybe_autoscroll_edit_drag`):
  - only when `editing && drag_app.is_some()`;
  - only when `drag_y` is within the page panel's vertical span (otherwise the
    icon is above/below the grid and autoscroll is suppressed);
  - the configured zone is `scaled(EDIT_EDGE_SCROLL_ZONE = 72.0)`, clamped to
    `panel_w * 0.25` and floored at `24.0`;
  - the *actual* left/right zones are further clamped to the gutter between the
    panel edge and the grid edge
    (`left_zone = zone.min((grid_left - panel_left).max(0))`,
    `right_zone = zone.min((panel_right - grid_right).max(0))`), so the
    rightmost tile columns stay reachable as drop targets while the icon is
    held in the gutter;
  - only fires when the scroller is `Idle` (it does not interrupt an existing
    drag/settle);
  - target = `current_page - 1` (left, if `current > 0`) or
    `current_page + 1` (right, if `current + 1 < page_count`); resolved by
    `scroller.settle_to_page`.
- **Hidden apps and order.** Hidden apps are kept in the registry (a rescan
  does not resurrect them) but excluded from the visible stream
  (`visible_app_ids`). On reorder, the registry order is recomputed over the
  concatenated visible-stream-then-hidden list and `drag_id` is moved to
  `insert_idx` (clamped to `visible.len()` by `live_reorder`), so a drop at the
  tail of the visible stream lands the dragged app at the join with the hidden
  block. The *visible* result is always the user-intended arrangement because the
  registry filters hidden apps out of the visible stream; the hidden apps keep
  their relative order. `commit_reorder` sets `settings.sort_order =
  SortOrder::Manual` and persists both settings and user order.
- **Persistence across restart.** `persist_user_order` writes the registry's
  `order()` (binary `count:u32` + repeated `len:u32 + utf-8 id`); `persist_hidden`
  writes the hidden id list in the same format. On startup `load_customization`
  loads these into the registry before the first scan so apps appear in the
  user's arrangement from the first frame. Pending persisted ids that have not
  yet been inserted by the scan are kept pending and applied as matching apps
  arrive (`set_order_preserves_ids_not_yet_inserted`).
- **Tile springs / slide animation.** Per-visible-app position springs keyed by
  `AppId` (`tile_springs: Vec<(AppId, Spring2)>`) follow an app across reorder
  operations: the spring keeps its previous cell as the current value and glides
  to the new home cell, producing the slide-in animation. `relayout`
  rebuilds/pushes tile+icon instances with spring positions applied;
  `step_tile_springs` advances them each frame and keeps redrawing while any are
  animating. This is independent of the wiggle animation (which advances
  `wiggle_phase`).

Current Phase 4 boundary (target):

- `layout/edit_mode.rs` owns edit-mode pure geometry and hit regions produced
  from the same calculations as rendering: the edit badge center/radius/slop,
  the badge hit test, the empty-cell drop hit (`hit_test_tile_cell` wrapper
  excluding labels), the edge-autoscroll zone with its gutter clamp, and the
  edge-autoscroll target decision. It reuses Phase 2's
  `layout::bottom_control` boundary for the edit-mode Done capsule + settings
  gear (no duplicate geometry). It compiles as part of the library target so it
  can be unit-tested without `wgpu`/`winit`/`ScrollBounds`.
- `features/edit_mode/` owns edit-mode state transitions, intent classification
  (long-press entry, edit-press classify, edit-release outcome), the reorder
  order computation, and a narrow `EditModeCommand` set for side effects. It
  does not execute side effects directly — the app boundary (`main.rs`) runs
  registry mutation, persistence, scroller mutation, and redraw.
- `main.rs` stays as an adapter: it still owns `editing` / `drag_app` /
  `drag_x` / `drag_y` / `wiggle_phase` (read directly by the renderer/scroller),
  `PendingPress` (also drives launch/passthrough/scroll-drag, migrated to the
  app shell in Phase 5), and the GPU-facing animation/instance work.
- GPU-facing adapters left in place intentionally (Phase 6+): `TileAnim`,
  `TileInstance`, `IconInstance`, `edit_anim`, `lift_dragged_instances`,
  `tile_springs`, `step_tile_springs`, `refresh_spring_instances`,
  `step_edit_control_width`, `edit_visual_progress`, and the renderer badge
  source geometry.

Screen verification required for this slice:

- Launcher opens and first frame is non-blank.
- Long-press an icon → edit mode entered; icons wiggle and badges appear.
- Dragged icon lifts, scales, and follows the pointer; draws on top.
- Drag reorder on the current page works live.
- Drag to an empty cell on the current page works.
- Drag/drop on the rightmost two columns works.
- Edge autoscroll (holding the dragged icon near a page edge) scrolls a page.
- Done capsule exits edit mode.
- Esc exits edit mode.
- Empty-space click inside the frame exits edit mode.
- Settings gear opens settings and exits edit mode.
- Delete badge hides the app (and later apps close the gap).
- Reorder persists across a process restart.
- Hidden app persists across a process restart.
- Search / bottom control / settings / click passthrough smoke check (pointer
  routing precedence unchanged).

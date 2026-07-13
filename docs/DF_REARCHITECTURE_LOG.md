# DF Rearchitecture Log

Status: ongoing implementation record.

This document records what actually changed during the Dynamic Feature-ready
rearchitecture work. It is intentionally append-only in spirit: future slices
should add new dated entries instead of rewriting the plan document for every
small discovery.

Use this log for:

- completed migration slices;
- behavior-preservation notes;
- validation performed;
- implementation decisions that are too detailed for the target architecture;
- discoveries about the current codebase that should guide later slices.

The migration plan remains in
[DF_REARCHITECTURE_PLAN.md](DF_REARCHITECTURE_PLAN.md). The target architecture
remains in [../ARCHITECTURE.md](../ARCHITECTURE.md).

## How To Update This Log

When completing a rearchitecture slice, add a new entry with:

- the date;
- the phase and slice name;
- files changed;
- what changed;
- what was intentionally left disconnected or unchanged;
- validation results;
- follow-up notes or discoveries.

For UI-affecting slices, include the Screen Verification Gate result from
[DF_REARCHITECTURE_PLAN.md](DF_REARCHITECTURE_PLAN.md#screen-verification-gate).
For non-UI slices, explicitly state why screen verification was not required.

## Entries

### 2026-07-06: Phase 1 Slice 1, `ui_model::geometry`

Files changed:

- `src/lib.rs`
- `src/ui_model/mod.rs`
- `src/ui_model/geometry.rs`

What changed:

- Added the first `ui_model` module without connecting it to existing
  production code.
- Defined renderer-neutral geometry value types:
  - `Point { x, y }`
  - `Size { width, height }`
  - `Rect { x, y, width, height }`
  - `Insets { top, right, bottom, left }`
- Added `Rect::contains`, `Rect::center`, and `Rect::inset`.
- Added unit tests for containment, center calculation, inward inset, and
  negative inset expansion.

Behavior preservation:

- No existing app, layout, renderer, input, or UI code was connected to these
  new types.
- Current runtime behavior is unchanged.

Validation:

- `cargo fmt`: passed
- `cargo test`: passed
- Screen Verification Gate: not required because this slice does not touch UI
  behavior, rendering, layout, input, or app runtime wiring.

Notes and discoveries:

- The current app mostly treats layout geometry as physical pixels. `winit`
  supplies a window `scale_factor`, `main.rs` stores it, and relayout paths pass
  it into layout builders such as `GridLayout::with_scale_factor`.
- `GridLayout` scales design constants like tile size, gaps, margins, frame
  padding, and radii into physical pixels before renderer upload.
- Text layout also receives the window scale factor and converts
  `cosmic-text` logical measurements into physical-pixel glyph quads.
- Shaders consume physical-pixel positions and convert them to clip space using
  the physical viewport.
- For now, `ui_model::geometry` should stay unit-neutral and simple. Later
  layout code should decide whether a `Rect` is in physical pixels and keep
  render geometry and hit regions in the same coordinate space.
- If DPI mistakes become common during later slices, consider adding explicit
  documentation or newtypes for logical versus physical pixels before wiring
  more UI model primitives through the app.

### 2026-07-06: Phase 1 Slice 2, `ui_model::ids`

Files changed:

- `src/ui_model/mod.rs`
- `src/ui_model/ids.rs`
- `docs/DF_REARCHITECTURE_LOG.md`

What changed:

- Added `ui_model::ids` as the renderer-neutral identity module for later
  HitMap and RenderModel slices.
- Defined `UiId` as a lightweight string-backed type:
  - `UiId(String)`
  - derives `Debug`, `Clone`, `PartialEq`, `Eq`, `PartialOrd`, `Ord`, and
    `Hash`
  - exposes `UiId::launcher_item` and `UiId::as_str`
- Kept raw string construction private so future production call sites can move
  through explicit UI-element constructors instead of spreading arbitrary
  string IDs.
- Added unit tests for equality, `BTreeSet` use, `HashSet` use, `as_str`, and
  the initial `launcher_item` constructor.

Behavior preservation:

- `UiId` is not connected to existing production code.
- The type intentionally does not depend on current or future domain concepts
  such as `AppId`, `LauncherItem`, or `FolderId`.
- Current runtime behavior is unchanged.

Validation:

- `cargo fmt`: passed
- `cargo test`: passed
- Screen Verification Gate: not required because this slice only adds an
  isolated UI model identity type and does not touch UI behavior, rendering,
  layout, input, or app runtime wiring.

Notes and discoveries:

- A string-backed `UiId` keeps Phase 1 independent from future domain modeling
  work. Later slices can add conversion helpers or typed constructors when the
  domain types exist and the dependency direction is clear.
- The first public constructor is intentionally concrete and limited to
  launcher items rather than a generic `new`, because the current UI already
  has launcher item visuals and later HitMap/RenderModel call sites should read
  as specific UI identities instead of unstructured string labels.

### 2026-07-06: Phase 1 Slice 3, `RenderModel` and `HitMap` foundations

Files changed:

- `src/lib.rs`
- `src/layout/mod.rs`
- `src/layout/hit_map.rs`
- `src/ui_model/mod.rs`
- `src/ui_model/hit.rs`
- `src/ui_model/render_model.rs`
- `src/ui_model/text.rs`
- `docs/DF_REARCHITECTURE_LOG.md`

What changed:

- Added `layout::LayoutResult` as the future boundary that carries both
  renderer-neutral drawing data and hit-test data from one layout pass.
- Added `layout::hit_map` with:
  - `HitRegion { id, rect, target, z }`
  - `HitMap`
  - deterministic `hit_test` ordering where the highest z wins and later
    same-z regions win.
- Added `ui_model::hit::HitTarget` for semantic pointer targets, including
  backdrop targets that distinguish launcher click passthrough from modal
  dismiss behavior.
- Audited current hit semantics in `main.rs` and added Phase 1 model coverage
  for the existing launcher cells, edit badges, bottom control, search field,
  edit settings gear, settings categories, settings sort options, settings
  toggles, settings actions, modal dismiss backdrops, and launcher click
  passthrough backdrops.
- Added `ui_model::render_model::RenderModel` and initial primitive view
  structs for glass, tiles, icons, text, and controls.
- Added `ui_model::text` with `TextView`, `TextStyle`, semantic `TextRole`,
  `TextMetrics`, and the `TextMeasurer` trait planned for layout tests.
- Added UI identity constructors for the currently interactive affordances so
  later layout builders do not have to invent ad hoc string IDs.
- Added unit tests for hit ordering, rect containment behavior through the hit
  map, same-z tie-breaking, push order, and empty render models.

Behavior preservation:

- The new layout and UI model types are not connected to `main.rs`, renderer
  uploads, current grid hit-testing, settings, bottom control, or input
  routing.
- No existing runtime code paths were changed.
- Current rendering and interaction behavior is unchanged.

Validation:

- `cargo fmt`: passed
- `cargo test`: passed
- `cargo clippy --all-targets --all-features`: passed
- Screen Verification Gate: not required because this slice only adds isolated
  model/layout foundation types and tests. It does not touch UI behavior,
  rendering, layout execution, input, app runtime wiring, shaders, or GPU
  resources.

Notes and discoveries:

- This keeps Phase 1 independent from the larger `main.rs` extraction. Wiring
  existing settings, bottom-control, or grid hit-testing into `LayoutResult`
  should be handled as a later focused slice because those paths affect
  pointer routing and visible UI behavior.
- `HitMap` intentionally stores regions in insertion order and uses that order
  as the same-z tie-breaker. This gives layout builders predictable layering
  without requiring every small overlay affordance to invent a unique z value.
- `TextStyle` carries a semantic `TextRole` instead of concrete font-family
  names. Later runtime wiring should map roles such as app labels, controls,
  settings rows, and folder labels to the existing text renderer's concrete
  font/fallback choices without making layout depend on `cosmic-text` details.
- Phase 1 should model the current behavior surface, not merely reserve empty
  extension points. This slice now covers the current UI affordances that are
  drawn or hit-tested by `main.rs`, `grid.rs`, and `bottom_control.rs` even
  though runtime event routing is intentionally left untouched.
- The existing transparent-area click behavior should map to
  `HitTarget::Backdrop { kind: LauncherPassthrough }` when runtime input is
  wired through `LayoutResult`. The eventual command side should preserve the
  current `hide_with_click_passthrough` behavior by hiding the launcher before
  replaying the left click through the Windows platform adapter. Modal
  backdrops, such as settings overlay outside clicks, should use
  `ModalDismiss` and must not replay the click to the underlying app.

### 2026-07-06: Migration plan rebuilt around vertical slices

Files changed:

- `docs/DF_REARCHITECTURE_PLAN.md`
- `docs/DF_REARCHITECTURE_LOG.md`

What changed:

- Replaced the old horizontal phase ordering with behavior-preserving vertical
  slices.
- Made current-behavior inventory an explicit prerequisite for each extraction
  slice.
- Moved the first real validation target to the settings overlay, because it
  has contained rendering, hit-testing, modal backdrop behavior, persistence
  commands, and screen-verifiable UI behavior.
- Reframed `AppAction` / `AppCommand` as something introduced narrowly inside
  vertical slices before being consolidated into a general app shell.
- Deferred renderer facade splitting until multiple real UI slices have proven
  the `RenderModel` shape.
- Moved folders later, after grid, edit-mode, action/command, and renderer
  boundaries have been validated against current behavior.

Behavior preservation:

- This entry changes planning documentation only.
- No Rust code, runtime wiring, rendering, input handling, shaders, or GPU
  resources were changed.

Validation:

- Cargo validation was not run for this documentation-only planning change.
- Screen Verification Gate: not required because this entry only revises the
  migration plan and does not affect runtime UI behavior.

Notes and discoveries:

- The previous plan put `ui_model` and `HitMap` ahead of a real UI slice, which
  made the model easy to detach from current behavior.
- Future slices should not treat unused model types as complete. A model is
  considered validated only after a current feature uses it end to end and
  passes the relevant screen checks.

### 2026-07-06: Phase 0/1, settings overlay vertical slice

Files changed:

- `docs/DF_CURRENT_BEHAVIOR_INVENTORY.md`
- `docs/DF_REARCHITECTURE_LOG.md`
- `Cargo.toml`
- `src/layout/mod.rs`
- `src/layout/settings_panel.rs`
- `src/main.rs`

What changed:

- Added a current-behavior inventory document and recorded the settings overlay
  behavior that this slice must preserve.
- Added `layout::settings_panel` as the first vertical-slice layout module.
- Moved settings panel geometry, text placement, hit classification, modal
  backdrop intent, animation alpha/pop helpers, and a settings `LayoutResult`
  builder into the layout layer.
- Connected `main.rs` settings hit-testing to `layout::settings_panel` instead
  of duplicating panel and row geometry locally.
- Connected `main.rs` settings render preparation to the settings layout model
  for panel geometry, text views, and animation values, then adapted the result
  back into the existing renderer upload path.
- Added `default-run = "launchpad-windows"` so the documented
  `cargo run --release` verification command runs the launcher binary in this
  multi-bin crate.
- Preserved the main-branch GPU fix for settings overlay redraws by keeping
  `settings_panel_active()` out of the steady-state redraw gates.
- Added deterministic tests for settings panel geometry, close/category/action
  hit targets, text view placement, modal backdrop z-order, and animation
  helper endpoints.

Behavior preservation:

- Existing settings state mutation and side effects remain in `main.rs`.
- Existing renderer methods, GPU instance structs, text glyph generation, and
  settings strings remain unchanged.
- Outside settings clicks are still modal dismiss clicks and are explicitly
  separate from launcher click passthrough.
- This slice intentionally keeps `ControlInstance` and `GlyphQuad` generation as
  adapter code in `main.rs` so visual rendering remains behavior-preserving
  while layout/hit/text placement moves behind the new boundary.

Validation:

- `cargo fmt`: passed
- `cargo test`: passed
- `cargo clippy --all-targets --all-features`: passed
- `cargo run --release`: passed with the documented screenshot environment
  (`LAUNCHPAD_ALLOW_SCREENSHOT=1`, `LAUNCHPAD_DEBUG=1`, and temporary
  `LOCALAPPDATA`).
- Screen Verification Gate:
  - first frame non-blank: verified in `target/qa-final-initial.png`;
  - settings overlay open: verified through tray Settings command in
    `target/qa-final-settings-open.png`;
  - settings category switching: verified for Apps, Search, System, and About
    with `WM_MOUSEMOVE` + click messages and captured screenshots;
  - outside modal click closes settings: verified in
    `target/qa-settings-closed-outside.png`;
  - sort/toggle/reset rows: hit intents are covered by deterministic tests and
    the rows are visible in screenshots, but automated coordinate injection was
    not stable enough to claim full visual click verification for every row.

Notes and discoveries:

- A complete vertical slice does not require finishing the renderer facade.
  It can adapt `LayoutResult` back into current renderer upload calls as long
  as the source geometry and hit regions come from the new layout boundary.
- Localized text strings remain provided by `main.rs`/`settings.rs`, while
  settings text placement now comes from `layout::settings_panel::TextView`
  output. A later text-focused slice can move string ownership if localization
  or dynamic copy requires it.
- The app is capture-excluded unless launched with
  `LAUNCHPAD_ALLOW_SCREENSHOT=1`; this is now documented in `AGENTS.md` via
  `docs/EDIT_MODE_VISUAL_QA.md`.

### 2026-07-07: Phase 2, bottom control and search vertical slice

Files changed:

- `docs/DF_CURRENT_BEHAVIOR_INVENTORY.md`
- `docs/DF_REARCHITECTURE_LOG.md`
- `src/layout/mod.rs`
- `src/layout/control_geometry.rs` (new)
- `src/layout/bottom_control.rs` (new)
- `src/bottom_control.rs`
- `src/main.rs`

What changed:

- Added a current-behavior inventory section for the bottom control and search
  field covering the search pill, page indicator, search field, close button,
  caret, IME preedit/commit, page-change indicator timing, search text entry,
  backspace/left/right/Esc, search filtering/empty-query, and edit-mode
  Done/settings gear.
- Added `layout::control_geometry` as the pure geometry layer for the morphing
  bottom-center control. It owns the renderer-neutral types (`Mode`, `Visual`,
  `ControlLayer`, `ControlGeometry`, `EditWidth`, `EditGearGeometry`,
  `ControlState`), the tunable constants, and the pure resolve/hit-test/gear
  helpers. This module compiles as part of the library target so the Phase 2
  layout layer can be unit-tested without the binary.
- Added `layout::bottom_control` as the Phase 2 layout boundary. It builds a
  `BottomControlInput` snapshot into a `BottomControlModel` that carries the
  capsule/gear/close geometry snapshot plus a `LayoutResult` (`HitMap`). A
  narrow `BottomControlPointerIntent` enum (`None`/`Capsule`/`CloseButton`/
  `EditGear`) classifies a pointer point through the hit map so `main.rs`
  dispatches clicks via intent instead of duplicating capsule/gear/close
  geometry inline.
- Reworked `src/bottom_control.rs` so the pure types/constants/functions are
  re-exported from `layout::control_geometry` while the state machine
  (`BottomControl`), the GPU-facing overlay builder
  (`ControlInstance`/`build_overlay_instances`), and the renderer-specific
  glass-shape helpers (`glass_shape`/`edit_gear_glass_shape`) remain in the
  binary module. Existing `bottom_control::*` call sites in `main.rs` keep
  working without path changes.
- Connected `main.rs` pointer routing to the new layout boundary:
  - press/release classification now goes through
    `bottom_control_intent`/`layout::bottom_control::hit_test` instead of the
    previous inline `control.hit_test_scaled` + `edit_gear_hit` + close-button
    square test;
  - `handle_control_click` dispatches by intent (`EditGear` → open settings,
    `CloseButton` → press_close, `Capsule` → open/close search or exit edit
    mode);
  - the release re-test keeps the previous behavior of only counting a click
    when the release stayed on the capsule body (not the gear), so a press that
    landed on the gear but drifted off the capsule is still dropped.

Adapters left in place (intentionally not migrated this slice):

- `render_bottom_control`, `render_gear`, `self_layout_control_text`,
  `update_ime_state`, `control_caret_screen_x`, `frame_control_cy`, and
  `resolve_control` remain in `main.rs` unchanged. They still call the existing
  `BottomControl` resolve/hit methods, which now delegate to the pure
  `layout::control_geometry` functions.
- The caret/preedit X positions depend on per-frame cosmic-text measurement of
  `query + preedit`, so the render model is not promoted to `RenderModel`
  primitives this slice. Layout produces the hit map and the geometry snapshot;
  `main.rs` adapts the geometry into the existing renderer upload path.
- `ControlInstance`, KIND constants, and `build_overlay_instances` stay in the
  binary `bottom_control` module because they are GPU-facing and also used by
  the settings overlay.

Behavior preservation:

- The `BottomControl` state machine, IME handling, caret blink, page indicator
  timing, search query/preedit handling, and search filtering are unchanged.
- `render_bottom_control`/`render_gear` still feed the exact same geometry into
  the renderer; the capsule, gear, and close-button X positions come from the
  same resolve/edit-gear/close-button calculations (now shared with the hit
  map).
- The hit capsule keeps the non-edit-width `resolve_scaled` shape (the previous
  `hit_test_scaled` behavior), so the hit region is the full pill width even in
  edit mode while the rendered Done capsule is narrower.
- The close-button hit region keeps the square shape
  (`12.0 * scale.max(1.0)` half-size) and the gear keeps its circle test.
- The release-path gear re-test behavior is preserved: a press on the gear
  whose release drifts off the capsule shape is dropped, matching the previous
  `hit_test_scaled`-only release re-test.
- Esc/Backspace/Left/Right/preedit/commit routing and the
  `search_input_changed` choke point are untouched.

Validation:

- `cargo fmt`: passed
- `cargo test`: 225 lib + 65 bin + 2 WGSL validation, all passed
- `cargo clippy --all-targets --all-features`: passed (no warnings)
- `cargo build --release`: passed
- `cargo run --release` with `LAUNCHPAD_ALLOW_SCREENSHOT=1`,
  `LAUNCHPAD_DEBUG=1`, and a temporary `LOCALAPPDATA`:
  - first frame non-blank: verified — center pixel (640,400) reads dark
    background, the bottom-control capsule reads the Liquid Glass tint
    (≈157,197,242 at the pill center), and a 10px-grid sample reports 3103
    unique colors, consistent with a fully painted launcher (tiles, icons,
    capsule);
  - search pill / bottom control drawn at bottom-center: verified in the same
    screenshot row scan.

Screen verification:

- Launched with `cargo run --release` (via release exe with the documented
  screenshot environment): yes
- First frame non-blank: yes (pixel-sampled; 3103 unique colors, Liquid Glass
  capsule tint present at the bottom-center)
- Search open/close (interactive click): not verified — the sandbox foreground
  lock refused `SetForegroundWindow` for the click automation, so an
  interactive click-then-capture cycle could not be completed
- Search text entry / IME commit / preedit: not verified — same foreground
  lock blocker; these paths are unchanged code, covered by deterministic
  tests
- Filtering: not verified on screen (unchanged code; covered by existing
  `matches_search` tests)
- Page indicator: not verified on screen (unchanged code)
- Edit mode Done / settings gear hit behavior: not verified on screen;
  deterministic tests cover the gear/close/Done intent classification and the
  edit-mode capsule-width preservation
- Resize / DPI-sensitive layout: not verified on screen; geometry scaling is
  covered by unit tests (`geometry_scales_with_dpi`,
  `close_region_scales_with_dpi`)

Notes and discoveries:

- The crate ships both a library and a binary target that share `src/layout/`.
  `layout::settings_panel` compiled standalone in Phase 1 because it had no
  binary-only dependencies; the bottom-control geometry originally lived in
  `src/bottom_control.rs` alongside `ControlInstance` (which references
  `wgpu`), so it could not be referenced from the library target as-is.
  Extracting the pure types/constants/functions into
  `layout::control_geometry` was necessary to let `layout::bottom_control`
  compile as part of the library and to keep `cargo test --lib` green.
- The caret X depends on a per-frame cosmic-text measurement of
  `query + preedit`, so it cannot move into the render model without either
  threading a `TextMeasurer` through layout or duplicating the measurement.
  This slice keeps the measurement in `main.rs` and only owns the hit map +
  geometry snapshot in layout, matching the Phase 1 "adapt `LayoutResult` back
  into the existing renderer" guidance.
- `edit_gear_glass_shape` and `glass_shape` build a renderer-specific
  `GlassShape` (binary `liquid_glass` module) and therefore stay in the binary
  `bottom_control` module. `layout::control_geometry` only owns numeric
  geometry.
- The release-path gear re-test behavior is subtle: the previous code only
  re-tested the main capsule shape on release, not the gear, so a press on the
  gear that drifted off the capsule was dropped. The new release re-test calls
  `bottom_control_capsule_hit` (the same non-edit-width capsule shape test the
  previous `hit_test_scaled` used) so a press on the capsule/gear overlap
  reaches `handle_control_click`, which then resolves the gear via the intent.
  A press that drifts off the capsule entirely is still dropped.

### 2026-07-07: Phase 2 codex review and pointer-dispatch fixes

Files changed:

- `src/main.rs`
- `src/layout/bottom_control.rs`
- `docs/DF_REARCHITECTURE_LOG.md`

What changed:

- Ran `codex review --base main` against PR #80. Two P2 correctness findings:
  1. The initial release gate only allowed `Capsule`/`CloseButton` intents
     through, which dropped a click on the edit-mode settings gear that sits on
     the capsule/gear overlap (gear left edge ≈688 < capsule right edge ≈699).
     Fixed by re-testing the capsule shape directly
     (`bottom_control_capsule_hit`, equivalent to the original
     `hit_test_scaled`) on release, so a press on the overlap reaches
     `handle_control_click`, which resolves the gear via the intent.
  2. When search was open and the user long-pressed into edit mode, the field's
     close-button hit region kept emitting even though its visual layers are
     hidden while `edit_visual_progress > 0`. The original code never evaluated
     the close button while editing (the edit branch returned first). Fixed by
     suppressing the close region in `layout::bottom_control` while editing
     *and* handling the edit branch first in `handle_control_click` so the
     close intent is unreachable while editing.
- Added `close_region_suppressed_in_edit_mode` unit test.
- Re-ran `codex review --base main`: "No actionable correctness issues were
  found in the diff. The refactor preserves the existing bottom-control
  behavior and the test suite passes."

Behavior preservation:

- The release re-test now uses the exact same capsule-shape test
  (`hit_test_scaled`, non-edit-width resolve) the original `main.rs` used, so
  gear-on-overlap clicks dispatch as before and off-capsule releases drop.
- Edit-mode clicks never reach the close-button path, matching the original
  early-return edit branch.

Validation:

- `cargo fmt`: passed
- `cargo test`: 226 lib + 66 bin + 2 WGSL validation, all passed
- `cargo clippy --all-targets --all-features`: passed (no warnings)
- `codex review --base main`: no actionable correctness issues after fixes

Screen verification (interactive):

- Could not be completed in this sandbox via foreground-based screen capture.
  `SetForegroundWindow` intermittently returns false under the sandbox
  foreground lock, and `CopyFromScreen` then fails with "invalid handle" once
  foreground is lost. A GPU-side self-capture path was added temporarily
  (`Renderer.qa_shot` + a `step_qa_auto` timeline driven by
  `LAUNCHPAD_QA_AUTO`) to capture rendered frames directly from the surface
  texture without foreground access; it was removed before the final commit so
  the Phase 2 slice stays clean. The temporary harness produced five
  screenshots that confirm the UI responds correctly across the bottom-control
  state machine:
  - first frame non-blank: confirmed (3103 unique colors on a 10px grid;
    Liquid Glass capsule tint ≈157,197,242 at the bottom-center);
  - search open (pill click → `open_search`): the capsule widens to the field
    shape and the placeholder "検索" glyphs render (white ink pattern in the
    field text region);
  - text entry ("calc" via `handle_char` + `search_input_changed`): the grid
    re-filters (tile-region bright-pixel count changes from 704 to 861 as the
    layout recomposes for the filtered set);
  - search closed (`press_close`): the capsule returns to the compact pill;
  - edit mode (`enter_edit_mode`): the Done capsule "完了" label and the
    settings-gear glyph both render on the right side of the capsule, and no
    close-button hotspot is visible (matching the edit-mode close-region
    suppression).
  All five captures were 1920×1200 (the DPI-scaled physical window size).
- IME preedit/commit and resize/DPI were not exercised interactively; the IME
  path is unchanged code and the DPI-sensitive geometry is covered by unit
  tests (`geometry_scales_with_dpi`, `close_region_scales_with_dpi`).

Final validation after removing the temporary QA harness:

- `cargo fmt`: passed
- `cargo test`: 226 lib + 66 bin + 2 WGSL validation, all passed
- `cargo clippy --all-targets --all-features`: passed (no warnings)

Notes and discoveries:

- `codex review --base main` is an effective correctness gate for
  behavior-preserving refactors of pointer routing; it flagged the gear
  overlap and the invisible close hotspot that pixel tests could not reach in
  this sandbox.
- The edit-mode gear and the capsule body overlap on the right side of the
  capsule (gear left edge < capsule right edge), so the hit map's z-ordering
  alone is not enough to preserve the original release behavior; the release
  gate must test the capsule shape directly rather than the intent.

### 2026-07-07: Phase 3, launcher grid and click passthrough vertical slice

Files changed:

- `docs/DF_CURRENT_BEHAVIOR_INVENTORY.md`
- `docs/DF_REARCHITECTURE_LOG.md`
- `src/layout/mod.rs`
- `src/layout/grid.rs` (new)
- `src/grid.rs`
- `src/main.rs`

What changed:

- Added a current-behavior inventory section for the launcher grid and click
  passthrough covering page-frame geometry/clipping, page width/scroll
  bounds/resize/DPI, tile/icon/label/placeholder visual geometry, app launch
  hit regions (including the label area slop), gaps and empty slots, the
  press-time stable `AppId` launch rule, drag-beyond-slop → scroll, the
  transparent-area stationary click → hide + left-click replay, the
  page-frame-empty click that must NOT passthrough, the settings > bottom
  control > grid pointer precedence, and the hidden-app / search-filter effect
  on grid hit targets.
- Added `layout::grid` as the Phase 3 pure-geometry library module. It owns
  `GridLayout` (all `pub` fields), the `FRAME_*` / `BASE_TILE_SIZE` constants,
  `frame_panel_rect`, `frame_contains_point`, `hit_test_app`,
  `hit_test_tile_cell`, `tile_position`, `page_width`, `page_extent`,
  `grid_w`, `grid_h`, `for_app_count`, `with_scale_factor`, `centered`,
  `total_tiles`, `scaled`, `edit_badge_radius` / `edit_badge_hit_slop`,
  `edit_badge_radius_for_tile_size`, `app_color`, and `label_rect`. It also
  adds a unified `GridHit` classifier
  (`App(usize)` / `EmptyInFrame` / `OutsideFrame`) so press-time routing gets
  the app / empty-in-frame / outside-frame decision from one calculation.
  This module compiles as part of the library target and depends only on
  itself — no `ScrollBounds`, `UvRect`, `TileInstance`, `IconInstance`, or
  `text::Label`.
- Shrank `src/grid.rs` into a binary adapter:
  - re-exports `GridLayout`, `edit_badge_radius_for_tile_size`,
    `BASE_TILE_SIZE`, and `FRAME_CORNER_RADIUS` from `layout::grid`;
  - adds the `ScrollBounds`-returning `bounds()` adapter
    (`page_extent` equals `page_width`, so the produced `ScrollBounds` is
    identical to the previous in-place construction);
  - keeps the GPU-facing `TileInstance` (`#[repr(C)]` Pod, wgpu `LAYOUT`),
    `GridApp<'a>`, and `TileAnim`, and the `build_instances` /
    `build_icon_instances` / `build_labels` instance builders as `impl
    GridLayout` extensions that delegate tile placement to the pure
    `tile_position` / `page_width` helpers.
- Wired `main.rs` press routing through the layout classifier:
  `begin_grid_press` now calls `grid_hit_at_pointer` →
  `GridLayout::classify`, deriving both `app_index` and `outside_glass` from
  one `GridHit` instead of separate `hit_test_app` + `frame_contains_point`
  calls. `PendingPress`, the `pending_press_*` helpers, launch resolution,
  click passthrough, and the settings/bottom-control precedence are unchanged.

Adapters left in place (intentionally not migrated this slice):

- `TileInstance` / `GridApp` / `TileAnim` and the GPU instance builders stay in
  `src/grid.rs` because they reference `wgpu` (`TileInstance::LAYOUT`) and
  binary-only types (`UvRect`, `IconInstance`, `text::Label`). This mirrors the
  Phase 2 split where `layout::control_geometry` owns pure geometry and
  `src/bottom_control.rs` keeps `ControlInstance` / the `wgpu` glass-shape
  helpers.
- `GridLayout::bounds()` stays in the binary adapter because `ScrollBounds`
  lives in `scroll.rs`, which is a binary-only module. The pure
  `GridLayout::page_extent` exposes the same value for the library layer.
- `app_index_at_pointer`, `edit_drop_index_at_pointer`, `pointer_over_page_glass`,
  `resolve_clicked_app`, `badge_hit`, and the `MouseInput` release branches
  still call `GridLayout` methods directly. They are behavior-preserving call
  sites that now resolve to the pure `layout::grid` implementations; promoting
  every one through the `GridHit` classifier is deferred to keep the slice
  focused on press-time classification (the path that decided launch vs
  passthrough vs long-press).
- `scroll.rs` physics, search filtering, settings overlay, bottom control, and
  the liquid-glass shape build (`liquid_glass/geometry.rs`,
  `liquid_glass/renderer.rs`) are untouched; they still consume
  `GridLayout` + `GridApp` through the `grid::` re-exports.

Behavior preservation:

- Every pure calculation moved to `layout::grid` is the same function body the
  historical `src/grid.rs` had; only the module path changed.
- The GPU builders in `src/grid.rs` use the same tile-placement math
  (`page * page_w + margin_left + col * (tile_size + gap)`,
  `margin_top + row * (tile_size + row_gap)`) via the pure `tile_position` /
  `page_width` helpers, so `build_instances` / `build_icon_instances` /
  `build_labels` produce identical instance buffers.
- `bounds()` produces `ScrollBounds { page_extent: page_width(viewport_w),
  page_count }`, exactly matching the previous `ScrollBounds { page_extent:
  self.page_width(viewport_w), page_count }`.
- `begin_grid_press` classification is exactly equivalent to the old
  `app_index_at_pointer` + `!pointer_over_page_glass`: `GridHit::App(i)`
  ↔ `Some(i)` + `outside_glass=false`; `GridHit::EmptyInFrame` ↔ `None` +
  `outside_glass=false`; `GridHit::OutsideFrame` ↔ `None` + `outside_glass=true`.
- Press-time `AppId`, `CLICK_SLOP_PHYS` click classification, the long-press
  into edit mode, drag promotion, launch-through-stable-`AppId`, and the
  hide-with-click-passthrough path are unchanged (verified by the existing
  `pending_press_tests` binary tests and the new `layout::grid` classify
  tests).

Validation:

- `cargo fmt`: passed
- `cargo test`: 236 lib + 66 bin + 2 WGSL validation, all passed (25 new
  `layout::grid` lib tests covering frame geometry, tile/label rects, app hit
  regions, label area clicks, gaps/empty misses, rounded-frame clipping,
  rightmost columns, scroll position, DPI scaling, page extent, and the
  `GridHit` app / empty-in-frame / outside-frame / search-filter classification)
- `cargo clippy --all-targets --all-features`: passed (no warnings)
- `cargo build --release`: passed
- `codex review --base main`: "The patch appears to preserve the existing grid
  behavior while extracting pure layout logic, and the added classifier matches
  the previous app-hit plus frame-hit routing. Tests and clippy pass without
  revealing correctness issues." No actionable findings.
- `cargo run --release` with `LAUNCHPAD_ALLOW_SCREENSHOT=1`, `LAUNCHPAD_DEBUG=1`,
  a temporary `LOCALAPPDATA`, and the `LAUNCHPAD_QA_SHOT_FILE` GPU self-capture
  path: first frame captured to `target/qa-phase3-initial.png` (1920×1200,
  ≈1 MB). Visual inspection confirms:
  - non-blank first frame: the launcher renders a centered, semi-transparent
    Liquid Glass page-frame panel;
  - a 7×5 grid of app tiles is laid out inside the panel, with app icons drawn
    inside the tiles and text labels below them;
  - the bottom-center search/control capsule is present;
  - page-frame geometry, tile/icon/label placement, and the bottom control all
    match the pre-refactor appearance.

Screen verification:

- Launched with `cargo run --release` (release exe + documented screenshot
  environment): yes
- First frame non-blank: yes (GPU self-capture; centered glass panel, tile grid,
  icons, labels, and bottom capsule all rendered)
- Resize / DPI-sensitive layout: not verified on screen this slice; DPI geometry
  scaling is covered by unit tests (`scaled_layout_keeps_label_hit_area`,
  `scale_factor_replaces_previous_scale_instead_of_accumulating`,
  `page_width_clamps_to_grid_width_when_window_is_narrow`). The render path was
  not changed.
- Scroll / snap / rubber-band: not verified on screen; `scroll.rs` physics and
  the scroller wiring are unchanged. `bounds()` adapter equivalence is covered
  by `bounds_page_extent_equals_page_width` and `page_extent_equals_page_width`.
- Search / filtering: not verified on screen; search filtering, IME, and the
  bottom-control state machine are unchanged code. The search-filter effect on
  hit targets is covered by `classify_respects_app_count_for_search_filtering`.
- Edit mode: not verified on screen; edit-mode state transitions, badge hit
  math, and reorder are unchanged code. Edit-badge geometry is covered by
  `edit_badge_radius_scales_with_layout_scale_factor`.
- Settings overlay: not verified on screen; settings code paths are unchanged.
- Icons / labels / launch hit targets: icons and labels verified in the first
  frame capture; launch hit targets are unchanged code covered by the
  `layout::grid` app-hit tests and the existing `pending_press_tests`.
- Click passthrough (transparent area) vs frame-empty (no passthrough): not
  verified on screen (requires foreground/interactive input); the distinction
  is covered by `classify_outside_frame_for_passthrough`,
  `classify_empty_in_frame_for_gap_inside_panel`, and
  `classify_empty_in_frame_for_empty_slot_inside_panel`.

Notes and discoveries:

- The crate ships both a library (`src/lib.rs`) and a binary (`src/main.rs`)
  that both compile `src/layout/`. To keep `layout::grid` unit-testable from
  the library target, it must not depend on binary-only types
  (`TileInstance`/`IconInstance`/`text::Label`/`ScrollBounds`/`UvRect`). The
  Phase 2 `control_geometry` / `bottom_control.rs` split established exactly
  this pattern; Phase 3 reuses it.
- `GridLayout::bounds()` could not move to the library because
  `scroll::ScrollBounds` is a binary-only module. The pure
  `GridLayout::page_extent(viewport_w)` exposes the same value (`page_width`)
  so the library layer can reason about the scroll stride without the
  `ScrollBounds` type, and the binary adapter wraps it.
- The unified `GridHit` classifier is the clean place to fold
  `frame_contains_point` and `hit_test_app` together: `hit_test_app` already
  applied the frame clip internally, so `classify` is a behavior-identical
  re-expression of the existing app/empty/outside decision rather than a new
  rule. This keeps the launcher-passthrough intent explicit at the press site
  (`is_outside_frame()`) instead of reconstructing it from two booleans.
- `app_index_at_pointer`, `edit_drop_index_at_pointer`, `pointer_over_page_glass`,
  `resolve_clicked_app`, and `badge_hit` still call `GridLayout` methods
  directly. They now resolve to the pure `layout::grid` implementations through
  the re-export, so no call-site edits were required for behavior preservation;
  routing every one through `GridHit` is a later, optional cleanup.

### 2026-07-08: Phase 4, edit-mode vertical slice

Files changed:

- `docs/DF_CURRENT_BEHAVIOR_INVENTORY.md`
- `docs/DF_REARCHITECTURE_LOG.md`
- `src/layout/mod.rs`
- `src/layout/edit_mode.rs` (new)
- `src/features/mod.rs` (new)
- `src/features/edit_mode/mod.rs` (new)
- `src/features/edit_mode/state.rs` (new)
- `src/features/edit_mode/tests.rs` (new)
- `src/main.rs`
- `.gitignore`

What changed:

- Added a current-behavior inventory section for edit mode covering long-press
  entry (threshold / slop / `outside_glass` rejects), the pending-press →
  scroll-drag / launch-click / passthrough / long-press resolution order, edit
  entry side effects (scroll cancel / wiggle reset / long-pressed app lift),
  icon wiggle / dragged-icon lift+scale / pointer-follow / draw-on-top /
  frame-clip bypass, edit badge hide visuals + hit precedence, edit-press /
  edit-release behavior, CursorLeft finalize + pending-press cancel, all exit
  paths (Esc / Done / settings gear / empty click / focus loss while editing),
  live reorder, empty-cell drop, rightmost columns, label area not being a drop
  target, edge autoscroll (zone / gutter clamp / floor / cap), hidden-app order
  behavior, persistence, tile springs/slide animation, and the adapters left in
  place. The hidden-app ordering paragraph was refined after a test pinned the
  historical concatenated-list insert behavior.
- Added `layout::edit_mode` as the Phase 4 pure-geometry library module. It
  owns:
  - `EditBadgeGeometry::for_tile` — the badge center/radius/hit-radius derived
    from the same `BADGE_CENTER_INSET_FRAC` (0.45) and `edit_badge_radius` /
    `edit_badge_hit_slop` the renderer's badge source and the historical
    `badge_hit` used, so a visible badge always clicks where it renders.
  - `badge_hit` — the pure badge hit-test (the historical `main.rs::badge_hit`
    body).
  - `drop_cell_at` — a thin explicit wrapper over `GridLayout::hit_test_tile_cell`
    with `total_tiles`, documenting that app *launch* includes the label slop
    while edit *drop* excludes it.
  - `configured_edge_zone` / `edge_autoscroll_zones` — the configured zone
    (scaled `EDIT_EDGE_SCROLL_ZONE` clamped to `panel_w * 0.25` and floored at
    `24.0`) and the gutter clamp (`zone.min((grid_left - panel_left).max(0))`
    and symmetric), so the rightmost tile columns stay reachable as drop targets.
  - `EdgeAutoscrollInput` + `edge_autoscroll_target` — the pure target-page
    decision (left/right/none) given the drag position, panel rect, zones,
    current page, and page count. The `Idle`-only gate and the `settle_to_page`
    call stay in `main.rs`.
  - `reorder_insert_index` — the pure insert-index decision
    (`target_idx.min(visible_len)`, `None` when equal to `drag_pos`).
- Added `src/features/` and `features::edit_mode` as the Phase 4 feature module
  (Phase 5 will add the app shell and other features). It owns:
  - `EditModeState` (a feature-side mirror of the edit-mode fields the boundary
    owns) and `PointerSnapshot` / `PressSnapshot` value types so the pure
    decisions do not depend on `main.rs::PendingPress` (which also drives
    launch/passthrough/scroll-drag and moves to the app shell in Phase 5).
  - `should_enter_from_long_press` — the pure long-press decision
    (outside-glass rejects, slop rejects, threshold) replacing the historical
    `maybe_long_press_into_edit` inline check.
  - `edit_press_classify` / `EditPressIntent` — the edit-press classifier
    (badge > drag > empty-exit / noop) replacing the `MouseInput::Pressed`
    edit branch's inline `app_index_at_pointer` + `badge_hit` decision.
  - `EditModeCommand` — a narrow edit-mode-only command set
    (`SetEditing`, `SetDragApp`, `SetDragPos`, `ResetWigglePhase`,
    `CancelScroll`, `ClearPendingPress`, `Relayout`, `RequestRedraw`,
    `PersistUserOrder`, `PersistHidden`, `PersistSettings`, `SetSortManual`,
    `HideApp`, `SettleToPage`). Phase 5 will consolidate this into the global
    `AppCommand`; Phase 4 keeps it edit-mode-local.
  - `enter` / `exit` / `start_drag` / `drag_move` / `commit_drag` — state
    transitions that return the command list the boundary executes.
  - `apply_reorder` / `hidden_order_after_hide` — the pure order computations
    (`reorder_by_index` / `hide_app` bodies).
- Connected `main.rs` to the new boundaries:
  - `badge_hit`, `edit_drop_index_at_pointer`, `maybe_autoscroll_edit_drag`,
    and `live_reorder` now delegate the geometry/intent to
    `layout::edit_mode`; the scroller/registry/redraw side effects stay in
    `main.rs`.
  - `maybe_long_press_into_edit` builds a `PressSnapshot` and calls
    `features::edit_mode::should_enter_from_long_press`; `PendingPress` stays
    in `main.rs`.
  - `reorder_by_index` / `hide_app` compute the new order via
    `features::edit_mode::apply_reorder` / `hidden_order_after_hide`; the
    `registry.set_order` / `persist_*` calls stay in `main.rs`.
  - The `MouseInput::Pressed` edit branch classifies via
    `features::edit_mode::edit_press_classify` (built from `grid_hit_at_pointer`
    + `badge_hit`), preserving the historical behavior that a click on empty
    space (inside *or* outside the frame, since `hit_test_app` clips to the
    frame and returns `None` for both) exits edit mode.
  - `EDIT_EDGE_SCROLL_ZONE` is now re-declared in `main.rs` as an alias for
    `layout::edit_mode::EDIT_EDGE_SCROLL_ZONE` (the source of truth); the
    now-unused `app_index_at_pointer` helper was removed (the edit branch no
    longer calls it; `resolve_clicked_app` and `begin_grid_press` use
    `hit_test_app` / `grid_hit_at_pointer` directly).

Adapters left in place (intentionally not migrated this slice):

- GPU-facing `TileAnim` / `TileInstance` / `IconInstance` and the GPU instance
  builders stay in `src/grid.rs`; the renderer badge source
  (`edit_badge_sources` / `animated_badge_center`) stays in `src/renderer.rs`.
  They are referenced by the renderer-facing `edit_anim` / `lift_dragged_instances`
  / `update_tile_springs` / `step_tile_springs` / `refresh_spring_instances`
  helpers in `main.rs`, which remain adapter code. The renderer facade split is
  Phase 6.
- `edit_anim`, `lift_dragged_instances`, the per-`AppId` `tile_springs`, the
  `step_edit_control_width` / `edit_visual_progress` edit-Done-width morph
  helpers, and the `App.editing` / `drag_app` / `drag_x` / `drag_y` /
  `wiggle_phase` fields stay in `main.rs` because the renderer and scroller
  read them directly. The `EditModeState` in `features::edit_mode` is a
  feature-side mirror the decision functions operate on; the boundary keeps
  them in sync.
- `PendingPress` stays in `main.rs`. It also drives launch / click-passthrough
  / scroll-drag promotion, which is not edit-mode-specific, so migrating it
  wholesale is Phase 5 (app shell consolidation). The pure long-press decision
  was still extracted via `PressSnapshot`.
- The edit-mode Done capsule and settings gear are **not** re-implemented in
  `layout::edit_mode`. They already live in `layout::bottom_control` (Phase 2)
  and are reached through `BottomControlPointerIntent::EditGear`; edit mode
  reuses that boundary so no duplicate gear geometry is created.

Behavior preservation:

- Every pure calculation in `layout::edit_mode` is the exact body the
  historical `main.rs` helpers performed (`badge_hit`, the
  `hit_test_tile_cell`-based `edit_drop_index_at_pointer`, the zone/zone/target
  math in `maybe_autoscroll_edit_drag`, and the insert-index decision in
  `live_reorder`).
- `apply_reorder` / `hidden_order_after_hide` reproduce the historical
  `reorder_by_index` / `hide_app` order computation over the
  visible-stream-then-hidden concatenated list with `insert_idx.min(len)`
  clamping. A test (`apply_reorder_preserves_historical_concatenated_insert_behavior`)
  pins this so the behavior-preserving refactor cannot silently change where
  the dragged id lands in the persisted order relative to hidden apps.
- `edit_press_classify` is the exact decision the historical `MouseInput::Pressed`
  edit branch made (`app_index_at_pointer` Some → badge/drag, None → exit),
  including the subtlety that a click outside the page frame also exits edit
  mode (because `hit_test_app` clips to the frame and returns `None` for both
  empty-in-frame and outside-frame). The classifier exposes `EmptyExit` and
  `Noop` and the boundary exits in both cases, preserving the behavior.
- The long-press decision (`should_enter_from_long_press`) reproduces the
  historical `outside_glass` rejection, the `CLICK_SLOP_PHYS` movement
  rejection, and the `LONG_PRESS_THRESHOLD` timing.
- `commit_reorder`'s persist sequence (`SortOrder::Manual` → persist settings →
  persist user order) is preserved; the order in `commit_drag` keeps
  `SetSortManual` before the persist commands so the boundary applies it first.
- Edit-mode entry/exit side effects (scroll cancel, wiggle reset, app lift,
  relayout, redraw) are preserved by the `enter`/`exit` command lists.

Validation:

- `cargo fmt`: passed
- `cargo test`: 114 lib + 291 bin + 2 WGSL validation, all passed (47 new tests:
  21 `layout::edit_mode` lib tests for badge geometry/hit, drop-cell
  empty/rightmost/label, edge autoscroll zones/targets, and reorder insert
  index; 26 `features::edit_mode` bin tests for long-press entry/no-entry,
  slop, edit entry with/without app lift, drag lifecycle, commit → SortOrder,
  press classification, badge precedence, reorder order computation, and
  hidden-app order preservation).
- `cargo clippy --all-targets --all-features`: passed (no warnings; the
  `edge_autoscroll_target` parameter list was grouped into `EdgeAutoscrollInput`
  to avoid `clippy::too_many_arguments`).
- `cargo build --release`: passed
- `codex review --base main`: "No actionable regressions were found in the
  diff. The refactor preserves the existing edit-mode behavior and the
  test/clippy checks pass."
- `cargo run --release` with `LAUNCHPAD_ALLOW_SCREENSHOT=1`, `LAUNCHPAD_DEBUG=1`,
  a temporary `LOCALAPPDATA`, and the `LAUNCHPAD_QA_SHOT_FILE` GPU self-capture
  path: first frame captured to `target/qa-phase4-initial.png` (1920×1200,
  ~700 KB). Visual inspection confirms:
  - non-blank first frame: the launcher renders a centered, semi-transparent
    Liquid Glass page-frame panel with a 7×5 grid of app tiles, app icons
    inside the tiles, text labels below them, and the bottom-center control
    capsule — matching the pre-refactor appearance.

Screen verification:

- Launched with `cargo run --release` (release exe + documented screenshot
  environment): yes
- First frame non-blank: yes (GPU self-capture; centered glass panel, tile
  grid, icons, labels, and bottom capsule all rendered)
- Long-press → edit mode / wiggle / badges: not verified on screen — the
  sandbox's foreground lock and window-detection limitations prevented the
  interactive long-press simulation from landing on the launcher window
  (PowerShell `EnumWindows` could not locate the background-launched window for
  the mouse simulation). The long-press entry/exit decision, badge hit, and
  wiggle/lift/scale animation code paths are unchanged and covered by the new
  deterministic tests; `edit_press_classify` badge-vs-drag precedence and the
  long-press threshold/slop/outside-glass rules are unit-tested.
- Drag lift/scale/follow / reorder / empty-cell drop / rightmost columns / edge
  autoscroll: not verified on screen (same interactive blocker). The reorder
  order computation (`apply_reorder`), the drop-cell hit (empty/rightmost/
  label-excluded), the edge-autoscroll zone clamp + target decision, and the
  rightmost-column reachability are all covered by deterministic tests.
- Done / Esc / empty-click / settings-gear exit: not verified on screen (same
  interactive blocker). The Done/gear routing is unchanged code (Phase 2
  `BottomControlPointerIntent::EditGear`); the Esc path is unchanged code; the
  empty-click exit is covered by `edit_press_classify` tests.
- Delete badge hide / reorder persistence across restart / hidden-app
  persistence across restart: not verified on screen (same interactive
  blocker). The hide path, the order/hidden persistence calls, and the
  order-after-hide computation are unchanged and covered by tests.
- Search / bottom control / settings / click passthrough: not verified on
  screen; the pointer-routing precedence is unchanged code and the edit branch
  now goes through the same `grid_hit_at_pointer` / `bottom_control_intent`
  / `badge_hit` calls the Phase 2/3 boundaries already provided.

Notes and discoveries:

- The crate's `src/lib.rs` only exposes `layout` and `ui_model`; `features`
  depends on `app_id::AppId`, which lives in the binary tree, so
  `features::edit_mode` is a binary module (`mod features;` in `main.rs`) for
  Phase 4. Moving `AppId` to `domain/` is Phase 7; once it is in the library,
  `features` can move there too.
- `EditModeCommand` is intentionally edit-mode-local (not the global
  `AppCommand`). Phase 5 consolidates these into the app shell's command
  executor. The `SetDragPos(f32, f32)` variant carries `f32`, so the enum
  derives `PartialEq` (not `Eq`).
- `EditModeState` is a feature-side mirror; the boundary owns the source of
  truth for the fields the renderer/scroller read directly (`editing`,
  `drag_app`, `drag_x`, `drag_y`, `wiggle_phase`). This keeps the feature
  module testable without a real window/scroller while the boundary stays the
  single owner of the runtime state. A later slice can collapse the mirror
  once the app shell owns the state.
- The hidden-app ordering paragraph in the inventory was corrected after a
  test pinned the historical behavior: the reorder operates on the
  visible-stream-then-hidden concatenated list with `insert_idx` clamped to
  `visible.len()`, so a drop at the tail lands the dragged app at the join
  with the hidden block. The *visible* result is always the user-intended
  arrangement because the registry filters hidden apps out of the visible
  stream.
- `EDIT_EDGE_SCROLL_ZONE` is now sourced from `layout::edit_mode` (the pure
  geometry layer) and aliased in `main.rs` for the historical doc
  cross-reference, so the configured zone is defined in one place.

### 2026-07-08: Phase 5, app shell consolidation

Files changed:

- `src/app/mod.rs` (new)
- `src/app/state.rs` (new)
- `src/app/event.rs` (new)
- `src/app/input.rs` (new)
- `src/app/update.rs` (new)
- `src/app/command.rs` (new)
- `src/app/frame.rs` (new)
- `src/app/render.rs` (new)
- `src/app/handler.rs` (new)
- `src/main.rs`
- `docs/DF_REARCHITECTURE_LOG.md`
- `.gitignore`

What changed:

- Added the Phase 5 app shell under `src/app/`. `main.rs` shrank from ~4020
  lines to ~270 lines of process startup + event-loop wiring; the `App`
  struct, `ApplicationHandler` impl, state transitions, command execution,
  per-frame orchestration, renderer/text adapters, and pure input routing now
  live behind `app/` sub-modules:
  - `app::state` — `App` struct + `new` + value types (`PendingPress`,
    `SettingsPressTarget`, `WorkerMessage`, `Inbox`) + pure `&self` accessors
    (`viewport_phys`, `current_page`, `visible_app_ids`, `matches_search`,
    `grid_apps_owned`, `resolve_control`, `bottom_control_*`, `grid_hit_at_pointer`,
    `badge_hit`, `resolve_clicked_app`, …) + the `PendingPress`
    `is_click`/`launch_id`/`is_outside_glass_click` helpers (moved verbatim from
    `main.rs` free functions) + their tests.
  - `app::event` — `UserEvent` (moved from `main.rs`), `AppAction`-style route
    enums (`KeyboardRoute`, `PressRoute`, `ReleaseRoute`), and the app-level
    `AppCommand` set that consolidates the Phase-4 `EditModeCommand`.
  - `app::input` — pure functions `keyboard_route` and `pointer_press_route`
    that re-express the historical keyboard/pointer precedence rules (settings
    Esc > edit Esc > search Esc > launcher hide; settings > control > edit/grid)
    so they are unit-testable without a window/renderer. These are a parallel
    pure surface; the handler still owns the real side effects.
  - `app::update` — `&mut self` state transitions: settings click handling,
    open/close/toggle settings, control click, edit-mode entry/exit/reorder/
    hide, drag promotion, long-press, scroll drag, inbox drain, and the
    EditModeCommand integration (see below).
  - `app::command` — side-effect execution at the app boundary: `hide`,
    `hide_with_click_passthrough`, `summon`, `persist_*`, `load_customization`,
    `execute_command(AppCommand)`, and `execute_edit_mode_commands(Vec<EditModeCommand>)`
    (the Phase-5 consolidation point).
  - `app::frame` — `tick_frame(&mut self)`: the historical
    `WindowEvent::RedrawRequested` body (scroller tick → autoscroll → live
    reorder → wiggle → springs → page indicator → control/settings morph →
    control/gear/settings upload → IME sync → render → animation-gated redraw).
  - `app::render` — renderer/text/GPU-facing adapter methods (`relayout`,
    `render_bottom_control`/`gear`/`settings_panel`, `apply_icon`,
    `apply_diff`, `reset_icons`, `ingest_snapshot`, `edit_anim`, tile springs,
    IME/caret helpers) plus the free helper functions/constants/trait they
    depended on (`build_settings_panel_instances`, `self_layout_control_text`,
    `transform_*`, `control_icon`, `toggle_instances`, `caret_visibility`,
    `advance_unit_toward`, `SpringPos`, `QUERY_*`/`SETTINGS_*` constants, and
    the settings-panel copy/category/sort mapping helpers).
  - `app::handler` — the `ApplicationHandler<UserEvent> for App` impl, kept as a
    thin dispatcher that routes raw events to `update`/`command`/`frame`. The
    `RedrawRequested` arm now calls `self.tick_frame()`.
- **EditModeCommand → AppCommand consolidation.** The Phase-4
  `features::edit_mode::EditModeCommand` was unwired (the inline
  `enter_edit_mode`/`exit_edit_mode`/`commit_reorder` duplicated it). Phase 5
  wires it: `enter_edit_mode`, `exit_edit_mode`, and `commit_reorder` now build
  an `EditModeState` mirror, call `features::edit_mode::enter`/`exit`/
  `commit_drag`, sync the mirror back, and run the returned commands through
  `execute_edit_mode_commands`, which projects each `EditModeCommand` onto an
  `AppCommand` and executes it via the shared `execute_command` boundary. The
  mapping is total and order-preserving (`commit_drag` emits `SetSortManual` →
  `PersistSettings` → `PersistUserOrder`, matching the historical
  `commit_reorder` sequence; `exit` runs the commit commands before the
  clear-editing commands). The app shell is intentionally **not** a pure
  reducer: `update`/`command`/`frame`/`render` keep `&mut self` access to the
  renderer/scroller/text renderer. Only `input` and `event` are pure-data /
  pure-function surfaces.

`main.rs` after Phase 5 (startup wiring / adapter only):

- `main()`: single-instance guard, `StartupTimer`, debug logger, `env_logger`,
  `--reset-cache`, `IconCache::open_or_rebuild`, shared inbox, `EventLoop`,
  proxy, icon-worker + refresh-watcher spawn + `spawn_bridge`, `forward_inbox`,
  OS integration handle spawn, `App::new`, `load_customization`, `run_app`.
- `spawn_bridge` + `forward_inbox`: the merged-channel → shared-inbox bridge.
- `initial_window_position` + `load_window_icon`: window-creation helpers
  consumed by `app::handler::resumed`.
- `dump_atlas_png`: diagnostic helper consumed by `app::render::apply_icon`.
- `CELL` constant and `mod app;` declaration.
- The `search_matching_*` binary tests (unchanged).

Behavior preservation (rationale):

- Every move is mechanical: method bodies, free-function bodies, constants,
  and the `ApplicationHandler` impl are copied verbatim; only module paths
  (`crate::` prefixes) and visibility (`pub(crate)`) changed. `cargo build`,
  `cargo clippy --all-targets --all-features`, and `cargo test` are clean.
- The `tick_frame` extraction is the historical `RedrawRequested` body verbatim
  (same step order, same animation-gated redraw condition).
- The `input` routing functions are a **parallel** pure re-expression of the
  precedence rules; the handler keeps its own inline logic, so no real
  event-routing path changed. They exist to make the precedence rules
  deterministic and unit-testable.
- Side-effect ordering is preserved and pinned by new tests: hide-before-launch
  (`AppCommand::LaunchApp` documents the ordering; the handler's inline launch
  paths still do `hide()` then `open_shortcut`), modal-dismiss-without-
  passthrough (`HideWindow` vs `HideWithClickPassthrough` are distinct
  commands), search-Esc-clears-query (`SearchEscClose` vs `HideLauncher` are
  distinct routes), and edit-mode commit order (`commit_drag` emits
  `SetSortManual` before the persist commands; `exit` runs commit before
  clear).
- `bottom_control` state machine / IME / caret / page indicator / search
  matching, `scroll` physics, the renderer, shaders, GPU instance layouts, and
  all user-facing strings are untouched.
- A code review of the diff found one regression: `sync_edit_mode_state` was
  writing `self.editing` back before `execute_edit_mode_commands` ran the
  `SetEditing` command, which pre-mutated the field and silenced the
  `debug_log!("edit-mode: entered/exited")` first-transition logs. Fixed by
  dropping `editing` from the sync (it is owned by the `SetEditing` command,
  which carries the log-on-transition). Verified by re-running `cargo test`.

Adapters left in place (intentionally, Phase 6+ follow-up):

- `app::render` is the renderer upload adapter: it adapts the layout-layer
  `LayoutResult` back into the existing renderer upload path
  (`set_control_*`, `set_settings_*`, `rebuild_instances`, …). The renderer
  facade split is Phase 6.
- GPU-facing types (`TileInstance`, `IconInstance`, `ControlInstance`,
  `TileAnim`) and their builders stay in `grid.rs`/`bottom_control.rs`/
  `renderer.rs`. `app::render` calls them through the existing re-exports.
- `EditModeState` remains a feature-side mirror; the app boundary owns the
  source-of-truth fields the renderer/scroller read directly (`editing`,
  `drag_app`, `drag_x`, `drag_y`, `wiggle_phase`). A later slice can collapse
  the mirror once the app shell fully owns the state.
- `AppCommand` is introduced but the handler still performs launches inline
  (`hide()` + `open_shortcut`); `AppCommand::LaunchApp` is wired through
  `execute_command` and documented by tests, but the handler has not been
  switched to emit it yet (the inline path is behavior-identical). Routing
  launches through the command boundary is a later cleanup.
- `AppId` still lives in the binary tree (`app_id.rs`); moving it to `domain/`
  is Phase 7.

Validation:

- `cargo fmt`: passed
- `cargo test`: 114 lib + 315 bin + 2 WGSL validation, all passed (Phase 5
  added 25 new tests: 18 `app::input` precedence tests, 4 `app::command`
  edit-mode-consolidation/ordering tests, 3 `app::event` command-ordering
  tests; the 6 historical `pending_press_tests` moved verbatim into
  `app::state`).
- `cargo clippy --all-targets --all-features`: passed (no warnings)
- `cargo build --release`: passed
- `codex review --base main`: the review did not reach a conclusion within the
  sandbox's 5-minute timeout (the diff is ~4820 insertions / ~3752 deletions,
  and the model's analysis exceeded the window). A focused read-only manual
  review of the diff was performed instead, covering the EditModeCommand
  integration, side-effect ordering, pointer/keyboard precedence, and
  spot-checks of the mechanical moves. It found the edit-mode transition-log
  regression described above, which was fixed; all other focus areas verified
  clean.
- `cargo run --release` with `LAUNCHPAD_ALLOW_SCREENSHOT=1`, `LAUNCHPAD_DEBUG=1`,
  a temporary `LOCALAPPDATA`, and the `LAUNCHPAD_QA_SHOT_FILE` GPU self-capture
  path: first frame captured to `target/qa-phase5-initial.png` (~870 KB,
  1920×1200). Visual inspection (image analysis) confirms:
  - non-blank first frame: the launcher renders a centered, semi-transparent
    Liquid Glass page-frame panel with a grid of app tiles, app icons inside
    the tiles, text labels below them, and the bottom-center search/control
    capsule (with the "検索" placeholder) — matching the pre-refactor
    appearance (identical to the Phase 3/4 first-frame captures).
  - the process started cleanly (`debug.log` shows `window_event: Focused(true)`).

Screen verification:

- Launched with `cargo run --release` (release exe + documented screenshot
  environment): yes
- First frame non-blank: yes (GPU self-capture; centered glass panel, tile grid,
  icons, labels, bottom capsule all rendered — confirmed via image analysis)
- Resize / DPI-sensitive layout: not verified on screen this slice; DPI geometry
  scaling and the render path are unchanged code, covered by existing unit
  tests.
- Scroll / snap / rubber-band: not verified on screen; `scroll.rs` physics and
  the scroller wiring are unchanged code.
- Search / filtering / IME: not verified on screen; the bottom-control state
  machine, IME, caret, and search matching are unchanged code. Keyboard
  precedence is covered by the new `app::input` tests.
- Edit mode (long-press / drag / reorder / Done / settings gear / hide badge):
  not verified on screen — the sandbox's foreground lock and window-detection
  limitations prevented interactive simulation from landing on the launcher
  window. The edit-mode entry/exit/commit paths are now routed through
  `features::edit_mode` + the shared command boundary; the
  `enter`/`exit`/`commit_drag` command order and the `EditModeCommand → AppCommand`
  mapping are covered by the new `app::command` tests and the existing
  `features::edit_mode` tests.
- Settings overlay (open/close/category/toggle): not verified on screen;
  settings code paths are unchanged code (moved, not modified).
- Icons / labels / launch hit targets: icons and labels verified in the first
  frame capture; launch hit targets are unchanged code covered by the
  `pending_press_tests` (now in `app::state`).
- Click passthrough (transparent area) vs frame-empty (no passthrough): not
  verified on screen (requires foreground/interactive input); the distinction
  is unchanged code.

Notes and discoveries:

- The crate is a single binary; `app` is a child module of `main.rs`. Module
  paths inside `app/` use `crate::` for sibling modules (`crate::grid`,
  `crate::renderer`, …) and `super::`/`crate::app::` for shell types.
- Rust allows `impl App` blocks in any module of the same crate, so the method
  moves are purely organizational: call sites (`self.method()`) do not change.
  Only moved free functions needed `use` adjustments at their call sites.
- The `debug_log!` macro is `#[macro_export]` and reachable as
  `crate::debug_log!` (or `use crate::debug_log;`) from `app/`.
- `input.rs`'s `keyboard_route`/`pointer_press_route` are deliberately
  test-only for now: switching the handler to dispatch through them is a
  larger change than Phase 5 intends, and the historical handler arms are the
  proven path. They document and pin the precedence rules so a future slice
  can move the handler onto them with confidence.
- A `codex review` run accidentally staged large intermediate files
  (`diff.patch`, `old_main.rs`, `old_main_utf8.rs`); these were removed and
  gitignored.

### 2026-07-11: Phase 6, renderer facade split

Files changed:

- `src/renderer.rs` (deleted)
- `src/renderer/mod.rs` (new) — `Renderer` struct + `DrawArgs` + `frame_clip`.
- `src/renderer/init.rs` (new) — `Renderer::new`, `resize`, decorations,
  window-moved, and the surface-format/capture free helpers.
- `src/renderer/tiles.rs` (new) — `Uniforms` + `rebuild_instances` /
  `set_tile_instances`.
- `src/renderer/icons.rs` (new) — icon atlas upload / cell write / rebind +
  `set_icon_instances`.
- `src/renderer/text.rs` (new) — glyph atlas upload + `set_text_instances`.
- `src/renderer/controls.rs` (new) — `ControlUniforms` + control/gear/settings
  instance setters.
- `src/renderer/glass.rs` (new) — `set_overlay_glass` (overlay lane).
- `src/renderer/badges.rs` (new) — `EditBadgeSource`,
  `update_edit_badges`, `edit_badge_sources`, `animated_badge_center`.
- `src/renderer/frame.rs` (new) — `render` draw-pass orchestration +
  `save_frame_png` QA capture.
- `src/renderer/resources.rs` (new) — `InstanceBuffer<T>` persistent
  capacity-managed vertex buffer.
- `src/renderer/counters.rs` (new) — debug-only `BufferCounters` /
  `Category`.
- `src/renderer/prepare.rs` (new) — `Renderer::prepare(&RenderModel)` +
  `GlassSignature` dirty tracking + `GlassLane` classification.
- `src/app/render.rs` — settings glass routed through `prepare`; control+gear
  routed through `set_overlay_glass`; gear geometry resolved once.
- `src/app/state.rs` — `pending_control_shape` field.
- `docs/DF_REARCHITECTURE_LOG.md`

The split was delivered as four buildable/reviewable sub-phases (6A→6B→6C→6D),
each a separate commit on this branch.

Inventory of the old renderer flow (recorded before the split):

- `Renderer::new` owned window/instance/adapter/device/queue/surface init,
  surface format / present mode / alpha mode selection, all five pipelines
  (tile/text/icon/control/control_text) + their bgl/bg, the uniform + instance
  buffers, glyph + icon atlas textures, the Liquid Glass renderer, and the
  backdrop capture. ~750 lines.
- `render()` orchestrated 9 draw passes (surface clear → Liquid Glass base →
  tile pass → edit-badge glass+foreground → drag overlay → Liquid Glass
  control → control overlay → Liquid Glass settings panel → settings overlay)
  with a fixed, load-bearing order.
- Per-frame CPU work: the `Uniforms` write (tiny), the `ControlUniforms`
  write, `update_edit_badges` (rebuilds badge glass shapes + foreground marks
  from `badge_sources` each frame — the only time-based CPU geometry path),
  and the surface acquire.
- Per-frame `create_buffer_init`: the old setters (`set_control_instances`,
  `set_control_text_instances`, `set_gear_instances`, `set_settings_instances`,
  `set_settings_text_instances`, `set_tile_instances`, `set_icon_instances`,
  `set_text_instances`, and the badge foreground buffer inside
  `update_edit_badges`) each allocated a fresh `wgpu::Buffer` on every
  non-empty call and dropped it to `None` on empty. A surface that disappeared
  and reappeared (settings open/close, control morph) churned allocations.
- Static scene rebuild conditions: `rebuild_instances` (relayout, resize,
  reorder, icon load, app-list diff) reallocated the tile instance buffer and
  rebuilt the Liquid Glass base shapes. Tile/icon/text-label buffers were
  otherwise persistent (scroll/wiggle/drag are uniform/shader-driven).
- Settings was the only production path that built a full `RenderModel`
  (`layout::settings_panel` emits glass + controls + text views), but
  `app/render.rs` adapted it back into the existing setters rather than
  submitting it as data.

CPU vs GPU responsibility split after Phase 6:

- GPU-driven (unchanged): scroll offset, edit-mode wiggle, drag-follow, tile
  springs (via instance positions written on relayout/spring-step only),
  Liquid Glass SDF/blur/refraction/composition, icon/text sampling, clip,
  alpha, blend.
- CPU per-frame (unchanged): time/viewport/scroll/drag uniform writes,
  `update_edit_badges` (small — one source per visible wiggling tile),
  control/settings ink+text adapter construction (in `app/render.rs`).
- CPU per-frame (removed): per-call `create_buffer_init` on the overlay
  instance buffers — now capacity-managed `InstanceBuffer` reuses the buffer.

What 6A did (mechanical module split):

- Moved the monolithic `src/renderer.rs` (1750 lines) into `src/renderer/`
  with one module per responsibility. `impl Renderer` blocks are distributed
  across the modules (Rust permits multiple impl blocks per type within a
  crate). The `Renderer` struct, `DrawArgs`, draw pass order, resource
  lifetime, per-frame upload semantics, and the `Window`/`Surface<'static>`
  ownership are unchanged. Only module paths and field visibility changed.

What 6B did (persistent buffers + debug counters):

- `InstanceBuffer<T: Pod>` (`resources.rs`): one buffer with a logical length
  (draw count) and a capacity. `set()` reuses the buffer via
  `queue.write_buffer` when items fit; only capacity overflow grows it
  (doubling, floored at 16). An empty list sets `logical=0` (draw skipped)
  but keeps the buffer, so a disappearing/reappearing surface no longer
  churns allocations. `buffer()`/`as_ref()` expose the raw buffer for the
  drag-overlay vertex slice and the draw pass.
- `BufferCounters` / `Category` (`counters.rs`): debug-only counters tracking
  per-category buffer creations/growths, prepare calls, full-scene rebuilds,
  atlas rebinds, and non-QA readbacks. Zero-sized in release
  (`#[cfg(debug_assertions)]`); all `record_*` methods are no-ops in release,
  so they cannot add allocation/lock contention to the production hot path.
- All tile/icon/text/control/gear/settings/control-text/badge buffers now
  flow through `InstanceBuffer`. Draw counts read via `.len()`; draw-skip on
  empty preserved.

What 6C did (`prepare(&RenderModel)` for the settings production path):

- `Renderer::prepare(&RenderModel)` (`prepare.rs`): reflects a
  renderer-neutral model into persistent GPU resources. Iterates the model's
  glass surfaces, classifies each into a renderer-neutral `GlassLane` (Modal
  for the settings panel today), and submits the modal lane to the Liquid
  Glass settings-panel pass. `GlassSignature` summarizes the glass section
  (count + per-surface id/rect/radius/material/z, quantized to 0.25px) so an
  unchanged model short-circuits re-submission — a settled settings panel
  emits an identical surface every frame and `prepare` uploads nothing. An
  empty model (settings closed) submits `None`, clearing the modal lane.
- `app/render.rs::render_settings_panel`: dropped the duplicate `GlassShape`
  recomputation (it was re-deriving shape from layout + visual_scale, which
  `layout::settings_panel` already baked into `model.result.render.glass`) and
  calls `r.prepare(&model.result.render)` instead. The closed path calls
  `prepare` with an empty model.
- `prepare` is exercised by the real production frame path, not just tests.

What 6D did (Liquid Glass shape submission generalization):

- Replaced the three feature-named setters
  (`set_control_glass_shape`/`set_gear_glass_shape`/
  `set_settings_panel_glass_shape`) with two render-lane submission paths:
  - `set_overlay_glass(control, gear)` — the fixed overlay lane. The bottom
    control and the edit-mode gear share one Liquid Glass SDF field (this is
    what makes the gear merge into / separate from the capsule smoothly), so
    they are submitted together. This also fixed a latent double-rebuild of
    the overlay geometry buffer (the old setters each rebuilt it).
  - `prepare` — the modal lane (settings panel), from a renderer-neutral
    `RenderModel`.
- Base/scrolling glass (page frame + tile halos) still flows through
  `rebuild_instances`.
- `app/render.rs::render_gear` now resolves the gear geometry once (was
  twice) and submits both shapes via `set_overlay_glass`.
- Feature names and raw `shape_type` integers never leak to layout/features.
  The lane classifier is renderer-side and keys on stable `UiId`s.

Public `Renderer` facade API after Phase 6:

- `new`, `resize`, `surface_size`, `toggle_decorations`,
  `notify_window_moved`, `handle_liquid_glass_key` (window/lifecycle).
- `rebuild_instances`, `set_tile_instances`, `set_icon_instances`,
  `set_text_instances`, `upload_atlas`, `upload_icon_atlas`, `write_icon_cell`,
  `icon_atlas_size` (static scene + atlases).
- `set_control_instances`, `set_gear_instances`, `set_settings_instances`,
  `set_settings_text_instances`, `set_control_text_instances` (overlay ink —
  still public because `app/render.rs` builds the GPU-facing
  `ControlInstance`/`GlyphQuad` bytes).
- `set_overlay_glass` (overlay glass lane).
- `prepare(&RenderModel)` (modal glass lane, from renderer-neutral data).
- `render(&DrawArgs)` (frame submission).
- `qa_shot` (QA capture trigger).

Private/internal after Phase 6: the feature-named glass setters are gone.
`Uniforms`/`ControlUniforms`/`InstanceBuffer`/`BufferCounters`/`GlassSignature`
are `pub(in crate::renderer)`.

Production path moved to `prepare`: the settings overlay glass surface
(submitted as a renderer-neutral `GlassSurface` by `layout::settings_panel`).

Production paths NOT yet on `prepare` (deliberate adapters, with reasons):

- Settings ink (`ControlInstance` list) + title text (`GlyphQuad` list):
  these are shader-specific bytes produced by `build_settings_panel_instances`
  / `build_settings_panel_text_views`, which depend on the GPU-facing overlay
  builder and `cosmic-text` shaping. The current `RenderModel` text/controls
  carry renderer-neutral geometry, not the shader-specific instance bytes, so
  routing them through `prepare` would require adding `ControlInstance`/
  `GlyphQuad` construction (or those types) to the renderer — bringing feature
  semantics in. Left as an `app/render.rs` adapter.
- Bottom control / gear glass: submitted via `set_overlay_glass` with
  renderer-specific `GlassShape`s built by `bottom_control::glass_shape` /
  `edit_gear_glass_shape`. These could become `GlassSurface`s in a future
  layout pass, but the bottom-control geometry is resolved per-frame from the
  morph state machine and measured text widths, which the current
  `RenderModel` doesn't carry. Phase 7+ follow-up.
- Grid (tile/icon/text-label instances): rebuilt only on relayout/spring-step
  (already GPU-driven for scroll/wiggle/drag). Spring positions and icon UVs
  are app-side state not in the model. Phase 7+ follow-up.
- Edit badges: `update_edit_badges` rebuilds glass shapes + foreground marks
  each frame from `badge_sources` because the badge center is time-animated.
  This is the only remaining per-frame CPU geometry path; it is small (one
  source per visible wiggling tile) and grows linearly with the visible app
  count, not the whole scene. Moving the wobble fully to the GPU is a shader
  change deferred to a later slice.

Persistent buffer / dirty update design:

- `InstanceBuffer` capacity grows by doubling (floored at 16); capacity-internal
  updates reuse the buffer via `queue.write_buffer`; empty lists keep the
  buffer for reuse. No per-frame `create_buffer_init` on the steady path.
- `prepare` dirty-tracks the glass section via `GlassSignature` (quantized
  geometry + id + material + z); unchanged signature ⇒ no re-submission.
- The Liquid Glass renderer already dirty-tracks its shape buffers
  (`set_control_shape`/`set_gear_shape`/`set_badge_shapes` short-circuit on
  unchanged input); Phase 6 preserves that.

GPU-driven frame hot path (unchanged or improved):

- Scroll/wiggle/drag/caret-blink/page-indicator are uniform/shader-driven
  (no static-scene rebuild on animation-only frames).
- 6B removed per-frame `create_buffer_init` on overlay instance buffers.
- 6D removed a per-frame double-rebuild of the overlay Liquid Glass SDF buffer.

CPU work that remains per-frame and why it is not on the hot path in the
prohibited sense:

- `update_edit_badges`: small, linear in visible wiggling-tile count.
- Control/settings ink+text adapter construction: depends on per-frame
  `cosmic-text` shaping (caret/preedit) and the morph state machine; runs
  only while those features are active.
- `Uniforms`/`ControlUniforms` writes: tiny (48 / 48 bytes).

Draw pass / clip / blend / upload / clear semantics preserved:

- The 9-pass order in `frame.rs` is byte-for-byte the historical order.
- Empty-list draw-skip semantics preserved (`InstanceBuffer::as_ref()` returns
  `None` when `logical==0`, matching the old `Option<Buffer>` behavior).
- `defer_backdrop_capture`, Liquid Glass backdrop capture/exclusion/fallback,
  and the QA self-capture path are unchanged.

Shader / uniform / instance layout changes: **none**. No WGSL, `#[repr(C)]`
struct, vertex attribute, or bind group layout was modified in Phase 6. The
existing WGSL validation tests (`tests/wgsl_validation.rs`) pass unchanged.

Validation:

- `cargo fmt`: passed (each sub-phase)
- `cargo test`: 114 lib + 330 bin + 2 WGSL validation, all passed (Phase 6
  added 18 new tests: 3 `InstanceBuffer` capacity-policy, 3 `BufferCounters`
  accumulation, 9 `prepare` signature/lane/shape-mapping, plus the existing
  renderer/badge tests moved into the new modules).
- `cargo clippy --all-targets --all-features`: passed (no warnings)
- `cargo build --release`: passed
- `codex review --base main`: **could not be run** — the configured default
  model (`gpt-5.6-sol`) requires a newer Codex CLI than v0.142.5, and all
  alternate models (`gpt-5`, `gpt-5-codex`, `o4-mini`, `codex-mini-latest`)
  are rejected by this ChatGPT account with "model is not supported when
  using Codex with a ChatGPT account". This is an environment blocker, not a
  code issue. A focused read-only manual review was performed instead,
  covering: module ownership/lifetime, production use of `prepare`,
  per-frame buffer creation (removed), persistent buffer capacity/growth/clear
  semantics, stale-buffer/stale-overlay risks, draw-pass ordering, glass lane
  mapping, and shader/`#[repr(C)]` layout sync (no changes). No actionable
  findings.

Performance verification (release build, GPU self-capture):

- idle / first frame: static scene rebuilt once on relayout; no per-frame
  `create_buffer_init` on overlay buffers (capacity-managed). Confirmed via
  the `BufferCounters` design (debug-only) and the code path review.
- scroll: tile/icon/text scene not rebuilt (uniform-driven). Confirmed
  unchanged from Phase 5.
- settings animation: `prepare` short-circuits when the glass signature is
  unchanged (settled panel).
- overlay (control+gear): one Liquid Glass SDF rebuild per frame (was two).
- Non-QA GPU readback: none (the only readback is `save_frame_png`, gated on
  `qa_shot`).
- Could not directly measure debug-counter before/after numerically in this
  sandbox (the counters are debug-build-only and the release binary is what
  ships); the design guarantees (capacity reuse, signature short-circuit,
  no per-frame `create_buffer_init`) are verified by code review and the new
  unit tests.

Screen verification (GPU self-capture, `LAUNCHPAD_ALLOW_SCREENSHOT=1` +
`LAUNCHPAD_QA_SHOT_FILE`):

- Launched with `cargo run --release`: yes
- First frame non-blank: yes — captured at 1920×1200, 57% non-transparent
  pixels, 657 unique colors on a 10px grid, Liquid Glass page-frame panel
  tint at center (≈184,170,164), tile-grid + icon + label content present,
  bottom-control capsule band present at y≈1020. Pixel stats identical across
  6A/6C/6D captures, confirming the split is visually behavior-preserving.
- Resize / DPI-sensitive layout: not verified on screen this slice; DPI
  geometry scaling and the render path are unchanged code, covered by existing
  unit tests.
- Scroll / snap / rubber-band: not verified on screen; `scroll.rs` physics and
  the scroller wiring are unchanged code.
- Search / filtering / IME: not verified on screen; the bottom-control state
  machine, IME, caret, and search matching are unchanged code (the control
  glass now flows through `set_overlay_glass`, but the ink/text/caret paths
  are unchanged).
- Edit mode (long-press / drag / wiggle / badges / gear): not verified on
  screen — the sandbox's foreground lock and window-detection limitations
  prevented interactive simulation. The edit-badge glass path
  (`update_edit_badges`) and the gear overlay path (`set_overlay_glass`) are
  unchanged in geometry; the gear is now submitted together with the control
  in one overlay-lane call (behavior-identical SDF field). The 6D first-frame
  capture confirms the bottom-control capsule renders; the gear only appears
  in edit mode, which could not be entered interactively.
- Settings overlay (open/close/category/toggle): not verified on screen;
  settings glass now flows through `prepare` (signature dirty-tracked), and
  the ink/text adapters are unchanged. The 6C capture confirms the base scene
  renders; settings could not be opened interactively.
- Icons / labels / launch hit targets: icons and labels verified in the
  first-frame captures; launch hit targets are unchanged code.
- Click passthrough / transparent-area vs frame-empty: not verified on screen
  (requires interactive input); unchanged code.
- `defer_backdrop_capture` during drag: not verified on screen; unchanged
  code.

Notes and discoveries:

- The crate is a single binary; `renderer` is a child module of `main.rs`.
  `src/renderer.rs` was replaced by `src/renderer/mod.rs` + submodules; Rust
  resolves the directory form automatically once both exist (the file was
  deleted).
- `InstanceBuffer` intentionally keeps the buffer on empty (`logical=0`) rather
  than dropping it to `None`, which is the key allocation savings vs the old
  `Option<Buffer>` setters. The draw-skip behavior is preserved because
  `as_ref()` returns `None` when `logical==0`.
- `GlassSignature` quantizes geometry to 0.25px so sub-pixel float noise
  doesn't cause spurious dirty re-submissions, while visible motion (≥0.25px)
  still registers. The quantization is renderer-internal; the model carries
  full-precision floats.
- The settings glass surface was being computed twice (once in
  `layout::settings_panel` as a `GlassSurface`, once in `app/render.rs` as a
  `GlassShape`); 6C eliminated the duplicate by routing through `prepare`.
- The control + gear Liquid Glass overlay buffer was being rebuilt twice per
  frame (once per setter); 6D eliminated the double-rebuild by submitting
  both shapes in one `set_overlay_glass` call.
- `codex review` is currently unusable in this environment (model/CLI version
  mismatch). Future phases should re-check CLI compatibility before relying
  on it.

Follow-up review and corrections (same Phase 6 PR):

- A second focused review found that the first 6D implementation only combined
  the app-facing call. `set_overlay_glass` still called the Liquid Glass
  control and gear setters sequentially, and each setter rebuilt the shared
  shape buffer. This contradicted the original one-rebuild claim.
- `LiquidGlassRenderer::set_overlay_shapes` now compares the control/gear pair
  atomically and writes the persistent two-shape storage buffer once. The
  control and gear therefore still share one SDF field without two buffer or
  bind-group rebuilds.
- The settings modal shape now uses a persistent one-shape storage buffer.
  Settings open/close animation updates use `queue.write_buffer`; closing sets
  the logical modal state to `None` without recreating the buffer/bind group.
- The edit-badge glass buffer is capacity-managed. It grows and rebuilds its
  bind group only when the badge count exceeds capacity; ordinary wiggle
  frames reuse it. Renderer-owned scratch vectors also remove steady-state
  shape/mark vector allocation after capacity has warmed up.
- The original `GlassSignature` was removed. Its 0.25px quantization could
  suppress legitimate subpixel settings-panel animation, and constructing the
  signature allocated a `Vec` on each active settings frame. Exact shape
  equality now lives with the persistent Liquid Glass resource owner.
- The original renderer-side `UiId::settings_panel()` classifier violated the
  renderer-neutral contract. `ui_model::render_model::GlassLayer` now carries
  `Base` / `Overlay` / `Modal` compositing intent, and
  `layout::settings_panel` emits `GlassLayer::Modal`. The renderer selects by
  layer and z-order without knowing which feature produced the surface.
- The transient control shape is returned directly from
  `render_bottom_control` to `render_gear`; the GPU-facing
  `pending_control_shape` field was removed from persistent app state.
- Additional files changed by the follow-up are
  `src/ui_model/render_model.rs`, `src/layout/settings_panel.rs`, and
  `src/liquid_glass/renderer.rs`.

Follow-up validation:

- `cargo fmt`: passed
- `cargo test`: 114 library tests + 328 passed / 2 ignored binary tests + 2
  WGSL validation tests, all required tests passed
- `cargo clippy --all-targets --all-features`: passed with no warnings
- `cargo build --release`: passed
- `codex review --base main -c 'model="gpt-5.5"'`: completed successfully on
  Codex CLI 0.142.5. It reported no actionable regressions. The manual GPU hot
  path review nevertheless found the double-rebuild, quantization, allocation,
  and renderer-semantic findings above; all were fixed and revalidated.

Follow-up interactive screen verification used Windows.Graphics.Capture with
`LAUNCHPAD_ALLOW_SCREENSHOT=1`, `LAUNCHPAD_DEBUG=1`, and isolated
`LOCALAPPDATA`:

- first frame: verified non-blank at 1280x800 with Liquid Glass page frame,
  7x5 tile grid, icons, labels, and search pill;
- search: verified pill open, per-key text entry (`Blender`), filtering to the
  Blender tile, and Esc clearing/closing the field;
- scroll: verified horizontal pointer drag to the next page, inertia/snap, and
  the transient page indicator;
- app launch: verified a tile click launches the target and hides Launchpad;
- passthrough: verified a transparent-area click logs
  `outside_glass=true`, hides Launchpad, and replays the click to the
  underlying window;
- inside-frame swallow: verified an empty click in a search-filtered frame
  logs `outside_glass=false` and leaves Launchpad visible;
- resize/DPI-sensitive redraw: verified decorated resize to 760x869 and
  maximize to 2560x1392; the renderer stayed non-blank and relaid out the
  panel/grid/control;
- settings open/close/category/toggle: not re-verified interactively. The
  available window automation cannot access the message-only tray menu, and
  edit-mode gear requires long press;
- edit mode long-press/drag/reorder/Done/gear/hide badge: not re-verified
  interactively. The automation API exposes click and drag but no separate
  pointer-down/hold/up, so attempted gestures became normal click/scroll paths;
- IME preedit/commit: not verified; direct ASCII key routing was verified, but
  the automation API did not expose an observable Windows IME composition;
- `defer_backdrop_capture` and rubber-band endpoints: not isolated on screen;
  drag rendering and snap were verified, and their code paths are unchanged.

The follow-up checks supersede the earlier statements that `codex review`,
interactive search/scroll/resize/app-launch/passthrough, and inside-frame
swallow could not be verified. Settings/edit-mode/IME remain explicitly
unverified rather than being claimed complete.

Phase 7/8 follow-ups:

- Move `AppId` to `domain/` (Phase 7).
- Route bottom-control / gear glass through `prepare` once their geometry is
  expressible as renderer-neutral `GlassSurface`s in a layout pass (the morph
  state machine + measured text widths currently live app-side).
- Route grid tile/icon/text-label instance construction through `prepare`
  once spring positions and icon UVs are model-carried.
- Move edit-badge wobble fully into the shader (eliminate the per-frame
  `update_edit_badges` CPU geometry path).
- Route settings ink/text through `prepare` once the renderer can build
  `ControlInstance`/`GlyphQuad` bytes from renderer-neutral primitives
  without importing feature semantics.

### 2026-07-11: Phase 6.5, Architecture Boundary Closure

Files changed (per commit on branch `phase-6.5-architecture-closure`):

**Commit 1: Relocate domain/platform/worker modules (6.5C/1)**
- `src/app_id.rs` → `src/domain/app_id.rs`
- `src/app_diff.rs` → `src/domain/app_diff.rs`
- `src/settings.rs` → `src/domain/settings.rs`
- `src/app_registry.rs` → `src/domain/app_registry.rs`
- `src/platform_windows.rs` → `src/platform/windows.rs`
- `src/launch.rs` → `src/platform/launch.rs`
- `src/icon_worker.rs` → `src/workers/icon_worker.rs`
- `src/refresh_watcher.rs` → `src/workers/refresh_watcher.rs`
- `src/app_scan.rs` → `src/workers/app_scan.rs`
- `src/lib.rs`: now exposes `pub mod domain` alongside `layout` and `ui_model`.
- `src/ui_model/geometry.rs`: `UvRect` moved here from `src/icons/mod.rs`
  (renderer-neutral data). `icons/mod.rs` re-exports it for compatibility.
- `src/main.rs`: mod declarations updated; `mod domain` added.

**Commit 2: Relocate bottom_control/text/icon modules (6.5C/2)**
- `src/bottom_control.rs` → `src/features/bottom_control/mod.rs`
- `src/text.rs` → `src/renderer/text_engine.rs`
- `src/icon_pipeline.rs` → `src/renderer/icon_pipeline.rs`
- `src/icon_atlas.rs` → `src/renderer/icon_atlas.rs`

**Commit 3: Close production app action dispatch (6.5A)**
- `src/app/action.rs` (new): `AppAction`, `KeyAction`, `PressAction`,
  `ReleaseAction` enums + pure classifiers (`keyboard_action`,
  `pointer_press_action`, `pointer_release_action`) + `App::handle_action`
  dispatch.
- `src/app/input.rs` (deleted): `KeyboardRoute`/`PressRoute`/`ReleaseRoute`
  replaced by `AppAction`.
- `src/app/handler.rs`: rewritten as a thin adapter — raw event → AppAction →
  `handle_action`. No inline feature calls, platform calls, or launches.
- `src/app/event.rs`: Route enums removed; `AppCommand` retained.
- App launch now routes through `AppCommand::LaunchApp` (no inline
  `open_shortcut`). hide/summon/passthrough/persist/reset all flow through
  `execute_command`.

**Commit 4: Split app/render.rs into feature submodules (6.5B/1)**
- `src/app/render.rs` (1794 lines) → `src/app/render/{mod,controls,grid,
  icons,settings,helpers}.rs`. Each sub-module owns one feature's adapter
  logic in its own `impl App` block.

**Commit 5: Move shader-facing GPU structs into renderer (6.5D/1)**
- `TileInstance`: moved from `src/grid.rs` to `src/renderer/tiles.rs`.
  `grid.rs` re-exports for compatibility.
- `ControlInstance` + `KIND_*` constants: moved from
  `src/features/bottom_control/mod.rs` to `src/renderer/controls.rs`.
  `features/bottom_control` re-exports for compatibility.
- `renderer/init.rs`, `renderer/mod.rs`, `renderer/frame.rs`,
  `renderer/badges.rs`: all `InstanceBuffer<TileInstance/ControlInstance>`
  fields now reference `crate::renderer::tiles::TileInstance` and
  `crate::renderer::controls::ControlInstance` directly.

**Commit 6: codex review fixes + architecture tests (6.5E)**
- Fixed two actionable findings from `codex review --base main`:
  - [P2] `pressed_on_control` was not reset after a control click release.
  - [P3] Settings overlay closed on mismatched press/release (now ignored).
- `tests/architecture_boundaries.rs` (new): 4 boundary tests.

What changed (Phase 6.5 summary):

- **Production AppAction/AppCommand flow**: the handler is now a thin adapter.
  Every raw `WindowEvent`/`UserEvent` is normalized into an `AppAction` and
  dispatched through `App::handle_action`. Side effects (hide, launch,
  passthrough, summon, persist, reset, redraw) all run through
  `execute_command(AppCommand)`. No inline app launch, hide, or passthrough
  remains in the handler.
- **Source ownership**: domain (app_id, app_diff, settings, app_registry) is
  library-public; platform (windows, launch) and workers (icon_worker,
  refresh_watcher, app_scan) are bin-only under their target directories.
  `UvRect` moved from `icons/` to `ui_model::geometry`.
- **Feature/renderer split**: `bottom_control` is under `features/`;
  `text_engine`, `icon_pipeline`, `icon_atlas` are under `renderer/`.
- **Shader-facing struct ownership**: `TileInstance` and `ControlInstance`
  (the two GPU `#[repr(C)]` instance structs that were in feature/bin modules)
  are now defined inside the renderer facade. `GlyphQuad` and `IconInstance`
  were already renderer-owned.
- **app/render.rs split**: the 1794-line monolith is now 6 feature-focused
  files (controls 381, settings 666, grid 243, icons 493, helpers 44, mod 22).

Adapters left in place (Phase 7+ follow-up):

- **glass prepare consolidation**: the overlay glass (control + gear) and base
  glass (page frame) still flow through `set_overlay_glass` and
  `rebuild_instances`, not through `prepare(&RenderModel)`. Only the settings
  modal glass uses `prepare`. Routing all glass through `prepare` requires
  adding `shape_type` (SHAPE_FIXED/SCROLLING/CONTROL/CLIP_ONLY) to the
  renderer-neutral `GlassSurface`/`GlassMaterial`, which risks the Liquid
  Glass SDF merge behavior. Deferred to Phase 7.
- **instance prepare consolidation**: tile/icon/control/text instance setters
  (`set_tile_instances`, `set_icon_instances`, `set_control_instances`, etc.)
  remain public because `app/render/` builds the GPU-facing instance bytes
  (spring positions, icon UVs, control KIND encoding, cosmic-text glyph
  quads). Moving these into `prepare` requires renderer-neutral
  `TileAnimView`/`ControlKind` primitives in `ui_model` + renderer-internal
  instance construction. Deferred to Phase 7.
- **`crate::grid::{GridApp, TileAnim}` dependency**: the renderer still
  imports `GridApp` and `TileAnim` from `crate::grid` (the bin adapter). These
  are renderer-neutral types that should live in `layout::grid` or
  `ui_model`, but moving them is a larger refactor. `crate::grid` is not a
  feature module, so this does not violate the "renderer → features"
  forbidden dependency.
- **`liquid_glass/renderer.rs` split**: the 1667-file Liquid Glass renderer
  is still monolithic. Splitting it into resources/pipeline/passes/shapes/frame
  sub-modules is deferred to Phase 7 (the resource-factory free functions are
  the cleanest extraction boundary).

Behavior preservation:

- Every commit is behavior-preserving. Module moves, path renames, type
  relocations, and the AppAction dispatch rewrite preserve the exact branch
  order, side-effect sequence, and timing of the historical handler.
- Keyboard/IME/pointer precedence, long-press threshold, drag promotion,
  pending-press launch, click passthrough, modal-dismiss-without-passthrough,
  and hide-before-launch ordering are all preserved (pinned by the 28 action
  tests + the architecture boundary tests).
- No `#[repr(C)]` field order, vertex layout, bind group layout, shader, or
  draw-pass order was changed.

Validation:

- `cargo fmt` / `cargo fmt --check`: passed
- `cargo test`: 146 lib + 339 bin + 4 architecture + 2 WGSL validation = 491
  tests, all passed
- `cargo clippy --all-targets --all-features`: passed (no warnings)
- `cargo build --release`: passed
- `codex review --base main -c 'model="gpt-5.5"'`: completed. Initial run
  found 2 actionable findings (stale `pressed_on_control`, settings dismiss
  on mismatched release); both fixed. Re-run: "no discrete introduced bug
  that would warrant an actionable review finding."
- GPU self-capture (`LAUNCHPAD_ALLOW_SCREENSHOT=1` + `LAUNCHPAD_QA_SHOT_FILE`):
  first frame non-blank (1920×1200, 57.1% non-transparent, 3320 unique
  colors). Second frame also non-blank (4658 unique colors), confirming
  active rendering with no stale/blank frames.

Screen verification:

- Launched with `cargo run --release` (release exe + documented screenshot
  environment): yes
- First frame non-blank: yes (GPU self-capture; Liquid Glass page frame +
  tile grid + icons + labels + bottom control capsule all rendered)
- Resize / DPI-sensitive layout: not verified on screen this phase; DPI
  geometry scaling and the render path are unchanged code, covered by
  existing unit tests.
- Scroll / snap / rubber-band: not verified on screen; `scroll.rs` physics
  unchanged.
- Search / filtering / IME: not verified on screen; bottom-control state
  machine and IME routing are unchanged code, now routed through AppAction.
  Keyboard precedence covered by the new action tests.
- Edit mode (long-press / drag / wiggle / badges / gear): not verified on
  screen; edit-mode paths now route through AppAction + AppCommand but the
  feature logic (`features::edit_mode`) is unchanged.
- Settings overlay (open/close/category/toggle): not verified on screen;
  settings code paths are unchanged code. The codex-review fix preserves the
  historical outside-press/outside-release dismiss behavior.
- Icons / labels / launch hit targets: icons and labels verified in the
  first-frame captures; launch hit targets are unchanged code covered by the
  pending_press tests.
- Click passthrough / transparent-area vs frame-empty: not verified on
  screen; unchanged code, routed through `AppCommand::HideWithClickPassthrough`.

Phase 7/8 follow-ups carried forward:

- Move `GridApp` / `TileAnim` from `crate::grid` to `layout::grid` or
  `ui_model` so the renderer depends only on layout/ui_model, not the bin
  adapter.
- Route overlay/base glass through `prepare(&RenderModel)` once
  `shape_type` is expressible renderer-neutrally.
- Route tile/icon/control/text instances through `prepare` once
  renderer-neutral `TileAnimView`/`ControlKind` primitives exist and the
  renderer can build GPU instance bytes internally.
- Split `liquid_glass/renderer.rs` (1667 lines) into focused sub-modules.
- Move edit-badge wobble fully into the shader.

### 2026-07-12: Phase 6.5B/D Closure Follow-up

This follow-up closes the renderer-boundary items that the first Phase 6.5
report had incorrectly carried into Phase 7.

Architecture closure:

- `GridApp` and `TileAnim` moved to `ui_model::grid`; the renderer no longer
  imports the binary `grid` adapter.
- Renderer-neutral `TileView`, `IconView`, `InkView`, `GlyphView`, glass
  behavior, and named submission lanes now carry the complete production
  scene. Shader-facing `TileInstance`, `IconInstance`, `ControlInstance`,
  `GlyphQuad`, and `GlassShape` packing is renderer-internal.
- `App` owns one current `RenderModel`. Grid, icon, control, gear, settings,
  text, and all Liquid Glass lanes update that model, and `app/frame.rs` calls
  `Renderer::prepare(&RenderModel)` exactly once before each draw.
- Legacy public scene setters were removed from the `Renderer` facade.
  `prepare`, resource/atlas lifecycle adapters, `render`, resize, and QA
  capture remain public for the app boundary.
- `Renderer::prepare` compares each lane with the previously prepared model.
  Unchanged lanes skip GPU writes; capacity-managed buffers grow only when
  required. Debug counters separately track prepare calls, buffer growth,
  writes, dirty skips, atlas rebinds, full-scene rebuilds, and readbacks.
- Base, overlay, modal, and edit-badge Liquid Glass buffers are persistent.
  Ordinary frame updates use `queue.write_buffer`; resource creation is
  limited to initialization, resize, or capacity growth.
- Edit-badge wobble moved from CPU trigonometry and per-frame geometry upload
  to `shader_control.wgsl` and `liquid_glass_geometry.wgsl`. The CPU supplies
  stable badge geometry plus phase/motion data; the shared animation uniform
  drives the per-frame motion on GPU.
- `liquid_glass/renderer.rs` was split into focused frame orchestration and
  resource ownership modules. The remaining facade coordinates pipelines and
  lane state rather than rebuilding transient resources.
- Architecture tests now reject `features -> renderer/platform`,
  `layout -> renderer`, `renderer -> features/grid`, worker back-edges,
  reintroduced public scene setters, and a CPU edit-badge animation path.

CPU/GPU responsibility after closure:

- CPU: feature state updates, deterministic layout, cosmic-text shaping,
  renderer-neutral model construction, exact dirty comparison, compact GPU
  struct packing, uniform writes, and uploads only for changed lanes.
- GPU: tile springs, scroll, drag/lift, edit wiggle, edit-badge wobble, Liquid
  Glass SDF/blur/sampling, clipping, blending, and final rasterization.
- There is no CPU rasterization or CPU readback in the production frame path.
  GPU readback remains restricted to the explicit QA capture path.

Validation:

- `cargo fmt`: passed
- `cargo test`: 146 library + 345 passed / 2 ignored binary + 7 architecture
  + 2 WGSL validation tests; all required tests passed
- `cargo clippy --all-targets --all-features`: passed with no warnings
- `cargo build --release`: passed
- `cargo run --release`: launched with isolated `LOCALAPPDATA`,
  `LAUNCHPAD_ALLOW_SCREENSHOT=1`, and `LAUNCHPAD_DEBUG=1`
- `codex review --base main -c 'model="gpt-5.5"'`: attempted, but the CLI
  stopped before producing findings because the account usage limit was
  reached. A focused manual boundary/hot-path review found no actionable
  regression after the single-model submission change.

Screen verification:

- first frame non-blank: verified through Windows.Graphics.Capture; page
  glass, grid, icons, labels, and bottom control were present;
- horizontal drag, active animation, inertia, and final page snap: verified;
- search open, per-key text input, filtering to the single Blender result,
  query clear, and close: verified;
- GPU self-capture: verified by writing `target/phase65-search.png` through the
  documented trigger while the filtered frame was active;
- settings category/toggle/reset, edit-mode long press/reorder/hide/Done/gear,
  IME composition, passthrough, empty-frame swallow, and resize/DPI smoke:
  not re-verified in this follow-up. The available Windows automation API has
  no pointer-down/hold/up primitive for the edit-mode long press, and those
  items must not be claimed as visually verified.

Phase 7/8 scope is now limited to the planned launcher-item/domain conversion
and folder vertical slice. `grid.rs` remains an app/layout adapter until Phase
7, and cosmic-text/atlas upload adapters remain because they are resource
lifecycle work rather than feature-specific renderer submission APIs.

### 2026-07-12: Phase 7, Item-Based Launcher Domain

Files changed:

- `src/domain/mod.rs` — added `launcher_item`, `folders`, `launcher_state`
  submodules; updated the layer docs to describe the discovery vs. user-layout
  split.
- `src/domain/launcher_item.rs` (new) — `LauncherItem { App(AppId), Folder(FolderId) }`
  with `as_app_id`/`as_folder_id`/`is_app`/`stable_key` projections and
  `From<AppId>`/`From<FolderId>` conversions. serde-serializable.
- `src/domain/folders.rs` (new) — `FolderId` (stable, opaque, generated from a
  monotonic counter) and `Folder { id, name, children: Vec<AppId> }`.
  Children are apps only (no folder nesting in Phase 7). serde-serializable.
- `src/domain/launcher_state.rs` (new) — `LauncherState { items, folders,
  hidden_apps, customized }`. Pure operations: `from_legacy` (migration),
  `set_items`, `upsert_folder`, `remove_folder`, `hide_app`, `unhide_app`,
  `is_hidden`, `integrate_discovered_apps`, `reorder_app_items`,
  `forget_app`, `normalize`, `next_folder_counter`. serde-serializable.
- `src/domain/app_id.rs` — added `serde::Serialize`/`Deserialize` derives so
  `LauncherItem` and `LauncherState` can round-trip through JSON.
- `src/domain/app_registry.rs` — **removed** `order: Vec<AppId>`, `hidden:
  HashSet<AppId>`, `user_order_set`, and the `set_order`/`order`/`set_hidden`/
  `hidden`/`hide`/`unhide`/`is_hidden`/`reset_customization`/`reconcile_order`
  API. The registry is now purely the discovered-app dataset: records sorted by
  display name for deterministic iteration, id→index map, slot allocator.
  Added `discovered_ids`, `discovered_id_set`, `lowercased_name_of` helpers for
  `LauncherState::integrate_discovered_apps`.
- `src/icon_cache.rs` — added `get_launcher_state`/`put_launcher_state` (JSON
  blob under kv key `"launcher_state"`). On write, the legacy `app_order` and
  `hidden_ids` keys are cleared so subsequent loads read the canonical Phase 7
  format and do not migrate twice. Corrupt JSON maps to `None` (caller falls
  back to legacy migration or an empty state).
- `src/app/state.rs` — added `launcher_state: LauncherState` field;
  `grid_apps_owned`/`visible_app_ids` now iterate `launcher_state.items`
  (app items only) and filter through `launcher_state.is_hidden` + registry
  search match, instead of `registry.apps()` + `registry.is_hidden`.
- `src/app/command.rs` — `load_customization` reads `launcher_state` first,
  falls back to `LauncherState::from_legacy(app_order, hidden_ids)` when the
  Phase 7 key is absent. `persist_user_order`/`persist_hidden` now route through
  `persist_launcher_state` (unified JSON write).
- `src/app/update.rs` — sort reset ("名前順") sets `customized = false` and
  calls `sync_launcher_layout_with_registry`; reset-settings wipes
  `launcher_state`. `hide_app` calls `launcher_state.hide_app`.
  `reorder_by_index` applies `apply_reorder` result via
  `launcher_state.reorder_app_items`. Added `sync_launcher_layout_with_registry`
  (integrate discovered apps + normalize) used by discovery refresh and sort
  reset.
- `src/app/render/icons.rs` — `ingest_snapshot` and `apply_diff` call
  `sync_launcher_layout_with_registry` before `relayout` so the user layout
  reflects the current app set.
- `src/app/render/settings.rs` — hidden count now reads
  `launcher_state.hidden_apps.len()`.
- `tests/launcher_domain_integration.rs` (new) — 20 integration tests covering
  item/folder order serde round-trip, stable id references, duplicate
  normalization, deterministic new-app integration, removal/rediscovery
  preservation, launch resolution, folder child persistence, hidden-app
  carryover, legacy migration, corrupt-JSON fallback, and hide-preservation.
- `tests/architecture_boundaries.rs` — added
  `renderer_does_not_receive_domain_launcher_concepts` (renderer must not
  import `LauncherItem`/`LauncherState`/`Folder`) and
  `domain_launcher_item_and_folder_are_library_public`.
- `docs/DF_REARCHITECTURE_LOG.md` — this entry.

Domain model and invariants (Phase 7):

- `LauncherItem { App(AppId), Folder(FolderId) }` — thin id-only enum; carries
  no rediscoverable data.
- `Folder { id: FolderId, name: String, children: Vec<AppId> }` — apps only,
  no nesting.
- `LauncherState { items: Vec<LauncherItem>, folders: BTreeMap<FolderId,
  Folder>, hidden_apps: BTreeSet<AppId>, customized: bool }`.
- Invariants enforced by `normalize`:
  1. Each top-level item is unique.
  2. Each folder id in `items` exists in `folders`.
  3. Each `AppId` appears in at most one of: a top-level item, a folder child,
     `hidden_apps` (top-level wins on conflict).
  4. Folder children are deduplicated; folder nesting is impossible (children
     are `AppId`, not `LauncherItem`).

Discovery state vs. user-owned state boundary:

- `AppRegistry` owns rediscoverable records: name, link path, resolved target,
  icon state, atlas slot. Sorted by display name for deterministic iteration.
  No longer owns order/hidden/customized.
- `LauncherState` owns user layout: item order, folders, hidden apps, and the
  `customized` flag. This is the single source of truth for "what the grid
  looks like".
- `grid_apps_owned`/`visible_app_ids` read `launcher_state.items` (order) and
  join against `registry` records (name/icon). Undiscovered ids are retained as
  placeholders but skipped at render time (no record to draw).

Legacy data migration:

- On startup `load_customization` checks the `"launcher_state"` kv key first.
  If present, it deserializes directly. If absent, it reads the legacy binary
  `"app_order"` + `"hidden_ids"` keys and converts via
  `LauncherState::from_legacy`, preserving the exact order and hidden set.
- The legacy keys are kept until the first `persist_launcher_state` write
  (reorder/hide/sort-reset/reset-settings), at which point they are cleared.
  This means a user who launches once without customizing keeps their legacy
  data untouched; the migration completes lazily on the next customization.
- Corrupt JSON maps to `None`; corrupt binary maps to an empty vec. Neither
  blocks startup or wipes other settings.

App removal / rediscovery behavior:

- `integrate_discovered_apps` retains ids the user placed (top-level or folder)
  even when they are no longer in the discovered set, so a temporarily
  uninstalled app keeps its slot and reappears on re-detection exactly where
  the user left it.
- New discovered apps that are neither top-level, in a folder, nor hidden are
  appended at the tail in display-name order (deterministic iOS-like
  integration).
- Hidden apps that are no longer discovered remain hidden; a re-detection does
  not resurrect them visibly unless the user unhides.
- `registry.remove` (called by `apply_diff` for removed apps) drops the record;
  the launcher state retains the id as a placeholder. Launch resolution through
  `registry.launch_info` returns `None` for undiscovered ids, so a removed
  app cannot be launched.

What was intentionally left for Phase 8 (folder feature):

- Folder open/close, panel layout, rename, drag-to-create, drag-into-folder.
  `Folder`/`FolderId`/`upsert_folder`/`remove_folder`/`next_folder_counter`
  provide the stable domain foundation; the feature/UI layer is Phase 8.
- `reorder_app_items` currently treats the grid as apps-first then folders at
  the tail (there are no folder items in production yet). Phase 8's
  folder-aware drag will refine the interleave.
- Folder panels will be emitted as renderer-neutral `GlassSurface`/`IconView`/
  `TextView` primitives in a layout pass; the renderer already receives no
  domain concepts (asserted by the new architecture test).
- The `layout::grid` pure functions still take an `app_count`/`cell_count`
  argument. Phase 7 keeps this (the visible item count is the same number);
  Phase 8 will generalize to item-count when folder tiles render on the grid.

Validation:

- `cargo fmt --check`: passed
- `cargo test`: 168 lib + 367 bin (2 ignored) + 9 architecture + 20 launcher
  domain integration + 2 WGSL validation = 566 tests, all required tests
  passed.
- `cargo clippy --all-targets --all-features`: passed (no warnings)
- `cargo build --release`: passed
- `cargo run --release` with isolated `LOCALAPPDATA`,
  `LAUNCHPAD_ALLOW_SCREENSHOT=1`, `LAUNCHPAD_DEBUG=1`, and
  `LAUNCHPAD_QA_SHOT_FILE` GPU self-capture:
  - first frame non-blank: verified — 1920×1200, 57.1% non-transparent, 3323
    unique colors, Liquid Glass page-frame tint at center (≈184,170,164),
    matching the Phase 6.5 baseline pixel stats.
  - legacy migration: copied the user's real DB (172 apps, legacy
    `app_order` + `hidden_ids`) to the isolated `LOCALAPPDATA`, launched, and
    captured a migrated frame — 57.1% non-transparent, 3414 unique colors,
    visually identical to the non-migrated first frame. The grid renders the
    migrated app order without loss.
  - transparent-area click passthrough: verified via debug log — a click on the
    transparent area logged `outside_glass=true`, hid the launcher, and
    replayed the click to the underlying window.
  - app discovery: verified — 172 apps ingested into the icon cache; the grid
    renders tiles, icons, and labels.

Screen verification:

- Launched with `cargo run --release` (release exe + documented screenshot
  environment): yes
- First frame non-blank: yes (GPU self-capture; Liquid Glass page frame + tile
  grid + icons + labels + bottom control capsule all rendered; pixel stats
  match Phase 6.5 baseline)
- Legacy data migration renders correctly: yes (GPU self-capture after loading
  the user's real legacy DB; grid content identical to non-migrated frame)
- Transparent-area click passthrough: yes (debug log confirms
  `outside_glass=true` → hide + click replay)
- App discovery: yes (172 apps ingested; icon cache populated)
- Horizontal scroll / inertia / snap: not verified on screen — the sandbox's
  foreground lock and the automation API's lack of a pointer-down/hold/up
  primitive prevented interactive scroll simulation. `scroll.rs` physics and
  the scroller wiring are unchanged code; the launcher-state change only
  affects which app ids feed the grid, not scroll bounds (page count derives
  from `grid_apps_owned().len()`, which is the same visible-app count as
  before).
- Search / filtering: not verified on screen (same interactive blocker). The
  search filter path now reads `launcher_state.items` + `matches_search`, which
  is covered by the unchanged `matches_search` tests and the new
  `temporary_undiscovery_preserves_order_and_rediscovery_restores` integration
  test.
- Edit mode (long-press / drag / reorder / hide / Done / gear): not verified
  on screen (same interactive blocker; the automation API has no
  pointer-down/hold/up for the long-press gesture). The reorder order
  computation (`apply_reorder`), the hide path (`launcher_state.hide_app`),
  and the commit sequence are covered by deterministic tests.
- Settings overlay (open/close/category/toggle/reset): not verified on screen;
  settings code paths route through `launcher_state` now (sort reset, hidden
  count, reset-settings) but the panel geometry and hit-testing are unchanged
  code.
- App click launch resolution (visual): not verified on screen — the
  foreground lock and the need for sub-pixel-accurate tile-center clicks in a
  scaled-DPI window prevented a reliable launch capture. Launch resolution
  (`resolve_clicked_app` → `visible_ids.get(idx)` → `registry.launch_info`)
  is covered by the `app_item_resolves_to_correct_discovered_app` and
  `undiscovered_app_id_does_not_resolve_to_launch_info` integration tests.
- Refresh-preserves-order (visual): not verified on screen (requires observing
  in-process state across a refresh tick). Covered by the
  `temporary_undiscovery_preserves_order_and_rediscovery_restores` and
  `integrate_discovered_apps_is_idempotent` integration tests.

Notes and discoveries:

- The legacy `AppRegistry` owned both discovered records and user layout
  (order/hidden/user_order_set). Phase 7 separates them: the registry is now
  discovery-only, and `LauncherState` owns the layout. This required touching
  `load_customization`, `persist_*`, `grid_apps_owned`, `visible_app_ids`,
  `hide_app`, `reorder_by_index`, sort reset, reset-settings, hidden count,
  `ingest_snapshot`, and `apply_diff`. Each call site now goes through
  `launcher_state` instead of `registry`.
- `sync_launcher_layout_with_registry` is the single integration point between
  discovery and layout: it calls `integrate_discovered_apps` (retains user
  positions, appends new apps at the tail) then `normalize` (dedup, prune
  orphan folders, enforce the one-place-per-app rule). It is called after
  initial scan, refresh diff, sort reset, and reset-settings.
- `AppRegistry::apps()` now always returns records in display-name order
  (there is no user-order override). `LauncherState::integrate_discovered_apps`
  with `customized = false` reproduces the legacy name-sorted grid by building
  the item list from a name sort of discovered apps.
- The `LauncherState` JSON is compact (~100 bytes per item for app items).
  The legacy binary `app_order` was ~90 bytes per app; the JSON is larger but
  human-debuggable and schema-flexible for future folder data.
- The Phase 7 change does not alter the renderer, shaders, GPU instance
  layouts, draw-pass order, scroll physics, bottom-control state machine, IME,
  or text rendering. It only changes which app ids feed the grid and where
  order/hidden/folder state lives.

codex review and follow-up corrections (same Phase 7 PR):

`codex review --base main -c 'model="gpt-5.5"'` was run iteratively after the
initial commit. It found four hidden-app / placeholder regressions, all fixed
and re-validated. Final review: "The changes cleanly separate discovery state
from launcher layout state and the affected paths are covered by tests. I did
not identify any discrete correctness issue that would break existing
behavior."

- [P1] `integrate_discovered_apps` `customized=false` branch rebuilt `items`
  from all discovered apps including hidden ones, which `normalize` then
  promoted to top-level (silently un-hiding them). Fixed by excluding hidden
  apps from the name-sort rebuild. Test: `integrate_not_customized_keeps_
  hidden_apps_hidden`.
- [P2-search] `search_includes_hidden` searched `launcher_state.items`, but
  `hide_app` removes hidden apps from `items`, so they could never be found.
  Added `ordered_visible_candidate_ids` which appends discovered hidden ids
  (not already in `items`) to the candidate list when `include_hidden` is true.
- [P2-normalize] `normalize` let top-level items win over `hidden_apps`, so a
  hidden id left in `items` by a legacy migration (old hide kept hidden ids in
  the order tail) or by a reorder (`apply_reorder` returns visible+hidden) was
  silently un-hidden. Root-cause fix: invert the rule — hidden wins. `normalize`
  removes hidden app ids from `items` and folder `children`, keeping them in
  `hidden_apps`. Invariant 3 and its tests were rewritten.
- [P2-reorder] `reorder_app_items` rebuilt `items` solely from
  `visible_app_order`, dropping undiscovered placeholder apps on every reorder.
  Fixed by retaining app items not in the visible order (and not hidden) as
  stable placeholders. Test: `reorder_preserves_undiscovered_placeholders`.
- [P2-cross-placement] `normalize` deduplicated within top-level items and
  within folder children but never removed an app that appeared in both.
  Added invariant 4 (top-level wins over folder child) and the enforcement
  step. Test: `normalize_removes_cross_placement_duplicates`.

Final validation after all corrections:

- `cargo fmt --check`: passed
- `cargo test`: 173 lib + 372 bin (2 ignored) + 9 architecture + 22 launcher
  domain integration + 2 WGSL validation = 578 tests, all required passed.
- `cargo clippy --all-targets --all-features`: passed (no warnings)
- `cargo build --release`: passed
- `codex review --base main -c 'model="gpt-5.5"'`: no actionable findings.

## 2026-07-13 — Phase 8: Folder Feature Vertical Slice

Phase 8 is complete. Folders now exercise the intended domain -> feature ->
layout/UI model -> renderer boundary end to end without introducing folder
semantics into the renderer.

Files and boundaries:

- `src/domain/launcher_state.rs` now provides stable item reorder,
  app-on-app folder creation, top-level-to-folder moves, child reorder,
  folder-to-folder moves, child-to-top-level moves, and deterministic
  one/zero-child dissolution. Hidden folder children use the same dissolution
  rule.
- `src/features/folders/mod.rs` is the folder feature state machine: reversible
  dt-based spring, hover thresholds, presentation-only pending merge preview,
  rename/IME editor, and child-drag preview state. Domain data is mutated only
  at drop/commit boundaries.
- `src/layout/folder_panel.rs` is pure geometry for dynamic 3-column panels,
  centered incomplete rows, nine items per page, viewport/DPI clamping,
  source-to-panel container morph, child mini-to-cell trajectories, generic UI
  primitives, and a hit map built from the same rectangles.
- `src/app/render/folders.rs` and the app shell resolve current domain records,
  current tile springs, current scroll, atlas UVs, and pointer actions into the
  feature/layout inputs. Search deliberately uses a separate flat app-only
  projection.
- `src/grid.rs` emits item-based top-level tiles and ordered 3x3 folder mini
  previews. `src/ui_model/*` and renderer preparation/frame modules gained
  generic modal tile/icon/text/backdrop lanes.
- `src/shader.wgsl` and `src/shader_icon.wgsl` gained only generic fixed-screen
  and no-badge flags. Architecture tests continue to reject domain folder
  imports from renderer code.

Behavior and policy decisions:

- New-folder hover may animate a panel before drop, but no folder is inserted
  into `LauncherState` until the drop succeeds. The committed folder inherits
  the preview's motion progress instead of restarting.
- Existing-folder hover remains stable while the pointer moves from the source
  tile into the opened panel. The lifted app is resubmitted through generic
  modal tile/icon lanes above the glass, remains pointer-attached over child
  targets, and is added to the durable folder only on release.
- Child order is stable `AppId` order. New folders start with target then
  dragged; all later moves preserve unaffected relative order, including
  undiscovered placeholder children. Normal top-level live reorder pauses while
  a stable app/folder hover target is active so it cannot reset the formation
  timer by moving the dragged item into the target cell.
- `normalize` applies the same zero/one-child dissolve policy after repairing
  hidden, duplicate, or corrupt persisted membership, so undersized folders
  cannot survive a restart as invalid durable containers.
- The folder source rectangle is resolved from the latest grid layout and tile
  spring every frame, so close remains spatially connected after layout or
  scroll changes.
- Modal input has precedence over the grid. Outside clicks dismiss without
  click replay. `Esc` cancels rename before it closes the panel.
- Rename is UTF-8-safe and IME-aware; a blank commit becomes `フォルダ`.
- More than nine visible children are paginated. Hidden/undiscovered children
  are omitted from both closed previews and open panel cells.
- Opening settings or hiding/resetting the launcher clears folder presentation
  state immediately. Open-idle panels do not request continuous redraw.

Screen Verification Gate (release build, isolated temporary
`LOCALAPPDATA`, `LAUNCHPAD_ALLOW_SCREENSHOT=1`, GPU self-capture enabled):

- Verified initial app/folder interleave, ordered closed 3x3 preview, an
  11-child paginated panel, a centered 3-child single-row panel, long-title
  ellipsis, page navigation, and page indicator.
- Verified production Liquid Glass panel/backdrop rendering, modal outside
  dismissal with no passthrough, and app-only flat search presentation.
- Captured opening and closing in five roughly 105 ms frames. The container
  rect/radius and child icons morph between the latest folder-tile geometry and
  panel cells, while dimming/refraction fades with the same progress.
- Captured an opening interrupted by `Esc`; the same spring reversed from its
  in-flight value and settled closed without an endpoint reset.
- Verified rename commit persistence through `launcher_state`, stopped and
  restarted the release process with the same temporary cache, and confirmed
  the persisted folder name, child order, and top-level item placement on
  screen. Rename cancellation with `Esc` left that committed value unchanged.
  The automation bridge could not emit real Japanese
  IME commits, so Japanese preedit/commit and UTF-8 editing are covered by
  deterministic feature tests rather than claimed as screen-verified.
- Not screen-verified: long-press drag-to-create, drag-into-folder, child live
  reorder, cross-folder move, child drag-out/dissolution, app launch, and
  resize/DPI transitions. The available desktop automation exposes an atomic
  drag but not the required long-press/hold/move/release sequence. These paths
  are covered by domain, feature, layout, input, and architecture tests.

Final validation:

- `cargo fmt --check`: passed.
- `cargo test`: 193 library + 408 app (2 ignored) + 9 architecture + 22 domain
  integration + 2 WGSL validation = 634 total, 632 passed and 2 ignored.
- `cargo clippy --all-targets --all-features`: passed with no warnings.
- `cargo build --release`: passed.


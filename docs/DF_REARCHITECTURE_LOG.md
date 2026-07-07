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

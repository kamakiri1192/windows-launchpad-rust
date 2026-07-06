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

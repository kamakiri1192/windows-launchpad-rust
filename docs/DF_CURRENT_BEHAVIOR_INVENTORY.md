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

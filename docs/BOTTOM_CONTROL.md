# Bottom-center morphing control (search pill ↔ page indicator ↔ search field)

The Launchpad shows a single iOS-style control pinned to the bottom center of
the window. Instead of a permanent page indicator, it **morphs** between three
visuals depending on what the user is doing, all rendered as the *same* Liquid
Glass capsule with the contents swapped on top.

## The three visuals

| Visual | When | Contents |
|--------|------|----------|
| **Search pill** | Default (idle) | 🔍 magnifier + 「検索」 label |
| **Page indicator** | Briefly, right after a page change | A row of dots; the current page is bright white, the rest are translucent |
| **Search field** | After clicking the pill | 🔍 + text input + close (×); the capsule expands sideways |

## State machine

```
                  startup
                     │
                     ▼
            ┌─────────────────┐
            │  IdleSearchPill │ ◄──────────────────────┐
            └────────┬────────┘                        │
       page change   │            click pill           │
            ┌────────▼────────┐              ┌─────────┴────────┐
            │ TransientPage   │              │    Expanding     │
            │   Indicator     │              │ (pill → field)   │
            └────────┬────────┘              └────────┬─────────┘
            timeout  │                                ▼
            ┌────────┘                        ┌─────────────────┐
            ▼                                 │ ExpandedSearch  │
   (back to IdleSearchPill)                   │     Field       │
                                             └────────┬─────────┘
                                          close / Esc │
                                             ┌────────▼────────┐
                                             │   Collapsing    │
                                             │ (field → pill)  │
                                             └────────┬────────┘
                                                      │
                              back to IdleSearchPill ──┘
```

Rules:

- **Page changes while the field is open are ignored.** A swipe during text
  entry does not yank focus to the indicator.
- The page indicator retires to the search pill automatically after ~1.8 s.
- The pill → field morph is an eased (~300 ms) horizontal expansion; the field
  → pill collapse is ~240 ms. The magnifier, label/dots/query, and close
  button cross-fade on top of the continuously resizing capsule.

## How it is built

The control is a dedicated component in `src/bottom_control.rs`
(`BottomControl`) that owns its mode, animation progress, indicator timer,
caret blink phase, and query text. It exposes:

- `on_page_change` — arm the transient indicator.
- `open_search` / `close_search` / `press_close` — field open/close.
- `handle_char` / `handle_backspace` / `handle_left` / `handle_right` /
  `handle_escape` — keyboard input while the field is focused.
- `tick(now, dt)` — advance animations/timers; returns whether it still needs
  more frames.
- `resolve(viewport, frame_bottom, page, page_count)` — produce the capsule
  geometry + the active content layers for one frame.
- `hit_test` / `close_button_x` — pointer hit-testing for clicks.

The renderer pieces:

- **Glass capsule** — drawn by the existing Liquid Glass pass. The capsule is a
  `GlassShape` of kind `control` (`shape_type == 2` in
  `assets/shaders/liquid_glass_geometry.wgsl`). This shape kind is **not**
  clipped to the page frame, so the control renders correctly below the frame.
- **Foreground ink** (magnifier, dots, caret, close ×) — a procedural SDF
  shader, `src/shader_control.wgsl`, drawn via a dedicated instance pipeline.
  One instance per element; the fragment shader picks the shape by `kind`.
- **Text** (label / query / placeholder) — `src/shader_control_text.wgsl`, a
  frame-clip-free variant of the label text shader, sampling the shared
  cosmic-text glyph atlas. Text is laid out by
  `TextRenderer::layout_centered_line`.

`main.rs` wires the component into the event loop:

- `RedrawRequested`: tick the scroller, detect a settled page change, tick the
  control, upload the capsule + overlays + text, render, and keep redrawing
  while either is animating.
- `MouseInput`: a press that starts on the capsule is tracked as a control
  click (not a scroll drag); a release on the capsule opens/closes the field or
  presses the close button.
- `KeyboardInput` / `Ime`: while the field is focused, the control consumes
  Backspace, ←, →, and Esc (close, not quit), and appends printable text /
  IME commits to the query.

## Interaction summary

| Input | Action |
|-------|--------|
| Click the search pill | Expand into the search field |
| Click the close (×) in the field | Clear the query and collapse back to the pill |
| `Esc` while the field is open | Collapse to the pill (does **not** quit) |
| Type | Append to the query (IME supported via `Ime::Commit`) |
| `Backspace` | Delete the character before the caret |
| `←` / `→` | Move the caret one character |
| Swipe / page change | Briefly show the page indicator, then return to the pill |

## Edit mode: Done + Settings gear

While the launcher is in edit mode (icons wiggling after a long-press), the
control shrinks to the Done capsule and a **second glass capsule** — a circular
settings gear — appears beside it:

```
        ┌──────┐
        │ 完了 │  ⚙
        └──────┘
```

- The Done capsule slides left so the pair stays centered.
- The gear capsule is a separate `GlassShape` (`edit_gear_geometry` in
  `bottom_control.rs`) rendered through the same `gear_shape` glass pass used
  by no other state, with a procedural gear glyph (`KIND_GEAR`).
- Both fade in/out together via `edit_visual_progress`.
- Clicking the Done capsule exits edit mode; clicking the gear opens the
  settings overlay (and exits edit mode first). Hit-testing is split inside
  `handle_control_click`.

## Resize behavior

The capsule is recomputed every frame from the current viewport and the
fixed page-frame's bottom edge (`GridLayout::frame_panel_rect`), so it stays
centered and correctly positioned through window resizes and DPI changes.

# Key Bindings

The app exposes a number of debug / tuning keys for inspecting and adjusting
the Liquid Glass effect at runtime. Press the key while the window has focus.

## Window & lifecycle

The launcher is a **resident** app: closing its window (Esc, clicking another
window, launching an app) only *hides* it — the process stays alive so it can
be summoned again instantly. Real quit happens via the tray icon.

| Key / Input | Action |
|-----|--------|
| `Win+Space` | **Summon** the launcher from anywhere (suppresses the system IME switch on this combo via a low-level keyboard hook) |
| `Esc` | Hide the launcher (stay resident). If the search field is open, the first Esc just closes the field. |
| Focus loss (clicking another window, Alt-Tab) | Auto-hide the launcher |
| Launch an app (click its tile) | Launch the app and hide the launcher |
| Tray icon → left click | Summon |
| Tray icon → right click → 表示 (Show) | Summon |
| Tray icon → right click → 設定 (Settings) | Open the settings overlay (summons first if hidden) |
| Tray icon → right click → 終了 (Quit) | **Really quit** the app |
| `M`   | Toggle the OS window frame on/off (borderless by default; bring the title bar + resize edges back for debugging) |
| `R`   | Clear the icon cache and re-extract every icon live (recover from a corrupted/stale cache without restarting) |

### How the hot key works

`Win+Space` is captured by a `WH_KEYBOARD_LL` hook installed on a dedicated
OS-integration thread (`src/platform_windows.rs`). When the combo is detected
the hook swallows the keystroke (`return 1`) so Windows never sees it — this
is what suppresses the IME-switch behavior on that combo — and posts a
`UserEvent::Summon` to the winit event loop. The hook callback does only the
state read + one `send_event` and returns, so it stays well under
`LowLevelHooksTimeout`. Auto-repeat is suppressed (one summon per press).

## Icon cache reset (CLI)

You can also wipe the on-disk cache before launch — handy if the launcher
won't start cleanly or you want a guaranteed cold extraction:

```
cargo run --release -- --reset-cache
```

`--reset-cache` deletes `%LOCALAPPDATA%\Launchpad\cache.sqlite3` (and its
WAL/SHM sidecars) before the cache is opened, so the next launch rebuilds it
from scratch. The `R` key does the equivalent at runtime without restarting.

## Scrolling

| Input | Action |
|-------|--------|
| Left-drag (horizontal) | Page swipe with rubber-band + spring snap |

## Bottom-center control (search pill / page indicator / search field)

The bottom-center control morphs between a search pill, a transient page
indicator, and a search field. See [BOTTOM_CONTROL.md](BOTTOM_CONTROL.md) for
the full state machine.

| Input | Action |
|-------|--------|
| Click the search pill | Expand into the search field |
| Click the close (×) in the field | Clear the query and collapse back to the pill |
| Type | Append to the query (IME supported) |
| `Backspace` | Delete the character before the caret |
| `←` / `→` | Move the caret one character |
| `Esc` (while the field is open) | Collapse to the pill (does **not** quit) |
| Page change (swipe) | Briefly show the page indicator, then return to the pill |

## Edit mode (drag-to-reorder)

Enter edit mode by long-pressing an app icon. While wiggling, a `[完了] [⚙]`
pair appears at the bottom center.

| Input | Action |
|-------|--------|
| Long-press an icon | Enter edit mode (icons wiggle) |
| Drag an icon | Reorder across pages |
| Click an icon's ✕ badge | Hide that app from the grid |
| Click `完了` (Done) | Exit edit mode (persist reorder) |
| Click `⚙` (gear) | Open the settings overlay |
| `Esc` / click empty space | Exit edit mode |

## Liquid Glass master switch

| Key | Action |
|-----|--------|
| `V` | Toggle Liquid Glass rendering on/off |

## Liquid Glass parameters

All parameters are adjusted live and the new value is logged to stderr.

| Key  | Parameter              | Range        |
|------|------------------------|--------------|
| `1`  | thickness −            | 6.0 .. 48.0  |
| `2`  | thickness +            | 6.0 .. 48.0  |
| `3`  | refractive_index −     | 1.02 .. 1.75 |
| `4`  | refractive_index +     | 1.02 .. 1.75 |
| `5`  | saturation −           | 0.5 .. 2.0   |
| `6`  | saturation +           | 0.5 .. 2.0   |
| `7`  | chromatic_aberration − | 0.0 .. 0.18  |
| `8`  | chromatic_aberration + | 0.0 .. 0.18  |
| `9`  | blur_radius −          | 0.0 .. 40.0  |
| `0`  | blur_radius +          | 0.0 .. 40.0  |

### `blur_radius` → pyramid depth

The blur runs a dual-Kawase pyramid whose depth is derived from the radius,
so weak blurs stay cheap and large radii stay smooth:

| `blur_radius` | Pyramid levels (down + up) | Effective levels |
|---------------|----------------------------|------------------|
| `< 6.0`       | 1                          | 1/2              |
| `6.0 .. 16.0` | 2                          | 1/2 → 1/4        |
| `>= 16.0`     | 3                          | 1/2 → 1/4 → 1/8  |

## Liquid Glass debug views

These overlay / isolate intermediate textures so you can tell capture problems
from shader problems when the glass looks wrong.

| Key | Flag | What it shows |
|-----|------|---------------|
| `B` | show_backdrop_texture        | The raw captured backdrop, fullscreen opaque; transparent areas are intentionally filled |
| `G` | show_geometry_texture        | The geometry texture RGB (displacement XY + height) |
| `D` | show_displacement            | The displacement vectors (R, G, 0.5) |
| `A` | show_alpha_mask              | The glass alpha mask as grayscale |
| `F` | show_final_glass_only        | The final glass render only (no tiles/text on top) |
| `C` | disable_chromatic_aberration | Turn chromatic aberration off |
| `E` | disable_edge_lighting        | Turn edge lighting / specular off |
| `L` | disable_blur                 | Turn the blur pyramid off (final samples the sharp backdrop) |
| `W` | force_white_backdrop         | Ignore capture and feed a solid white backdrop into Liquid Glass |

Debug flags are bit-packed into `debug_flags` in the stderr log (bit 0 = `B`,
bit 1 = `G`, ... bit 7 = `L`). `W` is logged separately as
`white_backdrop=true`.

## Transparency notes (Windows)

Real per-pixel transparency requires three things working together:

1. **`with_transparent(true)`** on the winit window attributes.
2. **`with_no_redirection_bitmap(true)`** — sets `WS_EX_NOREDIRECTIONBITMAP` at
   window creation so the DWM drops the classic HWND back buffer. Without this,
   alpha=0 pixels are filled with the window's white background brush and the
   "transparent" areas read as solid white. This flag can only be set at
   creation time, not added later via `SetWindowLongPtrW` (it fails with
   `ERROR_INVALID_PARAMETER`).
3. **`Dx12SwapchainKind::DxgiFromVisual`** (DirectComposition) as the DX12
   presentation system in wgpu's `Dx12BackendOptions`. The default
   `DxgiFromHwnd` swapchain advertises only `Opaque` alpha modes and cannot
   carry alpha to the DWM.

With all three set, the surface reports `PreMultiplied` alpha and the glass
shape's outside areas become truly see-through.

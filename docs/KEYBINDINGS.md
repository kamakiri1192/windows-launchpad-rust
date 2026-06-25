# Key Bindings

The app exposes a number of debug / tuning keys for inspecting and adjusting
the Liquid Glass effect at runtime. Press the key while the window has focus.

## Window

| Key | Action |
|-----|--------|
| `Esc` | Quit the app |
| `M`   | Toggle the OS window frame on/off (borderless by default; bring the title bar + resize edges back for debugging) |

## Scrolling

| Input | Action |
|-------|--------|
| Left-drag (horizontal) | Page swipe with rubber-band + spring snap |

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
| `B` | show_backdrop_texture        | The raw captured backdrop, fullscreen opaque |
| `G` | show_geometry_texture        | Signed distance and normalized height from the geometry texture |
| `D` | show_displacement            | The raw displacement vectors visualized around neutral gray |
| `A` | show_alpha_mask              | The glass inside mask derived from signed distance |
| `F` | show_final_glass_only        | The final glass render only (no tiles/text on top) |
| `C` | disable_chromatic_aberration | Turn chromatic aberration off |
| `E` | disable_edge_lighting        | Turn edge lighting / specular off |
| `L` | disable_blur                 | Turn the blur pyramid off (final samples the sharp backdrop) |
| `S` | show_shadow_only             | Isolate the adaptive outside drop shadow |
| `P` | show_spill_only              | Isolate the environmental spill outside the glass |

Debug flags are bit-packed into `debug_flags` in the stderr log (bit 0 = `B`,
bit 1 = `G`, ... bit 7 = `L`, bit 8 = `S`, bit 9 = `P`).

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

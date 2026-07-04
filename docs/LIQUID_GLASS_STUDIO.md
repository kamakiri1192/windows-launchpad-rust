# Liquid Glass Studio

`liquid_glass_studio` is a development-only simulator for tuning the shared
Liquid Glass shaders outside the launcher UI.

Run it by double-clicking:

```text
run_liquid_glass_studio.cmd
```

Or from a shell with:

```powershell
cargo run --bin liquid_glass_studio --locked
```

The simulator reuses the production shader files in `assets/shaders/`:

- `liquid_glass_geometry.wgsl`
- `liquid_glass_final.wgsl`
- `liquid_glass_blur_downsample.wgsl`
- `liquid_glass_blur_upsample.wgsl`

It supplies its own procedural backdrop and a small set of SDF shapes so merge
behavior can be tested without Start Menu scanning, icon extraction, window
capture, or the full launcher event flow.

Studio starts with the main launcher's `LiquidGlassParams::default()` values so
side-by-side comparisons begin from the production parameter set.

The backdrop can be cycled through contrast-oriented presets for checking glass
legibility over black, white, mixed tones, checker patterns, and text-heavy
content. Use the `PREV` / `NEXT` buttons in the right panel to switch presets.
On Windows, Studio uses the same transparent DirectComposition presentation path
as the main launcher so `APP MODE` comparisons exercise the same compositor path.

## Controls

- Move the mouse: drag the spring-following glass shape.
- Drag sliders in the right panel: tune thickness, IOR, chromatic aberration,
  blur, saturation, tint alpha, lighting, merge distance, spring stiffness,
  damping, and motion stretch.
- `APP RESET` button: restore the glass parameters to the main launcher's
  `LiquidGlassParams::default()` values and switch to app-composite mode.
- `PREVIEW` / `APP MODE` button: toggle whether the test backdrop is drawn as
  a visible background layer, or only used as the glass input like the main
  launcher. If the window surface cannot preserve alpha, Studio shows `APP SIM`
  and draws the selected test backdrop behind the glass to avoid black opaque
  fallback blending.
- `U`: show / hide the slider panel.
- `Space`: toggle the fixed anchor shape.
- `1` / `2`: decrease / increase glass thickness.
- `3` / `4`: decrease / increase shape merge distance.
- `5` / `6`: decrease / increase blur radius.
- `7` / `8`: decrease / increase chromatic aberration.
- `M`: restore the main launcher Liquid Glass defaults.
- `N` / `P`: next / previous backdrop preset.
- `B`: show backdrop texture.
- `G`: show geometry texture.
- `D`: show displacement texture.
- `A`: show alpha mask.
- `F`: show final glass only.
- `C`: toggle chromatic aberration.
- `L`: toggle blur.
- `R`: reset to the main launcher defaults.
- `Esc`: quit.

The shape merge behavior follows the same core idea as
`iyinchao/liquid-glass-studio`: multiple SDF shapes are evaluated in one field
and joined through smooth-min blending. This app keeps the implementation in
WGSL/wgpu so changes can be moved directly back into the launcher renderer.

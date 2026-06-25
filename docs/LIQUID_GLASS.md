# Liquid Glass Implementation Notes

The Liquid Glass effect keeps the existing wgpu DX12, DirectComposition, and
Windows.Graphics.Capture path. The current shader model is tuned around
lensing and environmental response instead of making blur the dominant cue.

## Shape model

Normal rendering now submits only the page panel as a Liquid Glass shape. Tile
halos are intentionally excluded from the always-on shape list so they can be
added later as hover or selected layers without creating SDF interference
inside the panel.

## Geometry texture

`assets/shaders/liquid_glass_geometry.wgsl` writes `Rgba16Float` data with this
layout:

| Channel | Meaning |
|---------|---------|
| R | `displacement.x` in pixels |
| G | `displacement.y` in pixels |
| B | normalized height, `0.0 .. 1.0` |
| A | signed distance in pixels |

The displacement channels are no longer encoded to `0.0 .. 1.0`; they are raw
pixel offsets. Signed distance is retained up to a 72 px outside margin so the
final pass can render adaptive shadow and environmental spill outside the glass
without adding fake geometry.

## Final composite

`assets/shaders/liquid_glass_final.wgsl` treats the sharp captured backdrop as
the primary refracted source. The blur pyramid is mixed in only as a weak frost
and scattering component. Chromatic aberration samples the sharp backdrop and
is weighted toward the rim so it stays subtle.

The composite now follows a screen-space dielectric approximation: the shader
uses Schlick Fresnel to split reflection and transmission, applies a
Beer-Lambert style thickness attenuation to the transmitted backdrop, and uses
separate entry/exit backdrop samples so the rim reads as a thick lens instead
of a flat translucent overlay. This is still not full ray-traced glass because
the app only has the captured 2D backdrop, not scene depth or a 3D environment,
but the energy split is now closer to real glass behavior.

Outside the panel, the signed distance field drives a soft adaptive drop
shadow. Surrounding color is sampled around the panel, but normal rendering
keeps that color mostly inside the glass edge and shadow tint instead of
painting a visible colored halo outside the panel. Use the `P` debug view to
inspect the spill signal in isolation.

## Capture constraints

The GPU capture path imports a shared D3D11 texture into wgpu. Its crop copy
uses `CopySubresourceRegion`, which is a copy operation only and does not
stretch. When the monitor crop size differs from the destination texture size,
the capture path now stays on GPU and uses a D3D11 VideoProcessor blit to crop
and scale into the shared texture before wgpu imports it. The CPU readback path
is reserved for actual GPU capture failures, not normal DPI or window-size
mismatches.

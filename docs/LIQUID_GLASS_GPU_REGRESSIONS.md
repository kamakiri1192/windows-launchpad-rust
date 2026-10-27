# Liquid Glass headless GPU regressions

`tools/liquid_glass_gpu_test.py` compiles and executes the production
`assets/shaders/liquid_glass_final.wgsl` vertex/fragment entry points, the completed-scene flatten shader, and the final
`src/shader_surface_present.wgsl` presentation boundary with real
wgpu render pipelines, textures, bind groups, premultiplied blending, an sRGB
render attachment, and texture readback. It does not substitute a simplified
shader or rely on text matching. The independent CPU oracle covers the flat
material equation and standard sRGB transfer function.

## Run

```sh
python -m venv /tmp/liquid-glass-test
/tmp/liquid-glass-test/bin/pip install -r tools/liquid_glass_gpu_requirements.txt
/tmp/liquid-glass-test/bin/python tools/liquid_glass_gpu_test.py --output /tmp/liquid-glass-results
```

A Vulkan-capable adapter is required. A software implementation is sufficient.
For example, if Chromium's SwiftShader is installed on Linux:

```sh
VK_ICD_FILENAMES=/usr/lib/chromium/vk_swiftshader_icd.json \
WGPU_BACKEND_TYPE=Vulkan \
/tmp/liquid-glass-test/bin/python tools/liquid_glass_gpu_test.py
```

The harness reports the actual adapter. Adapter/device/shader creation failure
is an error, not a skipped passing test. `--record` is explicitly diagnostic:
it prints observed and expected values without asserting equality, and must
not be reported as a regression pass. `--shader PATH --record` allows a saved
pre-change source to be rendered with the same bindings and fixtures.

Optional output contains JSON byte values and PPM images. Keep generated output
outside the repository; it is not a source dependency. Numeric comparisons allow
a two-byte tolerance for attachment quantization and backend transfer-function
rounding.

## Compositing contract

Captured and blurred SDR backdrops retain their existing encoded-sRGB material
math. Glass decodes the finished material color before multiplying by shape
coverage and presentation opacity. In-app targets use linear-premultiplied
source-over. Completed-scene flattening samples that linear scene, decodes the
desktop, composites once, and encodes the opaque result for the blur/material
pipeline. Toggle lenses likewise decode only their desktop fallback.

Base panels replace the captured desktop only after
an actual frame has arrived; resize resets this availability. Initial fallback
pixels keep the prior translucent behavior. The search/page/Done bottom bar uses its own pre-bar completed-scene
checkpoint and replaces that filtered backdrop when capture is ready. Gear,
interaction outlines, grid/drag overlays, and badges retain their material policies: their desktop-only sources do not contain all lower
in-app content, so forcing them opaque would punch through that content. Menu/settings transition replacement values and tint
defaults are unchanged.

One final fullscreen pass converts the completed linear-premultiplied scene
to the explicitly selected OS alpha convention. No later glass layer samples
this converted surface. This adds one viewport-sized texture and fullscreen
pass; native GPU performance remains to be measured. Auto/Inherit/Opaque retain
the prior attachment behavior because they do not specify the same transparent
stored-channel contract as PreMultiplied/PostMultiplied.

## Coverage and limitations

The 65 readback fixtures include a grayscale ramp, saturated and mixed colors, direct capture
view, filtered-versus-sharp capture, opaque replacement over different lower
colors, uncovered pixels, translucent no-capture fallback, partial edge coverage, presentation fade, the rim
branch, and the settings toggle's opaque/transparent/partially covered scene. Four
completed-scene fixtures also pass through nested glass. Presentation coverage
checks PreMultiplied and PostMultiplied output, sRGB and Unorm attachments,
alpha 0/0.25/0.5/1, and gray/color inputs. An end-to-end half-covered glass
fixture checks the encoded-premultiplied OS bytes, not only the in-app target.

For bottom-bar checkpoint, icon/drag retention, and fallback regressions, see
[BOTTOM_BAR_COMPLETED_SCENE_GPU_QA.md](BOTTOM_BAR_COMPLETED_SCENE_GPU_QA.md).

## Recorded cloud validation

Executed using Python wgpu 0.32.0 and Vulkan SwiftShader Device (Subzero), a
software CPU adapter. All 65 readback cases passed (two-byte tolerance).
Representative pre-change → corrected byte values:

| Fixture | Before | Corrected |
| --- | --- | --- |
| Direct capture gray 128 | 188,188,188,255 | 128,128,128,255 |
| Flat material over gray 128 | 187,189,190,255 | 127,130,132,255 |
| Half-covered in-app glass | 137,138,139,128 | 92,94,95,128 |
| Toggle desktop fallback | 188,188,188,255 | 128,128,128,255 |
| Toggle partial scene | 161,161,161,255 | 128,128,128,255 |
| Flatten partial scene | 156,156,156,255 | 128,128,128,255 |

The new final presentation pass converts half-covered in-app glass to
64,65,66,128 for the premultiplied OS surface. The brighter in-app encoded
bytes are intentional: sampling that sRGB target yields *linear*-premultiplied
RGB; they must not be sent directly to an encoded-premultiplied compositor.

To record the old implementation, supply both old shader paths and
`--record --legacy-scene-unorm --skip-presentation`; the old flatten pass used
a non-sRGB scene view, and no final presentation shader existed. The baseline
was rendered, not inferred from source text.

This is shader and render-target validation, not a Windows screenshot test.
It does not exercise Windows Graphics Capture, DirectComposition, HDR/color
profiles, window resize/DPI transitions, capture loss/recovery, or the real
animated geometry-generation pass. Those require platform integration QA.

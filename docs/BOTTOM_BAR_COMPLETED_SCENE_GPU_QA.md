# Bottom-bar completed-scene GPU QA

The bottom bar must sample the completed **pre-bar** scene, flattened over the
captured desktop. Using desktop capture alone discards in-app icons and dragged
content; retaining material translucency also lets unfiltered content show
through the settled interior. Neither requires changing the bar's silhouette,
geometry thickness, tint, or configured blur strength.

## Reproducible shader checks

After installing `tools/liquid_glass_gpu_requirements.txt`, run:

```sh
python tools/liquid_glass_gpu_test.py
python tools/liquid_glass_bar_gpu_test.py --output /tmp/bar-gpu-results
```

The first command retains the existing 65 color/compositing regressions. The
second executes 26 bottom-bar cases using the production flatten and final WGSL:

- Empty lower scene yields the same material as desktop-backed main glass.
- Opaque icon, partially transparent drag, and interaction-color fixtures are
  retained in the flattened checkpoint, rather than replaced by desktop color.
- Settled replacement interiors do not depend on a different existing target.
- Reusing the same pre-bar checkpoint produces byte-identical repeated output.
- Antialias coverage, presentation fade endpoints, and the translucent
  no-capture fallback retain their alpha contracts.

The no-feedback check verifies the GPU input/output contract. It does **not**
prove that the application checkpoints at the right time. Rust lane/routing
checks execute the production lane and verify search/page/Done membership.
Application checkpoint ordering is additionally code-reviewed, not exercised
by these isolated fixtures. Windows/macOS integration QA must check real
dragging, repeated frames, capture loss, and recovery. The GPU fixtures do not create a real application window.

## Diagnostic render

With Pillow 10.1 or newer installed, the optional image command executes the production
geometry shader, scene flatten shader, three-level dual-Kawase blur, glass
shader, and final OS presentation shader:

```sh
python tools/liquid_glass_bar_gpu_test.py --image /tmp/bar-gpu-images
```

It creates `bar-before.png`, `bar-after.png`, and `bar-before-after.png`. Both
sides use the same geometry, tint, and optical parameters. The old routing uses
desktop-only blur with material translucency; the corrected routing uses the
filtered completed scene with replacement coverage. Icon/drag color patches
are deterministic synthetic lower-scene fixtures. The encoded-premultiplied OS
source-over operation is modeled after GPU readback. These images are clearly
labeled shader diagnostics, **not Windows application screenshots**.

Keep generated JSON/images outside the checkout. Device or pipeline creation
errors fail the run; they are not counted as skipped/passing tests. Software
Vulkan (e.g. SwiftShader) is suitable for deterministic shader regression tests,
but does not validate Windows Graphics Capture or DirectComposition drivers.

## Production lane checks and cost

`cargo test actual_bottom_bar_lane_preserves_lower_content_fallback_and_resize`
executes the real Rust renderer, geometry and final pipelines, completed-scene
copy/flatten/blur (radius 16), and readback. It verifies foreground preservation,
no-capture alpha, resize/rebinding, half-fade, uncovered pixels, and partial
geometry coverage. Routing tests keep search/page/Done capsules separate from
interaction glass and the gear; the gear deliberately keeps its existing material.

The bottom capsule adds an ROI completed-scene checkpoint/blur on rendered
frames and a separate geometry lane. Because settings/menu reuse that workspace,
their completed-scene cache is invalidated after the bar checkpoint. Native
Windows/macOS performance and interactive capture/DPI behavior still need QA;
software GPU regression passes do not establish those properties.

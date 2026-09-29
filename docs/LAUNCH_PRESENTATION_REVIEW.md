# Launcher presentation review

Reviewed PR #210 through commit 9603f3a against its main-branch base.

## Findings and corrections

| Priority | Finding | Correction |
| --- | --- | --- |
| P1 | A launch-only folder shape replaced the existing scrolling/wiggling shape. The scroll regression was patched separately, but closed-folder edit wiggle was still lost. | Remove shape kind 7 and restore the original shape constructors, scrolling classification, bounds, and overlay selection. Apply reveal as a separate transform in the closed-folder rendering pass. |
| P1 | Glass used global progress without the tile's stagger or scale, and measured direction before scrolling. Folder miniatures independently used their own centers. | One shared WGSL timing/offset implementation; glass and miniatures use the same screen-space folder pivot, scale, stagger, and opacity. Existing scroll/wiggle run first. |
| P2 | Each character independently gathered/scaled, distorting labels and their attachment to icons. | Carry the owner pivot through text layout/rasterization and the neutral glyph model. All characters and shadows use that pivot; settled positions, UVs, and clipping remain unchanged. |
| P2 | The shared final glass shader read alpha from padding that the standalone Glass Studio initialized to zero. | Update Studio's matching uniform and initialize presentation to the fully visible state. |
| P2 | Summon restarted the reveal even when already visible. | Begin only on hidden-to-visible transitions. |
| P2 | Prior QA checked folder-internal paging, not closed folders on the main grid; prior output directories lacked completion manifests. | Add an explicit reveal + main-grid-scroll scenario, execute the production transform on GPU, and require a completed manifest and passing scroll assertions. |

## Implementation boundary

The reveal is presentation layered on existing geometry, not a new kind of
folder. Timing remains 340 ms, global fade 180 ms, start scale 0.84, radial
stagger 0.14, and radial travel 0.14 of the smaller frame half-size.

No scroll physics, folder persistence, drag/drop rules, input hit regions,
or normal modal/control geometry are replaced. Fixed and pointer-dragged
icons bypass radial gathering. Glass reveal geometry is enabled only for the
closed-folder pass; its clip frame stays fixed. Existing native pre-show
rendering/focus handling is retained.

The text adapters for settings, menus, controls, and folder panels explicitly
set the new optional owner pivot to None. Those one-line changes preserve
their behavior while keeping the shared model type complete. GlyphQuad
remains 80 bytes, using previously unused radius padding for the grid pivot.
Glass uniforms grow to 160 bytes so the full timing/frame values can match
the other renderers; all producers, including Studio, and shader consumers
are updated together.

## Validation and limits

- Unit coverage for retaining closed-folder scrolling/edit motion and sharing
  the label pivot across glyphs/shadows without changing geometry/UVs.
- GPU execution of the production glass transform at start, middle, and
  completion, with/without scroll and edit wiggle; fixed/control/clip shapes
  must keep their ordinary transform.
- WGSL module and render-pipeline validation, full Rust tests, clippy, release build.
- Headless DX12 reveal/main-scroll and edit/drag scenarios, with completed
  manifests and inspection of rendered PNGs. The new scenario pins scale
  factor to 1.0 so page-width assertions are independent of desktop DPI.

These checks do not prove native window/compositor behavior on macOS.
macOS shortcut/tray reopen still needs a real-machine check; shared Rust/WGSL
does not establish identical compositor timing. The existing cold-start
loading path is separate from resident-window summon: asynchronously arriving
icons are not guaranteed to be present during its first paint.

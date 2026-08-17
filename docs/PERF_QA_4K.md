# 4K Performance / Visual Regression QA

Deterministic 4K (3840×2160) QA scenarios for the `perf/4k-60fps` work.
Rendering is deterministic (hidden-window, fixed-step scenario clock, fallback
desktop backdrop), so two runs of the same scenario can be compared
frame-by-frame by index (`frame_000000.png` ↔ `frame_000000.png`).

## Scenarios

| Scenario | File | What it exercises | Duration | Frames @60fps |
|---|---|---|---|---|
| 4K main grid scroll | `qa/4k_main_scroll.json` | Two trackpad flings (−900 px each) with spring-settle momentum; scrolls 2 full pages of a 71-app grid (3 pages total) | 4000 ms | 250 |
| 4K context menu | `qa/4k_context_menu.json` | Right-click on a tile, context menu opens and stays open ≥ 1500 ms (Open phase ≈ 1.6 s) | 3000 ms | 188 |
| 4K settings panel | `qa/4k_settings.json` | Long-press enters edit mode, click the settings gear, panel opens and stays open for the rest of the run | 4000 ms | 250 |

All scenarios use `fps: 60` and `viewport: [3840, 2160]` (physical pixels; the
headless window is created at exactly that size — see
`App::resumed` in `src/app/handler.rs`). Tile clicks use `grid_item` targets,
which resolve from the app's own layout and are scale-factor independent. The
settings gear click uses an absolute point (see below).

## Run

```sh
cargo build --release --locked
LAUNCHPAD_QA_SCENARIO=qa/4k_main_scroll.json   LAUNCHPAD_QA_HEADLESS=1 WGPU_BACKEND=metal RUST_LOG=warn ./target/release/launchpad-windows
LAUNCHPAD_QA_SCENARIO=qa/4k_context_menu.json LAUNCHPAD_QA_HEADLESS=1 WGPU_BACKEND=metal RUST_LOG=warn ./target/release/launchpad-windows
LAUNCHPAD_QA_SCENARIO=qa/4k_settings.json    LAUNCHPAD_QA_HEADLESS=1 WGPU_BACKEND=metal RUST_LOG=warn ./target/release/launchpad-windows
```

Output goes to `target/qa-sequences/<scenario-name>-<unix-ms>/`:

- `frame_%06d.png` — 3840×2160 RGBA frames
- `manifest.json` — per-frame state (`pager_position_x`, `context_menu_phase`,
  `editing`, …), scroll/context-menu assertions, `completed: true`
- `scenario-source.txt` — path of the scenario that produced the run

Typical run time is a few seconds of scenario time per 4K frame set (the
runner paces by a fixed clock, not wall time; a slow GPU only delays, never
skips, frames).

## Comparing two runs (baseline vs candidate)

```sh
python3 scripts/compare_qa_runs.py \
  target/qa-sequences/4k-main-scroll-<baseline-ts> \
  target/qa-sequences/4k-main-scroll-<candidate-ts>
```

Per paired frame it reports `mean_abs_diff` (full-res per-channel mean),
`mean_abs_diff_d4` (1/4-downscale, LANCZOS — insensitive to sub-pixel jitter),
`max_abs_diff`, and `changed_pixels_pct` (pixels where any channel differs by
more than `--tolerance`, default 2).

Verdict: **PASS** if the run-wide `mean_abs_diff ≤ 1.0` AND run-wide
`changed_pixels_pct ≤ 0.5` (thresholds configurable via
`--mean-threshold` / `--changed-pct-threshold`). Exit code 0 = pass,
1 = fail, 2 = usage/runtime error (missing dir, scenario name mismatch, frame
size mismatch). Frame-count mismatches only warn — the shared prefix is
compared.

Useful options:

- `--max-frames N` — compare only the first N frames (fast spot check)
- `--report <path>` — write the per-frame table; `.json` produces JSON,
  anything else Markdown
- `--tolerance N` — changed-pixel threshold

Requires Python 3 with Pillow; numpy is used automatically when present
(strongly recommended — the pure-Python fallback is much slower). Install
user-local if missing: `python3 -m pip install --user pillow numpy`.

### Why frame-index comparison is valid

The QA runner drives a fixed-step scenario clock (`src/qa.rs`): actions fire
at exact `at_ms` deadlines, animation advances exactly `1/fps` per captured
frame, and the offscreen render target replaces the swapchain
(`LAUNCHPAD_QA_HEADLESS`). Backdrop capture is replaced by a deterministic
fallback ("desktop backdrop capture disabled for deterministic QA"), so the
pixels are a pure function of the binary + scenario + machine DPI. Two runs on
the same machine of the same binary diff only through scroll-spring
accumulation during flings (a few px; measured run-wide `mean_abs_diff ≈ 0.3`
for `4k-main-scroll`), which is exactly the sensitivity the thresholds target.

## Notes

- The scenarios assume a Retina (2×) display scale, like the 1280×800
  scenarios before them (`trackpad_reversal_main.json` targets −1440/−2880 =
  page widths at 2×). At 4K/2× the grid is 7×5 tiles of 168 px, page width
  1664 px; the second fling settles at −3328 (page 3).
- Settings gear: it only exists in edit mode, to the right of the Done
  capsule, center ≈ (2006, 1546) at 3840×2160/2× (viewport/2 − pair shift +
  Done half width + gap + gear radius, Y = frame bottom + margins). The
  scenario long-presses a tile (`long_press`), then does `pointer_down` /
  `pointer_up` on the gear — the same production path as a real click
  (`handle_control_click` → `open_settings`). The manifest has no settings
  field; verify by pixels (panel rect ≈ x 1160–2680, y 570–1590 lights up to
  ~(218–226) vs backdrop (56,58,60)).
- Context menu: the `Opening` lifecycle phase lasts until the position spring
  settles (~1.33 s); the right-click is therefore scheduled at 100 ms so the
  `Open` phase comfortably exceeds 1500 ms.

# macOS development

The macOS build requires Rust 1.89, Xcode with the macOS SDK, and macOS 14 or
later for the ScreenCaptureKit backdrop. Build and test it with:

```sh
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --locked
cargo build --release --locked
```

On first launch, allow Launchpad under **System Settings > Privacy & Security >
Screen & System Audio Recording**. If permission is denied or ScreenCaptureKit
cannot initialize, the launcher continues with its static Liquid Glass
fallback instead of exiting. Restart the app after changing the permission.

## Backdrop performance diagnostics

One persistent `SCStream` runs on `launchpad-macos-capture` at the display's
refresh rate while the launcher is visible. There is no intentional 30 FPS or
other timer-based update cap. The callback retains the newest complete frame;
if the renderer is temporarily busy, a newer frame replaces the stale pending
frame instead of building an unbounded queue. Hiding the launcher stops the
stream, and summoning it starts the stream again.

The renderer imports each frame's IOSurface as a Metal texture and performs a
GPU-to-GPU copy into its persistent BGRA backdrop texture. It does not convert
the frame to CPU RGBA pixels. The source CVPixelBuffer is released after the
copy submission completes so ScreenCaptureKit can immediately reuse its
bounded frame pool.

The capture rectangle is the union of the visible Liquid Glass shapes plus the
maximum refraction, reflection, and blur sampling radius. It is aligned and
given a small hysteresis margin so minor shape motion does not reconfigure the
stream. By default, on a display scale of 2x, the capture texture's width and
height are each 50% of the ROI's physical-pixel dimensions. That reduces its
pixel count by 75%. The shader maps the texture back to the physical ROI.

Liquid Glass geometry is independent of backdrop pixels. The expensive base
SDF for the page and tile halos therefore has its own cached texture, while
controls, badges, drag visuals, and panels share a separate transient geometry
texture. A changing video backdrop refreshes capture, blur, and final
compositing every frame without recalculating the static base SDF. During a
page scroll, the CPU submits only tile shapes whose smooth-union neighborhood
intersects the fixed frame boundary. Shapes outside the frame and shapes fully
contained far enough inside it cannot change the SDF and are excluded without
reducing backdrop cadence.

Periodic stderr lines separate capture cost from render-thread cost:

- `macOS capture stats` reports stream FPS, stale-frame replacements, and the
  number of CPU pixel copies (normally zero).
- `macOS capture geometry` reports the window, ROI, output resolution, linear
  dimension scale, target refresh rate, and pixel reduction versus a physical
  full-window capture.
- `liquid glass stats` reports render-thread polling/GPU-copy time, blur refresh
  rate, the refresh/reuse rates for cached blur and base geometry, and the
  average number of base shapes evaluated by refreshed geometry passes.

The default global shortcut is Option+Space. Set `LAUNCHPAD_HOTKEY` before
launching to use another `global-hotkey` key string, for example
`shift+alt+Space`.

## Repeatable performance runs

Use the checked-in harness to keep the release/profile features, Metal backend,
run count, environment metadata, and summary format consistent:

```sh
# Three deterministic 1280x800 runs of each required folder scenario.
scripts/profile_macos.sh qa

# Twenty seconds of the real window and a 60 Hz animated backdrop, using the
# ordinary release build and lightweight runtime/process metrics.
scripts/profile_macos.sh live

# Short, explicitly instrumented run for per-pass GPU timestamps. Keep this
# separate because detailed GPU instrumentation can perturb frame pacing.
scripts/profile_macos.sh gpu

# Continuous page scrolling with the production release renderer. Capture and
# presentation remain at their normal cadence.
scripts/profile_macos.sh scroll

# The same scroll workload with per-pass GPU timestamps enabled.
scripts/profile_macos.sh scroll-gpu

# A/B control: retain the ROI but capture it at physical 1:1 resolution.
CAPTURE_SCALE=1 scripts/profile_macos.sh live

# Measure a static real desktop instead of the animated fixture.
ANIMATED_BACKDROP=0 scripts/profile_macos.sh live
```

`RUNS`, `SCENARIOS` (a space-separated list), `DURATION_SECONDS`,
`WARMUP_SECONDS`, and `OUTPUT_DIR` override the defaults. The QA verifier
requires both default folder scenarios, so include them when extending the
scenario list. Live mode waits eight seconds for discovery and icon work, then
summons the resident process before starting its samples. Set `SKIP_BUILD=1` to
reuse a binary already built for the selected mode. Each run creates a new
`target/macos-profile-*` directory containing hardware/toolchain metadata, raw
logs, GPU JSON/Chrome trace files for `qa`/`gpu`/`scroll-gpu`, process samples for live
window runs, and
`summary.md` / `summary.json`. `CAPTURE_SCALE` overrides the automatic
display-scale-derived dimension ratio (clamped to 0.25–1.0); use `1` for a
visual/performance A/B control rather than as the production default.

The harness records how many periodic runtime-stat lines were emitted before
the measurement window. The summarizer excludes those warmup lines, so app
discovery, icon extraction, and initial pipeline setup do not distort the live
capture/render rates.

The QA mode includes PNG readback and is intended for repeatable comparisons,
not absolute full-screen throughput. `live`, `gpu`, `scroll`, and `scroll-gpu` sample macOS `ps` CPU
(where 100% means one fully occupied CPU core) and resident memory once per
second. `live` is the representative performance result; `gpu` is for locating
expensive render passes. The periodic capture/render logs remain the source for
ScreenCaptureKit, IOSurface copy, blur-refresh, geometry shape count, and
blur-reuse costs.

GPU timestamp results can occasionally arrive with an invalid negative or
wrapped duration on Metal. Non-finite, negative, and implausible durations over
60 seconds are excluded from percentiles and reported separately as
`invalid_samples`; a run with a high invalid count should be repeated rather
than treated as authoritative.

## Pull request artifacts

Pull request labels start opt-in macOS artifact workflows:

- `build:macos-binary` builds an ad-hoc signed Apple Silicon `Launchpad.app`,
  bundles its Swift runtime libraries, and uploads a ZIP for seven days.
- `qa:macos-visual` runs deterministic existing-folder and new-folder-creation
  scenarios through the production GPU render path and uploads the PNG
  sequences, manifests, and logs for seven days. The isolated runner sets
  `LAUNCHPAD_QA_HEADLESS=1` to
  render into an offscreen Metal texture because hosted runners do not expose
  a window surface; normal application rendering still uses the window surface.

The visual artifact verifies rendering without foreground or Screen Recording
access. It does not replace interactive testing of ScreenCaptureKit permission,
the global shortcut, menu-bar actions, or multi-monitor behavior.

## Release artifacts

Publishing a GitHub Release runs `release-assets.yml` for both supported
platforms. In addition to the Windows x86-64 ZIP, the workflow builds an Apple
Silicon `Launchpad.app` for macOS 14+, bundles the Swift runtime libraries,
ad-hoc signs the complete bundle, and attaches a `macos-arm64.zip` asset to the
release. The release tag is copied into `CFBundleShortVersionString` when it is
a version-like tag such as `v0.1.0`.

The macOS release is not Developer ID signed or notarized. A downloaded build
may therefore require removing quarantine as described in its bundled
`BUILD_INFO.txt` until signing credentials are configured in GitHub Actions.

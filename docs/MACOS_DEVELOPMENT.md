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

ScreenCaptureKit screenshots and their full-frame RGBA conversion run on the
`launchpad-macos-capture` worker. The render thread only polls a bounded latest-
frame channel, uploads a completed frame, and reuses the previous backdrop
while capture is in flight. Capture requests are capped at about 30 FPS, and
the GPU blur pyramid is rebuilt only when a new backdrop arrives.

Two periodic stderr lines separate capture cost from render-thread cost:

- `macOS capture stats` reports ScreenCaptureKit latency and RGBA copy time on
  the worker.
- `liquid glass stats` reports render-thread polling/upload time, blur refresh
  rate, and the percentage of frames that reused the cached blur.

The `renderer_poll_does_not_wait_for_slow_capture` regression test simulates a
100 ms capture and requires the render-thread poll to return within 40 ms. A
large `capture_ms` in the worker log is therefore expected to reduce backdrop
freshness rather than stall folder and drag animations.

The default global shortcut is Option+Space. Set `LAUNCHPAD_HOTKEY` before
launching to use another `global-hotkey` key string, for example
`shift+alt+Space`.

## Repeatable performance runs

Use the checked-in harness to keep the release/profile features, Metal backend,
run count, environment metadata, and summary format consistent:

```sh
# Three deterministic 1280x800 runs of each required folder scenario.
scripts/profile_macos.sh qa

# Twenty seconds of the real window and ScreenCaptureKit backdrop. Interact
# with the launcher while the command is running.
scripts/profile_macos.sh live
```

`RUNS`, `SCENARIOS` (a space-separated list), `DURATION_SECONDS`,
`WARMUP_SECONDS`, and `OUTPUT_DIR` override the defaults. The QA verifier
requires both default folder scenarios, so include them when extending the
scenario list. Live mode waits eight seconds for discovery and icon work, then
summons the resident process before starting its samples. Set `SKIP_BUILD=1` to
reuse an existing `gpu-profile` release binary. Each run creates a new
`target/macos-profile-*` directory containing hardware/toolchain metadata, raw
logs, GPU JSON/Chrome trace files, process samples for live runs, and
`summary.md` / `summary.json`.

The QA mode includes PNG readback and is intended for repeatable comparisons,
not absolute full-screen throughput. Live mode samples macOS `ps` CPU (where
100% means one fully occupied logical CPU) and resident memory once per second.
The periodic capture/render logs remain the source for ScreenCaptureKit,
conversion, upload, blur-refresh, and blur-reuse costs.

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

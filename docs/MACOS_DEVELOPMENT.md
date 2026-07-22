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

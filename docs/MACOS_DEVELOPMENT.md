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

The default global shortcut is Option+Space. Set `LAUNCHPAD_HOTKEY` before
launching to use another `global-hotkey` key string, for example
`shift+alt+Space`.

## Pull request artifacts

Pull request labels start opt-in macOS artifact workflows:

- `build:macos-binary` builds an ad-hoc signed Apple Silicon `Launchpad.app`,
  bundles its Swift runtime libraries, and uploads a ZIP for seven days.
- `qa:macos-visual` runs the deterministic folder-interaction scenario through
  the production GPU render path and uploads the PNG sequence, manifest, and
  logs for seven days. The isolated runner sets `LAUNCHPAD_QA_HEADLESS=1` to
  render into an offscreen Metal texture because hosted runners do not expose
  a window surface; normal application rendering still uses the window surface.

The visual artifact verifies rendering without foreground or Screen Recording
access. It does not replace interactive testing of ScreenCaptureKit permission,
the global shortcut, menu-bar actions, or multi-monitor behavior.

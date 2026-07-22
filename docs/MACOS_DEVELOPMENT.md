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

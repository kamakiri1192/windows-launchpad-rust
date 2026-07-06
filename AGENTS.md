# Repository Guidelines

## Project Structure & Module Organization

This is a single Rust crate for a GPU-accelerated Windows app launcher. Key files live in `src/`:

- `src/main.rs`: winit application entry point, event loop, input handling, and app wiring.
- `src/renderer.rs`: wgpu device, surface, render pipelines, buffers, and draw calls.
- `src/grid.rs`: launchpad page and tile layout data.
- `src/scroll.rs`: drag, inertia, snap, and rubber-band scroll physics.
- `src/text.rs`: text atlas and glyph quad generation.
- `src/shader.wgsl` and `src/shader_text.wgsl`: tile and text shaders.

Build artifacts go under `target/` and should not be committed. `Cargo.toml` and `Cargo.lock` define the toolchain and dependencies.

## Build, Test, and Development Commands

- `cargo run --release`: builds and runs the app with optimized rendering behavior.
- `cargo build`: performs a debug build for quick compile checks.
- `cargo build --release`: creates an optimized binary in `target/release/`.
- `cargo fmt`: formats Rust code using rustfmt.
- `cargo clippy --all-targets --all-features`: runs lint checks across crate targets.
- `cargo test`: runs all Rust tests once they are added.

Use a Rust toolchain compatible with `rust-version = "1.89"`.

## Coding Style & Naming Conventions

Follow Rust 2021 conventions and rustfmt defaults: four-space indentation, `snake_case` for functions and modules, `PascalCase` for types, and `SCREAMING_SNAKE_CASE` for constants. Keep renderer, layout, scroll, and text responsibilities separated by module. Keep WGSL entry points and buffer layouts in sync with matching Rust structs, especially `#[repr(C)]` uniform and instance data.

## Testing Guidelines

There are currently no committed tests. Add unit tests near deterministic logic, especially layout calculations in `grid.rs` and physics behavior in `scroll.rs`. Use clear test names such as `snap_targets_nearest_page`. Run `cargo test` before opening a pull request. For rendering changes, also run `cargo run --release` and manually verify window creation, resizing, dragging, snapping, and text rendering.

## Commit & Pull Request Guidelines

The repository currently has only an `Initial commit`, so no strict convention is established. Use concise imperative subjects, for example `Add text atlas upload path` or `Fix scroll snap overshoot`. Pull requests should include a summary, testing performed, and screenshots or recordings for visible rendering or interaction changes. Link related issues when available and call out GPU, driver, or Windows-version assumptions.

## Security & Configuration Tips

Do not commit generated binaries, logs, or local configuration. Review GPU API changes carefully because `wgpu`, `winit`, and shader layouts are tightly coupled.

## テスト時のスクリーンショットについて
liquid glass表現の理由によってキャプチャを無効にしています。
起動時にパラメータを設定しないとスクリーンショットできません。下記ドキュメントを参照すること。
docs\EDIT_MODE_VISUAL_QA.md
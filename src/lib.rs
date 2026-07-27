//! Library crate surface for `launchpad-windows`.
//!
//! The library exposes the layers that are free of `winit`/`wgpu`/Win32
//! dependencies so they can be unit-tested in isolation and consumed by both
//! the binary target and future tooling:
//!
//! - [`domain`]: durable launcher data and pure rules (app IDs, registry,
//!   settings, diffs).
//! - [`icons`]: icon normalization, visual-size analysis, and atlas packing.
//! - [`layout`]: renderer-neutral layout builders that emit `LayoutResult`.
//! - [`ui_model`]: renderer-neutral primitives (`RenderModel`, `HitMap`,
//!   `UiId`, geometry).
//!
//! The app shell, features, renderer, platform adapters, and workers remain in
//! the binary target (`src/main.rs`) because they depend on `winit`, `wgpu`,
//! or the `windows` crate.

pub mod domain;
pub mod icons;
pub mod input_probe_protocol;
pub mod input_routing;
pub mod layout;
pub mod scroll;
pub mod ui;
pub mod ui_model;

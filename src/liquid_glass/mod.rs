//! Liquid Glass backdrop rendering.
//!
//! The module is split so the renderer owns GPU resources while capture
//! backends can evolve independently. The current Windows backend excludes the
//! app window from OS capture and exposes a fallback backdrop until the full
//! Windows.Graphics.Capture upload path is wired in.

pub mod capture;
pub mod geometry;
pub mod params;
pub mod renderer;

#[cfg(windows)]
pub mod windows_capture;

pub use renderer::LiquidGlassRenderer;

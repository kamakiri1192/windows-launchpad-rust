//! Operating-system integration.
//!
//! Platform modules own OS-specific adapters: the global hotkey, tray menu,
//! window show/hide behavior, app launch, and capture exclusion. They are the
//! edge where Win32/Shell/COM or macOS framework calls live, behind narrow
//! Rust APIs consumed by the app shell.
//!
//! These modules are compiled into the binary target only (they depend on the
//! `windows` crate and `winit`), not the library.

#[cfg(windows)]
pub mod windows;

pub mod launch;
pub mod paths;

//! Background worker modules.
//!
//! Worker modules own expensive background work: Start Menu scanning, app
//! snapshot diff production, icon extraction, icon normalization, and icon
//! cache reads/writes. They run on their own OS threads and send owned data
//! back to the app shell via channels.
//!
//! Rules (see `ARCHITECTURE.md > Platform and Workers`):
//! - Workers send owned data back to the app shell.
//! - Workers do not mutate UI state.
//! - Workers do not touch the renderer.
//! - Windows handles and COM/GDI objects do not cross channels.
//!
//! These modules are compiled into the binary target only.

pub mod app_scan;
pub mod icon_worker;
pub mod refresh_watcher;

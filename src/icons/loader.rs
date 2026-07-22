//! Orchestrate the full icon pipeline: enumerate → extract → normalize → pack.
//!
//! **Legacy synchronous path.** Retained for reference and the
//! `IconAtlas::pack` unit tests; the live launcher now uses the async
//! [`crate::icon_worker`] + [`crate::icon_cache`] + [`crate::app_registry`]
//! pipeline (see `docs/STARTUP_PERFORMANCE.md`). This whole module is
//! `#[allow(dead_code)]` so it stays compilable without polluting the build
//! with warnings.

#![allow(dead_code)]

use super::extract::{self, ComScope};
use super::normalize::{normalize, DecodedIcon};
use super::{AppEntry, IconAtlas};

/// The output of [`load_all_icons`]: the packed atlas plus one `AppEntry` per
/// discovered shortcut (icon-less entries included, for fallback rendering).
#[derive(Debug)]
pub struct LoadedIcons {
    pub atlas: IconAtlas,
    pub apps: Vec<AppEntry>,
}

/// Load every Start Menu shortcut's icon.
///
/// Initializes COM for the current thread, walks the Start Menu, extracts and
/// normalizes each icon, and packs them into a single atlas. Shortcut entries
/// whose icon couldn't be extracted are still returned (with `uv: None`) so
/// the grid can still list them as color tiles.
///
/// Returns `LoadedIcons` even if zero shortcuts were found — the atlas will be
/// a single empty row and `apps` will be empty.
pub fn load_all_icons() -> LoadedIcons {
    let _com = ComScope::new();

    let shortcuts = extract::enumerate_start_menu();
    let mut icons: Vec<DecodedIcon> = Vec::with_capacity(shortcuts.len());
    let mut apps: Vec<AppEntry> = Vec::with_capacity(shortcuts.len());
    // Remember which `icons` index corresponds to each app so we can look up
    // its UV rect after packing (apps without an icon get `None`).
    let mut icon_idx_for: Vec<Option<usize>> = Vec::with_capacity(shortcuts.len());

    for sc in shortcuts {
        match extract::extract_icon_from_lnk(&sc.path) {
            Some(raw) => {
                let normalized = normalize(&raw);
                icon_idx_for.push(Some(icons.len()));
                icons.push(normalized);
            }
            None => {
                icon_idx_for.push(None);
            }
        }
        apps.push(AppEntry {
            name: sc.name,
            uv: None, // filled in after packing
            link_path: sc.path,
        });
    }

    let atlas = IconAtlas::pack(&icons);

    // Map each app to its UV rect from the packed atlas.
    for (app, idx) in apps.iter_mut().zip(icon_idx_for.iter()) {
        if let Some(i) = *idx {
            app.uv = atlas.entries.get(i).copied();
        }
    }

    LoadedIcons { atlas, apps }
}

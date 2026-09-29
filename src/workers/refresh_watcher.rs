//! Background discovered-app change watcher (Phase 4).
//!
//! Today this is a **polling** watcher: every `interval` it rescans the Start
//! Menu and Steam libraries, then computes a diff, sending non-empty
//! diffs to the UI. Polling is deliberately simple and robust (no
//! `ReadDirectoryChangesW` edge cases around buffer sizing, junction loops, or
//! overwritten-file notifications). The structure is isolated so a future
//! event-driven watcher can drop in behind the same [`RefreshMessage`] channel.
//!
//! The watcher never blocks the UI: it owns its own thread, and the only data
//! crossing back is plain Rust (an [`AppDiff`]).

use std::collections::BTreeMap;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

#[cfg(not(windows))]
use super::app_scan::scan_start_menu_checked;
#[cfg(not(windows))]
use crate::domain::app_diff::diff_snapshots;
use crate::domain::app_diff::{AppDiff, SnapshotEntry};
use crate::domain::app_id::AppId;
use crate::icon_cache::{IconCache, LayoutLoad};
#[cfg(not(windows))]
use crate::startup_timer::{self, prefix};
use std::sync::Arc;

/// Messages the watcher sends to the UI thread.
#[derive(Debug)]
pub enum RefreshMessage {
    #[cfg(windows)]
    Catalog(crate::domain::app_catalog::CatalogScan),
    /// First snapshot is ready (sent once, ~immediately after spawn).
    Initial {
        snapshot: BTreeMap<AppId, SnapshotEntry>,
        layout: Box<LayoutLoad>,
        status: super::scan_status::ScanStatus,
    },
    /// A subsequent rescan produced a non-empty diff.
    Diff(AppDiff),
}

/// Configuration for the watcher. Kept as a struct so tests/edge builds can
/// inject a tiny interval.
#[derive(Debug, Clone)]
pub struct RefreshConfig {
    /// Delay before the first scan (lets the UI paint first).
    pub initial_delay: Duration,
    /// Interval between rescans.
    pub poll_interval: Duration,
}

impl Default for RefreshConfig {
    fn default() -> Self {
        Self {
            // Let first paint + initial icon fan-out happen first.
            initial_delay: Duration::from_secs(2),
            // ~10s balances responsiveness against scan cost.
            poll_interval: Duration::from_secs(10),
        }
    }
}

/// Spawn the watcher. Returns immediately; messages arrive on `tx`. The thread
/// exits when the sender is dropped (i.e. when the UI goes away).
pub fn spawn(tx: Sender<RefreshMessage>, config: RefreshConfig, cache: Arc<IconCache>) {
    thread::Builder::new()
        .name("refresh-watcher".to_string())
        .spawn(move || run(tx, config, cache))
        .expect("spawn refresh-watcher");
}

#[cfg(windows)]
fn run(tx: Sender<RefreshMessage>, config: RefreshConfig, _cache: Arc<IconCache>) {
    thread::sleep(config.initial_delay);
    loop {
        let scan = super::app_scan::scan_catalog();
        if tx.send(RefreshMessage::Catalog(scan)).is_err() {
            return;
        }
        thread::sleep(config.poll_interval);
    }
}
#[cfg(not(windows))]
fn run(tx: Sender<RefreshMessage>, config: RefreshConfig, cache: Arc<IconCache>) {
    let timer = startup_timer::get();
    thread::sleep(config.initial_delay);

    let (mut snapshot, status) = scan_start_menu_checked();
    let layout = cache.prepare_initial_layout(
        &snapshot,
        status.complete(),
        crate::domain::initial_layout::catalog::Platform::current(),
        crate::domain::initial_layout::japanese_locale(sys_locale::get_locale().as_deref()),
    );
    timer.mark(prefix::APP_REFRESH, "initial layout committed");
    timer.mark_with(prefix::APP_REFRESH, "initial scan", || {
        format!("({} apps)", snapshot.len())
    });
    if tx
        .send(RefreshMessage::Initial {
            snapshot: snapshot.clone(),
            layout: Box::new(layout),
            status,
        })
        .is_err()
    {
        return;
    }

    loop {
        thread::sleep(config.poll_interval);
        let (mut new, status) = scan_start_menu_checked();
        // A failed source is not evidence of uninstallation. Keep known entries
        // until a complete scan can confirm removals.
        if !status.complete() {
            for (id, entry) in &snapshot {
                new.entry(id.clone()).or_insert_with(|| entry.clone());
            }
        }
        let diff = diff_snapshots(&snapshot, &new);
        if diff.is_empty() {
            snapshot = new;
            continue;
        }
        timer.mark_with(prefix::APP_REFRESH, "detected diff", || {
            format!(
                "added={} updated={} removed={}",
                diff.added.len(),
                diff.updated.len(),
                diff.removed.len()
            )
        });
        if tx.send(RefreshMessage::Diff(diff)).is_err() {
            return;
        }
        snapshot = new;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(name: &str) -> SnapshotEntry {
        let path = format!("C:\\Start Menu\\{name}.lnk");
        SnapshotEntry {
            launch_arguments: None,
            app_id: AppId::from_link_path(PathBuf::from(&path)),
            name: name.to_string(),
            link_path: path,
            source_modified_at: 1,
            target_path: String::new(),
            target_modified_at: 0,
            icon_location: String::new(),
            icon_index: 0,
            provider_icon: None,
            version: String::new(),
            publisher: String::new(),
            identifier: String::new(),
        }
    }

    #[test]
    fn scan_status_distinguishes_empty_success_and_partial_failure() {
        let mut status = super::super::scan_status::ScanStatus::default();
        assert!(!status.complete());
        status.record("applications", true);
        status.record("steam", true);
        assert!(status.complete());
        status.record("unreadable source", false);
        assert!(!status.complete());
    }
}

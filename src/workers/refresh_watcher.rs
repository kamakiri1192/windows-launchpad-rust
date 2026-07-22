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

use super::app_scan::scan_start_menu;
use crate::domain::app_diff::{diff_snapshots, AppDiff, SnapshotEntry};
use crate::domain::app_id::AppId;
use crate::startup_timer::{self, prefix};

/// Messages the watcher sends to the UI thread.
#[derive(Debug)]
pub enum RefreshMessage {
    /// First snapshot is ready (sent once, ~immediately after spawn).
    Initial(BTreeMap<AppId, SnapshotEntry>),
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
pub fn spawn(tx: Sender<RefreshMessage>, config: RefreshConfig) {
    thread::Builder::new()
        .name("refresh-watcher".to_string())
        .spawn(move || run(tx, config))
        .expect("spawn refresh-watcher");
}

fn run(tx: Sender<RefreshMessage>, config: RefreshConfig) {
    let timer = startup_timer::get();
    thread::sleep(config.initial_delay);

    let mut snapshot = scan_start_menu();
    timer.mark_with(
        prefix::APP_REFRESH,
        "initial scan",
        format!("({} apps)", snapshot.len()),
    );
    if tx.send(RefreshMessage::Initial(snapshot.clone())).is_err() {
        return;
    }

    loop {
        thread::sleep(config.poll_interval);
        let new = scan_start_menu();
        let diff = diff_snapshots(&snapshot, &new);
        if diff.is_empty() {
            snapshot = new;
            continue;
        }
        timer.mark_with(
            prefix::APP_REFRESH,
            "detected diff",
            format!(
                "added={} updated={} removed={}",
                diff.added.len(),
                diff.updated.len(),
                diff.removed.len()
            ),
        );
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
            app_id: AppId::from_link_path(PathBuf::from(&path)),
            name: name.to_string(),
            link_path: path,
            link_mtime: 1,
            target_path: String::new(),
            target_mtime: 0,
            icon_location: String::new(),
            icon_index: 0,
        }
    }

    #[test]
    fn refresh_message_initial_carries_snapshot() {
        let m = RefreshMessage::Initial(
            [entry("A")]
                .iter()
                .cloned()
                .map(|e| (e.app_id.clone(), e))
                .collect(),
        );
        match m {
            RefreshMessage::Initial(m) => assert_eq!(m.len(), 1),
            _ => panic!("wrong variant"),
        }
    }
}

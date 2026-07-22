//! Tiny file-backed debug logger for the resident launcher.
//!
//! Release builds use `windows_subsystem = "windows"`, so there is no console
//! and `eprintln!` goes nowhere. This writes timestamped lines to
//! `%LOCALAPPDATA%\Launchpad\debug.log` on Windows or
//! `~/Library/Logs/Launchpad/debug.log` on macOS, which survives release and is
//! easy to copy-paste back when debugging the hotkey hook / tray / lifecycle.
//!
//! The log is overwritten on each launch (truncate, not append) so it doesn't
//! grow unbounded — a single session's worth is enough to diagnose a bug.
//!
//! Usage: `debug_log!("hook: Win+Space → Summon");`. Macros are gated on the
//! `LAUNCHPAD_DEBUG` env var at runtime, so the default release build writes
//! nothing unless the user opts in — keeping disk writes off the hot path.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;

static LOG_FILE: OnceLock<Option<PathBuf>> = OnceLock::new();
static LOG_LOCK: Mutex<()> = Mutex::new(());

/// Resolve the platform-native diagnostic log path.
fn log_path() -> PathBuf {
    crate::platform::paths::debug_log_path()
}

/// Open (or reset) the log file. Called once at startup. Truncates any prior
/// content so each session starts fresh. After this call, [`enabled`]
/// reflects whether logging is on (gated by the `LAUNCHPAD_DEBUG` env var).
pub fn init() {
    // Logging is opt-in: only write when LAUNCHPAD_DEBUG is set. This keeps
    // the release binary silent by default and avoids per-frame disk writes.
    let on = std::env::var_os("LAUNCHPAD_DEBUG").is_some();
    if !on {
        let _ = LOG_FILE.set(None);
        return;
    }

    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Truncate on open so each run starts clean.
    if OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .is_ok()
    {
        let _ = LOG_FILE.set(Some(path.clone()));
        // Also print the path once so the user knows where to look.
        write_line(&format!("=== Launchpad debug log: {} ===", path.display()));
    } else {
        let _ = LOG_FILE.set(None);
    }
}

/// Whether logging is currently enabled.
pub fn enabled() -> bool {
    LOG_FILE.get().map(|o| o.is_some()).unwrap_or(false)
}

/// Append one line with a millisecond timestamp. No-op if disabled.
fn write_line(line: &str) {
    let Some(Some(path)) = LOG_FILE.get() else {
        return;
    };
    // Lock so concurrent threads don't interleave.
    let _guard = LOG_LOCK.lock().ok();
    if let Ok(mut f) = OpenOptions::new().append(true).open(path) {
        let stamp = {
            // Use std time for a cheap monotonic-ish wall clock. Format:
            // HH:MM:SS.mmm — good enough to order events within a session.
            use std::time::{SystemTime, UNIX_EPOCH};
            let dur = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default();
            let secs = dur.as_secs();
            let ms = dur.subsec_millis();
            let h = (secs / 3600) % 24;
            let m = (secs / 60) % 60;
            let s = secs % 60;
            format!("{h:02}:{m:02}:{s:02}.{ms:03}")
        };
        let _ = writeln!(f, "{stamp} {line}");
    }
}

/// Public entry point used by the `debug_log!` macro.
#[doc(hidden)]
pub fn log(args: std::fmt::Arguments<'_>) {
    if !enabled() {
        return;
    }
    write_line(&args.to_string());
}

/// Append one timestamped line to the debug log. Compiled in always, but does
/// nothing at runtime unless `LAUNCHPAD_DEBUG` is set and [`init`] succeeded.
#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        $crate::debug_logger::log(format_args!($($arg)*))
    };
}

//! Lightweight startup / icon-pipeline timing.
//!
//! The whole point of this module is to make "where does launch time go?"
//! answerable from a log file. It records `Instant`-based phase markers and
//! prints them as human-readable `startup:` / `icon-cache:` / `icon-worker:` /
//! `app-refresh:` lines with elapsed-since-process-start and step deltas.
//!
//! Design choices:
//!   - All printing goes through `eprintln!` (the app already uses `env_logger`
//!     at `warn` by default; these timing lines are intentionally always
//!     visible so a user can paste them into a bug report without fiddling
//!     with `RUST_LOG`).
//!   - Every phase records an absolute offset from `process_start`, so logs
//!     from two runs can be eyeballed against each other.
//!   - The same `StartupTimer` instance is threaded through `main`/`App`; the
//!     only API is [`StartupTimer::mark`], which is a no-op-safe call.

use std::sync::OnceLock;
use std::time::Instant;

/// Prefixes used in timing logs. Keeping them in one place keeps greps tidy.
pub mod prefix {
    pub const STARTUP: &str = "startup";
    pub const ICON_CACHE: &str = "icon-cache";
    pub const ICON_WORKER: &str = "icon-worker";
    pub const APP_REFRESH: &str = "app-refresh";
}

/// A point recorded on the startup timeline.
struct Mark {
    label: &'static str,
    at: Instant,
}

/// Records the startup timeline. Cheap to clone (shares the inner list via
/// `Arc<Mutex<…>>`) so it can be handed to worker threads that want to emit
/// `icon-worker:` marks without fighting the UI thread for `&mut`.
#[derive(Clone)]
pub struct StartupTimer {
    process_start: Instant,
    last: std::sync::Arc<std::sync::Mutex<Instant>>,
}

impl Default for StartupTimer {
    fn default() -> Self {
        Self::new()
    }
}

impl StartupTimer {
    /// Capture "now" as the process start reference.
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            process_start: now,
            last: std::sync::Arc::new(std::sync::Mutex::new(now)),
        }
    }

    /// Record a phase boundary and print it. `prefix` selects the log line's
    /// prefix (`startup:`, `icon-cache:`, …); `label` is the phase name.
    ///
    /// Prints two numbers: milliseconds since process start, and milliseconds
    /// since the previous mark on this timer. Example:
    ///
    /// ```text
    /// startup: first frame rendered in 180ms (total 180ms)
    /// startup: app list enumeration in 40ms (total 220ms)
    /// ```
    pub fn mark(&self, prefix: &str, label: &'static str) {
        let now = Instant::now();
        let total = now.duration_since(self.process_start).as_secs_f64() * 1000.0;
        let delta = {
            let mut last = self.last.lock().expect("timer mutex poisoned");
            let d = now.duration_since(*last).as_secs_f64() * 1000.0;
            *last = now;
            d
        };
        eprintln!("{prefix}: {label} in {delta:.0}ms (total {total:.0}ms)");
    }

    /// Variant of [`mark`][Self::mark] that reports an arbitrary scalar (e.g.
    /// "loaded 84 cached icons in 32ms"). The elapsed number is still the
    /// delta since the previous mark.
    pub fn mark_with(&self, prefix: &str, label: &'static str, detail: impl std::fmt::Display) {
        let now = Instant::now();
        let total = now.duration_since(self.process_start).as_secs_f64() * 1000.0;
        let delta = {
            let mut last = self.last.lock().expect("timer mutex poisoned");
            let d = now.duration_since(*last).as_secs_f64() * 1000.0;
            *last = now;
            d
        };
        eprintln!("{prefix}: {label} {detail} in {delta:.0}ms (total {total:.0}ms)");
    }

    /// The instant captured at construction. Useful for one-off
    /// `Duration` math outside the mark stream.
    pub fn process_start(&self) -> Instant {
        self.process_start
    }
}

/// A process-global timer captured on first access, so library code that doesn't
/// own a `StartupTimer` handle (e.g. the cache) can still stamp a log line.
static GLOBAL: OnceLock<StartupTimer> = OnceLock::new();

/// Install / fetch the global timer. First call wins; subsequent calls return
/// the same instance, so `install` at `main` and `get` everywhere else.
pub fn install(timer: StartupTimer) {
    let _ = GLOBAL.set(timer);
}

/// Fetch the global timer, or a freshly-made one if none was installed.
pub fn get() -> StartupTimer {
    GLOBAL.get().cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_does_not_panic_without_install() {
        // Even with no global timer installed, get() must produce a usable one.
        let t = get();
        t.mark(prefix::STARTUP, "test-mark");
    }
}

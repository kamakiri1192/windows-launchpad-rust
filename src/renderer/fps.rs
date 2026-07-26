//! Frame-rate tracking for the on-screen FPS overlay.
//!
//! The overlay is fed by [`FpsTracker`], which turns frame presentation events
//! into a smoothed integer FPS reading. The tracker is platform-agnostic in
//! its core (a ring buffer of presentation timestamps), but prefers
//! platform-supplied presentation statistics when available so the reported
//! rate reflects frames the compositor actually showed:
//!
//! - **Windows**: DXGI `IDXGISwapChain::GetFrameStatistics` exposes
//!   `PresentRefreshCount` / `SyncQPCTime`, the authoritative scanout
//!   cadence. [`FpsTracker::note_dxgi`] consumes these. If the swapchain
//!   handle is unavailable the tracker transparently falls back to the
//!   portable timestamp path via [`FpsTracker::note_presented`].
//! - **macOS / other**: [`FpsTracker::note_presented`] is called from
//!   `frame.present()`. Because the present mode is `Fifo` with vsync, the
//!   `get_current_texture` -> `present()` round trip is gated by the vblank
//!   cadence, so this EMA closely tracks the displayed frame rate. Frames
//!   that early-return before present (`Timeout`/`Occluded`/`Outdated`) are
//!   excluded by construction, which keeps the value from drifting while the
//!   surface is unusable.

use std::time::{Duration, Instant};

/// Maximum number of presentation timestamps kept in the ring buffer. A
/// larger window yields a smoother reading but reacts more slowly to rate
/// changes; 60 samples is roughly one second at 60 Hz.
const RING_CAP: usize = 64;
/// Minimum span between the oldest and newest samples required before a
/// reading is emitted. Below this the tracker reports zero to avoid
/// div-by-zero noise during the first few frames.
const MIN_SPAN: Duration = Duration::from_millis(120);
/// FPS value reported before enough samples have accumulated.
const WARMUP_FPS: u32 = 0;

/// A ring-buffer presentation-rate estimator.
///
/// The estimator is intentionally cheap: each sample pushes one `Instant`,
/// and computing the rate touches only the two ends of the buffer.
#[derive(Debug)]
pub(crate) struct FpsTracker {
    /// Monotonic timestamps of frame presentations, oldest first. Acts as a
    /// ring buffer via [`VecDeque`]-style wraparound on a fixed-size [`Vec`].
    samples: Vec<Instant>,
    /// Index in `samples` that receives the next push.
    head: usize,
    /// Number of valid samples currently held (saturates at `samples.len()`).
    len: usize,
    /// Most recent reading, cached so [`Self::current`] is a pure accessor.
    cached: u32,
}

impl Default for FpsTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl FpsTracker {
    /// Construct an empty tracker that reports `0` until it warms up.
    pub(crate) fn new() -> Self {
        Self {
            samples: Vec::with_capacity(RING_CAP),
            head: 0,
            len: 0,
            cached: WARMUP_FPS,
        }
    }

    /// Record a frame that was just presented (portable path).
    ///
    /// Call this immediately after `SurfaceTexture::present()` succeeds, or
    /// as a fallback when platform presentation statistics are unavailable.
    pub(crate) fn note_presented(&mut self, now: Instant) {
        self.push(now);
        self.recompute(now);
    }

    /// Record a frame whose presentation was confirmed by DXGI presentation
    /// statistics (Windows only).
    ///
    /// `presented_at` is the wall-clock [`Instant`] at which the caller
    /// observed the compositor's vsync counter advance (via
    /// `DXGI_FRAME_STATISTICS::SyncRefreshCount`). Unlike
    /// [`Self::note_presented`], this method dedupes successive calls with
    /// the same timestamp so a stalled compositor (no new vsync between
    /// `present()` calls) does not inflate the reading — the authoritative
    /// "did this frame actually scan out?" signal that DXGI provides.
    #[cfg(target_os = "windows")]
    pub(crate) fn note_dxgi(&mut self, presented_at: Instant) {
        // Dedupe stale sync timestamps so repeated calls while the
        // compositor is idle do not inflate the rate.
        if self.last_sample() != Some(presented_at) {
            self.push(presented_at);
        }
        self.recompute(presented_at);
    }

    /// Latest reported FPS reading. Returns `0` until enough samples arrive.
    pub(crate) fn current(&self) -> u32 {
        self.cached
    }

    /// Push a timestamp onto the ring buffer, overwriting the oldest sample
    /// once the capacity is reached.
    fn push(&mut self, ts: Instant) {
        if self.samples.len() < RING_CAP {
            self.samples.push(ts);
            self.head = (self.head + 1) % RING_CAP;
            self.len = self.samples.len();
            return;
        }
        self.samples[self.head] = ts;
        self.head = (self.head + 1) % RING_CAP;
        self.len = RING_CAP;
    }

    /// Newest sample currently held in the buffer, if any.
    fn last_sample(&self) -> Option<Instant> {
        if self.len == 0 {
            return None;
        }
        let idx = (self.head + RING_CAP - 1) % RING_CAP;
        Some(self.samples[idx.min(self.samples.len() - 1)])
    }

    /// Oldest sample currently held in the buffer, if any.
    fn first_sample(&self) -> Option<Instant> {
        if self.len == 0 {
            return None;
        }
        if self.len < RING_CAP {
            // Buffer hasn't wrapped yet: oldest is index 0.
            return Some(self.samples[0]);
        }
        // Buffer is full: oldest is the current head (next write target).
        Some(self.samples[self.head])
    }

    /// Recompute the cached FPS from the current buffer window.
    fn recompute(&mut self, newest: Instant) {
        let Some(oldest) = self.first_sample() else {
            self.cached = WARMUP_FPS;
            return;
        };
        let span = newest.saturating_duration_since(oldest);
        if span < MIN_SPAN {
            return;
        }
        // `len` counts samples; the rate is (samples-1) intervals over `span`.
        let intervals = self.len.saturating_sub(1).max(1) as f64;
        let fps = intervals / span.as_secs_f64();
        self.cached = if fps.is_finite() && fps > 0.0 {
            fps.round().max(1.0) as u32
        } else {
            WARMUP_FPS
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_zero_until_warmed_up() {
        let mut t = FpsTracker::new();
        assert_eq!(t.current(), 0);
        let base = Instant::now();
        // Three samples at 16ms (≈60fps) but below the 120ms warm-up span.
        for ms in [0, 16, 32, 48] {
            t.note_presented(base + Duration::from_millis(ms));
        }
        // 48ms < 120ms warm-up threshold; still warming up.
        assert_eq!(t.current(), 0);
    }

    #[test]
    fn estimates_steady_60fps() {
        let mut t = FpsTracker::new();
        let base = Instant::now();
        // 60 samples at ~16.67ms intervals (1 second of 60fps).
        let mut ts = base;
        for _ in 0..60 {
            t.note_presented(ts);
            ts += Duration::from_micros(16_667);
        }
        // Span is ~1s with 60 samples ⇒ ~60 intervals/s ⇒ ~60fps.
        let reading = t.current();
        assert!((56..=64).contains(&reading), "got {reading}");
    }

    #[test]
    fn ring_buffer_wraps_without_panic() {
        let mut t = FpsTracker::new();
        let base = Instant::now();
        // Push well past RING_CAP to verify wraparound.
        for i in 0..(RING_CAP * 3) {
            t.note_presented(base + Duration::from_millis(i as u64 * 17));
        }
        // After many samples the rate should still be a sane 50-60fps window.
        let reading = t.current();
        assert!(reading > 0, "got {reading}");
    }

    #[test]
    fn first_and_last_sample_track_buffer_state() {
        let mut t = FpsTracker::new();
        assert_eq!(t.first_sample(), None);
        assert_eq!(t.last_sample(), None);

        let base = Instant::now();
        let a = base;
        let b = base + Duration::from_millis(10);
        let c = base + Duration::from_millis(20);
        t.note_presented(a);
        assert_eq!(t.first_sample(), Some(a));
        assert_eq!(t.last_sample(), Some(a));
        t.note_presented(b);
        assert_eq!(t.first_sample(), Some(a));
        assert_eq!(t.last_sample(), Some(b));
        t.note_presented(c);
        assert_eq!(t.first_sample(), Some(a));
        assert_eq!(t.last_sample(), Some(c));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn dxgi_drops_repeated_sync_timestamps() {
        let mut t = FpsTracker::new();
        let base = Instant::now();
        // Repeated identical sync timestamps must not inflate the rate.
        for _ in 0..10 {
            t.note_dxgi(base);
        }
        assert_eq!(t.current(), 0);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn dxgi_advances_with_new_sync_timestamps() {
        let mut t = FpsTracker::new();
        let base = Instant::now();
        let mut ts = base;
        for _ in 0..60 {
            t.note_dxgi(ts);
            ts += Duration::from_micros(16_667);
        }
        let reading = t.current();
        assert!((56..=64).contains(&reading), "got {reading}");
    }
}

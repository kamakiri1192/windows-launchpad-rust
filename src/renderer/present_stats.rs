//! Windows-only presentation statistics extraction.
//!
//! DXGI's `IDXGISwapChain::GetFrameStatistics` reports the compositor's own
//! view of when frames were scanned out (`PresentRefreshCount`,
//! `SyncRefreshCount`, `SyncQPCTime`). This is the authoritative signal that
//! a submitted frame was actually displayed, which `frame.present()` cadence
//! alone cannot distinguish from compositor-dropped frames.
//!
//! The wgpu DX12 backend owns the swapchain; this module reaches it through
//! `Surface::as_hal::<Dx12>()` (mirroring the existing
//! `device.as_hal::<Dx12>()` precedent in `liquid_glass/windows_capture.rs`).
//!
//! This module is gated by `#[cfg(target_os = "windows")]` at the `mod`
//! declaration site (`renderer/mod.rs`); there is no inner `#![cfg]` here to
//! avoid clippy's `duplicated_attributes` lint.

use std::time::Instant;

use wgpu::hal::api::Dx12;
use windows::Win32::Graphics::Dxgi::DXGI_FRAME_STATISTICS;

/// Query DXGI for the timestamp of the most recent vsync the compositor
/// reported, expressed as a wall-clock [`Instant`].
///
/// Returns `None` when the swapchain is unavailable (non-DX12 backend,
/// surface not yet configured, or `GetFrameStatistics` failing — e.g. the
/// window is occluded). Callers should fall back to the portable
/// [`crate::renderer::fps::FpsTracker::note_presented`] path in that case.
///
/// # Safety
///
/// Forwards the `unsafe` surface `as_hal` contract — the returned reference
/// borrows wgpu's internal surface guard for the call duration. We only read
/// the swapchain's frame statistics and do not retain or destroy handles.
pub(crate) unsafe fn last_presented_instant(surface: &wgpu::Surface<'_>) -> Option<Instant> {
    let hal_surface = unsafe { surface.as_hal::<Dx12>() }?;
    let swapchain = hal_surface.swap_chain()?;
    // `IDXGISwapChain3` derefs to `IDXGISwapChain`, so the base interface's
    // `GetFrameStatistics` is reachable without a cast.
    let mut stats = DXGI_FRAME_STATISTICS::default();
    // S_OK indicates a successful query; anything else (e.g.
    // DXGI_ERROR_FRAME_STATISTICS_DISJOINT) means no fresh sample — treat as
    // "no reading" so the caller falls back to the present-cadence path.
    unsafe { swapchain.GetFrameStatistics(&mut stats).ok()? };
    qpc_to_instant(stats.SyncQPCTime)
}

/// Convert a Windows QPC (`QueryPerformanceCounter`) tick count into an
/// [`Instant`] anchored on the process's monotonic clock.
///
/// `Instant` is opaque, so we synthesize an equivalent monotonic value by
/// measuring the current QPC at call time and expressing the supplied tick
/// as a signed offset from it. This preserves relative ordering (which is
/// all [`crate::renderer::fps::FpsTracker`] needs) without depending on
/// `Instant`'s internal representation.
fn qpc_to_instant(qpc_ticks: i64) -> Option<Instant> {
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

    let mut freq: i64 = 0;
    // SAFETY: both functions write a single i64 through the supplied pointer
    // and have no other preconditions; they never fail on XP+.
    unsafe { QueryPerformanceFrequency(&mut freq).ok()? };
    if freq <= 0 {
        return None;
    }

    let mut now_qpc: i64 = 0;
    unsafe { QueryPerformanceCounter(&mut now_qpc).ok()? };

    // Offset of the reported timestamp relative to "now", in QPC ticks. QPC
    // is monotonic so `now_qpc >= qpc_ticks` holds in steady state; clamp
    // the rare backwards excursion to 0 to keep the result finite.
    let delta_ticks = (now_qpc - qpc_ticks).max(0) as u64;
    let delta_secs = delta_ticks as f64 / freq as u64 as f64;
    // Clamp at 60s so a stale/erroneous QPC reading can't pull the Instant
    // far below its real epoch (which `Instant::now() - d` would reject).
    let delta = std::time::Duration::from_secs_f64(delta_secs.min(60.0));
    Some(Instant::now() - delta)
}

//! Native Windows wheel capture and targeted delivery.
//!
//! The launcher stays visible and retains focus/Z-order. We walk downward from
//! its HWND, select exactly one visible hit-testable target, and enqueue the
//! original `WM_MOUSEWHEEL` packet without normalizing delta, flags, or screen
//! coordinates.

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{GetLastError, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_TYPE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEINPUT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, ChildWindowFromPointEx, GetAncestor, GetCursorPos,
    GetForegroundWindow, GetMessageExtraInfo, GetWindow, GetWindowLongPtrW, GetWindowRect,
    GetWindowThreadProcessId, IsWindow, IsWindowVisible, PostMessageW, CWP_SKIPDISABLED,
    CWP_SKIPINVISIBLE, CWP_SKIPTRANSPARENT, GA_ROOT, GWL_EXSTYLE, GW_HWNDNEXT, MSG, WM_MOUSEWHEEL,
    WM_NULL, WS_EX_TRANSPARENT,
};

use crate::input_routing::{DeliveryResult, InputRoutingPublisher, PointerButton};

const BURST_LOCK_MS: u32 = 250;
const FOCUS_GUARD_MS: u64 = 500;

#[derive(Debug, Clone, Copy)]
struct LockedTarget {
    hwnd: isize,
    pid: u32,
    last_time: u32,
}

// Access is confined to winit's UI/message thread. A mutex makes that
// invariant explicit and keeps tests/tools from depending on thread-local
// initialization order.
static WHEEL_TARGET: OnceLock<Mutex<Option<LockedTarget>>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingWheelFocusLoss {
    launcher: isize,
    target_root: isize,
    queued_at: u64,
}

// A directly queued wheel packet does not activate its receiver by itself,
// but some receivers choose to activate from their wheel handler. Keep a
// one-shot correlation token so that only that receiver-initiated transition
// can be distinguished from a real, unrelated focus loss.
static PENDING_WHEEL_FOCUS_LOSS: OnceLock<Mutex<Option<PendingWheelFocusLoss>>> = OnceLock::new();

pub fn handle_message(raw_message: *const c_void, publisher: &InputRoutingPublisher) -> bool {
    if raw_message.is_null() {
        return false;
    }
    let message = unsafe { &*(raw_message as *const MSG) };
    if message.message != WM_MOUSEWHEEL {
        return false;
    }
    if unsafe { GetMessageExtraInfo() }.0 as usize == super::INJECT_MAGIC {
        return false;
    }
    if !publisher.snapshot().forwards_vertical_scroll() {
        return false;
    }

    let result = route_wheel(message);
    crate::debug_log!(
        "input-routing: windows wheel result={result:?} source={:?} wparam=0x{:x} lparam=0x{:x}",
        message.hwnd,
        message.wParam.0,
        message.lParam.0
    );
    // An outside wheel is launcher-suppressed even when delivery fails. Letting
    // it continue would turn a failed passthrough into launcher page input.
    true
}

/// A receiver may synchronously activate itself while processing a directly
/// queued wheel packet. Consume only the resulting, tightly bounded transition
/// to that exact receiver. Unrelated focus loss is never suppressed, and each
/// successful wheel delivery can suppress at most one transition.
pub fn consume_correlated_wheel_receiver_activation() -> bool {
    let guard = PENDING_WHEEL_FOCUS_LOSS.get_or_init(|| Mutex::new(None));
    let Some(pending) = guard.lock().ok().and_then(|mut value| value.take()) else {
        return false;
    };
    let now = unsafe { windows::Win32::System::SystemInformation::GetTickCount64() };
    let foreground = unsafe { GetForegroundWindow() }.0 as isize;
    pending_focus_loss_matches(pending, now, foreground)
}

fn route_wheel(message: &MSG) -> DeliveryResult {
    let target = locked_or_resolve_target(message.hwnd, message.pt, message.time);
    let Some(target) = target else {
        return DeliveryResult::NoTarget;
    };
    if std::env::var_os(crate::input_probe_protocol::QA_WHEEL_RECEIVER_ACTIVATION_ENV).is_some() {
        // Test-only: let the probe emulate receivers that explicitly activate
        // themselves from WM_MOUSEWHEEL so the correlated lifecycle path is
        // exercised without relying on foreground-lock timing.
        let _ = unsafe { AllowSetForegroundWindow(target.pid) };
    }
    let launcher_was_foreground = unsafe { GetForegroundWindow() } == message.hwnd;
    match unsafe {
        PostMessageW(
            Some(HWND(target.hwnd as *mut c_void)),
            message.message,
            message.wParam,
            message.lParam,
        )
    } {
        Ok(()) => {
            if launcher_was_foreground {
                let target = HWND(target.hwnd as *mut c_void);
                let target_root = unsafe { GetAncestor(target, GA_ROOT) };
                let target_root = if target_root.is_invalid() {
                    target
                } else {
                    target_root
                };
                let pending = PendingWheelFocusLoss {
                    launcher: message.hwnd.0 as isize,
                    target_root: target_root.0 as isize,
                    queued_at: unsafe {
                        windows::Win32::System::SystemInformation::GetTickCount64()
                    },
                };
                if let Ok(mut guard) = PENDING_WHEEL_FOCUS_LOSS
                    .get_or_init(|| Mutex::new(None))
                    .lock()
                {
                    *guard = Some(pending);
                }
            }
            DeliveryResult::Queued
        }
        Err(error) if error.code().0 as u32 == 5 => DeliveryResult::PermissionDenied,
        Err(error) => DeliveryResult::Failed {
            os_error: error.code().0 as i64,
        },
    }
}

fn pending_focus_loss_matches(pending: PendingWheelFocusLoss, now: u64, foreground: isize) -> bool {
    pending.launcher != 0
        && pending.target_root != 0
        && foreground == pending.target_root
        && now.wrapping_sub(pending.queued_at) <= FOCUS_GUARD_MS
}

/// Target and client coordinates resolved while the launcher still has its
/// original Z-order. Delivery happens only after the launcher hides.
pub struct PreparedClick {
    launcher: HWND,
    target: HWND,
    target_pid: u32,
    point: POINT,
    button: PointerButton,
}

pub fn prepare_click_at_cursor(
    launcher_window: u64,
    button: PointerButton,
) -> Option<PreparedClick> {
    let mut point = POINT::default();
    if unsafe { GetCursorPos(&mut point) }.is_err() {
        return None;
    }
    if launcher_window == 0 {
        return None;
    }
    let launcher = HWND(launcher_window as usize as *mut c_void);
    let (target, target_pid) = unsafe { resolve_target(launcher, point) }?;
    Some(PreparedClick {
        launcher,
        target,
        target_pid,
        point,
        button,
    })
}

/// Inject one complete click after the launcher has hidden.
///
/// The target is frozen before hide, then re-resolved after hide. This gives
/// real applications the normal Win32 mouse activation, capture and context
/// menu semantics that direct `WM_*BUTTON*` messages cannot reproduce, while
/// refusing to click if the target changed during the transition.
pub fn deliver_prepared_click(click: PreparedClick) -> DeliveryResult {
    if unsafe { IsWindowVisible(click.launcher) }.as_bool() {
        return DeliveryResult::NoTarget;
    }
    let mut current_point = POINT::default();
    if unsafe { GetCursorPos(&mut current_point) }.is_err()
        || current_point.x != click.point.x
        || current_point.y != click.point.y
    {
        return DeliveryResult::NoTarget;
    }
    let Some((current_target, current_pid)) =
        (unsafe { resolve_target(click.launcher, click.point) })
    else {
        return DeliveryResult::NoTarget;
    };
    if current_target != click.target || current_pid != click.target_pid {
        return DeliveryResult::NoTarget;
    }

    // SendInput does not identify UIPI blocking in its return value or
    // GetLastError. A harmless targeted message gives us a structured UIPI
    // preflight without changing input, focus, or Z-order.
    if let Err(error) = unsafe { PostMessageW(Some(current_target), WM_NULL, WPARAM(0), LPARAM(0)) }
    {
        return if error.code().0 as u32 == 5 {
            DeliveryResult::PermissionDenied
        } else {
            DeliveryResult::Failed {
                os_error: error.code().0 as i64,
            }
        };
    }

    let (down, up) = match click.button {
        PointerButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        PointerButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
    };
    let inputs = [mouse_button_input(down), mouse_button_input(up)];
    let inserted = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if inserted as usize == inputs.len() {
        DeliveryResult::Delivered
    } else if inserted == 1 {
        // Avoid leaving the receiver in a held-button state if an unusual
        // partial insertion accepts down but not up.
        let recovered = unsafe { SendInput(&inputs[1..], std::mem::size_of::<INPUT>() as i32) };
        if recovered == 1 {
            DeliveryResult::Delivered
        } else {
            DeliveryResult::Failed {
                os_error: unsafe { GetLastError() }.0 as i64,
            }
        }
    } else {
        DeliveryResult::Failed {
            os_error: unsafe { GetLastError() }.0 as i64,
        }
    }
}

fn mouse_button_input(
    flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
) -> INPUT {
    INPUT {
        r#type: INPUT_TYPE(0),
        Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: super::INJECT_MAGIC,
            },
        },
    }
}

fn locked_or_resolve_target(
    launcher: HWND,
    point: POINT,
    message_time: u32,
) -> Option<LockedTarget> {
    let lock = WHEEL_TARGET.get_or_init(|| Mutex::new(None));
    let mut current = lock.lock().ok()?;
    if let Some(target) = *current {
        let age = message_time.wrapping_sub(target.last_time);
        if age <= BURST_LOCK_MS && target_is_still_valid(target) {
            let refreshed = LockedTarget {
                last_time: message_time,
                ..target
            };
            *current = Some(refreshed);
            return Some(refreshed);
        }
    }
    let resolved = unsafe { resolve_target(launcher, point) }.map(|(hwnd, pid)| LockedTarget {
        hwnd: hwnd.0 as isize,
        pid,
        last_time: message_time,
    });
    *current = resolved;
    resolved
}

fn target_is_still_valid(target: LockedTarget) -> bool {
    let hwnd = HWND(target.hwnd as *mut c_void);
    if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        return false;
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid == target.pid
}

unsafe fn resolve_target(launcher: HWND, point: POINT) -> Option<(HWND, u32)> {
    let own_pid = GetCurrentProcessId();
    let mut current = GetWindow(launcher, GW_HWNDNEXT).ok();
    while let Some(hwnd) = current {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid != own_pid && top_level_candidate(hwnd, point) {
            return Some((deepest_child_at(hwnd, point), pid));
        }
        current = GetWindow(hwnd, GW_HWNDNEXT).ok();
    }
    None
}

unsafe fn top_level_candidate(hwnd: HWND, point: POINT) -> bool {
    if !IsWindowVisible(hwnd).as_bool() || !IsWindowEnabled(hwnd).as_bool() {
        return false;
    }
    if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_TRANSPARENT.0 as isize != 0 {
        return false;
    }
    let mut cloaked = 0u32;
    if DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        (&mut cloaked as *mut u32).cast(),
        std::mem::size_of::<u32>() as u32,
    )
    .is_ok()
        && cloaked != 0
    {
        return false;
    }
    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect).is_err() {
        return false;
    }
    rect_contains(rect, point)
}

fn rect_contains(rect: RECT, point: POINT) -> bool {
    point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
}

unsafe fn deepest_child_at(root: HWND, screen_point: POINT) -> HWND {
    let mut current = root;
    for _ in 0..64 {
        let mut local = screen_point;
        if !ScreenToClient(current, &mut local).as_bool() {
            break;
        }
        let child = ChildWindowFromPointEx(
            current,
            local,
            CWP_SKIPINVISIBLE | CWP_SKIPDISABLED | CWP_SKIPTRANSPARENT,
        );
        if child.is_invalid() || child == current {
            break;
        }
        current = child;
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win32_rect_right_and_bottom_edges_are_exclusive() {
        let rect = RECT {
            left: -100,
            top: -50,
            right: 100,
            bottom: 50,
        };
        assert!(rect_contains(rect, POINT { x: -100, y: -50 }));
        assert!(rect_contains(rect, POINT { x: 99, y: 49 }));
        assert!(!rect_contains(rect, POINT { x: 100, y: 0 }));
        assert!(!rect_contains(rect, POINT { x: 0, y: 50 }));
    }

    #[test]
    fn wrapping_message_time_keeps_short_bursts_locked() {
        let previous: u32 = u32::MAX - 20;
        let next: u32 = 10;
        assert!(next.wrapping_sub(previous) <= BURST_LOCK_MS);
    }

    #[test]
    fn receiver_activation_matches_only_the_correlated_target_once() {
        let pending = PendingWheelFocusLoss {
            launcher: 10,
            target_root: 20,
            queued_at: 1_000,
        };
        assert!(pending_focus_loss_matches(pending, 1_100, 20));
        assert!(!pending_focus_loss_matches(pending, 1_100, 30));
        assert!(!pending_focus_loss_matches(
            pending,
            1_000 + FOCUS_GUARD_MS + 1,
            20
        ));
    }
}

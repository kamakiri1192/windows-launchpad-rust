//! Native Windows wheel capture and targeted delivery.
//!
//! The launcher stays visible and retains focus/Z-order. We walk downward from
//! its HWND, select exactly one visible hit-testable target, and deliver the
//! original `WM_MOUSEWHEEL` / `WM_POINTERWHEEL` packet without normalizing
//! delta, flags, pointer identity, or screen coordinates.

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{GetLastError, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::{
    CreateRectRgn, DeleteObject, GetWindowRgn, ScreenToClient, SetWindowRgn, HGDIOBJ,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    IsWindowEnabled, SendInput, INPUT, INPUT_TYPE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEINPUT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, ChildWindowFromPointEx, GetAncestor, GetClassNameW, GetCursorPos,
    GetForegroundWindow, GetMessageExtraInfo, GetWindow, GetWindowLongPtrW, GetWindowRect,
    GetWindowThreadProcessId, IsWindow, IsWindowVisible, PostMessageW, SendMessageTimeoutW,
    WindowFromPoint, CWP_SKIPDISABLED, CWP_SKIPINVISIBLE, CWP_SKIPTRANSPARENT, GA_ROOT,
    GWL_EXSTYLE, GW_HWNDNEXT, MSG, SMTO_ABORTIFHUNG, SMTO_ERRORONEXIT, WM_MOUSEWHEEL, WM_NULL,
    WM_POINTERWHEEL, WS_EX_TRANSPARENT,
};

use crate::input_routing::{DeliveryResult, InputRoutingPublisher, PointerButton};

const BURST_LOCK_MS: u32 = 250;
const FOCUS_GUARD_MS: u64 = 500;

#[derive(Debug, Clone, Copy)]
struct LockedTarget {
    hwnd: isize,
    root: isize,
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
    if message.message != WM_MOUSEWHEEL && message.message != WM_POINTERWHEEL {
        return false;
    }
    if unsafe { GetMessageExtraInfo() }.0 as usize == super::INJECT_MAGIC {
        return false;
    }
    if !publisher.snapshot().forwards_vertical_scroll() {
        return false;
    }

    let result = route_wheel(message);
    if std::env::var_os(crate::input_probe_protocol::INPUT_ROUTING_QA_ENV).is_some() {
        let root = unsafe { GetAncestor(message.hwnd, GA_ROOT) };
        let point = wheel_screen_point(message.lParam);
        let target = locked_or_resolve_target(root, point, message.time);
        eprintln!(
            "input-routing-qa: wheel message=0x{:x} source={:?} root={root:?} point=({}, {}) target={:?} class={} result={result:?}",
            message.message,
            message.hwnd,
            point.x,
            point.y,
            target.map(|value| value.hwnd),
            target.map_or_else(
                || "<none>".to_owned(),
                |value| window_class_name(HWND(value.hwnd as *mut c_void))
            )
        );
    }
    crate::debug_log!(
        "input-routing: windows wheel message=0x{:x} result={result:?} source={:?} wparam=0x{:x} lparam=0x{:x}",
        message.message,
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
    // Wheel messages may be addressed to a focused child/input sink rather
    // than the launcher's top-level HWND.
    // Z-order traversal is meaningful only between top-level windows.
    let launcher = unsafe { GetAncestor(message.hwnd, GA_ROOT) };
    let launcher = if launcher.is_invalid() {
        message.hwnd
    } else {
        launcher
    };
    // Both supported wheel messages own their screen-space point in lParam.
    // `MSG.pt` is queue metadata and can be DPI-virtualized differently
    // (observed on mixed-scale desktops), selecting the wrong window.
    let target =
        locked_or_resolve_target(launcher, wheel_screen_point(message.lParam), message.time);
    let Some(target) = target else {
        return DeliveryResult::NoTarget;
    };
    crate::debug_log!(
        "input-routing: windows wheel target root={:?} ({}) dispatch={:?} ({}) pid={}",
        HWND(target.root as *mut c_void),
        window_class_name(HWND(target.root as *mut c_void)),
        HWND(target.hwnd as *mut c_void),
        window_class_name(HWND(target.hwnd as *mut c_void)),
        target.pid
    );
    if std::env::var_os(crate::input_probe_protocol::QA_WHEEL_RECEIVER_ACTIVATION_ENV).is_some() {
        // Test-only: let the probe emulate receivers that explicitly activate
        // themselves from a wheel handler so the correlated lifecycle path is
        // exercised without relying on foreground-lock timing.
        let _ = unsafe { AllowSetForegroundWindow(target.pid) };
    }
    let launcher_was_foreground = unsafe { GetForegroundWindow() } == launcher;
    let pending = launcher_was_foreground.then(|| PendingWheelFocusLoss {
        launcher: launcher.0 as isize,
        target_root: target.root,
        queued_at: unsafe { windows::Win32::System::SystemInformation::GetTickCount64() },
    });
    if let Some(pending) = pending {
        // Arm before posting: the receiver processes its queue on another
        // thread and may activate itself before PostMessageW returns.
        if let Ok(mut guard) = PENDING_WHEEL_FOCUS_LOSS
            .get_or_init(|| Mutex::new(None))
            .lock()
        {
            *guard = Some(pending);
        }
    }
    let target_hwnd = HWND(target.hwnd as *mut c_void);
    let result = if window_class_name(target_hwnd) == "Chrome_WidgetWin_1" {
        send_chromium_wheel(launcher, target_hwnd, message)
    } else {
        match unsafe {
            PostMessageW(
                Some(target_hwnd),
                message.message,
                message.wParam,
                message.lParam,
            )
        } {
            Ok(()) => DeliveryResult::Queued,
            Err(error) => {
                if error.code().0 as u32 == 5 {
                    DeliveryResult::PermissionDenied
                } else {
                    DeliveryResult::Failed {
                        os_error: error.code().0 as i64,
                    }
                }
            }
        }
    };
    if !matches!(result, DeliveryResult::Queued | DeliveryResult::Delivered) {
        // Do not clear a newer token or one already consumed by a
        // receiver-triggered focus transition.
        if let Some(pending) = pending {
            if let Ok(mut guard) = PENDING_WHEEL_FOCUS_LOSS
                .get_or_init(|| Mutex::new(None))
                .lock()
            {
                if guard.as_ref() == Some(&pending) {
                    *guard = None;
                }
            }
        }
    }
    result
}

fn send_chromium_wheel(launcher: HWND, target: HWND, message: &MSG) -> DeliveryResult {
    // Chromium performs a second WindowFromPoint check before handing wheel
    // input to its view tree. Exclude only the launcher from spatial hit
    // testing while Chromium processes this exact packet synchronously.
    // Restoring the default region before returning leaves rendering,
    // visibility, focus, activation, window styles, and Z-order unchanged.
    let saved_region = unsafe { CreateRectRgn(0, 0, 0, 0) };
    if saved_region.is_invalid() || unsafe { GetWindowRgn(launcher, saved_region) }.0 != 0 {
        if !saved_region.is_invalid() {
            let _ = unsafe { DeleteObject(HGDIOBJ(saved_region.0)) };
        }
        return DeliveryResult::Unsupported;
    }
    let empty_region = unsafe { CreateRectRgn(0, 0, 0, 0) };
    if empty_region.is_invalid()
        || unsafe { SetWindowRgn(launcher, Some(empty_region), false) } == 0
    {
        let _ = unsafe { DeleteObject(HGDIOBJ(saved_region.0)) };
        if !empty_region.is_invalid() {
            let _ = unsafe { DeleteObject(HGDIOBJ(empty_region.0)) };
        }
        return DeliveryResult::Failed {
            os_error: unsafe { GetLastError() }.0 as i64,
        };
    }
    if std::env::var_os(crate::input_probe_protocol::INPUT_ROUTING_QA_ENV).is_some() {
        eprintln!(
            "input-routing-qa: chromium compatibility WindowFromPoint={:?}",
            unsafe { WindowFromPoint(wheel_screen_point(message.lParam)) }
        );
    }
    let mut receiver_result = 0usize;
    let sent = unsafe {
        SendMessageTimeoutW(
            target,
            message.message,
            message.wParam,
            message.lParam,
            SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT,
            100,
            Some(&mut receiver_result),
        )
    };
    // Successful SetWindowRgn transfers empty_region to the system. Passing
    // None restores the original default rectangular region.
    let restored = unsafe { SetWindowRgn(launcher, None, false) } != 0;
    let _ = unsafe { DeleteObject(HGDIOBJ(saved_region.0)) };
    if !restored {
        return DeliveryResult::Failed {
            os_error: unsafe { GetLastError() }.0 as i64,
        };
    }
    if sent.0 != 0 {
        DeliveryResult::Delivered
    } else {
        DeliveryResult::Failed {
            os_error: unsafe { GetLastError() }.0 as i64,
        }
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
    target_root: HWND,
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
    let (_, target_root, target_pid) = unsafe { resolve_target(launcher, point) }?;
    let target = unsafe { deepest_child_at(target_root, point) };
    Some(PreparedClick {
        launcher,
        target,
        target_root,
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
    let current_target = unsafe { WindowFromPoint(click.point) };
    if current_target.is_invalid() {
        return DeliveryResult::NoTarget;
    }
    let current_root = unsafe { GetAncestor(current_target, GA_ROOT) };
    let current_root = if current_root.is_invalid() {
        current_target
    } else {
        current_root
    };
    let current_target = unsafe { deepest_child_at(current_root, click.point) };
    let mut current_pid = 0;
    unsafe { GetWindowThreadProcessId(current_root, Some(&mut current_pid)) };
    if current_root != click.target_root
        || current_target != click.target
        || current_pid != click.target_pid
    {
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
    let resolved =
        unsafe { resolve_target(launcher, point) }.map(|(hwnd, root, pid)| LockedTarget {
            hwnd: hwnd.0 as isize,
            root: root.0 as isize,
            pid,
            last_time: message_time,
        });
    *current = resolved;
    resolved
}

fn target_is_still_valid(target: LockedTarget) -> bool {
    let hwnd = HWND(target.hwnd as *mut c_void);
    let root = HWND(target.root as *mut c_void);
    if !unsafe { IsWindow(Some(hwnd)) }.as_bool() || !unsafe { IsWindow(Some(root)) }.as_bool() {
        return false;
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(root, Some(&mut pid)) };
    pid == target.pid && unsafe { GetAncestor(hwnd, GA_ROOT) } == root
}

unsafe fn resolve_target(launcher: HWND, point: POINT) -> Option<(HWND, HWND, u32)> {
    let own_pid = GetCurrentProcessId();
    let mut current = GetWindow(launcher, GW_HWNDNEXT).ok();
    while let Some(hwnd) = current {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid != own_pid && top_level_candidate(hwnd, point) {
            // Post the wheel packet to the target application's top-level
            // sink. In modern frameworks (Chromium/Electron in particular),
            // a spatial child may accept a posted message without forwarding
            // it into the framework's input pipeline. Native mouse wheel input
            // is addressed to a focus sink and bubbles through DefWindowProc;
            // after the launcher takes foreground, however, the covered
            // thread no longer has a usable focus HWND to query. The stable
            // top-level sink preserves the packet and its target process
            // without changing focus or Z-order.
            return Some((hwnd, hwnd, pid));
        }
        current = GetWindow(hwnd, GW_HWNDNEXT).ok();
    }
    None
}

fn window_class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 128];
    let length = unsafe { GetClassNameW(hwnd, &mut buffer) };
    if length <= 0 {
        return "<unknown>".to_owned();
    }
    String::from_utf16_lossy(&buffer[..length as usize])
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

fn wheel_screen_point(lparam: LPARAM) -> POINT {
    let packed = lparam.0 as u32;
    POINT {
        x: (packed as u16 as i16) as i32,
        y: ((packed >> 16) as u16 as i16) as i32,
    }
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
    fn wheel_lparam_preserves_signed_virtual_screen_coordinates() {
        let x = -1_200i16;
        let y = 340i16;
        let packed = (x as u16 as u32) | ((y as u16 as u32) << 16);
        assert_eq!(
            wheel_screen_point(LPARAM(packed as isize)),
            POINT {
                x: x as i32,
                y: y as i32
            }
        );
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

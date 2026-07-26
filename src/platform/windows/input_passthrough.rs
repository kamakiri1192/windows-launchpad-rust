//! Native Windows wheel capture and targeted delivery.
//!
//! The launcher stays visible and retains focus/Z-order. We walk downward from
//! its HWND, select exactly one visible hit-testable target, and enqueue the
//! original `WM_MOUSEWHEEL` packet without normalizing delta, flags, or screen
//! coordinates.

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
use windows::Win32::UI::WindowsAndMessaging::{
    ChildWindowFromPointEx, GetCursorPos, GetMessageExtraInfo, GetWindow, GetWindowLongPtrW,
    GetWindowRect, GetWindowThreadProcessId, IsWindow, IsWindowVisible, PostMessageW,
    CWP_SKIPDISABLED, CWP_SKIPINVISIBLE, CWP_SKIPTRANSPARENT, GWL_EXSTYLE, GW_HWNDNEXT, MSG,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WS_EX_TRANSPARENT,
};

use crate::input_routing::{DeliveryResult, InputRoutingPublisher, PointerButton};

const BURST_LOCK_MS: u32 = 250;

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

fn route_wheel(message: &MSG) -> DeliveryResult {
    let target = locked_or_resolve_target(message.hwnd, message.pt, message.time);
    let Some(target) = target else {
        return DeliveryResult::NoTarget;
    };
    match unsafe {
        PostMessageW(
            Some(HWND(target.hwnd as *mut c_void)),
            message.message,
            message.wParam,
            message.lParam,
        )
    } {
        Ok(()) => DeliveryResult::Queued,
        Err(error) if error.code().0 as u32 == 5 => DeliveryResult::PermissionDenied,
        Err(error) => DeliveryResult::Failed {
            os_error: error.code().0 as i64,
        },
    }
}

/// Target and client coordinates resolved while the launcher still has its
/// original Z-order. Delivery happens only after the launcher hides.
pub struct PreparedClick {
    target: HWND,
    coordinates: LPARAM,
    down: u32,
    up: u32,
    down_keys: usize,
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
    let (target, _) = unsafe { resolve_target(launcher, point) }?;

    let mut local = point;
    if !unsafe { ScreenToClient(target, &mut local) }.as_bool() {
        return None;
    }
    let coordinates = LPARAM(
        (local.x as i16 as u16 as usize | ((local.y as i16 as u16 as usize) << 16)) as isize,
    );
    let (down, up, down_keys) = match button {
        PointerButton::Left => (WM_LBUTTONDOWN, WM_LBUTTONUP, 0x0001usize),
        PointerButton::Right => (WM_RBUTTONDOWN, WM_RBUTTONUP, 0x0002usize),
    };
    Some(PreparedClick {
        target,
        coordinates,
        down,
        up,
        down_keys,
    })
}

/// Enqueue one complete click without injecting into the global input stream.
pub fn deliver_prepared_click(click: PreparedClick) -> DeliveryResult {
    let down_result = unsafe {
        PostMessageW(
            Some(click.target),
            click.down,
            WPARAM(click.down_keys),
            click.coordinates,
        )
    };
    if let Err(error) = down_result {
        return delivery_error(error);
    }
    match unsafe { PostMessageW(Some(click.target), click.up, WPARAM(0), click.coordinates) } {
        Ok(()) => DeliveryResult::Queued,
        Err(error) => delivery_error(error),
    }
}

fn delivery_error(error: windows::core::Error) -> DeliveryResult {
    if error.code().0 as u32 == 5 {
        DeliveryResult::PermissionDenied
    } else {
        DeliveryResult::Failed {
            os_error: error.code().0 as i64,
        }
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
}

//! Native macOS input capture and targeted delivery.
//!
//! A local AppKit event monitor observes the original events before winit.
//! Scroll events in an outside region are copied and posted to the owner PID
//! of the window below the launcher. Mouse button events continue to winit so
//! the pure router can distinguish click from drag; original down/up CGEvents
//! are retained and replayed only after a click is confirmed.

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventPhase, NSEventType, NSView, NSWindow};
use objc2_core_graphics::{CGEvent, CGEventField};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::input_routing::{DeliveryResult, InputRoutingPublisher, PointerButton, RouterState};

#[derive(Debug, Clone, Copy)]
struct MacTarget {
    window_number: isize,
    pid: i32,
}

struct ClickCapture {
    button: PointerButton,
    target: MacTarget,
    down: Retained<CGEvent>,
    up: Option<Retained<CGEvent>>,
}

#[derive(Default)]
struct MonitorState {
    click: Option<ClickCapture>,
    scroll_target: Option<MacTarget>,
}

/// Keeps the AppKit local monitor registered for the window lifetime.
pub struct MacInputPassthrough {
    monitor: Retained<AnyObject>,
    state: Rc<RefCell<MonitorState>>,
}

impl MacInputPassthrough {
    pub fn install(
        window: &winit::window::Window,
        publisher: InputRoutingPublisher,
    ) -> Option<Self> {
        let launcher_window_number = launcher_window_number(window)?;
        let state = Rc::new(RefCell::new(MonitorState::default()));
        let callback_state = state.clone();
        let handler = RcBlock::new(move |event_ptr: NonNull<NSEvent>| -> *mut NSEvent {
            let event = unsafe { event_ptr.as_ref() };
            let original = event_ptr.as_ptr();
            let Some(cg_event) = event.CGEvent() else {
                return original;
            };
            if CGEvent::integer_value_field(Some(&cg_event), CGEventField::EventSourceUserData)
                == crate::input_probe_protocol::MACOS_PRODUCT_EVENT_TAG
            {
                return original;
            }

            let snapshot = publisher.snapshot();
            let event_type = event.r#type();
            if event_type == NSEventType::ScrollWheel {
                if !snapshot.forwards_vertical_scroll() {
                    callback_state.borrow_mut().scroll_target = None;
                    return original;
                }

                let mut state = callback_state.borrow_mut();
                let phase = event.phase();
                let momentum = event.momentumPhase();
                if phase.contains(NSEventPhase::Began)
                    || phase.contains(NSEventPhase::MayBegin)
                    || (phase.is_empty() && momentum.is_empty())
                    || state.scroll_target.is_none()
                {
                    state.scroll_target =
                        target_below(launcher_window_number, NSEvent::mouseLocation());
                }
                let result = state
                    .scroll_target
                    .map_or(DeliveryResult::NoTarget, |target| {
                        post_original(target, &cg_event)
                    });
                crate::debug_log!(
                    "input-routing: macos scroll result={result:?} target={:?} phase={phase:?} momentum={momentum:?}",
                    state.scroll_target
                );
                if phase.contains(NSEventPhase::Cancelled) || momentum.contains(NSEventPhase::Ended)
                {
                    state.scroll_target = None;
                }
                // Outside wheel input is never allowed to become launcher
                // input, including explicit delivery-failure cases.
                return std::ptr::null_mut();
            }

            match event_type {
                NSEventType::LeftMouseDown | NSEventType::RightMouseDown
                    if snapshot.visible
                        && matches!(
                            snapshot.region,
                            crate::input_routing::InputRegion::OutsideTransparent
                        )
                        && matches!(snapshot.router_state, RouterState::Idle) =>
                {
                    let button = if event_type == NSEventType::LeftMouseDown {
                        PointerButton::Left
                    } else {
                        PointerButton::Right
                    };
                    let target = target_below(launcher_window_number, NSEvent::mouseLocation());
                    callback_state.borrow_mut().click = target.map(|target| ClickCapture {
                        button,
                        target,
                        down: cg_event,
                        up: None,
                    });
                }
                NSEventType::LeftMouseUp | NSEventType::RightMouseUp => {
                    let button = if event_type == NSEventType::LeftMouseUp {
                        PointerButton::Left
                    } else {
                        PointerButton::Right
                    };
                    let expected_pending = match button {
                        PointerButton::Left => {
                            matches!(snapshot.router_state, RouterState::LeftPending { .. })
                        }
                        PointerButton::Right => {
                            matches!(snapshot.router_state, RouterState::RightPending { .. })
                        }
                    };
                    let mut state = callback_state.borrow_mut();
                    if expected_pending {
                        if let Some(capture) = state
                            .click
                            .as_mut()
                            .filter(|capture| capture.button == button)
                        {
                            capture.up = Some(cg_event);
                        }
                    } else if state
                        .click
                        .as_ref()
                        .is_some_and(|capture| capture.button == button)
                    {
                        state.click = None;
                    }
                }
                _ => {}
            }
            original
        });
        let mask = NSEventMask::ScrollWheel
            | NSEventMask::LeftMouseDown
            | NSEventMask::LeftMouseUp
            | NSEventMask::RightMouseDown
            | NSEventMask::RightMouseUp;
        let monitor =
            unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &handler) }?;
        Some(Self { monitor, state })
    }

    /// Deliver the retained native down/up pair after the router confirms a
    /// click and the launcher has hidden. No coordinate-only reconstruction
    /// or session-wide posting is used.
    pub fn deliver_click(&self, button: PointerButton) -> DeliveryResult {
        let capture = self.state.borrow_mut().click.take();
        let Some(capture) = capture.filter(|capture| capture.button == button) else {
            return DeliveryResult::NoTarget;
        };
        let Some(up) = capture.up else {
            return DeliveryResult::Failed { os_error: 0 };
        };
        let down_result = post_original(capture.target, &capture.down);
        if !matches!(down_result, DeliveryResult::Queued) {
            return down_result;
        }
        post_original(capture.target, &up)
    }
}

impl Drop for MacInputPassthrough {
    fn drop(&mut self) {
        unsafe { NSEvent::removeMonitor(&self.monitor) };
    }
}

fn launcher_window_number(window: &winit::window::Window) -> Option<isize> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    let view = unsafe { &*(handle.ns_view.as_ptr() as *const NSView) };
    view.window().map(|window| window.windowNumber())
}

fn post_original(target: MacTarget, event: &CGEvent) -> DeliveryResult {
    if !objc2_core_graphics::CGPreflightPostEventAccess() {
        return DeliveryResult::PermissionDenied;
    }
    CGEvent::set_integer_value_field(
        Some(event),
        CGEventField::EventSourceUserData,
        crate::input_probe_protocol::MACOS_PRODUCT_EVENT_TAG,
    );
    // `CGEventPostToPid` selects the receiver process but otherwise retains
    // the source event's window metadata. Point both native routing fields at
    // the window resolved below the launcher so AppKit dispatches to that
    // exact window and computes receiver-local coordinates from the original
    // screen position.
    CGEvent::set_integer_value_field(
        Some(event),
        CGEventField::MouseEventWindowUnderMousePointer,
        target.window_number as i64,
    );
    CGEvent::set_integer_value_field(
        Some(event),
        CGEventField::MouseEventWindowUnderMousePointerThatCanHandleThisEvent,
        target.window_number as i64,
    );
    CGEvent::post_to_pid(target.pid, Some(event));
    DeliveryResult::Queued
}

fn target_below(
    launcher_window_number: isize,
    point: objc2_foundation::NSPoint,
) -> Option<MacTarget> {
    let main_thread = MainThreadMarker::new()?;
    let target_window = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(
        point,
        launcher_window_number,
        main_thread,
    );
    if target_window <= 0 {
        return None;
    }
    let pid = owner_pid_for_window(target_window as u32)?;
    (pid != std::process::id() as i32).then_some(MacTarget {
        window_number: target_window,
        pid,
    })
}

fn owner_pid_for_window(window_number: u32) -> Option<i32> {
    unsafe {
        let descriptions = CGWindowListCopyWindowInfo(1 << 3, window_number);
        if descriptions.is_null() {
            return None;
        }
        let result = (CFArrayGetCount(descriptions) > 0).then(|| {
            let dictionary = CFArrayGetValueAtIndex(descriptions, 0);
            let number = CFDictionaryGetValue(dictionary, kCGWindowOwnerPID);
            let mut pid = 0i32;
            (!number.is_null() && CFNumberGetValue(number, 3, (&mut pid as *mut i32).cast()))
                .then_some(pid)
        });
        CFRelease(descriptions);
        result.flatten()
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    static kCGWindowOwnerPID: *const c_void;
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFArrayGetCount(array: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
    fn CFDictionaryGetValue(dictionary: *const c_void, key: *const c_void) -> *const c_void;
    fn CFNumberGetValue(number: *const c_void, number_type: isize, value: *mut c_void) -> bool;
    fn CFRelease(value: *const c_void);
}

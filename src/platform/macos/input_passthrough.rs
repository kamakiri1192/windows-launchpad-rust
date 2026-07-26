//! Native macOS input capture and targeted delivery.
//!
//! A local AppKit event monitor observes the original events before winit.
//! Scroll events in an outside region are copied and posted to the owner PID
//! of the window below the launcher. Mouse button events continue to winit so
//! the pure router can distinguish click from drag; original down/up CGEvents
//! are retained and replayed only after a click is confirmed.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventPhase, NSEventType, NSView, NSWindow};
use objc2_core_foundation::{
    kCFRunLoopCommonModes, CFMachPort, CFRetained, CFRunLoop, CFRunLoopSource,
};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::input_routing::{DeliveryResult, InputRoutingPublisher, PointerButton, RouterState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

struct EventTapRegistration {
    port: CFRetained<CFMachPort>,
    source: CFRetained<CFRunLoopSource>,
    run_loop: CFRetained<CFRunLoop>,
}

impl Drop for EventTapRegistration {
    fn drop(&mut self) {
        self.run_loop
            .remove_source(Some(&self.source), unsafe { kCFRunLoopCommonModes });
    }
}

struct TapContext {
    launcher_window_number: isize,
    publisher: InputRoutingPublisher,
    scroll_target: RefCell<Option<MacTarget>>,
    registration: RefCell<Option<EventTapRegistration>>,
    permission_prompted: Cell<bool>,
}

impl TapContext {
    fn permissions_available(&self) -> bool {
        objc2_core_graphics::CGPreflightListenEventAccess()
            && objc2_core_graphics::CGPreflightPostEventAccess()
    }

    fn request_permissions(&self) {
        if self.permission_prompted.replace(true) {
            return;
        }
        let listen = objc2_core_graphics::CGRequestListenEventAccess();
        let post = objc2_core_graphics::CGRequestPostEventAccess();
        eprintln!(
            "input-routing: macOS input passthrough requires Input Monitoring and Accessibility; \
             requested system approval (listen={listen}, post={post}). Enable Launchpad in \
             System Settings > Privacy & Security, then retry the gesture"
        );
    }

    fn ensure_event_tap(&self) -> bool {
        if self.registration.borrow().is_some() {
            return true;
        }
        if !self.permissions_available() {
            return false;
        }
        let mask = 1u64 << CGEventType::ScrollWheel.0;
        let user_info = (self as *const Self).cast_mut().cast::<c_void>();
        let Some(port) = (unsafe {
            CGEvent::tap_create(
                CGEventTapLocation::SessionEventTap,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::Default,
                mask,
                Some(scroll_event_tap_callback),
                user_info,
            )
        }) else {
            eprintln!(
                "input-routing: failed to create active macOS session event tap despite granted permissions"
            );
            return false;
        };
        let Some(source) = CFMachPort::new_run_loop_source(None, Some(&port), 0) else {
            eprintln!("input-routing: failed to create macOS event-tap run-loop source");
            return false;
        };
        let Some(run_loop) = CFRunLoop::main() else {
            eprintln!("input-routing: macOS main run loop unavailable for event tap");
            return false;
        };
        run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
        *self.registration.borrow_mut() = Some(EventTapRegistration {
            port,
            source,
            run_loop,
        });
        crate::debug_log!("input-routing: installed macOS active session scroll event tap");
        true
    }

    fn reenable_event_tap(&self) {
        if let Some(registration) = self.registration.borrow().as_ref() {
            CGEvent::tap_enable(&registration.port, true);
            eprintln!("input-routing: re-enabled disabled macOS scroll event tap");
        }
    }
}

/// Keeps the AppKit local monitor registered for the window lifetime.
pub struct MacInputPassthrough {
    monitor: Retained<AnyObject>,
    state: Rc<RefCell<MonitorState>>,
    _tap_context: Rc<TapContext>,
}

impl MacInputPassthrough {
    pub fn install(
        window: &winit::window::Window,
        publisher: InputRoutingPublisher,
    ) -> Option<Self> {
        let launcher_window_number = launcher_window_number(window)?;
        let state = Rc::new(RefCell::new(MonitorState::default()));
        let tap_context = Rc::new(TapContext {
            launcher_window_number,
            publisher: publisher.clone(),
            scroll_target: RefCell::new(None),
            registration: RefCell::new(None),
            permission_prompted: Cell::new(false),
        });
        crate::debug_log!(
            "input-routing: macOS permissions listen={} post={} launcher_window={} pid={}",
            objc2_core_graphics::CGPreflightListenEventAccess(),
            objc2_core_graphics::CGPreflightPostEventAccess(),
            launcher_window_number,
            std::process::id()
        );
        if std::env::var_os(crate::input_probe_protocol::INPUT_ROUTING_QA_ENV).is_some()
            && !tap_context.permissions_available()
        {
            tap_context.request_permissions();
        }
        tap_context.ensure_event_tap();
        let callback_state = state.clone();
        let callback_tap_context = tap_context.clone();
        let handler = RcBlock::new(move |event_ptr: NonNull<NSEvent>| -> *mut NSEvent {
            let event = unsafe { event_ptr.as_ref() };
            let original = event_ptr.as_ptr();
            let Some(cg_event) = event.CGEvent() else {
                return original;
            };
            if CGEvent::integer_value_field(Some(&cg_event), CGEventField::EventSourceUserData)
                == crate::input_probe_protocol::MACOS_PRODUCT_EVENT_TAG
            {
                crate::debug_log!(
                    "input-routing: macOS local monitor ignored private replacement event"
                );
                return original;
            }

            let snapshot = publisher.snapshot();
            let event_type = event.r#type();
            if event_type == NSEventType::ScrollWheel {
                if !snapshot.forwards_vertical_scroll() {
                    return original;
                }
                let result = if callback_tap_context.ensure_event_tap() {
                    // If a forwarded outside wheel reaches this local monitor,
                    // the session tap did not retarget the current packet.
                    DeliveryResult::Failed { os_error: -2 }
                } else {
                    callback_tap_context.request_permissions();
                    DeliveryResult::PermissionDenied
                };
                eprintln!(
                    "input-routing: macOS scroll fallback consumed result={result:?} \
                     window={} pid={} point={:?}; no launcher page input allowed",
                    launcher_window_number,
                    std::process::id(),
                    NSEvent::mouseLocation()
                );
                if matches!(result, DeliveryResult::Failed { .. }) {
                    callback_tap_context.scroll_target.borrow_mut().take();
                }
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
                    crate::debug_log!(
                        "input-routing: macOS captured {button:?} down target={target:?} \
                         launcher_window={launcher_window_number} event_window={} point={:?}",
                        event.windowNumber(),
                        NSEvent::mouseLocation()
                    );
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
        Some(Self {
            monitor,
            state,
            _tap_context: tap_context,
        })
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
        let current = current_target();
        crate::debug_log!(
            "input-routing: macOS click revalidation button={button:?} captured={:?} current={current:?}",
            capture.target
        );
        if current != Some(capture.target) {
            return DeliveryResult::NoTarget;
        }
        let down_result = post_hidden_click(&capture.down);
        if !matches!(down_result, DeliveryResult::Queued) {
            return down_result;
        }
        post_hidden_click(&up)
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

fn post_hidden_click(event: &CGEvent) -> DeliveryResult {
    if !objc2_core_graphics::CGPreflightPostEventAccess() {
        objc2_core_graphics::CGRequestPostEventAccess();
        eprintln!(
            "input-routing: macOS click delivery denied; enable Accessibility for Launchpad in \
             System Settings > Privacy & Security and retry"
        );
        return DeliveryResult::PermissionDenied;
    }
    CGEvent::set_integer_value_field(
        Some(event),
        CGEventField::EventSourceUserData,
        crate::input_probe_protocol::MACOS_PRODUCT_EVENT_TAG,
    );
    crate::debug_log!(
        "input-routing: macOS queued hidden-window replacement event type={:?} point={:?} \
         flags={:?}",
        CGEvent::r#type(Some(event)),
        CGEvent::location(Some(event)),
        CGEvent::flags(Some(event))
    );
    // Click delivery occurs only after the launcher is hidden. Reinsert the
    // retained complete native event before session annotation so
    // WindowServer/AppKit performs ordinary hit testing and window/view
    // dispatch. No event is reconstructed from coordinates, and the hidden
    // launcher cannot receive its own replacement click.
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(event));
    DeliveryResult::Queued
}

unsafe extern "C-unwind" fn scroll_event_tap_callback(
    _proxy: objc2_core_graphics::CGEventTapProxy,
    event_type: CGEventType,
    event_ptr: NonNull<CGEvent>,
    user_info: *mut c_void,
) -> *mut CGEvent {
    let event = unsafe { event_ptr.as_ref() };
    let context = unsafe { &*(user_info.cast::<TapContext>()) };
    if event_type == CGEventType::TapDisabledByTimeout
        || event_type == CGEventType::TapDisabledByUserInput
    {
        context.reenable_event_tap();
        return event_ptr.as_ptr();
    }
    if event_type != CGEventType::ScrollWheel {
        return event_ptr.as_ptr();
    }
    if !context.publisher.snapshot().forwards_vertical_scroll() {
        context.scroll_target.borrow_mut().take();
        return event_ptr.as_ptr();
    }
    let point_delta =
        CGEvent::integer_value_field(Some(event), CGEventField::ScrollWheelEventPointDeltaAxis1);
    let raw_delta =
        CGEvent::integer_value_field(Some(event), CGEventField::ScrollWheelEventDeltaAxis1);
    let fixed_delta =
        CGEvent::double_value_field(Some(event), CGEventField::ScrollWheelEventFixedPtDeltaAxis1);
    let phase =
        CGEvent::integer_value_field(Some(event), CGEventField::ScrollWheelEventScrollPhase);
    let momentum =
        CGEvent::integer_value_field(Some(event), CGEventField::ScrollWheelEventMomentumPhase);
    if point_delta == 0 && raw_delta == 0 && fixed_delta == 0.0 && phase == 0 && momentum == 0 {
        crate::debug_log!("input-routing: macOS outside horizontal scroll suppressed");
        return std::ptr::null_mut();
    }
    let Some(main_thread) = MainThreadMarker::new() else {
        return std::ptr::null_mut();
    };
    let pointer = NSEvent::mouseLocation();
    let front_window =
        NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(pointer, 0, main_thread);
    if front_window != context.launcher_window_number {
        context.scroll_target.borrow_mut().take();
        return event_ptr.as_ptr();
    }

    let phase_began =
        phase & (NSEventPhase::Began.bits() | NSEventPhase::MayBegin.bits()) as i64 != 0;
    let unphased = phase == 0 && momentum == 0;
    let mut scroll_target = context.scroll_target.borrow_mut();
    if phase_began || unphased || scroll_target.is_none() {
        *scroll_target = target_below(context.launcher_window_number, pointer);
    }
    let Some(target) = *scroll_target else {
        crate::debug_log!(
            "input-routing: macOS scroll suppressed: no target below launcher window={} point={pointer:?}",
            context.launcher_window_number
        );
        return std::ptr::null_mut();
    };
    CGEvent::set_integer_value_field(
        Some(event),
        CGEventField::EventTargetUnixProcessID,
        target.pid as i64,
    );
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
    crate::debug_log!(
        "input-routing: macOS scroll retargeted target={target:?} phase={phase:#x} \
         momentum={momentum:#x} point_delta={point_delta} raw_delta={raw_delta} \
         fixed_delta={fixed_delta}"
    );
    if phase & NSEventPhase::Cancelled.bits() as i64 != 0
        || momentum & NSEventPhase::Ended.bits() as i64 != 0
    {
        scroll_target.take();
    }
    event_ptr.as_ptr()
}

fn current_target() -> Option<MacTarget> {
    let main_thread = MainThreadMarker::new()?;
    let target_window = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(
        NSEvent::mouseLocation(),
        0,
        main_thread,
    );
    if target_window <= 0 {
        return None;
    }
    owner_pid_for_window(target_window as u32).map(|pid| MacTarget {
        window_number: target_window,
        pid,
    })
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

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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Instant;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventPhase, NSEventType, NSScreen, NSView, NSWindow};
use objc2_core_foundation::{
    kCFRunLoopDefaultMode, CFMachPort, CFRetained, CFRunLoop, CFRunLoopSource, CGPoint, CGRect,
    CGSize,
};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
};
use winit::event_loop::EventLoopProxy;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::app::event::UserEvent;
use crate::input_routing::{
    DeliveryResult, InputRoutingPublisher, NativeScrollPhase, PhysicalPoint, PointerButton,
    RawScrollEvent, RouterState, ScrollPhaseCapability, ScrollSource,
};

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

/// Maps AppKit's system-uptime timestamp onto the app-wide monotonic epoch.
/// The first native packet is anchored to its callback `Instant`; following
/// packets retain AppKit's higher-resolution event spacing.
pub(crate) struct NativeScrollClock {
    app_origin: Instant,
    anchor: Cell<Option<(f64, u64)>>,
    last_emitted_us: Cell<Option<u64>>,
}

impl NativeScrollClock {
    pub(crate) fn new(app_origin: Instant) -> Self {
        Self {
            app_origin,
            anchor: Cell::new(None),
            last_emitted_us: Cell::new(None),
        }
    }

    pub(crate) fn timestamp_us(&self, native_seconds: f64, callback_now: Instant) -> Option<u64> {
        if !native_seconds.is_finite() || native_seconds < 0.0 {
            return None;
        }
        let (native_anchor, app_anchor_us) = self.anchor.get().unwrap_or_else(|| {
            let app_anchor_us = callback_now
                .saturating_duration_since(self.app_origin)
                .as_micros()
                .min(u128::from(u64::MAX)) as u64;
            let anchor = (native_seconds, app_anchor_us);
            self.anchor.set(Some(anchor));
            anchor
        });
        let elapsed = native_seconds - native_anchor;
        if !elapsed.is_finite() || elapsed < 0.0 {
            return None;
        }
        let elapsed_us = (elapsed * 1_000_000.0).round();
        if !elapsed_us.is_finite() || elapsed_us < 0.0 {
            return None;
        }
        let callback_us = callback_now
            .saturating_duration_since(self.app_origin)
            .as_micros()
            .min(u128::from(u64::MAX)) as u64;
        let desired_us = app_anchor_us.saturating_add(elapsed_us.min(u64::MAX as f64) as u64);
        // Native timestamps can arrive in a burst after the main thread was
        // delayed. Never manufacture an app-space sample in the future. The
        // monotonic floor keeps identical callback instants deterministic.
        let previous_us = self.last_emitted_us.get().unwrap_or(0).min(callback_us);
        let mapped_us = desired_us.min(callback_us).max(previous_us);
        self.last_emitted_us.set(Some(mapped_us));
        Some(mapped_us)
    }
}

/// Wrapper to send a raw pointer through a channel (channels require `Send`).
struct SendPtr(*mut c_void);
unsafe impl Send for SendPtr {}

struct EventTapRegistration {
    port: CFRetained<CFMachPort>,
    source: CFRetained<CFRunLoopSource>,
    run_loop: CFRetained<CFRunLoop>,
    _thread: Option<JoinHandle<()>>,
}

impl Drop for EventTapRegistration {
    fn drop(&mut self) {
        self.run_loop
            .remove_source(Some(&self.source), unsafe { kCFRunLoopDefaultMode });
        CFRunLoop::stop(&self.run_loop);
        // Dropping _thread detaches it; CFRunLoop::stop causes the thread's
        // CFRunLoop::run() to exit, so the thread finishes shortly after.
    }
}

/// Cached coordinate transform that lets the background tap thread convert a
/// CGEvent global location into the launcher window's physical-pixel space
/// without touching any AppKit `MainThreadOnly` object.
///
/// NSWindow's `frame()` reports coordinates in the AppKit global space
/// (origin at the **bottom-left** of the primary display). CGEvent's global
/// location uses the CoreGraphics space (origin at the **top-left**). We store
/// the precomputed affine transform so the tap callback only needs arithmetic.
#[derive(Clone, Copy)]
struct GeometrySnapshot {
    /// CG global x subtracted by this gives the window-relative logical x.
    window_origin_cg_x: f64,
    /// CG global y subtracted by this gives the window-relative logical y
    /// (already flipped so that top of the window = 0).
    window_origin_cg_y: f64,
    scale_factor: f64,
}

impl Default for GeometrySnapshot {
    fn default() -> Self {
        Self {
            window_origin_cg_x: 0.0,
            window_origin_cg_y: 0.0,
            scale_factor: 1.0,
        }
    }
}

impl GeometrySnapshot {
    /// Convert a CGEvent global point to the launcher window's physical
    /// backing-store pixel space, matching what `physical_point_in_window`
    /// produced on the main thread.
    fn to_physical_point(self, cg_global: CGPoint) -> PhysicalPoint {
        let logical_x = cg_global.x - self.window_origin_cg_x;
        let logical_y = cg_global.y - self.window_origin_cg_y;
        PhysicalPoint::new(
            (logical_x * self.scale_factor) as f32,
            (logical_y * self.scale_factor) as f32,
        )
    }

    /// Populate the snapshot from the main thread where NSWindow APIs are safe.
    fn capture_from(window: &NSWindow, screen_height: f64) -> Self {
        let frame = window.frame(); // AppKit global space: origin bottom-left
        let scale = window.backingScaleFactor();
        Self {
            // CG x == AppKit x (both increase leftward).
            window_origin_cg_x: frame.origin.x,
            // AppKit y increases upward; CG y increases downward. The window's
            // top edge in AppKit space is origin.y + size.height. Convert to
            // CG space: cg_top = screen_height - (origin.y + size.height).
            window_origin_cg_y: screen_height - (frame.origin.y + frame.size.height),
            scale_factor: scale,
        }
    }
}

struct TapContext {
    launcher_window_number: isize,
    publisher: InputRoutingPublisher,
    scroll_target: Mutex<Option<MacTarget>>,
    registration: Mutex<Option<EventTapRegistration>>,
    permission_prompted: AtomicBool,
    geometry: Mutex<GeometrySnapshot>,
}

impl TapContext {
    fn permissions_available(&self) -> bool {
        objc2_core_graphics::CGPreflightListenEventAccess()
            && objc2_core_graphics::CGPreflightPostEventAccess()
    }

    fn request_permissions(&self) {
        if self.permission_prompted.swap(true, Ordering::SeqCst) {
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

    /// Update the cached geometry. Must be called from the main thread.
    fn update_geometry(&self, window: &NSWindow, screen_height: f64) {
        *self.geometry.lock().unwrap() = GeometrySnapshot::capture_from(window, screen_height);
    }

    fn ensure_event_tap(&self) -> bool {
        if self.registration.lock().unwrap().is_some() {
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

        // Spawn a dedicated thread with its own CFRunLoop so the tap callback
        // is never blocked by the main thread's frame rendering. The tap's
        // callback does not call any AppKit `MainThreadOnly` API; it uses only
        // CoreGraphics/Quartz (thread-safe) and cached geometry.
        //
        // CFRunLoopSource and CFRetained<CFRunLoop> are technically !Send in
        // objc2's type system, but CoreFoundation guarantees these are
        // thread-safe once retained. We wrap them in a Send shim and only use
        // them on the spawned thread.
        let (run_loop_tx, run_loop_rx) = std::sync::mpsc::channel::<SendPtr>();
        // Pass the run-loop source as a usize to avoid the `!Send` bound on
        // CFRetained/NonNull<CFRunLoopSource>. We retain() an extra reference
        // for the thread and release it there via CFRetained.
        let source_ptr_addr = CFRetained::as_ptr(&source).as_ptr() as usize;
        let thread = std::thread::Builder::new()
            .name("launchpad-event-tap".to_string())
            .spawn(move || {
                // Reconstruct the retained source from the raw pointer. The
                // original `source` in the main thread still holds its own
                // retain, so this is a +1 for the thread.
                let source_for_thread = unsafe {
                    CFRetained::retain(std::ptr::NonNull::new_unchecked(
                        source_ptr_addr as *mut CFRunLoopSource,
                    ))
                };
                let run_loop = CFRunLoop::current().unwrap();
                run_loop.add_source(Some(&source_for_thread), unsafe { kCFRunLoopDefaultMode });
                let ptr = CFRetained::as_ptr(&run_loop).as_ptr().cast::<c_void>();
                let _ = run_loop_tx.send(SendPtr(ptr));
                CFRunLoop::run();
            })
            .expect("spawn launchpad-event-tap thread");
        let run_loop_ptr = match run_loop_rx.recv() {
            Ok(send_ptr) => send_ptr.0,
            Err(_) => {
                eprintln!("input-routing: event-tap thread failed to start its CFRunLoop");
                return false;
            }
        };
        let run_loop = unsafe {
            CFRetained::retain(std::ptr::NonNull::new_unchecked(
                run_loop_ptr.cast::<CFRunLoop>(),
            ))
        };
        *self.registration.lock().unwrap() = Some(EventTapRegistration {
            port,
            source,
            run_loop,
            _thread: Some(thread),
        });
        crate::debug_log!("input-routing: installed macOS active session scroll event tap");
        true
    }

    fn reenable_event_tap(&self) {
        if let Some(registration) = self.registration.lock().unwrap().as_ref() {
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
        event_proxy: EventLoopProxy<UserEvent>,
        scroll_clock_origin: Instant,
    ) -> Option<Self> {
        let launcher_window = launcher_ns_window(window)?;
        let launcher_window_number = launcher_window.windowNumber();
        let screen_height = primary_screen_height().unwrap_or(0.0);
        let initial_geometry = GeometrySnapshot::capture_from(&launcher_window, screen_height);
        let state = Rc::new(RefCell::new(MonitorState::default()));
        let tap_context = Rc::new(TapContext {
            launcher_window_number,
            publisher: publisher.clone(),
            scroll_target: Mutex::new(None),
            registration: Mutex::new(None),
            permission_prompted: AtomicBool::new(false),
            geometry: Mutex::new(initial_geometry),
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
        let callback_event_proxy = event_proxy;
        let callback_scroll_clock = NativeScrollClock::new(scroll_clock_origin);
        let handler = RcBlock::new(move |event_ptr: NonNull<NSEvent>| -> *mut NSEvent {
            let event = unsafe { event_ptr.as_ref() };
            let original = event_ptr.as_ptr();
            let cg_event = event.CGEvent();
            if cg_event.as_ref().is_some_and(|cg_event| {
                CGEvent::integer_value_field(Some(cg_event), CGEventField::EventSourceUserData)
                    == crate::input_probe_protocol::MACOS_PRODUCT_EVENT_TAG
            }) {
                crate::debug_log!(
                    "input-routing: macOS local monitor ignored private replacement event"
                );
                return original;
            }

            let snapshot = publisher.snapshot();
            let event_type = event.r#type();
            if event_type == NSEventType::ScrollWheel {
                let Some(main_thread) = MainThreadMarker::new() else {
                    return original;
                };
                let Some(event_window) = event.window(main_thread) else {
                    return original;
                };
                if event.windowNumber() != launcher_window_number {
                    return original;
                }
                let point = physical_point_in_window(&event_window, event.locationInWindow());
                let Some(point) = point else {
                    return original;
                };
                if snapshot.forwards_vertical_scroll_at(point) {
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
                        *callback_tap_context.scroll_target.lock().unwrap() = None;
                    }
                    return std::ptr::null_mut();
                }
                if !snapshot.visible || !snapshot.region_at(point).is_launchpad_owned() {
                    return original;
                }
                let scale_factor = event_window.backingScaleFactor() as f32;
                let timestamp_seconds = event.timestamp();
                let timestamp_us =
                    callback_scroll_clock.timestamp_us(timestamp_seconds, Instant::now());
                let timestamp_valid = timestamp_us.is_some();
                let timestamp_us = timestamp_us.unwrap_or(0);
                let mut delta = (
                    event.scrollingDeltaX() as f32 * scale_factor,
                    event.scrollingDeltaY() as f32 * scale_factor,
                );
                if !timestamp_valid {
                    // RawScrollEvent uses an integer timestamp. Preserve an
                    // invalid native timestamp as a malformed packet so the
                    // adapter safely cancels an active contact.
                    delta.0 = f32::NAN;
                }
                let raw = RawScrollEvent {
                    timestamp_us,
                    delta_physical_px: delta,
                    source: if event.hasPreciseScrollingDeltas() {
                        ScrollSource::Precise
                    } else {
                        ScrollSource::Line
                    },
                    contact_phase: map_event_phase(event.phase()),
                    momentum_phase: map_event_phase(event.momentumPhase()),
                    sequence_complete: false,
                    direction_inverted_from_device: event.isDirectionInvertedFromDevice(),
                    scale_factor,
                    phase_capability: ScrollPhaseCapability::Separate,
                };
                let _ = callback_event_proxy.send_event(UserEvent::NativeScroll(raw));
                return original;
            }
            let Some(cg_event) = cg_event else {
                return original;
            };

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
                    let appkit_point = NSEvent::mouseLocation();
                    let target =
                        target_below(launcher_window_number, appkit_to_cg_point(appkit_point));
                    crate::debug_log!(
                        "input-routing: macOS captured {button:?} down target={target:?} \
                         launcher_window={launcher_window_number} event_window={} point={:?}",
                        event.windowNumber(),
                        appkit_point
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

    /// Refresh the cached window geometry used by the background tap thread.
    /// Must be called from the main thread (e.g. on `WindowEvent::Moved`).
    pub fn refresh_geometry(&self, window: &winit::window::Window) {
        let Some(ns_window) = launcher_ns_window(window) else {
            return;
        };
        let screen_height = primary_screen_height().unwrap_or(0.0);
        self._tap_context.update_geometry(&ns_window, screen_height);
    }
}

fn map_event_phase(phase: NSEventPhase) -> NativeScrollPhase {
    if phase.contains(NSEventPhase::Cancelled) {
        NativeScrollPhase::Cancelled
    } else if phase.contains(NSEventPhase::Ended) {
        NativeScrollPhase::Ended
    } else if phase.contains(NSEventPhase::Changed) || phase.contains(NSEventPhase::Stationary) {
        NativeScrollPhase::Changed
    } else if phase.contains(NSEventPhase::Began) || phase.contains(NSEventPhase::MayBegin) {
        NativeScrollPhase::Began
    } else {
        NativeScrollPhase::None
    }
}

impl Drop for MacInputPassthrough {
    fn drop(&mut self) {
        unsafe { NSEvent::removeMonitor(&self.monitor) };
    }
}

fn launcher_ns_window(window: &winit::window::Window) -> Option<Retained<NSWindow>> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    let view = unsafe { &*(handle.ns_view.as_ptr() as *const NSView) };
    view.window()
}

fn physical_point_in_window(
    window: &NSWindow,
    window_point: CGPoint,
) -> Option<crate::input_routing::PhysicalPoint> {
    let view = window.contentView()?;
    let view_point = view.convertPoint_fromView(window_point, None);
    let scale = window.backingScaleFactor() as f32;
    if !view_point.x.is_finite() || !view_point.y.is_finite() || !scale.is_finite() || scale <= 0.0
    {
        return None;
    }
    Some(crate::input_routing::PhysicalPoint::new(
        view_point.x as f32 * scale,
        view_point.y as f32 * scale,
    ))
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
    // CGEvent::location returns the global CoreGraphics point (origin at the
    // top-left of the primary display). This is thread-safe and does not touch
    // any AppKit MainThreadOnly object, so the callback can run on the
    // dedicated tap thread without blocking on the main thread.
    let pointer = CGEvent::location(Some(event));
    let geom = *context.geometry.lock().unwrap();
    let physical_point = geom.to_physical_point(pointer);
    if !context
        .publisher
        .snapshot()
        .forwards_vertical_scroll_at(physical_point)
    {
        *context.scroll_target.lock().unwrap() = None;
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
    let Some(target) = resolve_scroll_target(context, pointer, phase, momentum) else {
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
    event_ptr.as_ptr()
}

/// Resolve and cache the scroll target window below the launcher. This runs on
/// the dedicated tap thread and uses only CoreGraphics/Quartz APIs (thread-safe).
fn resolve_scroll_target(
    context: &TapContext,
    pointer: CGPoint,
    phase: i64,
    momentum: i64,
) -> Option<MacTarget> {
    let phase_began =
        phase & (NSEventPhase::Began.bits() | NSEventPhase::MayBegin.bits()) as i64 != 0;
    let unphased = phase == 0 && momentum == 0;
    let mut scroll_target = context.scroll_target.lock().unwrap();
    if phase_began || unphased || scroll_target.is_none() {
        *scroll_target = target_below(context.launcher_window_number, pointer);
    }
    let target = (*scroll_target)?;
    if phase & NSEventPhase::Cancelled.bits() as i64 != 0
        || momentum & NSEventPhase::Ended.bits() as i64 != 0
    {
        *scroll_target = None;
    }
    Some(target)
}

/// Re-validate the current target at the cursor position. Called from the
/// main thread (in `deliver_click`) after a confirmed click. We use
/// `NSEvent::mouseLocation()` (AppKit coordinates, origin bottom-left) and
/// flip Y to CoreGraphics coordinates before delegating to the shared
/// `target_below` resolver.
fn current_target() -> Option<MacTarget> {
    let appkit_point = NSEvent::mouseLocation();
    let cg_point = appkit_to_cg_point(appkit_point);
    target_below(0, cg_point)
}

/// Convert an AppKit global point (origin bottom-left) to a CoreGraphics
/// global point (origin top-left). We need the primary screen height.
fn appkit_to_cg_point(point: objc2_foundation::NSPoint) -> CGPoint {
    let screen_height = primary_screen_height().unwrap_or(0.0);
    CGPoint {
        x: point.x,
        y: screen_height - point.y,
    }
}

/// Primary screen height in points (AppKit coordinate space). Cached after the
/// first lookup since it does not change during a session in practice.
fn primary_screen_height() -> Option<f64> {
    static HEIGHT: OnceLock<Option<f64>> = OnceLock::new();
    *HEIGHT.get_or_init(|| {
        let main_thread = MainThreadMarker::new()?;
        let screen = NSScreen::mainScreen(main_thread)?;
        Some(screen.frame().size.height)
    })
}

/// Find the topmost on-screen window at `point` that is not owned by this
/// process. Uses only CoreGraphics/Quartz (thread-safe), so it can run on the
/// dedicated tap thread. `point` is in global CoreGraphics coordinates.
fn target_below(launcher_window_number: isize, point: CGPoint) -> Option<MacTarget> {
    // kCGWindowListOptionOnScreenOnly (1<<0) | kCGWindowListExcludeDesktopElements (1<<4)
    let descriptions = unsafe { CGWindowListCopyWindowInfo((1 << 0) | (1 << 4), 0) };
    if descriptions.is_null() {
        return None;
    }
    let result = unsafe {
        let count = CFArrayGetCount(descriptions);
        let mut found = None;
        for index in 0..count {
            let dict = CFArrayGetValueAtIndex(descriptions, index);
            if dict.is_null() {
                continue;
            }
            // Skip the launcher window itself.
            let window_number = read_cf_number_i64(dict, kCGWindowNumber);
            if window_number == Some(launcher_window_number as i64) {
                continue;
            }
            // Skip windows on non-zero layers (menu bar, dock, etc.).
            let layer = read_cf_number_i64(dict, kCGWindowLayer).unwrap_or(0);
            if layer != 0 {
                continue;
            }
            // Check if the point is inside this window's bounds.
            let bounds_ptr = CFDictionaryGetValue(dict, kCGWindowBounds);
            if bounds_ptr.is_null() {
                continue;
            }
            let mut rect = CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize {
                    width: 0.0,
                    height: 0.0,
                },
            };
            if !CGRectMakeWithDictionaryRepresentation(bounds_ptr, &mut rect) {
                continue;
            }
            if point.x < rect.origin.x
                || point.x >= rect.origin.x + rect.size.width
                || point.y < rect.origin.y
                || point.y >= rect.origin.y + rect.size.height
            {
                continue;
            }
            // Found the topmost window at this point that is not the launcher.
            // CGWindowListCopyWindowInfo returns windows front-to-back, so the
            // first match is the topmost.
            let pid = read_cf_number_i64(dict, kCGWindowOwnerPID).map(|p| p as i32);
            if let Some(pid) = pid {
                if pid != std::process::id() as i32 {
                    found = Some(MacTarget {
                        window_number: window_number.unwrap_or(0) as isize,
                        pid,
                    });
                    break;
                }
            }
        }
        found
    };
    unsafe { CFRelease(descriptions) };
    result
}

/// Read a CFNumber value from a CGWindow dictionary as i64.
unsafe fn read_cf_number_i64(dict: *const c_void, key: *const c_void) -> Option<i64> {
    unsafe {
        let number = CFDictionaryGetValue(dict, key);
        if number.is_null() {
            return None;
        }
        let mut value: i64 = 0;
        // kCFNumberSInt64Type = 4
        if CFNumberGetValue(number, 4, (&mut value as *mut i64).cast()) {
            Some(value)
        } else {
            None
        }
    }
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
    static kCGWindowNumber: *const c_void;
    static kCGWindowLayer: *const c_void;
    static kCGWindowBounds: *const c_void;
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> *const c_void;
    fn CGRectMakeWithDictionaryRepresentation(dict: *const c_void, rect: *mut CGRect) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFArrayGetCount(array: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
    fn CFDictionaryGetValue(dictionary: *const c_void, key: *const c_void) -> *const c_void;
    fn CFNumberGetValue(number: *const c_void, number_type: isize, value: *mut c_void) -> bool;
    fn CFRelease(value: *const c_void);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn appkit_contact_and_momentum_fields_use_the_same_lossless_phase_mapping() {
        assert_eq!(
            map_event_phase(NSEventPhase::MayBegin),
            NativeScrollPhase::Began
        );
        assert_eq!(
            map_event_phase(NSEventPhase::Stationary),
            NativeScrollPhase::Changed
        );
        assert_eq!(
            map_event_phase(NSEventPhase::Ended),
            NativeScrollPhase::Ended
        );
        assert_eq!(
            map_event_phase(NSEventPhase::Cancelled),
            NativeScrollPhase::Cancelled
        );
        assert_eq!(map_event_phase(NSEventPhase::None), NativeScrollPhase::None);
    }

    #[test]
    fn native_timestamp_uses_app_epoch_and_reaches_pager_velocity_diagnostics() {
        let app_origin = Instant::now();
        let first_callback = app_origin + Duration::from_secs(5);
        let clock = NativeScrollClock::new(app_origin);
        let first_us = clock.timestamp_us(10_000.0, first_callback).unwrap();
        let second_us = clock
            .timestamp_us(10_000.016, first_callback + Duration::from_millis(40))
            .unwrap();
        assert!(first_us >= 5_000_000);
        assert!((second_us - first_us).abs_diff(16_000) <= 1);

        let mut adapter = crate::input_routing::ScrollSampleAdapter::default();
        let mut router = crate::input_routing::PagerInputRouter::default();
        let context = crate::input_routing::ScrollRouteContext {
            settings_active: false,
            folder_phase: crate::input_routing::FolderRoutePhase::Closed,
            blocking_interaction: false,
            main_available: true,
        };
        let mut scroller = crate::scroll::Scroller::new(crate::scroll::ScrollBounds {
            page_extent: 1000.0,
            page_count: 3,
        });
        for (timestamp_us, dx, phase) in [
            (first_us, 10.0, NativeScrollPhase::Began),
            (second_us, 20.0, NativeScrollPhase::Changed),
        ] {
            let sample = adapter
                .adapt_native(RawScrollEvent {
                    timestamp_us,
                    delta_physical_px: (dx, 0.0),
                    source: ScrollSource::Precise,
                    contact_phase: phase,
                    momentum_phase: NativeScrollPhase::None,
                    sequence_complete: false,
                    direction_inverted_from_device: false,
                    scale_factor: 2.0,
                    phase_capability: ScrollPhaseCapability::Separate,
                })
                .unwrap();
            let routed = router.route(sample, context);
            assert_eq!(routed.route, crate::input_routing::ScrollRoute::MainPager);
            assert!(crate::app::update::apply_scroll_sample_to_pager(
                &mut scroller,
                routed.sample,
                app_origin,
            ));
        }
        let diagnostics = scroller.wheel_diagnostics();
        assert!(
            diagnostics.filtered_velocity > 100.0,
            "16ms native spacing must survive App dispatch: {diagnostics:?}"
        );
    }

    #[test]
    fn native_timestamp_never_leads_callback_and_remains_nondecreasing() {
        let app_origin = Instant::now();
        let first_callback = app_origin + Duration::from_secs(5);
        let clock = NativeScrollClock::new(app_origin);

        let cases = [
            (100.000, first_callback),
            // A delayed callback batch contains a native packet 16ms later,
            // but app time has not advanced. It must not enter the future.
            (100.016, first_callback),
            (100.017, first_callback + Duration::from_millis(8)),
            // A duplicate native timestamp remains a legal monotonic sample.
            (100.017, first_callback + Duration::from_millis(9)),
        ];
        let mut previous = None;
        for (native, callback) in cases {
            let mapped = clock.timestamp_us(native, callback).unwrap();
            let callback_us = callback.saturating_duration_since(app_origin).as_micros() as u64;
            assert!(
                mapped <= callback_us,
                "{mapped} must not lead {callback_us}"
            );
            if let Some(previous) = previous {
                assert!(mapped >= previous, "{mapped} regressed behind {previous}");
            }
            previous = Some(mapped);
        }
        assert_eq!(previous, Some(5_009_000));
    }
}

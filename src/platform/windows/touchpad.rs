//! Native Windows Precision Touchpad input.
//!
//! winit 0.30 currently exposes the default Windows touchpad path as a
//! `MouseWheel::LineDelta`. That loses the physical-contact lifecycle and is
//! therefore unsuitable for the launcher's page pager: the pager needs a
//! pixel delta and an explicit `Began/Changed/Ended` sequence. Windows 11 can
//! opt a window into `WM_POINTER*` touchpad packets with
//! `RegisterTouchpadCapableWindow`; Interaction Context then turns the
//! pointer frames into pixel translation deltas.
//!
//! The registration API is currently exported by user32 ordinal rather than
//! exposed by the `windows` crate version used by this project, so it is
//! resolved at runtime. This also leaves older Windows versions on the
//! existing winit wheel fallback instead of failing to start.

use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use windows::core::{BOOL, PCSTR};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows::Win32::UI::Controls::{POINTER_TYPE_INFO, POINTER_TYPE_INFO_0};
use windows::Win32::UI::Input::Pointer::{
    GetPointerInfo, GetPointerType, SkipPointerFrameMessages, POINTER_FLAG_CANCELED, POINTER_INFO,
    POINTER_TOUCH_INFO,
};
use windows::Win32::UI::InteractionContext::{
    CreateInteractionContext, DestroyInteractionContext, RegisterOutputCallbackInteractionContext,
    ResetInteractionContext, SetInteractionConfigurationInteractionContext,
    SetPropertyInteractionContext, HINTERACTIONCONTEXT,
    INTERACTION_CONFIGURATION_FLAG_MANIPULATION, INTERACTION_CONFIGURATION_FLAG_MANIPULATION_EXACT,
    INTERACTION_CONFIGURATION_FLAG_MANIPULATION_RAILS_X,
    INTERACTION_CONFIGURATION_FLAG_MANIPULATION_RAILS_Y,
    INTERACTION_CONFIGURATION_FLAG_MANIPULATION_TRANSLATION_X,
    INTERACTION_CONFIGURATION_FLAG_MANIPULATION_TRANSLATION_Y, INTERACTION_CONTEXT_CONFIGURATION,
    INTERACTION_CONTEXT_OUTPUT, INTERACTION_CONTEXT_PROPERTY_FILTER_POINTERS,
    INTERACTION_CONTEXT_PROPERTY_MEASUREMENT_UNITS, INTERACTION_FLAGS, INTERACTION_FLAG_BEGIN,
    INTERACTION_FLAG_CANCEL, INTERACTION_FLAG_END, INTERACTION_FLAG_INERTIA,
    INTERACTION_ID_MANIPULATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    MSG, POINTER_INPUT_TYPE, PT_TOUCHPAD, WM_MOUSEWHEEL, WM_POINTERCAPTURECHANGED, WM_POINTERDOWN,
    WM_POINTERUP, WM_POINTERUPDATE,
};
use winit::event_loop::EventLoopProxy;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::app::UserEvent;
use crate::input_routing::{
    InputRegion, InputRoutingPublisher, NativeScrollPhase, RawScrollEvent, RouterState,
    ScrollPhaseCapability, ScrollSource,
};

/// The ordinal documented by Microsoft for RegisterTouchpadCapableWindow.
const REGISTER_TOUCHPAD_CAPABLE_WINDOW_ORDINAL: usize = 2689;
const GET_POINTER_FRAME_TOUCHPAD_INFO_HISTORY_ORDINAL: usize = 2694;
const PROCESS_POINTER_FRAMES_INTERACTION_CONTEXT2_ORDINAL: usize = 2507;
/// `MEASUREMENT_UNITS_SCREEN_PIXELS` from interactioncontext.h.
const MEASUREMENT_UNITS_SCREEN_PIXELS: u32 = 1;

#[derive(Clone)]
pub struct WindowsTouchpadInput {
    shared: Arc<SharedState>,
}

struct SharedState {
    inner: Mutex<InnerState>,
}

struct InnerState {
    proxy: Option<EventLoopProxy<UserEvent>>,
    clock_origin: Instant,
    scale_factor: f32,
    interaction_context: Option<usize>,
    registered: bool,
    /// True after the first packet of a gesture was accepted by the launcher.
    /// It keeps ownership stable even if the cursor crosses the transparent
    /// edge while the two fingers remain down.
    launcher_owns_gesture: bool,
    forward_to_underlying: bool,
    interaction_active: bool,
    publisher: Option<InputRoutingPublisher>,
    launcher_hwnd: isize,
    last_screen_point: (i32, i32),
    wheel_remainder_y: f32,
    last_message_time: u32,
}

type GetPointerFrameTouchpadInfoHistory =
    unsafe extern "system" fn(u32, *mut u32, *mut u32, *mut POINTER_TOUCH_INFO) -> BOOL;
type ProcessPointerFramesInteractionContext2 =
    unsafe extern "system" fn(HINTERACTIONCONTEXT, u32, u32, *const POINTER_TYPE_INFO) -> i32;

struct TouchpadFrame {
    entries_count: u32,
    pointer_count: u32,
    pointers: Vec<POINTER_TYPE_INFO>,
}

impl WindowsTouchpadInput {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(SharedState {
                inner: Mutex::new(InnerState {
                    proxy: None,
                    clock_origin: Instant::now(),
                    scale_factor: 1.0,
                    interaction_context: None,
                    registered: false,
                    launcher_owns_gesture: false,
                    forward_to_underlying: false,
                    interaction_active: false,
                    publisher: None,
                    launcher_hwnd: 0,
                    last_screen_point: (0, 0),
                    wheel_remainder_y: 0.0,
                    last_message_time: 0,
                }),
            }),
        }
    }

    pub fn set_proxy(&self, proxy: EventLoopProxy<UserEvent>) {
        if let Ok(mut state) = self.shared.inner.lock() {
            state.proxy = Some(proxy);
        }
    }

    pub fn set_clock_origin(&self, origin: Instant) {
        if let Ok(mut state) = self.shared.inner.lock() {
            state.clock_origin = origin;
        }
    }

    pub fn set_scale_factor(&self, scale_factor: f32) {
        if !scale_factor.is_finite() || scale_factor <= 0.0 {
            return;
        }
        if let Ok(mut state) = self.shared.inner.lock() {
            state.scale_factor = scale_factor;
        }
    }

    /// Register the winit HWND and install the pixel-translation context.
    /// Returns false when the OS/API is unavailable; the caller keeps the
    /// normal winit wheel path in that case.
    pub fn register_window(&self, window: &winit::window::Window) -> bool {
        let Some(hwnd) = hwnd_from_window(window) else {
            return false;
        };

        if self
            .shared
            .inner
            .lock()
            .ok()
            .is_some_and(|state| state.registered)
        {
            return true;
        }

        let context = match create_interaction_context(&self.shared) {
            Ok(context) => context,
            Err(error) => {
                eprintln!("input-routing: Windows Interaction Context unavailable: {error}");
                return false;
            }
        };

        if !touchpad_pointer_apis_available() {
            unsafe {
                let _ = DestroyInteractionContext(context);
            }
            eprintln!(
                "input-routing: Windows Precision Touchpad pointer-frame APIs are unavailable"
            );
            return false;
        }

        if let Err(error) = register_touchpad_capable_window(hwnd) {
            unsafe {
                let _ = DestroyInteractionContext(context);
            }
            eprintln!(
                "input-routing: Windows Precision Touchpad registration unavailable: {error}"
            );
            return false;
        }

        if let Ok(mut state) = self.shared.inner.lock() {
            state.interaction_context = Some(context.0 as usize);
            state.registered = true;
        } else {
            unsafe {
                let _ = DestroyInteractionContext(context);
            }
            return false;
        }

        crate::debug_log!(
            "input-routing: Windows Precision Touchpad native path enabled hwnd={hwnd:?}"
        );
        true
    }

    /// Runs before winit's window procedure. A touchpad gesture accepted by
    /// the launchpad is consumed here so DefWindowProc cannot also turn it
    /// into a duplicate line-based mouse wheel event.
    pub fn handle_message(
        &self,
        raw_message: *const c_void,
        publisher: &InputRoutingPublisher,
    ) -> bool {
        if raw_message.is_null() {
            return false;
        }
        let message = unsafe { &*(raw_message as *const MSG) };

        if message.message == WM_POINTERCAPTURECHANGED {
            let owns_gesture =
                self.shared.inner.lock().ok().is_some_and(|state| {
                    state.launcher_owns_gesture || state.forward_to_underlying
                });
            if owns_gesture {
                self.cancel_gesture();
                return true;
            }
            return false;
        }

        if !is_pointer_message(message.message) {
            return false;
        }

        let pointer_id = (message.wParam.0 & 0xffff) as u32;

        let should_consume = {
            let Ok(mut state) = self.shared.inner.lock() else {
                return false;
            };
            if state.interaction_context.is_none() {
                return false;
            }
            if state.launcher_owns_gesture || state.forward_to_underlying {
                state.publisher = Some(publisher.clone());
                state.last_screen_point = (message.pt.x, message.pt.y);
                state.last_message_time = message.time;
                true
            } else {
                if !is_touchpad_pointer(pointer_id) {
                    return false;
                }
                let snapshot = publisher.snapshot();
                let owns = snapshot.visible
                    && matches!(snapshot.region, InputRegion::LaunchpadOwned)
                    && matches!(snapshot.router_state, RouterState::Idle);
                if owns {
                    state.launcher_owns_gesture = true;
                    state.publisher = Some(publisher.clone());
                    state.launcher_hwnd = message.hwnd.0 as isize;
                    state.last_screen_point = (message.pt.x, message.pt.y);
                    state.last_message_time = message.time;
                } else if snapshot.visible
                    && matches!(snapshot.region, InputRegion::OutsideTransparent)
                    && matches!(snapshot.router_state, RouterState::Idle)
                {
                    // winit consumes WM_POINTER* as WindowEvent::Touch and
                    // does not call DefWindowProc, so transparent-area
                    // touchpad input would otherwise never reach the existing
                    // wheel passthrough. Convert its translation output into
                    // a high-resolution WM_MOUSEWHEEL below.
                    state.forward_to_underlying = true;
                    state.publisher = Some(publisher.clone());
                    state.launcher_hwnd = message.hwnd.0 as isize;
                    state.last_screen_point = (message.pt.x, message.pt.y);
                    state.last_message_time = message.time;
                }
                owns || state.forward_to_underlying
            }
        };

        if !should_consume {
            // The window is not interested in this gesture. Returning false
            // leaves other pointer devices and hidden/non-owned input on the
            // normal winit path.
            return false;
        }

        if pointer_is_cancelled(pointer_id) {
            self.cancel_gesture();
            unsafe {
                let _ = SkipPointerFrameMessages(pointer_id);
            }
            return true;
        }

        if let Some(frame) = pointer_frame(pointer_id) {
            self.process_frame(&frame);
        } else if message.message == WM_POINTERUP {
            self.cancel_gesture();
        }
        unsafe {
            let _ = SkipPointerFrameMessages(pointer_id);
        }

        if message.message == WM_POINTERUP {
            if let Ok(mut state) = self.shared.inner.lock() {
                state.launcher_owns_gesture = false;
                state.forward_to_underlying = false;
                state.wheel_remainder_y = 0.0;
            }
        }
        true
    }

    fn process_frame(&self, frame: &TouchpadFrame) {
        if frame.pointers.is_empty() {
            return;
        }
        let context = self
            .shared
            .inner
            .lock()
            .ok()
            .and_then(|state| state.interaction_context)
            .map(|handle| HINTERACTIONCONTEXT(handle as *mut c_void));
        let Some(context) = context else {
            return;
        };

        let Some(process_frames) = process_pointer_frames_interaction_context2() else {
            return;
        };
        let result = unsafe {
            process_frames(
                context,
                frame.entries_count,
                frame.pointer_count,
                frame.pointers.as_ptr(),
            )
        };
        if result < 0 {
            crate::debug_log!(
                "input-routing: ProcessPointerFramesInteractionContext2 failed HRESULT=0x{result:08x}"
            );
        }
    }

    fn cancel_gesture(&self) {
        let Ok(mut state) = self.shared.inner.lock() else {
            return;
        };
        let emit_cancel = state.interaction_active && !state.forward_to_underlying;
        let context = state
            .interaction_active
            .then_some(state.interaction_context)
            .flatten()
            .map(|handle| HINTERACTIONCONTEXT(handle as *mut c_void));
        state.launcher_owns_gesture = false;
        state.forward_to_underlying = false;
        state.wheel_remainder_y = 0.0;
        state.interaction_active = false;
        drop(state);
        if let Some(context) = context {
            unsafe {
                let _ = ResetInteractionContext(context);
            }
            if emit_cancel {
                self.shared.emit_cancel();
            }
        }
    }
}

impl SharedState {
    fn emit_cancel(&self) {
        let (proxy, timestamp_us, scale_factor) = {
            let Ok(mut state) = self.inner.lock() else {
                return;
            };
            state.interaction_active = false;
            let timestamp_us = timestamp_us(state.clock_origin);
            (state.proxy.clone(), timestamp_us, state.scale_factor)
        };
        let Some(proxy) = proxy else {
            return;
        };
        let _ = proxy.send_event(UserEvent::NativeScroll(RawScrollEvent {
            timestamp_us,
            delta_physical_px: (0.0, 0.0),
            source: ScrollSource::Precise,
            contact_phase: NativeScrollPhase::Cancelled,
            momentum_phase: NativeScrollPhase::None,
            sequence_complete: true,
            direction_inverted_from_device: false,
            scale_factor,
            phase_capability: ScrollPhaseCapability::Separate,
        }));
    }

    fn on_output(&self, output: &INTERACTION_CONTEXT_OUTPUT) {
        if output.interactionId != INTERACTION_ID_MANIPULATION
            || output.interactionFlags.contains(INTERACTION_FLAG_INERTIA)
        {
            return;
        }

        let flags = output.interactionFlags;
        let phase = output_phase(flags);
        let manipulation = unsafe { output.arguments.manipulation };
        let delta = (
            manipulation.delta.translationX,
            manipulation.delta.translationY,
        );

        let (proxy, timestamp_us, scale_factor, phase, passthrough) = {
            let Ok(mut state) = self.inner.lock() else {
                return;
            };
            // Some drivers omit BEGIN on the first frame. Make that frame a
            // valid contact start so the shared adapter can assign a gesture
            // id instead of silently dropping the entire swipe.
            let phase = if phase == NativeScrollPhase::Changed && !state.interaction_active {
                NativeScrollPhase::Began
            } else {
                phase
            };
            state.interaction_active = !phase.is_terminal();
            let passthrough = if state.forward_to_underlying {
                let total_y = delta.1 + state.wheel_remainder_y;
                let wheel_delta = total_y.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                state.wheel_remainder_y = total_y - f32::from(wheel_delta);
                let passthrough = (
                    state.publisher.clone(),
                    state.launcher_hwnd,
                    POINT {
                        x: state.last_screen_point.0,
                        y: state.last_screen_point.1,
                    },
                    state.last_message_time,
                    wheel_delta,
                );
                if phase.is_terminal() {
                    state.forward_to_underlying = false;
                    state.wheel_remainder_y = 0.0;
                }
                Some(passthrough)
            } else {
                None
            };
            (
                state.proxy.clone(),
                timestamp_us(state.clock_origin),
                state.scale_factor,
                phase,
                passthrough,
            )
        };
        if let Some((Some(publisher), launcher_hwnd, point, message_time, wheel_delta)) =
            passthrough
        {
            forward_vertical_wheel(publisher, launcher_hwnd, point, message_time, wheel_delta);
            return;
        }
        let Some(proxy) = proxy else {
            return;
        };
        let _ = proxy.send_event(UserEvent::NativeScroll(RawScrollEvent {
            timestamp_us,
            delta_physical_px: delta,
            source: ScrollSource::Precise,
            contact_phase: phase,
            momentum_phase: NativeScrollPhase::None,
            sequence_complete: phase.is_terminal(),
            direction_inverted_from_device: false,
            scale_factor,
            phase_capability: ScrollPhaseCapability::Separate,
        }));
    }
}

impl Drop for SharedState {
    fn drop(&mut self) {
        let context = self
            .inner
            .get_mut()
            .ok()
            .and_then(|state| state.interaction_context.take())
            .map(|handle| HINTERACTIONCONTEXT(handle as *mut c_void));
        if let Some(context) = context {
            unsafe {
                let _ = DestroyInteractionContext(context);
            }
        }
    }
}

unsafe extern "system" fn interaction_output_callback(
    client_data: *const c_void,
    output: *const INTERACTION_CONTEXT_OUTPUT,
) {
    if client_data.is_null() || output.is_null() {
        return;
    }
    let shared = unsafe { &*(client_data as *const SharedState) };
    shared.on_output(unsafe { &*output });
}

fn create_interaction_context(shared: &Arc<SharedState>) -> Result<HINTERACTIONCONTEXT, String> {
    let context = unsafe { CreateInteractionContext() }.map_err(|error| error.to_string())?;
    let configuration = [INTERACTION_CONTEXT_CONFIGURATION {
        interactionId: INTERACTION_ID_MANIPULATION,
        enable: INTERACTION_CONFIGURATION_FLAG_MANIPULATION
            | INTERACTION_CONFIGURATION_FLAG_MANIPULATION_TRANSLATION_X
            | INTERACTION_CONFIGURATION_FLAG_MANIPULATION_TRANSLATION_Y
            | INTERACTION_CONFIGURATION_FLAG_MANIPULATION_EXACT
            | INTERACTION_CONFIGURATION_FLAG_MANIPULATION_RAILS_X
            | INTERACTION_CONFIGURATION_FLAG_MANIPULATION_RAILS_Y,
    }];

    let result = unsafe {
        RegisterOutputCallbackInteractionContext(
            context,
            Some(interaction_output_callback),
            Some(Arc::as_ptr(shared) as *const c_void),
        )
        .and_then(|_| SetInteractionConfigurationInteractionContext(context, &configuration))
        .and_then(|_| {
            SetPropertyInteractionContext(
                context,
                INTERACTION_CONTEXT_PROPERTY_MEASUREMENT_UNITS,
                MEASUREMENT_UNITS_SCREEN_PIXELS,
            )
        })
        .and_then(|_| {
            SetPropertyInteractionContext(context, INTERACTION_CONTEXT_PROPERTY_FILTER_POINTERS, 0)
        })
    };
    if let Err(error) = result {
        unsafe {
            let _ = DestroyInteractionContext(context);
        }
        return Err(error.to_string());
    }
    Ok(context)
}

fn pointer_frame(pointer_id: u32) -> Option<TouchpadFrame> {
    let get_frame = get_pointer_frame_touchpad_info_history()?;
    let mut entries_count = 0u32;
    let mut pointer_count = 0u32;
    if !unsafe {
        get_frame(
            pointer_id,
            &mut entries_count,
            &mut pointer_count,
            std::ptr::null_mut(),
        )
    }
    .as_bool()
    {
        return None;
    }
    if entries_count == 0 || pointer_count == 0 || entries_count > 64 || pointer_count > 16 {
        return None;
    }
    let total = usize::try_from(entries_count.checked_mul(pointer_count)?).ok()?;
    let mut touch_infos = vec![POINTER_TOUCH_INFO::default(); total];
    if !unsafe {
        get_frame(
            pointer_id,
            &mut entries_count,
            &mut pointer_count,
            touch_infos.as_mut_ptr(),
        )
    }
    .as_bool()
    {
        return None;
    }
    let entries_count = entries_count.min(64);
    let pointer_count = pointer_count.min(16);
    let total = usize::try_from(entries_count.checked_mul(pointer_count)?).ok()?;
    touch_infos.truncate(total);
    let pointers = touch_infos
        .into_iter()
        .map(|touch_info| POINTER_TYPE_INFO {
            r#type: touch_info.pointerInfo.pointerType,
            Anonymous: POINTER_TYPE_INFO_0 {
                touchInfo: touch_info,
            },
        })
        .collect();
    Some(TouchpadFrame {
        entries_count,
        pointer_count,
        pointers,
    })
}

fn is_touchpad_pointer(pointer_id: u32) -> bool {
    let mut pointer_type = POINTER_INPUT_TYPE(0);
    unsafe { GetPointerType(pointer_id, &mut pointer_type) }.is_ok() && pointer_type == PT_TOUCHPAD
}

fn pointer_is_cancelled(pointer_id: u32) -> bool {
    let mut pointer_info = POINTER_INFO::default();
    unsafe { GetPointerInfo(pointer_id, &mut pointer_info) }.is_ok()
        && pointer_info.pointerFlags.contains(POINTER_FLAG_CANCELED)
}

fn touchpad_pointer_apis_available() -> bool {
    get_pointer_frame_touchpad_info_history().is_some()
        && process_pointer_frames_interaction_context2().is_some()
}

fn get_pointer_frame_touchpad_info_history() -> Option<GetPointerFrameTouchpadInfoHistory> {
    let user32 = unsafe { GetModuleHandleA(PCSTR(c"user32.dll".as_ptr() as *const u8)) }.ok()?;
    let address = unsafe {
        GetProcAddress(
            user32,
            PCSTR(GET_POINTER_FRAME_TOUCHPAD_INFO_HISTORY_ORDINAL as *const u8),
        )
    }?;
    Some(unsafe {
        std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            GetPointerFrameTouchpadInfoHistory,
        >(address)
    })
}

fn process_pointer_frames_interaction_context2() -> Option<ProcessPointerFramesInteractionContext2>
{
    let ninput = unsafe { GetModuleHandleA(PCSTR(c"ninput.dll".as_ptr() as *const u8)) }.ok()?;
    let address = unsafe {
        GetProcAddress(
            ninput,
            PCSTR(PROCESS_POINTER_FRAMES_INTERACTION_CONTEXT2_ORDINAL as *const u8),
        )
    }?;
    Some(unsafe {
        std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            ProcessPointerFramesInteractionContext2,
        >(address)
    })
}

fn is_pointer_message(message: u32) -> bool {
    matches!(message, WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP)
}

fn forward_vertical_wheel(
    publisher: InputRoutingPublisher,
    launcher_hwnd: isize,
    point: POINT,
    message_time: u32,
    translation_y: i16,
) {
    if launcher_hwnd == 0 || translation_y == 0 {
        return;
    }
    // A positive manipulation translation means the finger moved down. A
    // positive WM_MOUSEWHEEL delta means scroll up, so invert only this
    // legacy wheel boundary. Keep sub-notch deltas intact for precision-wheel
    // receivers; the residual accumulator above prevents small frames from
    // disappearing entirely.
    let wheel_delta = (-(translation_y as i32)).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    let packed_point = (point.x as u16 as u32) | ((point.y as u16 as u32) << 16);
    let message = MSG {
        hwnd: HWND(launcher_hwnd as *mut c_void),
        message: WM_MOUSEWHEEL,
        wParam: WPARAM((wheel_delta as u16 as usize) << 16),
        lParam: LPARAM(packed_point as isize),
        time: message_time,
        pt: point,
    };
    let _ = super::input_passthrough::handle_message(
        &message as *const MSG as *const c_void,
        &publisher,
    );
}

fn output_phase(flags: INTERACTION_FLAGS) -> NativeScrollPhase {
    if flags.contains(INTERACTION_FLAG_BEGIN) {
        NativeScrollPhase::Began
    } else if flags.contains(INTERACTION_FLAG_CANCEL) {
        NativeScrollPhase::Cancelled
    } else if flags.contains(INTERACTION_FLAG_END) {
        NativeScrollPhase::Ended
    } else {
        NativeScrollPhase::Changed
    }
}

fn timestamp_us(origin: Instant) -> u64 {
    Instant::now()
        .saturating_duration_since(origin)
        .as_micros()
        .min(u128::from(u64::MAX)) as u64
}

fn hwnd_from_window(window: &winit::window::Window) -> Option<HWND> {
    let handle = window.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut c_void)),
        _ => None,
    }
}

fn register_touchpad_capable_window(hwnd: HWND) -> Result<(), String> {
    type RegisterTouchpadCapableWindow = unsafe extern "system" fn(HWND, BOOL) -> BOOL;

    let user32 = unsafe { GetModuleHandleA(PCSTR(c"user32.dll".as_ptr() as *const u8)) }
        .map_err(|error| error.to_string())?;
    let ordinal = PCSTR(REGISTER_TOUCHPAD_CAPABLE_WINDOW_ORDINAL as *const u8);
    let Some(address) = (unsafe { GetProcAddress(user32, ordinal) }) else {
        return Err("RegisterTouchpadCapableWindow ordinal is not exported".to_owned());
    };
    let register: RegisterTouchpadCapableWindow = unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, RegisterTouchpadCapableWindow>(
            address,
        )
    };
    if unsafe { register(hwnd, BOOL(1)) }.as_bool() {
        Ok(())
    } else {
        Err(windows::core::Error::from_thread().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_frame_messages_are_the_only_messages_handled() {
        assert!(is_pointer_message(WM_POINTERDOWN));
        assert!(is_pointer_message(WM_POINTERUPDATE));
        assert!(is_pointer_message(WM_POINTERUP));
        assert!(!is_pointer_message(0x020a));
    }

    #[test]
    fn interaction_output_flags_map_to_contact_phases() {
        assert_eq!(
            output_phase(INTERACTION_FLAG_BEGIN),
            NativeScrollPhase::Began
        );
        assert_eq!(output_phase(INTERACTION_FLAG_END), NativeScrollPhase::Ended);
        assert_eq!(
            output_phase(INTERACTION_FLAG_CANCEL),
            NativeScrollPhase::Cancelled
        );
        assert_eq!(
            output_phase(INTERACTION_FLAGS::default()),
            NativeScrollPhase::Changed
        );
    }
}

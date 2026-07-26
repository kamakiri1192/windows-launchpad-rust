//! Independent native window that records received input as JSONL.

#[cfg(windows)]
mod windows_probe {
    use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};

    use launchpad_windows::input_probe_protocol::{
        NativePhase, NativePoint, NativeRect, ProbeButton, ProbeEvent, ProbeRecord,
    };
    use windows::core::w;
    use windows::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{ClientToScreen, UpdateWindow};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetAncestor, GetForegroundWindow,
        GetMessageTime, GetMessageW, GetWindowRect, LoadCursorW, PostQuitMessage, RegisterClassW,
        SetForegroundWindow, ShowWindow, TranslateMessage, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW,
        GA_ROOT, HMENU, IDC_ARROW, MSG, SW_SHOW, WINDOW_EX_STYLE, WM_ACTIVATE, WM_CLOSE,
        WM_CONTEXTMENU, WM_DESTROY, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
        WM_MOUSEWHEEL, WM_NCCREATE, WM_POINTERWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETFOCUS,
        WNDCLASSW, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    };

    static SERIAL: AtomicU64 = AtomicU64::new(0);
    static TOP_LEVEL: AtomicIsize = AtomicIsize::new(0);

    fn handle_value(hwnd: HWND) -> u64 {
        hwnd.0 as usize as u64
    }

    fn signed_low(value: isize) -> i32 {
        (value as u16 as i16) as i32
    }

    fn signed_high(value: isize) -> i32 {
        ((value as usize >> 16) as u16 as i16) as i32
    }

    fn emit(record: ProbeRecord) {
        if let Ok(line) = record.to_json_line() {
            println!("{line}");
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
    }

    fn event_points(
        hwnd: HWND,
        lparam: LPARAM,
        screen_in_lparam: bool,
    ) -> (NativePoint, NativePoint) {
        if screen_in_lparam {
            let screen = NativePoint {
                x: signed_low(lparam.0),
                y: signed_high(lparam.0),
            };
            let mut origin = POINT { x: 0, y: 0 };
            let _ = unsafe { ClientToScreen(hwnd, &mut origin) };
            let local = NativePoint {
                x: screen.x - origin.x,
                y: screen.y - origin.y,
            };
            (screen, local)
        } else {
            let local = NativePoint {
                x: signed_low(lparam.0),
                y: signed_high(lparam.0),
            };
            let mut screen = POINT {
                x: local.x,
                y: local.y,
            };
            let _ = unsafe { ClientToScreen(hwnd, &mut screen) };
            (
                NativePoint {
                    x: screen.x,
                    y: screen.y,
                },
                local,
            )
        }
    }

    fn log_event(hwnd: HWND, lparam: LPARAM, event: ProbeEvent, screen_in_lparam: bool) {
        let (screen, local) = event_points(hwnd, lparam, screen_in_lparam);
        let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
        emit(ProbeRecord::Input {
            serial: SERIAL.fetch_add(1, Ordering::Relaxed) + 1,
            timestamp: unsafe { GetMessageTime() } as u32 as u64,
            event,
            target: handle_value(hwnd),
            root: handle_value(root),
            pid: unsafe { GetCurrentProcessId() },
            screen,
            local,
            foreground: handle_value(unsafe { GetForegroundWindow() }),
        });
    }

    extern "system" fn wnd_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_NCCREATE => {
                let _create = lparam.0 as *const CREATESTRUCTW;
                return LRESULT(1);
            }
            WM_MOUSEMOVE => log_event(hwnd, lparam, ProbeEvent::MouseMove, false),
            WM_LBUTTONDOWN => log_event(
                hwnd,
                lparam,
                ProbeEvent::ButtonDown {
                    button: ProbeButton::Left,
                },
                false,
            ),
            WM_LBUTTONUP => log_event(
                hwnd,
                lparam,
                ProbeEvent::ButtonUp {
                    button: ProbeButton::Left,
                },
                false,
            ),
            WM_RBUTTONDOWN => log_event(
                hwnd,
                lparam,
                ProbeEvent::ButtonDown {
                    button: ProbeButton::Right,
                },
                false,
            ),
            WM_RBUTTONUP => {
                log_event(
                    hwnd,
                    lparam,
                    ProbeEvent::ButtonUp {
                        button: ProbeButton::Right,
                    },
                    false,
                );
                // Preserve default Win32 processing so a real right click also
                // produces WM_CONTEXTMENU. Returning zero here made direct
                // message delivery look successful while real apps did not
                // receive normal context-menu semantics.
                return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            }
            WM_CONTEXTMENU => log_event(hwnd, lparam, ProbeEvent::ContextMenu, true),
            WM_MOUSEWHEEL | WM_POINTERWHEEL => {
                log_event(
                    hwnd,
                    lparam,
                    ProbeEvent::VerticalWheel {
                        delta: ((wparam.0 >> 16) as u16 as i16) as i32,
                        delta_x: 0.0,
                        delta_y: ((wparam.0 >> 16) as u16 as i16) as f64,
                        precise: ((wparam.0 >> 16) as u16 as i16).unsigned_abs() < 120,
                        key_state: wparam.0 as u16,
                        phase: NativePhase::Unavailable,
                        momentum_phase: NativePhase::Unavailable,
                    },
                    true,
                );
                if std::env::var_os(
                    launchpad_windows::input_probe_protocol::QA_WHEEL_RECEIVER_ACTIVATION_ENV,
                )
                .is_some()
                {
                    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
                    let _ = unsafe { SetForegroundWindow(root) };
                }
            }
            WM_SETFOCUS => log_event(hwnd, LPARAM(0), ProbeEvent::FocusGained, false),
            WM_KILLFOCUS => log_event(hwnd, LPARAM(0), ProbeEvent::FocusLost, false),
            WM_ACTIVATE => log_event(
                hwnd,
                LPARAM(0),
                ProbeEvent::Activated {
                    active: (wparam.0 as u16) != 0,
                },
                false,
            ),
            WM_CLOSE => unsafe {
                windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd).ok();
            },
            WM_DESTROY => {
                if handle_value(hwnd) == TOP_LEVEL.load(Ordering::Relaxed) as u64 {
                    unsafe { PostQuitMessage(0) };
                }
            }
            _ => return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
        LRESULT(0)
    }

    pub fn run() -> Result<(), String> {
        unsafe {
            let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                .map_err(|error| error.to_string())?;
            let class_name = w!("LaunchpadInputProbe");
            let class = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                hInstance: instance.into(),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: class_name,
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 && GetLastError().0 != 1410 {
                return Err(format!("RegisterClassW failed: {:?}", GetLastError()));
            }

            let top = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class_name,
                w!("Launchpad Native Input Probe"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                100,
                100,
                1000,
                700,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .map_err(|error| error.to_string())?;
            TOP_LEVEL.store(top.0 as isize, Ordering::Relaxed);
            let child = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class_name,
                w!("Probe Child"),
                WS_CHILD | WS_VISIBLE,
                10,
                10,
                940,
                620,
                Some(top),
                Some(HMENU(std::ptr::dangling_mut())),
                Some(instance.into()),
                None,
            )
            .map_err(|error| error.to_string())?;
            let _ = ShowWindow(top, SW_SHOW);
            if !UpdateWindow(top).as_bool() {
                return Err(format!("UpdateWindow failed: {:?}", GetLastError()));
            }

            let mut rect = RECT::default();
            GetWindowRect(top, &mut rect).map_err(|error| error.to_string())?;
            emit(ProbeRecord::Ready {
                pid: GetCurrentProcessId(),
                top_level: handle_value(top),
                child: handle_value(child),
                rect: NativeRect {
                    left: rect.left,
                    top: rect.top,
                    right: rect.right,
                    bottom: rect.bottom,
                },
            });

            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod macos_probe {
    use std::cell::Cell;
    use std::ptr::NonNull;
    use std::rc::Rc;

    use block2::RcBlock;
    use launchpad_windows::input_probe_protocol::{
        NativePhase, NativePoint, NativeRect, ProbeButton, ProbeEvent, ProbeRecord,
    };
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSEvent, NSEventMask, NSEventPhase, NSEventType, NSScrollView, NSView,
        NSWindow,
    };
    use winit::application::ApplicationHandler;
    use winit::dpi::{PhysicalPosition, PhysicalSize};
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::{Window, WindowId};

    fn emit(record: ProbeRecord) {
        if let Ok(line) = record.to_json_line() {
            println!("{line}");
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
    }

    fn phase(value: NSEventPhase, momentum: bool) -> NativePhase {
        if value.contains(NSEventPhase::Began) {
            if momentum {
                NativePhase::MomentumBegan
            } else {
                NativePhase::Began
            }
        } else if value.contains(NSEventPhase::Changed) || value.contains(NSEventPhase::Stationary)
        {
            if momentum {
                NativePhase::MomentumChanged
            } else {
                NativePhase::Changed
            }
        } else if value.contains(NSEventPhase::Ended) {
            if momentum {
                NativePhase::MomentumEnded
            } else {
                NativePhase::Ended
            }
        } else if value.contains(NSEventPhase::Cancelled) {
            NativePhase::Cancelled
        } else {
            NativePhase::Unavailable
        }
    }

    fn foreground_window_number() -> u64 {
        let Some(main_thread) = MainThreadMarker::new() else {
            return 0;
        };
        NSApplication::sharedApplication(main_thread)
            .keyWindow()
            .map_or(0, |window| window.windowNumber() as u64)
    }

    fn install_monitor(probe_window: Retained<NSWindow>) -> Option<Retained<AnyObject>> {
        let probe_window_number = probe_window.windowNumber();
        let serial = Rc::new(Cell::new(0u64));
        let callback_serial = serial.clone();
        let handler = RcBlock::new(move |event_ptr: NonNull<NSEvent>| -> *mut NSEvent {
            let event = unsafe { event_ptr.as_ref() };
            let product_delivery = event.CGEvent().is_some_and(|cg_event| {
                objc2_core_graphics::CGEvent::integer_value_field(
                    Some(&cg_event),
                    objc2_core_graphics::CGEventField::EventSourceUserData,
                ) == launchpad_windows::input_probe_protocol::MACOS_PRODUCT_EVENT_TAG
            });
            if event.windowNumber() != probe_window_number && !product_delivery {
                return event_ptr.as_ptr();
            }
            let event_type = event.r#type();
            let probe_event = match event_type {
                NSEventType::MouseMoved
                | NSEventType::LeftMouseDragged
                | NSEventType::RightMouseDragged => Some(ProbeEvent::MouseMove),
                NSEventType::LeftMouseDown => Some(ProbeEvent::ButtonDown {
                    button: ProbeButton::Left,
                }),
                NSEventType::LeftMouseUp => Some(ProbeEvent::ButtonUp {
                    button: ProbeButton::Left,
                }),
                NSEventType::RightMouseDown => Some(ProbeEvent::ButtonDown {
                    button: ProbeButton::Right,
                }),
                NSEventType::RightMouseUp => Some(ProbeEvent::ButtonUp {
                    button: ProbeButton::Right,
                }),
                NSEventType::ScrollWheel => Some(ProbeEvent::VerticalWheel {
                    delta: event.deltaY().round() as i32,
                    delta_x: event.scrollingDeltaX(),
                    delta_y: event.scrollingDeltaY(),
                    precise: event.hasPreciseScrollingDeltas(),
                    key_state: (event.modifierFlags().bits() & 0xffff) as u16,
                    phase: phase(event.phase(), false),
                    momentum_phase: phase(event.momentumPhase(), true),
                }),
                _ => None,
            };
            if let Some(probe_event) = probe_event {
                let next = callback_serial.get() + 1;
                callback_serial.set(next);
                let screen = NSEvent::mouseLocation();
                // CGEventPostToPid delivers to this process but AppKit retains
                // the source NSEvent's windowNumber/locationInWindow metadata
                // even when both CG window-under-pointer fields are updated.
                // A tagged event was explicitly addressed to this probe PID,
                // which has exactly one input window, so report that actual
                // receiver and derive its local point from the preserved
                // screen coordinate. Untagged generator events remain fully
                // native and use their original NSEvent metadata.
                let (target, local) = if product_delivery {
                    (
                        probe_window_number,
                        probe_window.convertPointFromScreen(screen),
                    )
                } else {
                    (event.windowNumber(), event.locationInWindow())
                };
                emit(ProbeRecord::Input {
                    serial: next,
                    timestamp: (event.timestamp() * 1_000_000.0) as u64,
                    event: probe_event,
                    target: target as u64,
                    root: target as u64,
                    pid: std::process::id(),
                    screen: NativePoint {
                        x: screen.x.round() as i32,
                        y: screen.y.round() as i32,
                    },
                    local: NativePoint {
                        x: local.x.round() as i32,
                        y: local.y.round() as i32,
                    },
                    foreground: foreground_window_number(),
                });
            }
            event_ptr.as_ptr()
        });
        let mask = NSEventMask::MouseMoved
            | NSEventMask::LeftMouseDragged
            | NSEventMask::RightMouseDragged
            | NSEventMask::LeftMouseDown
            | NSEventMask::LeftMouseUp
            | NSEventMask::RightMouseDown
            | NSEventMask::RightMouseUp
            | NSEventMask::ScrollWheel;
        unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &handler) }
    }

    #[derive(Default)]
    struct ProbeApp {
        window: Option<Window>,
        _monitor: Option<Retained<AnyObject>>,
        _scroll_view: Option<Retained<NSScrollView>>,
        ready: Option<ProbeRecord>,
    }

    impl ProbeApp {
        fn emit_ready(&mut self) {
            if let Some(ready) = self.ready.take() {
                emit(ready);
                if std::env::var_os(
                    launchpad_windows::input_probe_protocol::QA_PASSIVE_MACOS_PROBE_ENV,
                )
                .is_some()
                {
                    let main_thread = MainThreadMarker::new().expect("probe main thread");
                    NSApplication::sharedApplication(main_thread).deactivate();
                }
            }
        }
    }

    impl ApplicationHandler for ProbeApp {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let attributes = Window::default_attributes()
                .with_title("Launchpad Native Input Probe")
                .with_position(PhysicalPosition::new(100, 100))
                .with_inner_size(PhysicalSize::new(1000, 700));
            let window = event_loop.create_window(attributes).expect("probe window");
            let handle = window.window_handle().expect("probe native window");
            let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
                unreachable!()
            };
            let view = unsafe { &*(handle.ns_view.as_ptr() as *const NSView) };
            let ns_window = view.window().expect("probe NSWindow");
            let main_thread = MainThreadMarker::new().expect("probe main thread");
            let scroll_view = ns_window.contentView().map(|content| {
                let scroll =
                    NSScrollView::initWithFrame(NSScrollView::alloc(main_thread), content.bounds());
                content.addSubview(&scroll);
                scroll
            });
            self._monitor = install_monitor(ns_window.clone());
            self._scroll_view = scroll_view;
            self.ready = Some(ProbeRecord::Ready {
                pid: std::process::id(),
                top_level: ns_window.windowNumber() as u64,
                child: self
                    ._scroll_view
                    .as_ref()
                    .map_or(0, |view| (&**view as *const NSScrollView) as usize as u64),
                rect: NativeRect {
                    left: 100,
                    top: 100,
                    right: 1100,
                    bottom: 800,
                },
            });
            self.window = Some(window);
            NSApplication::sharedApplication(main_thread).activate();
            self.window.as_ref().expect("probe window").focus_window();
            if ns_window.isKeyWindow() {
                self.emit_ready();
            }
        }

        fn window_event(
            &mut self,
            event_loop: &ActiveEventLoop,
            _window_id: WindowId,
            event: WindowEvent,
        ) {
            match event {
                WindowEvent::Focused(true) => self.emit_ready(),
                WindowEvent::CloseRequested => event_loop.exit(),
                _ => {}
            }
        }
    }

    pub fn run() -> Result<(), String> {
        let event_loop = EventLoop::new().map_err(|error| error.to_string())?;
        let mut app = ProbeApp::default();
        event_loop
            .run_app(&mut app)
            .map_err(|error| error.to_string())
    }
}

fn main() {
    #[cfg(windows)]
    if let Err(error) = windows_probe::run() {
        eprintln!("native input probe: {error}");
        std::process::exit(1);
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        eprintln!("native input probe is supported on Windows and macOS");
        std::process::exit(2);
    }

    #[cfg(target_os = "macos")]
    if let Err(error) = macos_probe::run() {
        eprintln!("native input probe: {error}");
        std::process::exit(1);
    }
}

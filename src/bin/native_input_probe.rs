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
        ShowWindow, TranslateMessage, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, GA_ROOT, HMENU,
        IDC_ARROW, MSG, SW_SHOW, WINDOW_EX_STYLE, WM_ACTIVATE, WM_CLOSE, WM_DESTROY, WM_KILLFOCUS,
        WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCREATE, WM_RBUTTONDOWN,
        WM_RBUTTONUP, WM_SETFOCUS, WNDCLASSW, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
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
            WM_RBUTTONUP => log_event(
                hwnd,
                lparam,
                ProbeEvent::ButtonUp {
                    button: ProbeButton::Right,
                },
                false,
            ),
            WM_MOUSEWHEEL => log_event(
                hwnd,
                lparam,
                ProbeEvent::VerticalWheel {
                    delta: ((wparam.0 >> 16) as u16 as i16) as i32,
                    key_state: wparam.0 as u16,
                    phase: NativePhase::Unavailable,
                },
                true,
            ),
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
                80,
                80,
                800,
                500,
                Some(top),
                Some(HMENU(1usize as *mut _)),
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
    {
        eprintln!("macOS native input probe is built by the macOS adapter commit");
        std::process::exit(2);
    }
}

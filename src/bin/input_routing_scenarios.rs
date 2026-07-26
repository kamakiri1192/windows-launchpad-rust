//! Native, JSONL-driven input scenario runner.
//!
//! The initial mode validates the probe and OS input generator independently
//! from the product delivery adapter. Product scenarios use the same protocol
//! and are added by the platform adapter modules.

#[cfg(windows)]
mod windows_runner {
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use launchpad_windows::input_probe_protocol::{ProbeButton, ProbeEvent, ProbeRecord};
    use windows::Win32::Foundation::{HWND, POINT};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_TYPE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindow, GetWindowThreadProcessId, IsWindow, IsWindowVisible,
        SetCursorPos, SetForegroundWindow, SetWindowPos, WindowFromPoint, GW_HWNDPREV,
        HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    };

    fn sibling_binary(name: &str) -> Result<std::path::PathBuf, String> {
        let current = std::env::current_exe().map_err(|error| error.to_string())?;
        let path = current.with_file_name(format!("{name}.exe"));
        path.is_file()
            .then_some(path.clone())
            .ok_or_else(|| format!("missing sibling binary {}", path.display()))
    }

    fn start_probe() -> Result<(Child, mpsc::Receiver<ProbeRecord>), String> {
        let mut child = Command::new(sibling_binary("native_input_probe")?)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| error.to_string())?;
        let stdout = child.stdout.take().ok_or("probe stdout unavailable")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(record) = ProbeRecord::from_json_line(&line) {
                    if std::env::var_os("LAUNCHPAD_INPUT_ROUTING_TRACE").is_some() {
                        eprintln!("launcher-record: {record:?}");
                    }
                    let _ = tx.send(record);
                }
            }
        });
        Ok((child, rx))
    }

    fn start_launcher() -> Result<(Child, mpsc::Receiver<ProbeRecord>), String> {
        let mut child = Command::new(sibling_binary("launchpad-windows")?)
            .env(
                launchpad_windows::input_probe_protocol::INPUT_ROUTING_QA_ENV,
                "1",
            )
            .env("LAUNCHPAD_ALLOW_SCREENSHOT", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| error.to_string())?;
        let stdout = child.stdout.take().ok_or("launcher stdout unavailable")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(record) = ProbeRecord::from_json_line(&line) {
                    if std::env::var_os("LAUNCHPAD_INPUT_ROUTING_TRACE").is_some() {
                        eprintln!("product-record: {record:?}");
                    }
                    let _ = tx.send(record);
                }
            }
        });
        Ok((child, rx))
    }

    fn wait_for(
        rx: &mpsc::Receiver<ProbeRecord>,
        timeout: Duration,
        mut predicate: impl FnMut(&ProbeRecord) -> bool,
    ) -> Result<ProbeRecord, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let record = rx
                .recv_timeout(remaining)
                .map_err(|error| format!("probe timeout: {error}"))?;
            if predicate(&record) {
                return Ok(record);
            }
        }
    }

    fn mouse_input(flags: u32, mouse_data: u32) -> INPUT {
        INPUT {
            r#type: INPUT_TYPE(0),
            Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: mouse_data,
                    dwFlags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS(flags),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn send(inputs: &[INPUT]) -> Result<(), String> {
        let inserted = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
        if inserted as usize == inputs.len() {
            Ok(())
        } else {
            Err(format!(
                "SendInput inserted {inserted}/{} events",
                inputs.len()
            ))
        }
    }

    fn wait_until(
        timeout: Duration,
        mut predicate: impl FnMut() -> bool,
        description: &str,
    ) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if predicate() {
                return Ok(());
            }
            std::thread::yield_now();
        }
        Err(format!("timed out waiting for {description}"))
    }

    fn z_order_index(hwnd: HWND) -> usize {
        let mut index = 0;
        let mut current = hwnd;
        while let Ok(previous) = unsafe { GetWindow(current, GW_HWNDPREV) } {
            index += 1;
            current = previous;
            if index > 10_000 {
                break;
            }
        }
        index
    }

    fn assert_same_window(hwnd: HWND, pid: u32) -> Result<(), String> {
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            return Err("launcher HWND was destroyed".to_owned());
        }
        let mut current_pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut current_pid)) };
        if current_pid != pid {
            return Err(format!("launcher HWND PID changed: {pid} -> {current_pid}"));
        }
        Ok(())
    }

    fn drain(receiver: &mpsc::Receiver<ProbeRecord>) {
        while receiver.try_recv().is_ok() {}
    }

    fn wait_launcher_snapshot(
        rx: &mpsc::Receiver<ProbeRecord>,
        predicate: impl FnMut(&ProbeRecord) -> bool,
    ) -> Result<ProbeRecord, String> {
        wait_for(rx, Duration::from_secs(20), predicate)
    }

    fn assert_no_probe_input(
        rx: &mpsc::Receiver<ProbeRecord>,
        forbidden: impl Fn(&ProbeEvent) -> bool,
        case_name: &str,
    ) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_millis(350);
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(remaining) {
                Ok(ProbeRecord::Input { event, .. }) if forbidden(&event) => {
                    return Err(format!("{case_name}: unexpected probe input {event:?}"));
                }
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => return Ok(()),
                Err(error) => return Err(format!("{case_name}: probe disconnected: {error}")),
            }
        }
        Ok(())
    }

    fn run_product_case(case_name: &str) -> Result<(), String> {
        let (mut probe, probe_rx) = start_probe()?;
        let mut launcher_slot: Option<Child> = None;
        let result = (|| {
            let ready = wait_for(&probe_rx, Duration::from_secs(10), |record| {
                matches!(record, ProbeRecord::Ready { .. })
            })?;
            let ProbeRecord::Ready {
                top_level: probe_window,
                rect,
                ..
            } = ready
            else {
                unreachable!()
            };
            let (launcher, launcher_rx) = start_launcher()?;
            launcher_slot = Some(launcher);
            let initial = wait_launcher_snapshot(&launcher_rx, |record| {
                matches!(
                    record,
                    ProbeRecord::LauncherSnapshot {
                        window,
                        visible: true,
                        ..
                    } if *window != 0
                )
            })?;
            let ProbeRecord::LauncherSnapshot {
                pid,
                window,
                generation,
                ..
            } = initial
            else {
                unreachable!()
            };
            let hwnd = HWND(window as usize as *mut _);
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;
            unsafe {
                SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    rect.left,
                    rect.top,
                    width,
                    height,
                    SWP_SHOWWINDOW,
                )
                .map_err(|error| error.to_string())?;
                SetWindowPos(
                    HWND(probe_window as usize as *mut _),
                    Some(hwnd),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                )
                .map_err(|error| error.to_string())?;
                let _ = SetForegroundWindow(hwnd);
                SetCursorPos(rect.left + 300, rect.top + 300).map_err(|error| error.to_string())?;
                SetCursorPos(rect.left + 50, rect.top + 50).map_err(|error| error.to_string())?;
                let hit = WindowFromPoint(POINT {
                    x: rect.left + 50,
                    y: rect.top + 50,
                });
                if hit != hwnd {
                    return Err(format!(
                        "{case_name}: launcher did not own the outside test point (expected {window:#x}, got {:#x})",
                        hit.0 as usize
                    ));
                }
            }
            let outside = wait_launcher_snapshot(&launcher_rx, |record| {
                matches!(
                    record,
                    ProbeRecord::LauncherSnapshot {
                        generation: next_generation,
                        region,
                        visible: true,
                        ..
                    } if *next_generation > generation
                        && region == "OutsideTransparent"
                )
            })?;
            let ProbeRecord::LauncherSnapshot {
                generation: outside_generation,
                ..
            } = outside
            else {
                unreachable!()
            };
            drain(&probe_rx);
            let foreground_before = unsafe { GetForegroundWindow() };
            let z_before = z_order_index(hwnd);

            match case_name {
                "left_click" | "right_click" => {
                    let (down, up, button) = if case_name == "left_click" {
                        (
                            MOUSEEVENTF_LEFTDOWN.0,
                            MOUSEEVENTF_LEFTUP.0,
                            ProbeButton::Left,
                        )
                    } else {
                        (
                            MOUSEEVENTF_RIGHTDOWN.0,
                            MOUSEEVENTF_RIGHTUP.0,
                            ProbeButton::Right,
                        )
                    };
                    send(&[mouse_input(down, 0)])?;
                    let pending_prefix = if case_name == "left_click" {
                        "LeftPending"
                    } else {
                        "RightPending"
                    };
                    wait_launcher_snapshot(&launcher_rx, |record| {
                        matches!(
                            record,
                            ProbeRecord::LauncherSnapshot {
                                generation: next_generation,
                                router_state,
                                visible: true,
                                ..
                            } if *next_generation > outside_generation
                                && router_state.starts_with(pending_prefix)
                        )
                    })?;
                    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
                        return Err(format!("{case_name}: launcher hid before physical up"));
                    }
                    send(&[mouse_input(up, 0)])?;
                    wait_until(
                        Duration::from_secs(5),
                        || !unsafe { IsWindowVisible(hwnd) }.as_bool(),
                        "launcher hide after click",
                    )?;
                    let down_event = wait_for(&probe_rx, Duration::from_secs(5), |record| {
                        matches!(
                            record,
                            ProbeRecord::Input {
                                event: ProbeEvent::ButtonDown { button: observed },
                                ..
                            } if *observed == button
                        )
                    })?;
                    let up_event = wait_for(&probe_rx, Duration::from_secs(5), |record| {
                        matches!(
                            record,
                            ProbeRecord::Input {
                                event: ProbeEvent::ButtonUp { button: observed },
                                ..
                            } if *observed == button
                        )
                    })?;
                    let (
                        ProbeRecord::Input {
                            serial: down_serial,
                            target: down_target,
                            pid: target_pid,
                            ..
                        },
                        ProbeRecord::Input {
                            serial: up_serial,
                            target: up_target,
                            ..
                        },
                    ) = (down_event, up_event)
                    else {
                        unreachable!()
                    };
                    if down_serial >= up_serial || down_target != up_target {
                        return Err(format!("{case_name}: incomplete or misordered click"));
                    }
                    if target_pid == pid {
                        return Err(format!("{case_name}: click looped back to launcher"));
                    }
                    assert_no_probe_input(
                        &probe_rx,
                        |event| {
                            matches!(
                                event,
                                ProbeEvent::ButtonDown { button: observed }
                                    | ProbeEvent::ButtonUp { button: observed }
                                    if *observed == button
                            )
                        },
                        case_name,
                    )?;
                }
                "left_drag" => {
                    send(&[mouse_input(MOUSEEVENTF_LEFTDOWN.0, 0)])?;
                    unsafe {
                        SetCursorPos(rect.left + 80, rect.top + 50)
                            .map_err(|error| error.to_string())?;
                    }
                    let drag = wait_launcher_snapshot(&launcher_rx, |record| {
                        matches!(
                            record,
                            ProbeRecord::LauncherSnapshot {
                                generation: next_generation,
                                router_state,
                                page_position,
                                ..
                            } if *next_generation > outside_generation
                                && router_state.starts_with("PageDrag")
                                && page_position.abs() > 0.01
                        )
                    })?;
                    if !matches!(drag, ProbeRecord::LauncherSnapshot { visible: true, .. }) {
                        return Err("left_drag: launcher did not stay visible".to_owned());
                    }
                    send(&[mouse_input(MOUSEEVENTF_LEFTUP.0, 0)])?;
                    assert_no_probe_input(
                        &probe_rx,
                        |event| {
                            matches!(
                                event,
                                ProbeEvent::ButtonDown { .. } | ProbeEvent::ButtonUp { .. }
                            )
                        },
                        case_name,
                    )?;
                }
                "right_drag_cancel" => {
                    send(&[mouse_input(MOUSEEVENTF_RIGHTDOWN.0, 0)])?;
                    unsafe {
                        SetCursorPos(rect.left + 80, rect.top + 50)
                            .map_err(|error| error.to_string())?;
                    }
                    wait_launcher_snapshot(&launcher_rx, |record| {
                        matches!(
                            record,
                            ProbeRecord::LauncherSnapshot {
                                generation: next_generation,
                                router_state,
                                visible: true,
                                ..
                            } if *next_generation > outside_generation
                                && router_state == "RightCancelled"
                        )
                    })?;
                    send(&[mouse_input(MOUSEEVENTF_RIGHTUP.0, 0)])?;
                    assert_no_probe_input(
                        &probe_rx,
                        |event| {
                            matches!(
                                event,
                                ProbeEvent::ButtonDown { .. } | ProbeEvent::ButtonUp { .. }
                            )
                        },
                        case_name,
                    )?;
                }
                "vertical_wheel" => {
                    send(&[mouse_input(MOUSEEVENTF_WHEEL.0, 30)])?;
                    let wheel = wait_for(&probe_rx, Duration::from_secs(5), |record| {
                        matches!(
                            record,
                            ProbeRecord::Input {
                                event: ProbeEvent::VerticalWheel { delta: 30, .. },
                                ..
                            }
                        )
                    })?;
                    if !matches!(wheel, ProbeRecord::Input { pid: target_pid, .. } if target_pid != pid)
                    {
                        return Err("vertical_wheel: wrong target PID".to_owned());
                    }
                    assert_no_probe_input(
                        &probe_rx,
                        |event| matches!(event, ProbeEvent::VerticalWheel { .. }),
                        case_name,
                    )?;
                    if unsafe { GetForegroundWindow() } != foreground_before
                        || z_order_index(hwnd) != z_before
                    {
                        return Err(
                            "vertical_wheel: adapter changed focus or launcher Z-order".to_owned()
                        );
                    }
                }
                "hover" => {
                    unsafe {
                        SetCursorPos(rect.left + 55, rect.top + 55)
                            .map_err(|error| error.to_string())?;
                    }
                    wait_launcher_snapshot(&launcher_rx, |record| {
                        matches!(
                            record,
                            ProbeRecord::LauncherSnapshot {
                                generation: next_generation,
                                visible: true,
                                ..
                            } if *next_generation > outside_generation
                        )
                    })?;
                    assert_no_probe_input(
                        &probe_rx,
                        |event| matches!(event, ProbeEvent::MouseMove),
                        case_name,
                    )?;
                }
                _ => return Err(format!("unknown product case {case_name}")),
            }
            assert_same_window(hwnd, pid)?;
            println!("input-routing-e2e: {case_name} passed");
            Ok(())
        })();
        if let Some(mut launcher) = launcher_slot {
            let _ = launcher.kill();
            let _ = launcher.wait();
        }
        let _ = probe.kill();
        let _ = probe.wait();
        result
    }

    pub fn run_probe_self_test() -> Result<(), String> {
        let (mut probe, rx) = start_probe()?;
        let result = (|| {
            let ready = wait_for(&rx, Duration::from_secs(10), |record| {
                matches!(record, ProbeRecord::Ready { .. })
            })?;
            let ProbeRecord::Ready {
                top_level, rect, ..
            } = ready
            else {
                unreachable!()
            };
            let point = POINT {
                x: rect.left + 200,
                y: rect.top + 200,
            };
            unsafe {
                let _ = SetForegroundWindow(HWND(top_level as usize as *mut _));
                SetCursorPos(point.x, point.y).map_err(|error| error.to_string())?;
            }
            send(&[
                mouse_input(MOUSEEVENTF_LEFTDOWN.0, 0),
                mouse_input(MOUSEEVENTF_LEFTUP.0, 0),
            ])?;
            let down = wait_for(&rx, Duration::from_secs(5), |record| {
                matches!(
                    record,
                    ProbeRecord::Input {
                        event: ProbeEvent::ButtonDown {
                            button: ProbeButton::Left
                        },
                        ..
                    }
                )
            })?;
            let up = wait_for(&rx, Duration::from_secs(5), |record| {
                matches!(
                    record,
                    ProbeRecord::Input {
                        event: ProbeEvent::ButtonUp {
                            button: ProbeButton::Left
                        },
                        ..
                    }
                )
            })?;
            let (
                ProbeRecord::Input {
                    serial: down_serial,
                    ..
                },
                ProbeRecord::Input {
                    serial: up_serial, ..
                },
            ) = (down, up)
            else {
                unreachable!()
            };
            if down_serial >= up_serial {
                return Err("probe observed button events out of order".to_owned());
            }

            send(&[mouse_input(MOUSEEVENTF_WHEEL.0, 30_u32)])?;
            let wheel = wait_for(&rx, Duration::from_secs(5), |record| {
                matches!(
                    record,
                    ProbeRecord::Input {
                        event: ProbeEvent::VerticalWheel { delta: 30, .. },
                        ..
                    }
                )
            })?;
            println!("{}", wheel.to_json_line().map_err(|e| e.to_string())?);
            Ok(())
        })();
        let _ = probe.kill();
        let _ = probe.wait();
        result
    }

    pub fn run_product() -> Result<(), String> {
        for case_name in [
            "left_click",
            "left_drag",
            "right_click",
            "right_drag_cancel",
            "vertical_wheel",
            "hover",
        ] {
            run_product_case(case_name)?;
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod macos_runner {
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use launchpad_windows::input_probe_protocol::{
        NativePhase, ProbeButton, ProbeEvent, ProbeRecord,
    };
    use objc2_core_graphics::{
        CGEvent, CGEventField, CGEventTapLocation, CGEventType, CGMouseButton, CGScrollEventUnit,
    };
    use objc2_foundation::NSPoint;

    fn sibling_binary(name: &str) -> Result<std::path::PathBuf, String> {
        let current = std::env::current_exe().map_err(|error| error.to_string())?;
        let path = current.with_file_name(name);
        path.is_file()
            .then_some(path.clone())
            .ok_or_else(|| format!("missing sibling binary {}", path.display()))
    }

    fn start_process(
        name: &str,
        qa_product: bool,
    ) -> Result<(Child, mpsc::Receiver<ProbeRecord>), String> {
        let mut command = Command::new(sibling_binary(name)?);
        if qa_product {
            command
                .env(
                    launchpad_windows::input_probe_protocol::INPUT_ROUTING_QA_ENV,
                    "1",
                )
                .env_remove("LAUNCHPAD_QA_SCENARIO");
        }
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| error.to_string())?;
        let stdout = child.stdout.take().ok_or("child stdout unavailable")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(record) = ProbeRecord::from_json_line(&line) {
                    let _ = tx.send(record);
                }
            }
        });
        Ok((child, rx))
    }

    fn wait_for(
        rx: &mpsc::Receiver<ProbeRecord>,
        timeout: Duration,
        mut predicate: impl FnMut(&ProbeRecord) -> bool,
    ) -> Result<ProbeRecord, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let record = rx
                .recv_timeout(remaining)
                .map_err(|error| format!("JSONL timeout: {error}"))?;
            if predicate(&record) {
                return Ok(record);
            }
        }
    }

    fn post_mouse(
        event_type: CGEventType,
        button: CGMouseButton,
        x: f64,
        y: f64,
    ) -> Result<(), String> {
        let event = CGEvent::new_mouse_event(None, event_type, NSPoint { x, y }, button)
            .ok_or("CGEventCreateMouseEvent failed")?;
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
        Ok(())
    }

    fn move_pointer(x: f64, y: f64) -> Result<(), String> {
        post_mouse(CGEventType::MouseMoved, CGMouseButton::Left, x, y)
    }

    fn click_event(button: ProbeButton, down: bool, x: f64, y: f64) -> Result<(), String> {
        let (event_type, mouse_button) = match (button, down) {
            (ProbeButton::Left, true) => (CGEventType::LeftMouseDown, CGMouseButton::Left),
            (ProbeButton::Left, false) => (CGEventType::LeftMouseUp, CGMouseButton::Left),
            (ProbeButton::Right, true) => (CGEventType::RightMouseDown, CGMouseButton::Right),
            (ProbeButton::Right, false) => (CGEventType::RightMouseUp, CGMouseButton::Right),
        };
        post_mouse(event_type, mouse_button, x, y)
    }

    fn drag_event(button: ProbeButton, x: f64, y: f64) -> Result<(), String> {
        let (event_type, mouse_button) = match button {
            ProbeButton::Left => (CGEventType::LeftMouseDragged, CGMouseButton::Left),
            ProbeButton::Right => (CGEventType::RightMouseDragged, CGMouseButton::Right),
        };
        post_mouse(event_type, mouse_button, x, y)
    }

    fn post_precise_scroll(delta_y: i32, phase: i64, momentum: i64) -> Result<(), String> {
        let event =
            CGEvent::new_scroll_wheel_event2(None, CGScrollEventUnit::Pixel, 2, delta_y, 0, 0)
                .ok_or("CGEventCreateScrollWheelEvent2 failed")?;
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::ScrollWheelEventIsContinuous,
            1,
        );
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::ScrollWheelEventPointDeltaAxis1,
            delta_y as i64,
        );
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::ScrollWheelEventScrollPhase,
            phase,
        );
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::ScrollWheelEventMomentumPhase,
            momentum,
        );
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
        Ok(())
    }

    fn assert_post_permission() -> Result<(), String> {
        objc2_core_graphics::CGPreflightPostEventAccess()
            .then_some(())
            .ok_or_else(|| {
                "macOS input-post permission is unavailable; use a dedicated runner with Accessibility permission"
                    .to_owned()
            })
    }

    fn drain(rx: &mpsc::Receiver<ProbeRecord>) {
        while rx.try_recv().is_ok() {}
    }

    fn assert_no_input(
        rx: &mpsc::Receiver<ProbeRecord>,
        predicate: impl Fn(&ProbeEvent) -> bool,
        case_name: &str,
    ) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_millis(350);
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(remaining) {
                Ok(ProbeRecord::Input { event, .. }) if predicate(&event) => {
                    return Err(format!("{case_name}: unexpected probe event {event:?}"));
                }
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => return Ok(()),
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(())
    }

    fn run_product_case(case_name: &str) -> Result<(), String> {
        let (mut probe, probe_rx) = start_process("native_input_probe", false)?;
        let mut launcher_slot: Option<Child> = None;
        let result = (|| {
            wait_for(&probe_rx, Duration::from_secs(10), |record| {
                matches!(record, ProbeRecord::Ready { .. })
            })?;
            let (launcher, launcher_rx) = start_process("launchpad-windows", true)?;
            launcher_slot = Some(launcher);
            let initial = wait_for(&launcher_rx, Duration::from_secs(45), |record| {
                matches!(
                    record,
                    ProbeRecord::LauncherSnapshot {
                        window,
                        visible: true,
                        ..
                    } if *window != 0
                )
            })?;
            let ProbeRecord::LauncherSnapshot {
                pid,
                window,
                generation,
                ..
            } = initial
            else {
                unreachable!()
            };
            move_pointer(150.0, 150.0)?;
            let outside = wait_for(&launcher_rx, Duration::from_secs(10), |record| {
                matches!(
                    record,
                    ProbeRecord::LauncherSnapshot {
                        generation: next,
                        region,
                        visible: true,
                        ..
                    } if *next > generation && region == "OutsideTransparent"
                )
            })?;
            let ProbeRecord::LauncherSnapshot {
                generation: outside_generation,
                ..
            } = outside
            else {
                unreachable!()
            };
            drain(&probe_rx);

            match case_name {
                "left_click" | "right_click" => {
                    let button = if case_name == "left_click" {
                        ProbeButton::Left
                    } else {
                        ProbeButton::Right
                    };
                    click_event(button, true, 150.0, 150.0)?;
                    let pending = if button == ProbeButton::Left {
                        "LeftPending"
                    } else {
                        "RightPending"
                    };
                    wait_for(&launcher_rx, Duration::from_secs(5), |record| {
                        matches!(
                            record,
                            ProbeRecord::LauncherSnapshot {
                                generation: next,
                                router_state,
                                visible: true,
                                ..
                            } if *next > outside_generation && router_state.starts_with(pending)
                        )
                    })?;
                    click_event(button, false, 150.0, 150.0)?;
                    wait_for(&launcher_rx, Duration::from_secs(5), |record| {
                        matches!(
                            record,
                            ProbeRecord::LauncherSnapshot {
                                visible: false,
                                pid: observed_pid,
                                window: observed_window,
                                ..
                            } if *observed_pid == pid && *observed_window == window
                        )
                    })?;
                    let down = wait_for(&probe_rx, Duration::from_secs(5), |record| {
                        matches!(
                            record,
                            ProbeRecord::Input {
                                event: ProbeEvent::ButtonDown { button: observed },
                                ..
                            } if *observed == button
                        )
                    })?;
                    let up = wait_for(&probe_rx, Duration::from_secs(5), |record| {
                        matches!(
                            record,
                            ProbeRecord::Input {
                                event: ProbeEvent::ButtonUp { button: observed },
                                ..
                            } if *observed == button
                        )
                    })?;
                    if !matches!(
                        (down, up),
                        (
                            ProbeRecord::Input { serial: a, .. },
                            ProbeRecord::Input { serial: b, .. }
                        ) if a < b
                    ) {
                        return Err(format!("{case_name}: click ordering failed"));
                    }
                }
                "left_drag" | "right_drag_cancel" => {
                    let button = if case_name == "left_drag" {
                        ProbeButton::Left
                    } else {
                        ProbeButton::Right
                    };
                    click_event(button, true, 150.0, 150.0)?;
                    drag_event(button, 180.0, 150.0)?;
                    let expected = if button == ProbeButton::Left {
                        "PageDrag"
                    } else {
                        "RightCancelled"
                    };
                    wait_for(&launcher_rx, Duration::from_secs(5), |record| {
                        matches!(
                            record,
                            ProbeRecord::LauncherSnapshot {
                                router_state,
                                visible: true,
                                ..
                            } if router_state.starts_with(expected)
                        )
                    })?;
                    click_event(button, false, 180.0, 150.0)?;
                    assert_no_input(
                        &probe_rx,
                        |event| {
                            matches!(
                                event,
                                ProbeEvent::ButtonDown { .. } | ProbeEvent::ButtonUp { .. }
                            )
                        },
                        case_name,
                    )?;
                }
                "vertical_wheel" => {
                    post_precise_scroll(-7, 1, 0)?;
                    let wheel = wait_for(&probe_rx, Duration::from_secs(5), |record| {
                        matches!(
                            record,
                            ProbeRecord::Input {
                                event: ProbeEvent::VerticalWheel {
                                    delta_y,
                                    precise: true,
                                    phase: NativePhase::Began,
                                    ..
                                },
                                ..
                            } if (*delta_y + 7.0).abs() < 0.01
                        )
                    })?;
                    if !matches!(wheel, ProbeRecord::Input { pid: target, .. } if target != pid) {
                        return Err("vertical_wheel: self-delivery detected".to_owned());
                    }
                    assert_no_input(
                        &probe_rx,
                        |event| matches!(event, ProbeEvent::VerticalWheel { .. }),
                        case_name,
                    )?;
                }
                "hover" => {
                    move_pointer(155.0, 155.0)?;
                    wait_for(&launcher_rx, Duration::from_secs(5), |record| {
                        matches!(
                            record,
                            ProbeRecord::LauncherSnapshot {
                                generation: next,
                                visible: true,
                                ..
                            } if *next > outside_generation
                        )
                    })?;
                    assert_no_input(
                        &probe_rx,
                        |event| matches!(event, ProbeEvent::MouseMove),
                        case_name,
                    )?;
                }
                _ => return Err(format!("unknown case {case_name}")),
            }
            println!("input-routing-e2e: {case_name} passed");
            Ok(())
        })();
        if let Some(mut launcher) = launcher_slot {
            let _ = launcher.kill();
            let _ = launcher.wait();
        }
        let _ = probe.kill();
        let _ = probe.wait();
        result
    }

    pub fn run_probe_self_test() -> Result<(), String> {
        assert_post_permission()?;
        let (mut probe, rx) = start_process("native_input_probe", false)?;
        let result = (|| {
            wait_for(&rx, Duration::from_secs(10), |record| {
                matches!(record, ProbeRecord::Ready { .. })
            })?;
            move_pointer(150.0, 150.0)?;
            click_event(ProbeButton::Left, true, 150.0, 150.0)?;
            click_event(ProbeButton::Left, false, 150.0, 150.0)?;
            wait_for(&rx, Duration::from_secs(5), |record| {
                matches!(
                    record,
                    ProbeRecord::Input {
                        event: ProbeEvent::ButtonUp {
                            button: ProbeButton::Left
                        },
                        ..
                    }
                )
            })?;
            post_precise_scroll(-7, 1, 0)?;
            wait_for(&rx, Duration::from_secs(5), |record| {
                matches!(
                    record,
                    ProbeRecord::Input {
                        event: ProbeEvent::VerticalWheel { precise: true, .. },
                        ..
                    }
                )
            })?;
            Ok(())
        })();
        let _ = probe.kill();
        let _ = probe.wait();
        result
    }

    pub fn run_product() -> Result<(), String> {
        assert_post_permission()?;
        for case_name in [
            "left_click",
            "left_drag",
            "right_click",
            "right_drag_cancel",
            "vertical_wheel",
            "hover",
        ] {
            run_product_case(case_name)?;
        }
        Ok(())
    }
}

fn main() {
    #[cfg(windows)]
    let result = if std::env::args().any(|arg| arg == "--product") {
        windows_runner::run_product()
    } else {
        windows_runner::run_probe_self_test()
    };

    #[cfg(windows)]
    if let Err(error) = result {
        eprintln!("input routing scenarios: {error}");
        std::process::exit(1);
    }

    #[cfg(target_os = "macos")]
    let result = if std::env::args().any(|arg| arg == "--product") {
        macos_runner::run_product()
    } else {
        macos_runner::run_probe_self_test()
    };

    #[cfg(target_os = "macos")]
    if let Err(error) = result {
        eprintln!("input routing scenarios: {error}");
        std::process::exit(1);
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        eprintln!("input routing scenarios are supported on Windows and macOS");
        std::process::exit(2);
    }
}

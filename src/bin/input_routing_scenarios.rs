//! Native, JSONL-driven input scenario runner.
//!
//! The initial mode validates the probe and OS input generator independently
//! from the product delivery adapter. Product scenarios use the same protocol
//! and are added by the platform adapter modules.

#[cfg(windows)]
mod windows_runner {
    use std::io::{BufRead, BufReader, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
    use std::sync::{mpsc, Arc};
    use std::time::{Duration, Instant};

    use launchpad_windows::input_probe_protocol::{ProbeButton, ProbeEvent, ProbeRecord};
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, POINT};
    use windows::Win32::Graphics::Gdi::{CreateRectRgn, DeleteObject, GetWindowRgn, HGDIOBJ};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_TYPE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE,
        MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetForegroundWindow, GetWindow, GetWindowLongPtrW, GetWindowTextLengthW,
        GetWindowTextW, GetWindowThreadProcessId, IsWindow, IsWindowVisible, SetCursorPos,
        SetForegroundWindow, SetWindowPos, SystemParametersInfoW, WindowFromPoint, GWL_EXSTYLE,
        GW_HWNDPREV, HWND_TOPMOST, MOUSEWHEEL_ROUTING_FOCUS, SPI_GETMOUSEWHEELROUTING,
        SPI_SETMOUSEWHEELROUTING, SWP_NOACTIVATE, SWP_SHOWWINDOW,
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WS_EX_TOPMOST,
    };

    struct MouseWheelRoutingGuard {
        previous: u32,
    }

    impl MouseWheelRoutingGuard {
        fn focus() -> Result<Self, String> {
            let mut previous = 0u32;
            unsafe {
                SystemParametersInfoW(
                    SPI_GETMOUSEWHEELROUTING,
                    0,
                    Some((&mut previous as *mut u32).cast()),
                    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                )
                .map_err(|error| error.to_string())?;
                SystemParametersInfoW(
                    SPI_SETMOUSEWHEELROUTING,
                    MOUSEWHEEL_ROUTING_FOCUS,
                    None,
                    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                )
                .map_err(|error| error.to_string())?;
            }
            Ok(Self { previous })
        }
    }

    impl Drop for MouseWheelRoutingGuard {
        fn drop(&mut self) {
            let _ = unsafe {
                SystemParametersInfoW(
                    SPI_SETMOUSEWHEELROUTING,
                    self.previous,
                    None,
                    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                )
            };
        }
    }

    const EDGE_PAGE_TITLE: &str = "Launchpad Input Routing Scroll Compatibility";
    const EDGE_PAGE: &str = r#"<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <title>Launchpad Input Routing Scroll Compatibility</title>
  <style>
    html, body { margin: 0; min-height: 6000px; }
    body { background: linear-gradient(#fff, #ddd); }
  </style>
</head>
<body>
  <script>
    const report = () => fetch(`/position?y=${window.scrollY}`, {
      cache: "no-store"
    }).catch(() => {});
    window.scrollTo(0, 0);
    window.addEventListener("load", report);
    window.addEventListener("scroll", report, { passive: true });
    setInterval(report, 250);
  </script>
</body>
</html>
"#;

    struct ScrollServer {
        address: SocketAddr,
        scroll_y: Arc<AtomicI64>,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl ScrollServer {
        fn start() -> Result<Self, String> {
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .map_err(|error| format!("bind Edge compatibility server: {error}"))?;
            let address = listener
                .local_addr()
                .map_err(|error| format!("read Edge compatibility address: {error}"))?;
            listener
                .set_nonblocking(true)
                .map_err(|error| format!("configure Edge compatibility server: {error}"))?;
            let scroll_y = Arc::new(AtomicI64::new(-1));
            let thread_scroll_y = Arc::clone(&scroll_y);
            let stop = Arc::new(AtomicBool::new(false));
            let thread_stop = Arc::clone(&stop);
            let thread = std::thread::spawn(move || {
                while !thread_stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if let Err(error) = serve_scroll_request(stream, &thread_scroll_y) {
                                if !matches!(
                                    error.kind(),
                                    std::io::ErrorKind::BrokenPipe
                                        | std::io::ErrorKind::ConnectionAborted
                                        | std::io::ErrorKind::ConnectionReset
                                        | std::io::ErrorKind::UnexpectedEof
                                ) {
                                    eprintln!("browser-compat server request failed: {error}");
                                }
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => {
                            eprintln!("browser-compat server accept failed: {error}");
                            break;
                        }
                    }
                }
            });
            Ok(Self {
                address,
                scroll_y,
                stop,
                thread: Some(thread),
            })
        }

        fn url(&self) -> String {
            format!("http://{}/", self.address)
        }

        fn scroll_y(&self) -> i64 {
            self.scroll_y.load(Ordering::Acquire)
        }
    }

    impl Drop for ScrollServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(100));
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn serve_scroll_request(
        mut stream: TcpStream,
        scroll_y: &AtomicI64,
    ) -> Result<(), std::io::Error> {
        // Accepted sockets inherit the listener's nonblocking mode on
        // Windows. Requests are tiny, so restore blocking reads with a tight
        // timeout instead of racing the first browser write.
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        let mut request_line = String::new();
        BufReader::new(stream.try_clone()?).read_line(&mut request_line)?;
        let path = request_line.split_whitespace().nth(1).unwrap_or("/");
        if let Some(value) = path.strip_prefix("/position?y=") {
            if let Ok(value) = value.parse::<f64>() {
                scroll_y.store(value.round() as i64, Ordering::Release);
            }
            write_http_response(&mut stream, "text/plain", b"ok")
        } else {
            write_http_response(
                &mut stream,
                "text/html; charset=utf-8",
                EDGE_PAGE.as_bytes(),
            )
        }
    }

    fn write_http_response(
        stream: &mut TcpStream,
        content_type: &str,
        body: &[u8],
    ) -> Result<(), std::io::Error> {
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
            body.len()
        )?;
        stream.write_all(body)?;
        stream.flush()
    }

    fn edge_executable() -> Result<std::path::PathBuf, String> {
        if let Some(path) = std::env::var_os("LAUNCHPAD_EDGE_EXE").map(std::path::PathBuf::from) {
            if path.is_file() {
                return Ok(path);
            }
            return Err(format!(
                "LAUNCHPAD_EDGE_EXE does not name a file: {}",
                path.display()
            ));
        }
        for root in ["ProgramFiles(x86)", "ProgramFiles"] {
            if let Some(root) = std::env::var_os(root) {
                let path = std::path::PathBuf::from(root)
                    .join("Microsoft")
                    .join("Edge")
                    .join("Application")
                    .join("msedge.exe");
                if path.is_file() {
                    return Ok(path);
                }
            }
        }
        Err("Microsoft Edge executable not found".to_owned())
    }

    struct WindowTitleSearch<'a> {
        title_fragment: &'a str,
        found: Option<HWND>,
    }

    unsafe extern "system" fn find_window_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = unsafe { &mut *(lparam.0 as *mut WindowTitleSearch<'_>) };
        if unsafe { IsWindowVisible(hwnd) }.as_bool() {
            let title_length = unsafe { GetWindowTextLengthW(hwnd) };
            if title_length > 0 {
                let mut title = vec![0_u16; title_length as usize + 1];
                let copied = unsafe { GetWindowTextW(hwnd, &mut title) };
                let title = String::from_utf16_lossy(&title[..copied.max(0) as usize]);
                if title.contains(search.title_fragment) {
                    search.found = Some(hwnd);
                    return BOOL(0);
                }
            }
        }
        BOOL(1)
    }

    fn find_window_by_title(title_fragment: &str) -> Option<HWND> {
        let mut search = WindowTitleSearch {
            title_fragment,
            found: None,
        };
        let _ = unsafe {
            EnumWindows(
                Some(find_window_callback),
                LPARAM((&mut search as *mut WindowTitleSearch<'_>) as isize),
            )
        };
        search.found
    }

    fn sibling_binary(name: &str) -> Result<std::path::PathBuf, String> {
        let current = std::env::current_exe().map_err(|error| error.to_string())?;
        let path = current.with_file_name(format!("{name}.exe"));
        path.is_file()
            .then_some(path.clone())
            .ok_or_else(|| format!("missing sibling binary {}", path.display()))
    }

    fn start_probe(
        activate_on_wheel: bool,
    ) -> Result<(Child, mpsc::Receiver<ProbeRecord>), String> {
        let mut command = Command::new(sibling_binary("native_input_probe")?);
        command.stdout(Stdio::piped()).stderr(Stdio::inherit());
        if activate_on_wheel {
            command.env(
                launchpad_windows::input_probe_protocol::QA_WHEEL_RECEIVER_ACTIVATION_ENV,
                "1",
            );
        }
        let mut child = command.spawn().map_err(|error| error.to_string())?;
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

    fn start_launcher(
        allow_receiver_activation: bool,
    ) -> Result<(Child, mpsc::Receiver<ProbeRecord>), String> {
        let mut command = Command::new(sibling_binary("launchpad-windows")?);
        command
            .env(
                launchpad_windows::input_probe_protocol::INPUT_ROUTING_QA_ENV,
                "1",
            )
            .env("LAUNCHPAD_ALLOW_SCREENSHOT", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if allow_receiver_activation {
            command.env(
                launchpad_windows::input_probe_protocol::QA_WHEEL_RECEIVER_ACTIVATION_ENV,
                "1",
            );
        }
        let mut child = command.spawn().map_err(|error| error.to_string())?;
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

    fn relative_mouse_move(dx: i32, dy: i32) -> INPUT {
        INPUT {
            r#type: INPUT_TYPE(0),
            Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: 0,
                    dwFlags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS(
                        MOUSEEVENTF_MOVE.0,
                    ),
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

    fn window_region_type(hwnd: HWND) -> Result<i32, String> {
        let region = unsafe { CreateRectRgn(0, 0, 0, 0) };
        if region.is_invalid() {
            return Err("CreateRectRgn failed".to_owned());
        }
        let region_type = unsafe { GetWindowRgn(hwnd, region) }.0;
        let _ = unsafe { DeleteObject(HGDIOBJ(region.0)) };
        Ok(region_type)
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

    fn assert_probe_destination(
        record: &ProbeRecord,
        expected_pid: u32,
        expected_target: u64,
        expected_root: u64,
        expected_x: i32,
        expected_y: i32,
        case_name: &str,
    ) -> Result<(), String> {
        match record {
            ProbeRecord::Input {
                target,
                root,
                pid,
                screen,
                ..
            } if *pid == expected_pid
                && *target == expected_target
                && *root == expected_root
                && screen.x == expected_x
                && screen.y == expected_y =>
            {
                Ok(())
            }
            other => Err(format!(
                "{case_name}: wrong PID/window/coordinate destination: {other:?}"
            )),
        }
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

    fn assert_window_state_stable(
        hwnd: HWND,
        foreground: HWND,
        z_order: usize,
        duration: Duration,
        case_name: &str,
    ) -> Result<(), String> {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
                return Err(format!("{case_name}: launcher closed"));
            }
            if unsafe { GetForegroundWindow() } != foreground {
                return Err(format!("{case_name}: foreground window changed"));
            }
            if z_order_index(hwnd) != z_order {
                return Err(format!("{case_name}: launcher Z-order changed"));
            }
            std::thread::yield_now();
        }
        Ok(())
    }

    fn run_product_case(case_name: &str) -> Result<(), String> {
        let receiver_activation = case_name == "vertical_wheel_receiver_activation";
        let (mut probe, probe_rx) = start_probe(receiver_activation)?;
        let mut launcher_slot: Option<Child> = None;
        let result = (|| {
            let ready = wait_for(&probe_rx, Duration::from_secs(10), |record| {
                matches!(record, ProbeRecord::Ready { .. })
            })?;
            let ProbeRecord::Ready {
                pid: probe_pid,
                top_level: probe_window,
                child: probe_child,
                rect,
                ..
            } = ready
            else {
                unreachable!()
            };
            let (launcher, launcher_rx) = start_launcher(receiver_activation)?;
            launcher_slot = Some(launcher);
            let initial = wait_launcher_snapshot(&launcher_rx, |record| {
                matches!(
                    record,
                    ProbeRecord::LauncherSnapshot {
                        window,
                        visible: true,
                        focused: true,
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
            // This lies inside the probe's nested child but remains outside
            // the launcher's page frame. The observed product snapshot below
            // is the authoritative geometry check.
            let point_x = rect.left + 50;
            let point_y = rect.top + 50;
            unsafe {
                SetWindowPos(
                    HWND(probe_window as usize as *mut _),
                    Some(HWND_TOPMOST),
                    rect.left,
                    rect.top,
                    width,
                    height,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                )
                .map_err(|error| error.to_string())?;
                // Put the launcher at the head of the same topmost band, with
                // the probe immediately below it. This keeps unrelated
                // user/runner windows from slipping between the frozen target
                // and the launcher when the click scenario hides it.
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
                let _ = SetForegroundWindow(hwnd);
                SetCursorPos(rect.left + 300, rect.top + 300).map_err(|error| error.to_string())?;
                SetCursorPos(point_x, point_y).map_err(|error| error.to_string())?;
                let hit = WindowFromPoint(POINT {
                    x: point_x,
                    y: point_y,
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
            let ex_style_before = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };

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
                    assert_probe_destination(
                        &down_event,
                        probe_pid,
                        probe_child,
                        probe_window,
                        point_x,
                        point_y,
                        case_name,
                    )?;
                    assert_probe_destination(
                        &up_event,
                        probe_pid,
                        probe_child,
                        probe_window,
                        point_x,
                        point_y,
                        case_name,
                    )?;
                    let context_menu = if case_name == "right_click" {
                        let context_menu = wait_for(&probe_rx, Duration::from_secs(5), |record| {
                            matches!(
                                record,
                                ProbeRecord::Input {
                                    event: ProbeEvent::ContextMenu,
                                    ..
                                }
                            )
                        })?;
                        assert_probe_destination(
                            &context_menu,
                            probe_pid,
                            probe_child,
                            probe_window,
                            point_x,
                            point_y,
                            case_name,
                        )?;
                        if !matches!(
                            context_menu,
                            ProbeRecord::Input {
                                pid: target_pid,
                                ..
                            } if target_pid != pid
                        ) {
                            return Err(
                                "right_click: native context menu reached wrong PID".to_owned()
                            );
                        }
                        Some(context_menu)
                    } else {
                        None
                    };
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
                    if !matches!(
                        context_menu.as_ref(),
                        Some(&ProbeRecord::Input {
                            serial: context_serial,
                            target: context_target,
                            ..
                        }) if up_serial < context_serial && context_target == up_target
                    ) && context_menu.is_some()
                    {
                        return Err(
                            "right_click: context menu was misordered or reached another target"
                                .to_owned(),
                        );
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
                            ) || (button == ProbeButton::Right
                                && matches!(event, ProbeEvent::ContextMenu))
                        },
                        case_name,
                    )?;
                }
                "left_drag" => {
                    send(&[mouse_input(MOUSEEVENTF_LEFTDOWN.0, 0)])?;
                    unsafe {
                        SetCursorPos(point_x + 30, point_y).map_err(|error| error.to_string())?;
                    }
                    // SetCursorPos updates the pointer on hosted runners but
                    // can occasionally fail to enqueue the corresponding
                    // WM_MOUSEMOVE while the launchpad is busy rendering. A
                    // one-pixel SendInput nudge forces the native input path
                    // to observe the already-positioned cursor.
                    send(&[relative_mouse_move(1, 0)])?;
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
                        SetCursorPos(point_x + 30, point_y).map_err(|error| error.to_string())?;
                    }
                    send(&[relative_mouse_move(1, 0)])?;
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
                "vertical_wheel" | "vertical_wheel_receiver_activation" => {
                    if unsafe { GetForegroundWindow() } != hwnd {
                        return Err(format!(
                            "{case_name}: launcher was not foreground before OS wheel generation"
                        ));
                    }
                    // Use a partial high-resolution delta so the product path
                    // proves that it preserves the original signed wParam
                    // rather than rounding to a canonical wheel detent.
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
                    // Wheel delivery uses the receiver's stable top-level
                    // framework sink. Clicks still use the spatial child so
                    // normal hit-testing and context-menu semantics remain
                    // unchanged.
                    assert_probe_destination(
                        &wheel,
                        probe_pid,
                        probe_window,
                        probe_window,
                        point_x,
                        point_y,
                        case_name,
                    )?;
                    if !matches!(
                        &wheel,
                        ProbeRecord::Input {
                            event: ProbeEvent::VerticalWheel { key_state: 0, .. },
                            ..
                        }
                    ) {
                        return Err(format!("{case_name}: modifier/key state changed"));
                    }
                    if probe_pid == pid {
                        return Err("vertical_wheel: wrong target PID".to_owned());
                    }
                    assert_no_probe_input(
                        &probe_rx,
                        |event| matches!(event, ProbeEvent::VerticalWheel { .. }),
                        case_name,
                    )?;
                    if receiver_activation {
                        wait_until(
                            Duration::from_secs(2),
                            || unsafe { GetForegroundWindow() }
                                == HWND(probe_window as usize as *mut _),
                            "wheel receiver activation",
                        )?;
                        let deadline = Instant::now() + Duration::from_millis(750);
                        while Instant::now() < deadline {
                            if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
                                return Err("vertical_wheel_receiver_activation: launcher closed"
                                    .to_owned());
                            }
                            // The receiver deliberately calls
                            // SetForegroundWindow and therefore legitimately
                            // moves itself ahead of the launcher. Verify that
                            // the launcher remains in its original topmost
                            // band; the ordinary wheel case above retains the
                            // exact absolute Z-order assertion.
                            if unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } != ex_style_before
                                || ex_style_before & WS_EX_TOPMOST.0 as isize == 0
                            {
                                return Err(
                                    "vertical_wheel_receiver_activation: launcher window band changed"
                                        .to_owned()
                                );
                            }
                            std::thread::yield_now();
                        }
                    } else {
                        // Observe beyond the bounded receiver-activation
                        // interval so a delayed focus-loss hide cannot pass.
                        assert_window_state_stable(
                            hwnd,
                            foreground_before,
                            z_before,
                            Duration::from_millis(750),
                            case_name,
                        )?;
                    }
                }
                "hover" => {
                    unsafe {
                        SetCursorPos(point_x + 5, point_y + 5)
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

    pub fn run_browser_compatibility() -> Result<(), String> {
        let server = ScrollServer::start()?;
        let profile = std::env::temp_dir().join(format!(
            "launchpad-input-routing-edge-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_millis()
        ));
        std::fs::create_dir_all(&profile)
            .map_err(|error| format!("create isolated Edge profile: {error}"))?;
        let mut edge = Command::new(edge_executable()?)
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("--guest")
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-extensions")
            .arg("--disable-gpu")
            .arg("--disable-backgrounding-occluded-windows")
            .arg("--disable-renderer-backgrounding")
            .arg("--disable-background-timer-throttling")
            .arg("--window-position=100,100")
            .arg("--window-size=1000,700")
            .arg(server.url())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("start isolated Edge: {error}"))?;
        let mut launcher_slot: Option<Child> = None;
        let result = (|| {
            wait_until(
                Duration::from_secs(30),
                || find_window_by_title(EDGE_PAGE_TITLE).is_some(),
                "Edge compatibility window",
            )?;
            wait_until(
                Duration::from_secs(10),
                || server.scroll_y() == 0,
                "Edge page initial scroll position",
            )?;
            let edge_hwnd = find_window_by_title(EDGE_PAGE_TITLE)
                .ok_or("Edge compatibility window disappeared")?;
            let mut edge_pid = 0;
            unsafe { GetWindowThreadProcessId(edge_hwnd, Some(&mut edge_pid)) };
            if edge_pid == 0 {
                return Err("Edge compatibility window had no PID".to_owned());
            }

            unsafe {
                // Deterministically keep the compatibility receiver directly
                // below the launcher. This setup-only ordering does not
                // participate in product delivery.
                SetWindowPos(
                    edge_hwnd,
                    Some(HWND_TOPMOST),
                    100,
                    100,
                    1000,
                    700,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                )
                .map_err(|error| format!("position Edge compatibility window: {error}"))?;
            }

            let (launcher, launcher_rx) = start_launcher(false)?;
            launcher_slot = Some(launcher);
            let initial = wait_launcher_snapshot(&launcher_rx, |record| {
                matches!(
                    record,
                    ProbeRecord::LauncherSnapshot {
                        window,
                        visible: true,
                        focused: true,
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
            let point_x = 150;
            // Stay below Edge's browser chrome while remaining horizontally
            // outside the centered Launchpad page frame.
            let point_y = 400;
            unsafe {
                SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    100,
                    100,
                    1000,
                    700,
                    SWP_SHOWWINDOW,
                )
                .map_err(|error| format!("position launcher for Edge compatibility: {error}"))?;
                let _ = SetForegroundWindow(hwnd);
                SetCursorPos(point_x + 200, point_y).map_err(|error| error.to_string())?;
                SetCursorPos(point_x, point_y).map_err(|error| error.to_string())?;
                let hit = WindowFromPoint(POINT {
                    x: point_x,
                    y: point_y,
                });
                if hit != hwnd {
                    return Err(format!(
                        "browser-compat: launcher did not own outside point (expected {window:#x}, got {:#x})",
                        hit.0 as usize
                    ));
                }
            }
            wait_launcher_snapshot(&launcher_rx, |record| {
                matches!(
                    record,
                    ProbeRecord::LauncherSnapshot {
                        generation: next_generation,
                        region,
                        visible: true,
                        focused: true,
                        ..
                    } if *next_generation > generation && region == "OutsideTransparent"
                )
            })?;

            if unsafe { GetForegroundWindow() } != hwnd {
                return Err("browser-compat: launcher was not foreground before wheel".to_owned());
            }
            let foreground_before = unsafe { GetForegroundWindow() };
            let z_before = z_order_index(hwnd);
            let ex_style_before = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
            let region_before = window_region_type(hwnd)?;
            if ex_style_before & WS_EX_TOPMOST.0 as isize == 0 {
                return Err("browser-compat: launcher was not in the topmost band".to_owned());
            }

            // Preserve the machine's real wheel-routing setting. Unlike the
            // native-probe scenarios, this compatibility path intentionally
            // does not construct a MouseWheelRoutingGuard. A burst exercises
            // repeated Chromium hit-hole creation/restoration, which is the
            // path that previously made the whole launcher visibly flash.
            let wheel_burst = (0..8)
                .map(|_| mouse_input(MOUSEEVENTF_WHEEL.0, (-120_i32) as u32))
                .collect::<Vec<_>>();
            send(&wheel_burst)?;
            wait_until(
                Duration::from_secs(10),
                || server.scroll_y() > 0,
                "Edge page scrollY to increase",
            )?;
            assert_window_state_stable(
                hwnd,
                foreground_before,
                z_before,
                Duration::from_millis(750),
                "browser-compat",
            )?;
            assert_same_window(hwnd, pid)?;
            if unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } != ex_style_before {
                return Err("browser-compat: launcher window style changed".to_owned());
            }
            if window_region_type(hwnd)? != region_before {
                return Err("browser-compat: launcher window region changed".to_owned());
            }
            if !unsafe { IsWindow(Some(edge_hwnd)) }.as_bool() {
                return Err("browser-compat: Edge receiver window was destroyed".to_owned());
            }
            let mut current_edge_pid = 0;
            unsafe { GetWindowThreadProcessId(edge_hwnd, Some(&mut current_edge_pid)) };
            if current_edge_pid != edge_pid {
                return Err(format!(
                    "browser-compat: Edge window PID changed: {edge_pid} -> {current_edge_pid}"
                ));
            }
            println!(
                "input-routing-e2e: browser-compat passed (scrollY={})",
                server.scroll_y()
            );
            Ok(())
        })();
        if let Some(mut launcher) = launcher_slot {
            let _ = launcher.kill();
            let _ = launcher.wait();
        }
        let _ = edge.kill();
        let _ = edge.wait();
        let _ = std::fs::remove_dir_all(&profile);
        result
    }

    pub fn run_probe_self_test() -> Result<(), String> {
        let (mut probe, rx) = start_probe(false)?;
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
        // Hosted Windows desktops can use hover-based wheel routing, whose
        // target selection races a newly positioned transparent test window.
        // Focus routing makes SendInput deterministic while product delivery
        // remains the independent targeted PostMessageW path. The prior user
        // setting is restored even when a scenario fails.
        let _wheel_routing = MouseWheelRoutingGuard::focus()?;
        for case_name in [
            "left_click",
            "left_drag",
            "right_click",
            "right_drag_cancel",
            "vertical_wheel",
            "vertical_wheel_receiver_activation",
            "hover",
        ] {
            run_product_case(case_name)?;
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod macos_runner {
    use std::ffi::c_void;
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
    use objc2_foundation::{NSPoint, NSRect};

    fn window_z_order(window_number: u32) -> Option<usize> {
        unsafe {
            let descriptions = CGWindowListCopyWindowInfo(1, 0);
            if descriptions.is_null() {
                return None;
            }
            let count = CFArrayGetCount(descriptions);
            let mut result = None;
            for index in 0..count {
                let dictionary = CFArrayGetValueAtIndex(descriptions, index);
                let number = CFDictionaryGetValue(dictionary, kCGWindowNumber);
                let mut observed = 0i32;
                if !number.is_null()
                    && CFNumberGetValue(number, 3, (&mut observed as *mut i32).cast())
                    && observed as u32 == window_number
                {
                    result = Some(index as usize);
                    break;
                }
            }
            CFRelease(descriptions);
            result
        }
    }

    fn window_bounds(window_number: u32) -> Option<NSRect> {
        unsafe {
            let descriptions = CGWindowListCopyWindowInfo(1, 0);
            if descriptions.is_null() {
                return None;
            }
            let count = CFArrayGetCount(descriptions);
            let mut result = None;
            for index in 0..count {
                let dictionary = CFArrayGetValueAtIndex(descriptions, index);
                let number = CFDictionaryGetValue(dictionary, kCGWindowNumber);
                let mut observed = 0i32;
                if !number.is_null()
                    && CFNumberGetValue(number, 3, (&mut observed as *mut i32).cast())
                    && observed as u32 == window_number
                {
                    let bounds = CFDictionaryGetValue(dictionary, kCGWindowBounds);
                    let mut rect = NSRect::ZERO;
                    if !bounds.is_null()
                        && CGRectMakeWithDictionaryRepresentation(bounds, &mut rect)
                    {
                        result = Some(rect);
                    }
                    break;
                }
            }
            CFRelease(descriptions);
            result
        }
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        static kCGWindowBounds: *const c_void;
        static kCGWindowNumber: *const c_void;
        fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> *const c_void;
        fn CGRectMakeWithDictionaryRepresentation(
            dictionary: *const c_void,
            rect: *mut NSRect,
        ) -> bool;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFArrayGetCount(array: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
        fn CFDictionaryGetValue(dictionary: *const c_void, key: *const c_void) -> *const c_void;
        fn CFNumberGetValue(number: *const c_void, number_type: isize, value: *mut c_void) -> bool;
        fn CFRelease(value: *const c_void);
    }

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
        passive_probe: bool,
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
        if passive_probe {
            command.env(
                launchpad_windows::input_probe_protocol::QA_PASSIVE_MACOS_PROBE_ENV,
                "1",
            );
        }
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| error.to_string())?;
        let stdout = child.stdout.take().ok_or("child stdout unavailable")?;
        let (tx, rx) = mpsc::channel();
        let trace_name = name.to_owned();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(record) = ProbeRecord::from_json_line(&line) {
                    if std::env::var_os("LAUNCHPAD_INPUT_ROUTING_TRACE").is_some() {
                        eprintln!("{trace_name}-record: {record:?}");
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
                .map_err(|error| format!("JSONL timeout: {error}"))?;
            if predicate(&record) {
                return Ok(record);
            }
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
        if objc2_core_graphics::CGPreflightPostEventAccess() {
            Ok(())
        } else {
            Err(
                "macOS Accessibility/post-event permission is unavailable; use an explicitly approved real-machine runner"
                    .to_owned(),
            )
        }
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

    fn assert_no_receiver_effect(
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
                Ok(ProbeRecord::UiState { .. }) => {
                    return Err(format!("{case_name}: receiver UI state changed"));
                }
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => return Ok(()),
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(())
    }

    fn assert_probe_destination(
        record: &ProbeRecord,
        expected_pid: u32,
        expected_window: u64,
        expected_local_x: i32,
        expected_local_y: i32,
        case_name: &str,
    ) -> Result<(), String> {
        match record {
            ProbeRecord::Input {
                target,
                root,
                pid,
                local,
                ..
            } if *pid == expected_pid
                && *target == expected_window
                && *root == expected_window
                && local.x == expected_local_x
                && local.y == expected_local_y =>
            {
                Ok(())
            }
            other => Err(format!(
                "{case_name}: wrong PID/window/coordinate destination: {other:?}"
            )),
        }
    }

    fn wait_for_wheel_step(
        rx: &mpsc::Receiver<ProbeRecord>,
        expected_delta_y: f64,
        expected_phase: NativePhase,
        expected_momentum: NativePhase,
        semantic_effect: &mut Option<ProbeRecord>,
    ) -> Result<ProbeRecord, String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let record = rx
                .recv_timeout(remaining)
                .map_err(|error| format!("wheel JSONL timeout: {error}"))?;
            if matches!(
                &record,
                ProbeRecord::UiState {
                    scroll_offset_y,
                    ..
                } if scroll_offset_y.abs() > f64::EPSILON
            ) {
                *semantic_effect = Some(record.clone());
            }
            if matches!(
                &record,
                ProbeRecord::Input {
                    event: ProbeEvent::VerticalWheel {
                        delta_y,
                        precise: true,
                        key_state: 0,
                        phase,
                        momentum_phase,
                        ..
                    },
                    ..
                } if (*delta_y - expected_delta_y).abs() < 0.01
                    && *phase == expected_phase
                    && *momentum_phase == expected_momentum
            ) {
                return Ok(record);
            }
        }
    }

    fn wait_for_button_up(
        rx: &mpsc::Receiver<ProbeRecord>,
        button: ProbeButton,
        semantic_effect: &mut Option<ProbeRecord>,
    ) -> Result<ProbeRecord, String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let record = rx
                .recv_timeout(remaining)
                .map_err(|error| format!("button-up JSONL timeout: {error}"))?;
            if matches!(record, ProbeRecord::UiState { .. }) {
                *semantic_effect = Some(record.clone());
            }
            if matches!(
                &record,
                ProbeRecord::Input {
                    event: ProbeEvent::ButtonUp { button: observed },
                    ..
                } if *observed == button
            ) {
                return Ok(record);
            }
        }
    }

    fn assert_launcher_stable(
        rx: &mpsc::Receiver<ProbeRecord>,
        pid: u32,
        window: u64,
        z_order: usize,
        duration: Duration,
        case_name: &str,
    ) -> Result<(), String> {
        let deadline = Instant::now() + duration;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(remaining) {
                Ok(ProbeRecord::LauncherSnapshot {
                    pid: observed_pid,
                    window: observed_window,
                    visible,
                    focused,
                    ..
                }) if observed_pid != pid || observed_window != window || !visible || !focused => {
                    return Err(format!(
                        "{case_name}: launcher identity/visibility/focus changed"
                    ));
                }
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(error) => return Err(error.to_string()),
            }
        }
        if window_z_order(window as u32) != Some(z_order) {
            return Err(format!("{case_name}: launcher Z-order changed"));
        }
        Ok(())
    }

    fn run_product_case(case_name: &str) -> Result<(), String> {
        let (mut probe, probe_rx) = start_process("native_input_probe", false, true)?;
        let mut launcher_slot: Option<Child> = None;
        let result = (|| {
            let ready = wait_for(&probe_rx, Duration::from_secs(10), |record| {
                matches!(record, ProbeRecord::Ready { .. })
            })?;
            let ProbeRecord::Ready {
                pid: probe_pid,
                top_level: probe_window,
                ..
            } = ready
            else {
                unreachable!()
            };
            let (launcher, launcher_rx) = start_process("launchpad-windows", true, false)?;
            launcher_slot = Some(launcher);
            let initial = wait_for(&launcher_rx, Duration::from_secs(45), |record| {
                matches!(
                    record,
                    ProbeRecord::LauncherSnapshot {
                        window,
                        visible: true,
                        focused: true,
                        ..
                    } if *window != 0
                )
            })?;
            let ProbeRecord::LauncherSnapshot {
                pid,
                window,
                input_listen_permission,
                input_post_permission,
                ..
            } = initial
            else {
                unreachable!()
            };
            if input_listen_permission != Some(true) || input_post_permission != Some(true) {
                return Err(format!(
                    "Launchpad process {pid} lacks required macOS permissions \
                     (listen={input_listen_permission:?}, post={input_post_permission:?})"
                ));
            }
            // The focused notification can precede Core Graphics publishing
            // the matching global window-order snapshot. Wait for that
            // read-only view to converge. Focus itself is asserted from the
            // launcher's native key-window notification in the JSONL record.
            wait_until(
                Duration::from_secs(2),
                || window_z_order(window as u32).is_some(),
                "launcher macOS Z-order publication",
            )?;
            let stable_z_order = window_z_order(window as u32)
                .ok_or("launcher missing from macOS on-screen Z-order")?;
            let probe_z_order = window_z_order(probe_window as u32)
                .ok_or("passive probe missing from macOS on-screen Z-order")?;
            if stable_z_order >= probe_z_order {
                return Err(format!(
                    "launcher was not above passive probe ({stable_z_order} >= {probe_z_order})"
                ));
            }
            let launcher_bounds =
                window_bounds(window as u32).ok_or("launcher bounds unavailable")?;
            let probe_bounds =
                window_bounds(probe_window as u32).ok_or("probe bounds unavailable")?;
            let left = launcher_bounds.origin.x.max(probe_bounds.origin.x);
            let top = launcher_bounds.origin.y.max(probe_bounds.origin.y);
            let right = (launcher_bounds.origin.x + launcher_bounds.size.width)
                .min(probe_bounds.origin.x + probe_bounds.size.width);
            let bottom = (launcher_bounds.origin.y + launcher_bounds.size.height)
                .min(probe_bounds.origin.y + probe_bounds.size.height);
            if right - left < 40.0 || bottom - top < 40.0 {
                return Err(format!(
                    "launcher/probe bounds do not overlap: launcher={launcher_bounds:?} probe={probe_bounds:?}"
                ));
            }
            let input_x = left + 12.0;
            let input_y = top + 12.0;
            move_pointer(input_x, input_y)?;
            let outside = wait_for(&launcher_rx, Duration::from_secs(5), |record| {
                matches!(
                    record,
                    ProbeRecord::LauncherSnapshot {
                        region,
                        visible: true,
                        focused: true,
                        ..
                    } if region == "OutsideTransparent"
                )
            })?;
            let ProbeRecord::LauncherSnapshot {
                generation: outside_generation,
                ..
            } = outside
            else {
                unreachable!()
            };
            let expected_local_x = (input_x - probe_bounds.origin.x).round() as i32;
            let expected_local_y =
                (probe_bounds.size.height - (input_y - probe_bounds.origin.y)).round() as i32;
            drain(&probe_rx);

            match case_name {
                "left_click" | "right_click" => {
                    let button = if case_name == "left_click" {
                        ProbeButton::Left
                    } else {
                        ProbeButton::Right
                    };
                    click_event(button, true, input_x, input_y)?;
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
                    click_event(button, false, input_x, input_y)?;
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
                    let mut semantic_effect = None;
                    let up = wait_for_button_up(&probe_rx, button, &mut semantic_effect)?;
                    assert_probe_destination(
                        &down,
                        probe_pid,
                        probe_window,
                        expected_local_x,
                        expected_local_y,
                        case_name,
                    )?;
                    assert_probe_destination(
                        &up,
                        probe_pid,
                        probe_window,
                        expected_local_x,
                        expected_local_y,
                        case_name,
                    )?;
                    if !matches!(
                        (&down, &up),
                        (
                            ProbeRecord::Input { serial: a, .. },
                            ProbeRecord::Input { serial: b, .. }
                        ) if a < b
                    ) {
                        return Err(format!("{case_name}: click ordering failed"));
                    }
                    let semantic_matches = |record: &ProbeRecord| {
                        matches!(
                            record,
                            ProbeRecord::UiState {
                                left_actions: 1,
                                right_mouse_downs: 0,
                                ..
                            } if button == ProbeButton::Left
                        ) || matches!(
                            record,
                            ProbeRecord::UiState {
                                left_actions: 0,
                                right_mouse_downs: 1,
                                ..
                            } if button == ProbeButton::Right
                        )
                    };
                    let semantic = match semantic_effect.filter(semantic_matches) {
                        Some(record) => record,
                        None => wait_for(&probe_rx, Duration::from_secs(5), semantic_matches)?,
                    };
                    if !matches!(
                        semantic,
                        ProbeRecord::UiState {
                            pid: observed_pid,
                            window: observed_window,
                            ..
                        } if observed_pid == probe_pid && observed_window == probe_window
                    ) {
                        return Err(format!(
                            "{case_name}: semantic receiver identity changed: {semantic:?}"
                        ));
                    }
                    assert_no_receiver_effect(
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
                "left_drag" | "right_drag_cancel" => {
                    let button = if case_name == "left_drag" {
                        ProbeButton::Left
                    } else {
                        ProbeButton::Right
                    };
                    click_event(button, true, input_x, input_y)?;
                    drag_event(button, input_x + 30.0, input_y)?;
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
                    click_event(button, false, input_x + 30.0, input_y)?;
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
                    let steps = [
                        (-7, 1, 0, NativePhase::Began, NativePhase::Unavailable),
                        (-6, 2, 0, NativePhase::Changed, NativePhase::Unavailable),
                        (0, 8, 0, NativePhase::Ended, NativePhase::Unavailable),
                        (
                            -5,
                            0,
                            1,
                            NativePhase::Unavailable,
                            NativePhase::MomentumBegan,
                        ),
                        (
                            -4,
                            0,
                            2,
                            NativePhase::Unavailable,
                            NativePhase::MomentumChanged,
                        ),
                        (
                            0,
                            0,
                            8,
                            NativePhase::Unavailable,
                            NativePhase::MomentumEnded,
                        ),
                    ];
                    let mut semantic_effect = None;
                    let mut previous_serial = None;
                    for (delta, phase, momentum, expected_phase, expected_momentum) in steps {
                        post_precise_scroll(delta, phase, momentum)?;
                        let wheel = wait_for_wheel_step(
                            &probe_rx,
                            delta as f64,
                            expected_phase,
                            expected_momentum,
                            &mut semantic_effect,
                        )?;
                        assert_probe_destination(
                            &wheel,
                            probe_pid,
                            probe_window,
                            expected_local_x,
                            expected_local_y,
                            case_name,
                        )?;
                        let ProbeRecord::Input { serial, .. } = wheel else {
                            unreachable!()
                        };
                        if previous_serial.is_some_and(|previous| serial <= previous) {
                            return Err(
                                "vertical_wheel: receiver event order was not monotonic".to_owned()
                            );
                        }
                        previous_serial = Some(serial);
                    }
                    if probe_pid == pid {
                        return Err("vertical_wheel: self-delivery detected".to_owned());
                    }
                    let semantic = semantic_effect.ok_or_else(|| {
                        "vertical_wheel: NSScrollView content offset did not change".to_owned()
                    })?;
                    if !matches!(
                        semantic,
                        ProbeRecord::UiState {
                            pid: observed_pid,
                            window: observed_window,
                            ..
                        } if observed_pid == probe_pid && observed_window == probe_window
                    ) {
                        return Err(format!(
                            "vertical_wheel: semantic receiver identity changed: {semantic:?}"
                        ));
                    }
                    assert_no_input(
                        &probe_rx,
                        |event| matches!(event, ProbeEvent::VerticalWheel { .. }),
                        case_name,
                    )?;
                    assert_launcher_stable(
                        &launcher_rx,
                        pid,
                        window,
                        stable_z_order,
                        Duration::from_millis(750),
                        case_name,
                    )?;
                }
                "hover" => {
                    move_pointer(input_x + 5.0, input_y + 5.0)?;
                    assert_no_receiver_effect(
                        &probe_rx,
                        |event| matches!(event, ProbeEvent::MouseMove),
                        case_name,
                    )?;
                    assert_launcher_stable(
                        &launcher_rx,
                        pid,
                        window,
                        stable_z_order,
                        Duration::from_millis(350),
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
        let (mut probe, rx) = start_process("native_input_probe", false, false)?;
        let result = (|| {
            let ready = wait_for(&rx, Duration::from_secs(10), |record| {
                matches!(record, ProbeRecord::Ready { .. })
            })?;
            let ProbeRecord::Ready {
                pid: probe_pid,
                top_level: probe_window,
                ..
            } = ready
            else {
                unreachable!()
            };
            move_pointer(150.0, 150.0)?;
            click_event(ProbeButton::Left, true, 150.0, 150.0)?;
            click_event(ProbeButton::Left, false, 150.0, 150.0)?;
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
            if !matches!(
                (&down, &up),
                (
                    ProbeRecord::Input {
                        serial: down_serial,
                        pid: down_pid,
                        target: down_window,
                        ..
                    },
                    ProbeRecord::Input {
                        serial: up_serial,
                        pid: up_pid,
                        target: up_window,
                        ..
                    }
                ) if down_serial < up_serial
                    && *down_pid == probe_pid
                    && *up_pid == probe_pid
                    && *down_window == probe_window
                    && *up_window == probe_window
            ) {
                return Err(format!(
                    "probe self-test click did not reach the real receiver in order: down={down:?} up={up:?}"
                ));
            }
            wait_for(&rx, Duration::from_secs(5), |record| {
                matches!(
                    record,
                    ProbeRecord::UiState {
                        pid,
                        window,
                        left_actions: 1,
                        right_mouse_downs: 0,
                        ..
                    } if *pid == probe_pid && *window == probe_window
                )
            })?;
            post_precise_scroll(-7, 1, 0)?;
            let wheel = wait_for(&rx, Duration::from_secs(5), |record| {
                matches!(
                    record,
                    ProbeRecord::Input {
                        event: ProbeEvent::VerticalWheel {
                            delta_y,
                            precise: true,
                            phase: NativePhase::Began,
                            momentum_phase: NativePhase::Unavailable,
                            ..
                        },
                        ..
                    } if (*delta_y + 7.0).abs() < 0.01
                )
            })?;
            if !matches!(
                wheel,
                ProbeRecord::Input {
                    pid,
                    target,
                    root,
                    local,
                    ..
                } if pid == probe_pid
                    && target == probe_window
                    && root == probe_window
                    && local.x >= 0
                    && local.y >= 0
            ) {
                return Err(format!(
                    "probe self-test wheel receiver identity/coordinates were invalid: {wheel:?}"
                ));
            }
            wait_for(&rx, Duration::from_secs(5), |record| {
                matches!(
                    record,
                    ProbeRecord::UiState {
                        pid,
                        window,
                        scroll_offset_y,
                        ..
                    } if *pid == probe_pid
                        && *window == probe_window
                        && scroll_offset_y.abs() > f64::EPSILON
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

    pub fn permission_status() -> Result<(), String> {
        assert_post_permission()?;
        assert_wheel_delivery()?;
        println!("input-routing-e2e: macOS generator post-event permission available");
        Ok(())
    }

    /// Verifies that synthetic wheel events are reliably delivered on this
    /// runner. Hosted macOS runners (e.g. `macos-14`) report post-event
    /// permission as granted but drop scroll-wheel events probabilistically,
    /// so a single success is not enough to gate the full E2E. We spin up a
    /// probe and require several consecutive precise-scroll events to arrive
    /// before treating the runner as capable of running the wheel scenarios.
    fn assert_wheel_delivery() -> Result<(), String> {
        const REQUIRED_CONSECUTIVE: usize = 3;
        let (mut probe, rx) = start_process("native_input_probe", false, false)?;
        let result = (|| {
            let ready = wait_for(&rx, Duration::from_secs(10), |record| {
                matches!(record, ProbeRecord::Ready { .. })
            })?;
            let ProbeRecord::Ready {
                pid: probe_pid,
                top_level: probe_window,
                ..
            } = ready
            else {
                unreachable!()
            };
            move_pointer(150.0, 150.0)?;
            // Require several back-to-back deliveries. Any miss resets the
            // streak; if the streak never reaches the threshold before the
            // overall deadline, the runner is considered unreliable for
            // wheel-based scenarios.
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut streak = 0usize;
            let mut attempts = 0usize;
            while streak < REQUIRED_CONSECUTIVE {
                if Instant::now() >= deadline {
                    return Err(format!(
                        "only {streak}/{REQUIRED_CONSECUTIVE} consecutive wheel events arrived \
                         in {attempts} attempts"
                    ));
                }
                attempts += 1;
                post_precise_scroll(-7, 1, 0)?;
                let received = wait_for(&rx, Duration::from_secs(2), |record| {
                    matches!(
                        record,
                        ProbeRecord::Input {
                            event: ProbeEvent::VerticalWheel {
                                delta_y,
                                precise: true,
                                ..
                            },
                            pid,
                            target,
                            root,
                            ..
                        } if (*delta_y + 7.0).abs() < 0.01
                            && *pid == probe_pid
                            && *target == probe_window
                            && *root == probe_window
                    )
                });
                match received {
                    Ok(_) => streak += 1,
                    Err(_) => streak = 0,
                }
            }
            Ok(())
        })();
        let _ = probe.kill();
        let _ = probe.wait();
        result.map_err(|error| {
            format!(
                "wheel event delivery unreliable on this runner ({error}); \
                 hosted runners drop scroll events even with post-event permission"
            )
        })
    }

    pub fn request_generator_permission() -> Result<(), String> {
        let granted = objc2_core_graphics::CGRequestPostEventAccess();
        println!(
            "input-routing-e2e: requested Accessibility/post-event approval for the scenario generator (granted={granted})"
        );
        if granted {
            Ok(())
        } else {
            Err(
                "enable input_routing_scenarios in System Settings > Privacy & Security > Accessibility, then rerun --permission-status"
                    .to_owned(),
            )
        }
    }

    pub fn permission_denied_contract() -> Result<(), String> {
        // Reached when the permission gate reported the runner as not capable
        // of full E2E. Two reasons are valid here:
        //   1. The generator lacks post-event permission entirely (strict
        //      hosted runners).
        //   2. The generator has post-event permission but cannot deliver
        //      scroll-wheel events reliably (macos-14 hosted runners report
        //      the preflight as granted yet drop wheel events).
        // In both cases the full semantic product E2E was intentionally not
        // run, which is the contract this step asserts.
        let post = objc2_core_graphics::CGPreflightPostEventAccess();
        println!(
            "input-routing-e2e: runner not approved for full native E2E \
             (post-event preflight={post}, wheel delivery unreliable or unavailable); \
             semantic product E2E was not reported as passed"
        );
        Ok(())
    }
}

fn main() {
    #[cfg(windows)]
    let result = if std::env::args().any(|arg| arg == "--browser-compat") {
        windows_runner::run_browser_compatibility()
    } else if std::env::args().any(|arg| arg == "--product") {
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
    let result = if std::env::args().any(|arg| arg == "--permission-status") {
        macos_runner::permission_status()
    } else if std::env::args().any(|arg| arg == "--request-permissions") {
        macos_runner::request_generator_permission()
    } else if std::env::args().any(|arg| arg == "--permission-denied-contract") {
        macos_runner::permission_denied_contract()
    } else if std::env::args().any(|arg| arg == "--product") {
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

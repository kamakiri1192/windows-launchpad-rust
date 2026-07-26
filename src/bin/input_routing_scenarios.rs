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
        SendInput, INPUT, INPUT_TYPE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_WHEEL,
        MOUSEINPUT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{SetCursorPos, SetForegroundWindow};

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

    pub fn run() -> Result<(), String> {
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
}

fn main() {
    #[cfg(windows)]
    if let Err(error) = windows_runner::run() {
        eprintln!("input routing scenarios: {error}");
        std::process::exit(1);
    }

    #[cfg(target_os = "macos")]
    {
        eprintln!("macOS scenario runner is built by the macOS adapter commit");
        std::process::exit(2);
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        eprintln!("input routing scenarios are supported on Windows and macOS");
        std::process::exit(2);
    }
}

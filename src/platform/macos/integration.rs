//! macOS resident-process integration: menu-bar icon, global shortcut, and
//! per-user single-instance handoff.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io;
use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use winit::event_loop::EventLoopProxy;

use crate::{app_icon, UserEvent};

const MENU_SHOW: &str = "launchpad.show";
const MENU_SETTINGS: &str = "launchpad.settings";
const MENU_QUIT: &str = "launchpad.quit";
const SUMMON_MESSAGE: &[u8] = b"show";

/// Make the accessory application active before asking its window to become
/// key. `Window::focus_window` alone does not reliably activate an unbundled
/// accessory process launched from a terminal or profiling harness.
pub fn activate_application() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    let Some(main_thread) = MainThreadMarker::new() else {
        eprintln!("macos-integration: activation requested off the main thread");
        return;
    };
    NSApplication::sharedApplication(main_thread).activate();
}

/// Best-effort vertical wheel passthrough for transparent launcher areas.
///
/// Mirrors the Windows path (`SendInput` of `MOUSEEVENTF_WHEEL`): synthesize a
/// one-axis scroll event via Core Graphics and post it to the system event
/// stream at the current cursor location. The caller has already decided the
/// pointer is over the transparent area. The launcher stays visible — unlike
/// the click passthrough, the window does not need to get out of the way
/// because CGEvent posting targets the live cursor position.
///
/// `delta_y_lines` is in "lines" (winit `MouseScrollDelta::LineDelta` units,
/// where `+1.0` is one line up). We pass it straight to Core Graphics with
/// `kCGScrollEventUnitLine`. Only a single vertical axis is created, so
/// horizontal scroll is structurally never forwarded.
pub fn replay_vertical_wheel_at_cursor(delta_y_lines: f32) -> bool {
    use objc2_core_graphics::{
        CGEvent, CGEventSource, CGEventSourceStateID, CGEventTapLocation, CGScrollEventUnit,
    };

    if delta_y_lines.abs() < f32::EPSILON {
        return true; // nothing to do, treat as success
    }
    // The objc2-core-graphics method-form wrappers (CGEventSource::new,
    // CGEvent::new_scroll_wheel_event2, CGEvent::post) encapsulate the
    // underlying unsafe C calls, so no unsafe block is needed here. The source
    // state HIDSystemState (1) is the standard "as if from the hardware"
    // source used for synthesized input; it is the same state
    // SendInput-equivalent tools use on macOS.
    let source = match CGEventSource::new(CGEventSourceStateID::HIDSystemState) {
        Some(s) => s,
        None => {
            eprintln!("macos-wheel: CGEventSource::new failed");
            return false;
        }
    };
    // One axis only (vertical). delta is in line units; macOS expects an
    // i32 line count so we round and require at least one tick of intent.
    let line_count = delta_y_lines.round() as i32;
    if line_count == 0 {
        return true;
    }
    // CGEvent::new_scroll_wheel_event2 mirrors the C ABI:
    //   (source, unit, wheelCount, w1, w2, w3).
    // wheel_count=1 → only the vertical axis (wheel1) is read; wheel2/wheel3
    // must still be passed (0) but are ignored. Horizontal scroll is
    // therefore structurally impossible here.
    //
    // `source` is a CFRetained<CGEventSource>; Deref it to &CGEventSource
    // for the Option<&CGEventSource> the ABI expects (Option won't
    // auto-coerce the inner reference).
    let event = CGEvent::new_scroll_wheel_event2(
        Some(&*source),
        CGScrollEventUnit::Line,
        /* wheel_count */ 1,
        /* wheel1 (vertical) */ line_count,
        /* wheel2 (horizontal) */ 0,
        /* wheel3 */ 0,
    );
    let Some(event) = event else {
        eprintln!("macos-wheel: CGEvent::new_scroll_wheel_event2 failed");
        return false;
    };
    // kCGHIDEventTap posts at the hardware tap so the event reaches the app
    // under the cursor just like a real trackpad/wheel scroll.
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&*event));
    true
}

/// Best-effort left-click passthrough for transparent launcher areas.
///
/// Mirrors the Windows path (`SendInput` of `MOUSEEVENTF_LEFTDOWN` then
/// `LEFTUP`). The UI thread hides the launcher first, then calls this while
/// the cursor is still at the user's release point. We synthesize a left
/// mouse down + up at the current cursor location and post them at the HID
/// event tap so the window now underneath receives the click the launcher
/// itself consumed.
///
/// The cursor location is read from a scratch event (`CGEventCreate` +
/// `CGEventGetLocation`), the standard idiom for "where is the mouse right
/// now" on macOS when you don't have a real event in hand.
pub fn replay_left_click_at_cursor() -> bool {
    use objc2_core_graphics::{
        CGEvent, CGEventSource, CGEventSourceStateID, CGEventTapLocation, CGEventType,
        CGMouseButton,
    };

    let source = match CGEventSource::new(CGEventSourceStateID::HIDSystemState) {
        Some(s) => s,
        None => {
            eprintln!("macos-click: CGEventSource::new failed");
            return false;
        }
    };
    // CGEventCreate with a source yields a "null" event whose location is the
    // current mouse position — the idiomatic way to read the cursor without a
    // real event in hand.
    let scratch = match CGEvent::new(Some(&*source)) {
        Some(e) => e,
        None => {
            eprintln!("macos-click: CGEvent::new (scratch) failed");
            return false;
        }
    };
    let cursor = CGEvent::location(Some(&*scratch));
    let down = CGEvent::new_mouse_event(
        Some(&*source),
        CGEventType::LeftMouseDown,
        cursor,
        CGMouseButton::Left,
    );
    let up = CGEvent::new_mouse_event(
        Some(&*source),
        CGEventType::LeftMouseUp,
        cursor,
        CGMouseButton::Left,
    );
    let (Some(down), Some(up)) = (down, up) else {
        eprintln!("macos-click: new_mouse_event failed");
        return false;
    };
    // Post at the HID tap so the events are delivered as if from real hardware
    // to whatever window is under the cursor now (the launcher is hidden).
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&*down));
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&*up));
    true
}

/// Owns the menu-bar item and registered global shortcut for the process.
pub struct MacOsIntegration {
    hotkey_manager: Option<GlobalHotKeyManager>,
    hotkey: Option<HotKey>,
    _tray: Option<TrayIcon>,
}

impl MacOsIntegration {
    /// Install the integration on the main thread after winit has created its
    /// event loop. Failure of either optional facility is logged but does not
    /// prevent the launcher window from running.
    pub fn install(proxy: EventLoopProxy<UserEvent>) -> Self {
        let (hotkey_manager, hotkey) = install_hotkey(proxy.clone());
        let tray = install_menu_bar(proxy);
        Self {
            hotkey_manager,
            hotkey,
            _tray: tray,
        }
    }
}

impl Drop for MacOsIntegration {
    fn drop(&mut self) {
        if let (Some(manager), Some(hotkey)) = (&self.hotkey_manager, self.hotkey) {
            let _ = manager.unregister(hotkey);
        }
    }
}

fn install_hotkey(
    proxy: EventLoopProxy<UserEvent>,
) -> (Option<GlobalHotKeyManager>, Option<HotKey>) {
    let hotkey = std::env::var("LAUNCHPAD_HOTKEY")
        .ok()
        .and_then(|value| match value.parse::<HotKey>() {
            Ok(hotkey) => Some(hotkey),
            Err(error) => {
                eprintln!("macos-integration: invalid LAUNCHPAD_HOTKEY: {error}");
                None
            }
        })
        .unwrap_or_else(|| HotKey::new(Some(Modifiers::ALT), Code::Space));

    let manager = match GlobalHotKeyManager::new() {
        Ok(manager) => manager,
        Err(error) => {
            eprintln!("macos-integration: global hotkey manager failed: {error}");
            return (None, None);
        }
    };
    if let Err(error) = manager.register(hotkey) {
        eprintln!("macos-integration: failed to register {hotkey}: {error}");
        return (Some(manager), None);
    }

    let hotkey_id = hotkey.id();
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.id == hotkey_id && event.state == HotKeyState::Pressed {
            let _ = proxy.send_event(UserEvent::Summon);
        }
    }));
    (Some(manager), Some(hotkey))
}

fn install_menu_bar(proxy: EventLoopProxy<UserEvent>) -> Option<TrayIcon> {
    let menu = Menu::new();
    let show = MenuItem::with_id(MENU_SHOW, "Show Launchpad", true, None);
    let settings = MenuItem::with_id(MENU_SETTINGS, "Settings…", true, None);
    let separator = PredefinedMenuItem::separator();
    let quit = MenuItem::with_id(MENU_QUIT, "Quit Launchpad", true, None);
    if let Err(error) = menu.append_items(&[&show, &settings, &separator, &quit]) {
        eprintln!("macos-integration: menu creation failed: {error}");
        return None;
    }

    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let user_event = match event.id.as_ref() {
            MENU_SHOW => Some(UserEvent::Summon),
            MENU_SETTINGS => Some(UserEvent::ToggleSettings),
            MENU_QUIT => Some(UserEvent::QuitRequested),
            _ => None,
        };
        if let Some(user_event) = user_event {
            let _ = proxy.send_event(user_event);
        }
    }));

    let icon = app_icon::load_rgba(Some(32)).and_then(|image| {
        Icon::from_rgba(image.rgba, image.width, image.height)
            .map_err(|error| eprintln!("macos-integration: menu-bar icon failed: {error}"))
            .ok()
    });
    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Launchpad");
    if let Some(icon) = icon {
        builder = builder.with_icon(icon);
    } else {
        builder = builder.with_title("Launchpad");
    }
    match builder.build() {
        Ok(tray) => Some(tray),
        Err(error) => {
            eprintln!("macos-integration: menu-bar item failed: {error}");
            None
        }
    }
}

/// Bound Unix datagram socket proving this is the user's resident instance.
pub struct SingleInstanceGuard {
    socket_path: PathBuf,
    socket: UnixDatagram,
    quit_tx: Option<mpsc::Sender<()>>,
    listener: Option<thread::JoinHandle<()>>,
}

#[derive(Debug)]
pub enum SingleInstanceError {
    AlreadyRunning,
    Io(io::Error),
}

impl SingleInstanceError {
    pub fn is_already_running(&self) -> bool {
        matches!(self, Self::AlreadyRunning)
    }
}

impl std::fmt::Display for SingleInstanceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyRunning => {
                formatter.write_str("another Launchpad instance is already running")
            }
            Self::Io(error) => write!(formatter, "single-instance socket failed: {error}"),
        }
    }
}

impl std::error::Error for SingleInstanceError {}

impl SingleInstanceGuard {
    pub fn acquire() -> Result<Self, SingleInstanceError> {
        let socket_path = single_instance_path();
        let socket = match UnixDatagram::bind(&socket_path) {
            Ok(socket) => socket,
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                let client = UnixDatagram::unbound().map_err(SingleInstanceError::Io)?;
                if client.send_to(SUMMON_MESSAGE, &socket_path).is_ok() {
                    return Err(SingleInstanceError::AlreadyRunning);
                }
                // A crashed process can leave the filesystem entry behind.
                std::fs::remove_file(&socket_path).map_err(SingleInstanceError::Io)?;
                UnixDatagram::bind(&socket_path).map_err(SingleInstanceError::Io)?
            }
            Err(error) => return Err(SingleInstanceError::Io(error)),
        };
        socket
            .set_read_timeout(Some(Duration::from_millis(200)))
            .map_err(SingleInstanceError::Io)?;
        Ok(Self {
            socket_path,
            socket,
            quit_tx: None,
            listener: None,
        })
    }

    pub fn start_listener(&mut self, proxy: EventLoopProxy<UserEvent>) -> io::Result<()> {
        let socket = self.socket.try_clone()?;
        let (quit_tx, quit_rx) = mpsc::channel();
        let listener = thread::Builder::new()
            .name("macos-single-instance".to_owned())
            .spawn(move || {
                let mut buffer = [0u8; 16];
                while quit_rx.try_recv().is_err() {
                    match socket.recv(&mut buffer) {
                        Ok(length) if &buffer[..length] == SUMMON_MESSAGE => {
                            let _ = proxy.send_event(UserEvent::Summon);
                        }
                        Ok(_) => {}
                        Err(error)
                            if matches!(
                                error.kind(),
                                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                            ) => {}
                        Err(_) => break,
                    }
                }
            })?;
        self.quit_tx = Some(quit_tx);
        self.listener = Some(listener);
        Ok(())
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        if let Some(quit_tx) = self.quit_tx.take() {
            let _ = quit_tx.send(());
        }
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

fn single_instance_path() -> PathBuf {
    let mut hasher = DefaultHasher::new();
    std::env::var_os("HOME").hash(&mut hasher);
    std::env::temp_dir().join(format!("launchpad-{:016x}.sock", hasher.finish()))
}

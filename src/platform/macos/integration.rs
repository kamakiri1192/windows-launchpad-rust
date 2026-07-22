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

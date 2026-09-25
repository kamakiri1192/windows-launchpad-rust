//! Launch Start Menu shortcuts and protocol URLs selected from the grid.

use std::path::Path;

/// Open `url` in the user's default browser. macOS uses `/usr/bin/open`,
/// Windows shells to the default http handler via `ShellExecuteW("open", ...)`.
/// Both return as soon as the browser process spawns, so the result only
/// reflects whether the browser was *started*, not whether the page loaded.
#[cfg(target_os = "macos")]
pub fn open_url(url: &str) -> Result<(), String> {
    std::process::Command::new("/usr/bin/open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("open url failed: {error}"))
}

#[cfg(windows)]
pub fn open_url(url: &str) -> Result<(), String> {
    // ShellExecuteW("open", <url>) routes to the registered http(s) handler,
    // which is the user's default browser. `shell_execute` takes a path-like
    // string; URLs work identically.
    let wide = wide_null(url);
    let operation = wide_null("open");
    // SAFETY: all PCWSTR arguments point to NUL-terminated UTF-16 buffers that
    // live for the duration of the call. ShellExecuteW returns synchronously.
    let result = unsafe {
        use windows::core::PCWSTR;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        ShellExecuteW(
            None,
            PCWSTR(operation.as_ptr()),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    let code = result.0 as usize;
    if code > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecuteW failed with code {code}"))
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn open_url(_url: &str) -> Result<(), String> {
    Err("opening a URL is unsupported on this platform".to_string())
}

/// Open the Steam library page for a numeric app id (`appid`) via the
/// `steam://` protocol handler. The URL is `steam://nav/games/details/<appid>`,
/// which Steam's registered protocol handler turns into the app's library
/// details page.
///
/// Windows uses `ShellExecuteW("open", url)` (same path as `open_url`, since
/// `steam://` flows to its registered handler the same way `http://` does);
/// macOS uses `/usr/bin/open`. Both return as soon as the handler process
/// spawns, so the result only reflects whether Steam was *started*, not
/// whether the page appeared (or whether Steam is even installed).
#[cfg(target_os = "macos")]
pub fn open_steam_page(appid: &str) -> Result<(), String> {
    let url = steam_url(appid);
    std::process::Command::new("/usr/bin/open")
        .arg(&url)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("open steam url failed: {error}"))
}

#[cfg(windows)]
pub fn open_steam_page(appid: &str) -> Result<(), String> {
    // ShellExecuteW("open", <url>) routes to the registered steam:// handler,
    // identical in mechanism to how open_url reaches the http(s) handler.
    let url = steam_url(appid);
    let wide = wide_null(&url);
    let operation = wide_null("open");
    // SAFETY: all PCWSTR arguments point to NUL-terminated UTF-16 buffers that
    // live for the duration of the call. ShellExecuteW returns synchronously.
    let result = unsafe {
        use windows::core::PCWSTR;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        ShellExecuteW(
            None,
            PCWSTR(operation.as_ptr()),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    let code = result.0 as usize;
    if code > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecuteW failed with code {code}"))
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn open_steam_page(_appid: &str) -> Result<(), String> {
    Err("opening a Steam URL is unsupported on this platform".to_string())
}

/// Build the `steam://nav/games/details/<appid>` library-details URL from a
/// numeric Steam app id. Kept as a free, pure function so the app-command
/// layer can unit-test the URL shape without spawning a handler.
fn steam_url(appid: &str) -> String {
    format!("steam://nav/games/details/{appid}")
}

#[cfg(windows)]
pub fn open_shortcut(path: &Path) -> Result<(), String> {
    shell_execute(path).map_err(|error| error.message)
}

#[cfg(windows)]
struct LaunchError {
    unresolved: bool,
    message: String,
}

/// Refresh only when parsing failed before invocation. Shell cancellation and
/// execution errors must never trigger another route or a second launch.
#[cfg(windows)]
pub fn open_shortcut_with_refresh(
    path: &Path,
    refresh: impl FnOnce() -> Option<std::path::PathBuf>,
) -> Result<(), String> {
    launch_with_refresh(path, shell_execute, refresh)
}

#[cfg(windows)]
fn launch_with_refresh(
    path: &Path,
    mut launch: impl FnMut(&Path) -> Result<(), LaunchError>,
    refresh: impl FnOnce() -> Option<std::path::PathBuf>,
) -> Result<(), String> {
    match launch(path) {
        Err(error) if error.unresolved => {
            if let Some(next) = refresh().filter(|next| next != path) {
                launch(&next).map_err(|error| error.message)
            } else {
                Err(error.message)
            }
        }
        result => result.map_err(|error| error.message),
    }
}

/// Open the OS file manager (Explorer / Finder) with `path` selected.
///
/// Windows reveals a file by launching `explorer.exe /select,<path>`; macOS
/// uses `open -R`. Both return as soon as the child process spawns, so the
/// result only reflects whether the manager was *started*, not whether the
/// selection actually appeared.
#[cfg(windows)]
pub fn reveal(path: &Path) -> Result<(), String> {
    // `/select,<path>` must arrive as a single command-line argument (the
    // comma is part of the switch, no space after it). Building the string
    // here keeps `Command` from splitting on the comma.
    let arg = format!("/select,{}", path.to_string_lossy());
    std::process::Command::new("explorer.exe")
        .arg(arg)
        .spawn()
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// Open the native properties/info view for an application path.
///
/// Windows uses the Shell `properties` verb, which opens the normal shortcut
/// or executable properties dialog. macOS asks Finder to open the item's Get
/// Info window. Keeping this at the platform boundary lets the context-menu
/// action offer the same affordance on both desktop platforms without making
/// the app shell know about either OS's UI automation details.
#[cfg(windows)]
pub fn show_properties(path: &Path) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let operation = wide_null("properties");
    let path_text = path.to_string_lossy();
    let file = wide_null(path_text.as_ref());

    // SAFETY: all PCWSTR arguments point to NUL-terminated UTF-16 buffers
    // that live for the duration of the synchronous ShellExecuteW call.
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(operation.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    let code = result.0 as usize;
    if code > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecuteW properties failed with code {code}"))
    }
}

#[cfg(target_os = "macos")]
pub fn show_properties(path: &Path) -> Result<(), String> {
    let path = escape_applescript_string(&path.to_string_lossy());
    let script = format!(
        "tell application \"Finder\"\n    activate\n    open information window of (POSIX file \"{path}\" as alias)\nend tell"
    );
    std::process::Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("open Finder info failed: {error}"))
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn show_properties(_path: &Path) -> Result<(), String> {
    Err("showing application properties is unsupported on this platform".to_string())
}

#[cfg(target_os = "macos")]
pub fn reveal(path: &Path) -> Result<(), String> {
    std::process::Command::new("/usr/bin/open")
        .arg("-R")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("open -R failed: {error}"))
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn reveal(_path: &Path) -> Result<(), String> {
    Err("revealing in a file manager is unsupported on this platform".to_string())
}

#[cfg(windows)]
fn shell_execute(path: &Path) -> Result<(), LaunchError> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Com::{CoTaskMemFree, IBindCtx};
    use windows::Win32::UI::Shell::{
        SHParseDisplayName, ShellExecuteExW, SEE_MASK_INVOKEIDLIST, SEE_MASK_NOASYNC,
        SHELLEXECUTEINFOW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let _com = crate::icons::extract::ComScope::new();
    let name = wide_null(&path.to_string_lossy());
    // Provider protocol URLs are lpFile targets, not filesystem/Shell PIDLs.
    if path.to_string_lossy().starts_with("steam://") {
        let mut execute = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOASYNC,
            lpFile: PCWSTR(name.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        return unsafe { ShellExecuteExW(&mut execute) }.map_err(|error| LaunchError {
            unresolved: false,
            message: error.to_string(),
        });
    }
    let mut pidl = std::ptr::null_mut();
    unsafe { SHParseDisplayName(PCWSTR(name.as_ptr()), None::<&IBindCtx>, &mut pidl, 0, None) }
        .map_err(|error| LaunchError {
            unresolved: true,
            message: format!("Shell could not resolve {}: {error}", path.display()),
        })?;
    let mut execute = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_INVOKEIDLIST | SEE_MASK_NOASYNC,
        lpIDList: pidl.cast(),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // Invoke the registered default verb exactly once. No explorer.exe fallback:
    // starting Explorer does not establish that an application launch succeeded.
    let result = unsafe { ShellExecuteExW(&mut execute) };
    unsafe {
        CoTaskMemFree(Some(pidl.cast()));
    }
    result.map_err(|error| LaunchError {
        unresolved: false,
        message: format!("Shell rejected launch of {}: {error}", path.display()),
    })
}

#[cfg(target_os = "macos")]
pub fn open_shortcut(path: &Path) -> Result<(), String> {
    std::process::Command::new("/usr/bin/open")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Launch Services open failed: {error}"))
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn open_shortcut(_path: &Path) -> Result<(), String> {
    Err("launching applications is unsupported on this platform".to_string())
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(target_os = "macos")]
fn escape_applescript_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn only_unresolved_locators_refresh_and_retry_once() {
        use std::cell::Cell;
        let old = Path::new("old.lnk");
        for rejected in [false, true] {
            let calls = Cell::new(0);
            let result = launch_with_refresh(
                old,
                |_| {
                    calls.set(calls.get() + 1);
                    if rejected {
                        Err(LaunchError {
                            unresolved: false,
                            message: "cancelled".into(),
                        })
                    } else {
                        Ok(())
                    }
                },
                || panic!("accepted or rejected invocation must not refresh"),
            );
            assert_eq!(result.is_err(), rejected);
            assert_eq!(calls.get(), 1);
        }
        let mut attempted = Vec::new();
        let result = launch_with_refresh(
            old,
            |path| {
                attempted.push(path.to_path_buf());
                if path == old {
                    Err(LaunchError {
                        unresolved: true,
                        message: "missing".into(),
                    })
                } else {
                    Ok(())
                }
            },
            || Some("new.lnk".into()),
        );
        assert!(result.is_ok());
        assert_eq!(attempted, [old.to_path_buf(), "new.lnk".into()]);
        for replacement in [None, Some(old.to_path_buf())] {
            let calls = Cell::new(0);
            assert!(launch_with_refresh(
                old,
                |_| {
                    calls.set(calls.get() + 1);
                    Err(LaunchError {
                        unresolved: true,
                        message: "missing".into(),
                    })
                },
                || replacement
            )
            .is_err());
            assert_eq!(calls.get(), 1);
        }
    }

    #[test]
    fn steam_url_targets_library_details() {
        assert_eq!(steam_url("620"), "steam://nav/games/details/620");
        assert_eq!(steam_url("0"), "steam://nav/games/details/0");
    }
}

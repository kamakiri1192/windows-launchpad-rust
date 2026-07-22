//! Launch Start Menu shortcuts and protocol URLs selected from the grid.

use std::path::Path;

#[cfg(windows)]
pub fn open_shortcut(path: &Path) -> Result<(), String> {
    match shell_execute(path) {
        Ok(()) => Ok(()),
        Err(shell_err) => open_via_explorer(path).map_err(|explorer_err| {
            format!("{shell_err}; explorer.exe fallback failed: {explorer_err}")
        }),
    }
}

#[cfg(windows)]
fn shell_execute(path: &Path) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let operation = wide_null("open");
    let path_text = path.to_string_lossy();
    let file = wide_null(path_text.as_ref());

    // SAFETY: all PCWSTR arguments point to NUL-terminated UTF-16 buffers that
    // live for the duration of the call. ShellExecuteW returns synchronously.
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

    // ShellExecuteW reports success with any value greater than 32. Values at
    // or below 32 are historical SE_ERR_* / Win32 error codes.
    let code = result.0 as usize;
    if code > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecuteW failed with code {code}"))
    }
}

#[cfg(windows)]
fn open_via_explorer(path: &Path) -> Result<(), String> {
    std::process::Command::new("explorer.exe")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|err| err.to_string())
}

#[cfg(not(windows))]
pub fn open_shortcut(_path: &Path) -> Result<(), String> {
    Err("launching shortcuts is only supported on Windows".to_string())
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

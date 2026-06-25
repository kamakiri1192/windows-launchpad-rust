//! Launch Start Menu shortcuts selected from the grid.

use std::path::Path;

#[cfg(windows)]
pub fn open_shortcut(path: &Path) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let operation = wide_null("open");
    let file = wide_null(&path.to_string_lossy());

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
    let code = result.0 as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecuteW failed with code {code}"))
    }
}

#[cfg(not(windows))]
pub fn open_shortcut(_path: &Path) -> Result<(), String> {
    Err("launching shortcuts is only supported on Windows".to_string())
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

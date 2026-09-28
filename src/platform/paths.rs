//! Platform-specific persistent data and diagnostic-log directories.

use std::path::PathBuf;

const APP_DIR: &str = "Launchpad";

/// Directory containing the SQLite database and other durable launcher state.
pub fn app_data_dir() -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::Win32::UI::Shell::{
            FOLDERID_LocalAppData, SHGetKnownFolderPath, KF_FLAG_DEFAULT,
        };
        // The returned allocation belongs to the caller, including for a
        // redirected Known Folder. Preserve UTF-16 paths without lossy conversion.
        unsafe {
            let raw = SHGetKnownFolderPath(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT, None)
                .map_err(|e| e.to_string())?;
            let path = PathBuf::from(std::ffi::OsString::from_wide(raw.as_wide()));
            CoTaskMemFree(Some(raw.0.cast()));
            if path.as_os_str().is_empty() {
                return Err("LocalAppData is unavailable".into());
            }
            Ok(path.join(APP_DIR))
        }
    }

    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::{NSFileManager, NSSearchPathDirectory, NSSearchPathDomainMask};
        let urls = NSFileManager::defaultManager().URLsForDirectory_inDomains(
            NSSearchPathDirectory::ApplicationSupportDirectory,
            NSSearchPathDomainMask::UserDomainMask,
        );
        let path = urls
            .firstObject()
            .and_then(|url| url.path())
            .ok_or_else(|| "Application Support is unavailable".to_string())?;
        let path = path.to_string();
        if path.is_empty() {
            return Err("Application Support is unavailable".into());
        }
        Ok(PathBuf::from(path).join(APP_DIR))
    }
}

/// Directory containing opt-in diagnostic logs.
pub fn log_dir() -> PathBuf {
    #[cfg(windows)]
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local).join(APP_DIR);
    }

    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join("Library")
            .join("Logs")
            .join(APP_DIR);
    }

    executable_directory()
}

/// Full path to the shared application DB. No alternate-directory fallback.
pub fn database_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("launchpad.sqlite3"))
}

/// Full path to the opt-in debug log, preserving the established Windows
/// executable-directory fallback name.
pub fn debug_log_path() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local).join(APP_DIR).join("debug.log");
        }
        executable_directory().join("launchpad-debug.log")
    }

    #[cfg(not(windows))]
    log_dir().join("debug.log")
}

fn executable_directory() -> PathBuf {
    let mut path = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    path.pop();
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_directory_has_platform_app_name() {
        let directory = app_data_dir().unwrap();
        assert!(directory.is_absolute());
        assert!(directory.ends_with(APP_DIR));
        if let Some(expected) = std::env::var_os("LAUNCHPAD_TEST_EXPECTED_DATA_DIR") {
            assert_eq!(directory, PathBuf::from(expected));
        }
    }

    #[cfg(windows)]
    #[test]
    fn data_directory_does_not_depend_on_localappdata_environment_variable() {
        // A child test isolates environment changes from parallel tests.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "platform::paths::tests::data_directory_has_platform_app_name",
            ])
            .env_remove("LOCALAPPDATA")
            .env("LAUNCHPAD_TEST_EXPECTED_DATA_DIR", app_data_dir().unwrap())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }

    #[test]
    fn log_directory_has_platform_app_name_or_executable_fallback() {
        let directory = log_dir();
        assert!(directory.ends_with(APP_DIR) || directory == executable_directory());
    }

    #[test]
    fn cache_and_log_paths_have_expected_filenames() {
        let cache_path = database_path().unwrap();
        let log_path = debug_log_path();
        let cache_name = cache_path.file_name().and_then(|name| name.to_str());
        let log_name = log_path.file_name().and_then(|name| name.to_str());
        assert!(matches!(cache_name, Some("launchpad.sqlite3")));
        assert!(matches!(
            log_name,
            Some("debug.log" | "launchpad-debug.log")
        ));
    }
}

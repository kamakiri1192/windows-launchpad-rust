//! Platform-specific persistent data and diagnostic-log directories.

use std::path::PathBuf;

const APP_DIR: &str = "Launchpad";

/// Directory containing the SQLite database and other durable launcher state.
pub fn app_data_dir() -> PathBuf {
    #[cfg(windows)]
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local).join(APP_DIR);
    }

    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join(APP_DIR);
    }

    executable_directory().join("launchpad-data")
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

/// Full path to the icon and layout cache database.
///
/// Keep the historical Windows executable-directory fallback filename so an
/// environment without `LOCALAPPDATA` does not silently start with a new
/// database after the platform-path refactor.
pub fn cache_db_path() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local).join(APP_DIR).join("cache.sqlite3");
        }
        executable_directory().join("launchpad-cache.sqlite3")
    }

    #[cfg(not(windows))]
    app_data_dir().join("cache.sqlite3")
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
        assert!(app_data_dir().ends_with(APP_DIR));
    }

    #[test]
    fn log_directory_has_platform_app_name_or_executable_fallback() {
        let directory = log_dir();
        assert!(directory.ends_with(APP_DIR) || directory == executable_directory());
    }

    #[test]
    fn cache_and_log_paths_have_expected_filenames() {
        let cache_path = cache_db_path();
        let log_path = debug_log_path();
        let cache_name = cache_path.file_name().and_then(|name| name.to_str());
        let log_name = log_path.file_name().and_then(|name| name.to_str());
        assert!(matches!(
            cache_name,
            Some("cache.sqlite3" | "launchpad-cache.sqlite3")
        ));
        assert!(matches!(
            log_name,
            Some("debug.log" | "launchpad-debug.log")
        ));
    }
}

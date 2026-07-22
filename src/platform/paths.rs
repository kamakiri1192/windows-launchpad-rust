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
}

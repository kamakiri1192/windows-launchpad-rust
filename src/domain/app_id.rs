//! Stable application identity derived from a shortcut's `.lnk` path.
//!
//! The launcher used to key everything off a positional `app_index`. That works
//! when the app list is immutable, but as soon as shortcuts are added or removed
//! while the launcher is open, a positional index silently "shifts" — clicking
//! the same physical tile can resolve to a different app. `AppId` fixes this:
//! it is derived from the *normalized* `.lnk` path, so the same shortcut always
//! maps to the same id regardless of where it sits in the display order.
//!
//! Normalization is intentionally simple and portable (no `std::fs::canonicalize`,
//! which would touch the disk and fail for missing files): lowercase, drive letter
//! preserved, backslashes turned into `/`. Good enough to dedupe the two Start
//! Menu roots and stay stable across rescans.

use std::fmt;
use std::path::Path;

/// Opaque, stable identifier for one Start Menu shortcut.
///
/// Wraps the normalized `.lnk` path. Cheap to clone (an `Arc<str>` would be
/// overkill for our sizes). Compared by the inner string, so it works as a
/// `HashMap`/`BTreeMap` key out of the box.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct AppId(String);

impl AppId {
    /// Build an `AppId` from a `.lnk` path. The path is normalized first so two
    /// equivalent spellings collapse to the same id. Accepts anything path-like
    /// (`&Path`, `&str`, `PathBuf`, …).
    pub fn from_link_path(path: impl AsRef<Path>) -> Self {
        Self(normalize_link_path(path.as_ref()))
    }

    /// Construct directly from an already-normalized string (e.g. when reading
    /// back from the cache, where the value was stored normalized).
    pub fn from_normalized(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// The normalized id string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AppId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for AppId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Normalize a filesystem path into a canonical id string.
///
/// Rules:
///   - lowercase (Windows paths are case-insensitive)
///   - replace `\` with `/`
///   - collapse runs of separators
///   - trim a trailing separator
///   - strip a `\\?\` extended-length prefix if present
///
/// We deliberately avoid `fs::canonicalize`: it would hit the disk, resolve
/// symlinks/junctions (Start Menu folders are often junctions), and fail for
/// shortcuts that disappear between scans. Lexical normalization is enough to
/// dedupe per-user vs all-users Start Menu duplicates.
pub fn normalize_link_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    let s = s.strip_prefix(r"\??\").unwrap_or(s);

    let mut out = String::with_capacity(s.len());
    let mut prev_sep = false;
    for ch in s.chars() {
        let lower = ch.to_ascii_lowercase();
        let is_sep = lower == '\\' || lower == '/';
        if is_sep {
            if !prev_sep {
                out.push('/');
            }
            prev_sep = true;
        } else {
            out.push(lower);
            prev_sep = false;
        }
    }
    while out.ends_with('/') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn same_path_different_case_and_seps_collapses() {
        let a = AppId::from_link_path(PathBuf::from(
            r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\App.lnk",
        ));
        let b = AppId::from_link_path(PathBuf::from(
            r"c:/programdata/microsoft/windows/start menu/programs/app.lnk",
        ));
        assert_eq!(a, b);
        assert_eq!(
            a.as_ref(),
            "c:/programdata/microsoft/windows/start menu/programs/app.lnk"
        );
    }

    #[test]
    fn extended_prefix_is_stripped() {
        let a = AppId::from_link_path(PathBuf::from(r"\\?\C:\Users\Me\App.lnk"));
        let b = AppId::from_link_path(PathBuf::from(r"C:\Users\Me\App.lnk"));
        assert_eq!(a, b);
    }

    #[test]
    fn duplicate_separators_collapse() {
        let a = AppId::from_link_path(PathBuf::from(r"C:\\Users\\Me//App.lnk"));
        assert_eq!(a.as_ref(), "c:/users/me/app.lnk");
    }

    #[test]
    fn trailing_separator_trimmed() {
        let a = AppId::from_link_path(PathBuf::from(r"C:\Users\Me\"));
        assert_eq!(a.as_ref(), "c:/users/me");
    }

    #[test]
    fn distinct_paths_get_distinct_ids() {
        let a = AppId::from_link_path(PathBuf::from(r"C:\A.lnk"));
        let b = AppId::from_link_path(PathBuf::from(r"C:\B.lnk"));
        assert_ne!(a, b);
    }

    #[test]
    fn round_trips_through_from_normalized() {
        let a = AppId::from_link_path(PathBuf::from(r"C:\App.lnk"));
        let b = AppId::from_normalized(a.as_ref().to_string());
        assert_eq!(a, b);
    }

    #[test]
    fn orders_lexicographically() {
        let a = AppId::from_link_path(PathBuf::from(r"C:\Aaa.lnk"));
        let b = AppId::from_link_path(PathBuf::from(r"C:\Bbb.lnk"));
        let mut v = vec![b.clone(), a.clone()];
        v.sort();
        assert_eq!(v, vec![a, b]);
    }
}

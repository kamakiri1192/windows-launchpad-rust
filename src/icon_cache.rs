//! Shared SQLite application store: rebuildable icons and user-owned data.
//!
//! Schema changes run through rusqlite_migration before the store is exposed.
//! Opening failures leave the database intact and stop normal startup.
//! Icon resets only clear the icons table, never settings, layout, or aliases.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use crate::domain::app_diff::IconSource;
use crate::domain::app_id::AppId;
use crate::domain::initial_layout::{
    self, catalog::Platform, InitialLayoutRecord, InitializationStatus,
};
use crate::domain::launcher_state::LauncherState;
use crate::domain::settings::Settings;
use crate::icons::normalize::DecodedIcon;
use crate::icons::sizing::IconCategory;
use crate::startup_timer::{self, prefix};

#[cfg(test)]
#[path = "initial_layout_cache_tests.rs"]
mod initial_layout_tests;

/// Bumped when the *extraction* itself changes (new normalization target size,
/// different alpha handling, a different extraction strategy, etc.) so
/// previously-extracted pixels are discarded even if the `.lnk` is
/// byte-identical.
///
/// v2: invalidates icons cached during the IShellItemImageFactory-primary
/// experiment (which produced blank icons for Blender/Discord). Bumping forces
/// a full re-extraction with the restored main image-list strategy.
/// v3: replaces Steam's 32px client icons with high-resolution local sources.
/// v4: removes wide Steam library logos and extracts square executable icons
/// at 256px before normalizing them into the launcher atlas.
/// v5: resolves macOS app icons through Launch Services so asset catalogs and
/// modern ICNS encodings render consistently with Finder.
/// v6/v5: Issue #48 アイコンサイズ自動正規化（solid_fill 分類 + カテゴリ別 scale）
/// v7: macOS FullBleedアイコンの影を本体のサイズ・中心計算から除外。
/// v7: prefers high-resolution icons from installed macOS Steam app bundles
/// instead of Steam's small library-cache artwork.
/// v8: crops macOS FullBleed icons to the hard-alpha body envelope so cached
/// soft-fringe pixels from the previous normalizer are regenerated.
/// v9: recognizes dense, symmetric translucent macOS tile bodies when the
/// opaque envelope contains only non-square foreground artwork.
/// Windows v6: resolve desktop AppsFolder icons through the Shell image list;
/// invalidate thumbnail-frame artwork without touching saved app placement.
#[cfg(target_os = "macos")]
pub const EXTRACTION_VERSION: u32 = 9;
#[cfg(not(target_os = "macos"))]
pub const EXTRACTION_VERSION: u32 = 6;

/// Expected edge length of a cached icon's RGBA square. A mismatch invalidates
/// the entry (matches the `normalized icon size changed` invalidation rule).
pub const CACHED_ICON_SIZE: u32 = crate::icons::normalize::TARGET;

/// One row of the cache, as needed for validity checks + reload. The image
/// bytes are kept separately in [`CachedIcon::image`] for the common "is this
/// valid?" fast path that doesn't need the pixels.
#[derive(Debug, Clone)]
pub struct CachedIcon {
    pub app_id: AppId,
    pub link_path: String,
    pub display_name: String,
    pub source_modified_at: u64,
    pub target_path: String,
    pub target_modified_at: u64,
    pub icon_location: String,
    pub icon_index: i32,
    pub provider_icon: Option<IconSource>,
    pub image: DecodedIcon,
    pub extracted_at_version: u32,
    pub category: IconCategory,
    pub scale: f32,
}

/// The fields the caller already knows about a shortcut (from the latest scan),
/// used to decide whether a cache entry is still fresh.
#[derive(Debug, Clone)]
pub struct CacheProbe<'a> {
    pub app_id: &'a AppId,
    pub source_modified_at: u64,
    pub target_path: &'a str,
    pub target_modified_at: u64,
    pub icon_location: &'a str,
    pub icon_index: i32,
    pub provider_icon: Option<&'a IconSource>,
}

/// A opened cache DB. Cheap to share behind a `Mutex` since SQLite serializes
/// its own writes.
pub struct IconCache {
    conn: Mutex<Connection>,
    new_data_area: bool,
    /// Path to the DB file for diagnostics. Icon resets preserve the kv table.
    path: PathBuf,
}

impl std::fmt::Debug for IconCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IconCache")
            .field("path", &self.path)
            .finish()
    }
}

impl IconCache {
    /// Open the shared database at the OS-provided application data path.
    pub fn open() -> Result<Self, String> {
        let path = crate::platform::paths::database_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let t = startup_timer::get();
        t.mark(prefix::ICON_CACHE, "cache open");
        let cache = open_at(&path)?;
        t.mark(prefix::ICON_CACHE, "cache load");
        Ok(cache)
    }

    /// Load the cached entry for `probe`, *if* it is still valid per the
    /// invalidation rules. Returns `Ok(None)` when there is no entry or it is
    /// stale (caller should re-extract).
    pub fn get_if_valid(&self, probe: &CacheProbe<'_>) -> rusqlite::Result<Option<CachedIcon>> {
        let conn = self.conn.lock().expect("cache mutex poisoned");
        let row = conn
            .query_row(
                "SELECT link_path, display_name, source_modified_at, target_path, target_modified_at,
                        icon_location, icon_index, image_w, image_h, image_rgba,
                        extraction_version, category, scale, provider_icon_path, provider_icon_modified_at
                 FROM icons WHERE app_id = ?1",
                params![probe.app_id.as_ref()],
                |r| {
                    let image_w: u32 = r.get::<_, i64>("image_w")? as u32;
                    let image_h: u32 = r.get::<_, i64>("image_h")? as u32;
                    let rgba: Vec<u8> = r.get("image_rgba")?;
                    let cat_str: String = r.get("category")?;
                    let cat = IconCategory::from_str_lossy(&cat_str);
                    let provider_modified_at = r.get::<_, i64>("provider_icon_modified_at")? as u64;
                    Ok(CachedIcon {
                        app_id: probe.app_id.clone(),
                        link_path: r.get::<_, String>("link_path")?,
                        display_name: r.get::<_, String>("display_name")?,
                        source_modified_at: r.get::<_, i64>("source_modified_at")? as u64,
                        target_path: r.get::<_, String>("target_path")?,
                        target_modified_at: r.get::<_, i64>("target_modified_at")? as u64,
                        icon_location: r.get::<_, String>("icon_location")?,
                        icon_index: r.get::<_, i64>("icon_index")? as i32,
                        provider_icon: r.get::<_, Option<String>>("provider_icon_path")?.map(
                            |path| IconSource {
                                path,
                                modified_at: provider_modified_at,
                            },
                        ),
                        image: DecodedIcon {
                            rgba,
                            w: image_w,
                            h: image_h,
                        },
                        extracted_at_version: r.get::<_, i64>("extraction_version")? as u32,
                        category: cat,
                        scale: r.get::<_, f64>("scale")? as f32,
                    })
                },
            )
            .optional()?;

        let Some(row) = row else { return Ok(None) };

        if is_cache_valid(probe, &row) {
            Ok(Some(row))
        } else {
            Ok(None)
        }
    }

    /// Upsert one fully-extracted icon. Safe to call from the worker thread.
    pub fn put(&self, entry: &CachedIcon) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("cache mutex poisoned");
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO icons (app_id, link_path, display_name, source_modified_at, target_path,
                                target_modified_at, icon_location, icon_index, image_w, image_h,
                                image_rgba, extraction_version, category, scale,
                                last_seen_at, provider_icon_path, provider_icon_modified_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)
             ON CONFLICT(app_id) DO UPDATE SET
               link_path=excluded.link_path,
               display_name=excluded.display_name,
               source_modified_at=excluded.source_modified_at,
               target_path=excluded.target_path,
               target_modified_at=excluded.target_modified_at,
               icon_location=excluded.icon_location,
               icon_index=excluded.icon_index,
               image_w=excluded.image_w,
               image_h=excluded.image_h,
               image_rgba=excluded.image_rgba,
               extraction_version=excluded.extraction_version,
               category=excluded.category,
               scale=excluded.scale,
               last_seen_at=excluded.last_seen_at,
               provider_icon_path=excluded.provider_icon_path,
               provider_icon_modified_at=excluded.provider_icon_modified_at",
            params![
                entry.app_id.as_ref(),
                entry.link_path,
                entry.display_name,
                entry.source_modified_at as i64,
                entry.target_path,
                entry.target_modified_at as i64,
                entry.icon_location,
                entry.icon_index as i64,
                entry.image.w as i64,
                entry.image.h as i64,
                entry.image.rgba,
                entry.extracted_at_version as i64,
                entry.category.as_str(),
                entry.scale as f64,
                now_unix(),
                entry.provider_icon.as_ref().map(|icon| icon.path.as_str()),
                entry
                    .provider_icon
                    .as_ref()
                    .map_or(0, |icon| icon.modified_at) as i64,
            ],
        )?;
        tx.commit()
    }

    /// Update display metadata without re-encoding or replacing icon pixels.
    /// OS localization and bundle marketing names can change independently of
    /// an app binary's icon invalidation fields.
    pub fn update_display_name(
        &self,
        app_id: &AppId,
        display_name: &str,
    ) -> rusqlite::Result<bool> {
        let conn = self.conn.lock().expect("cache mutex poisoned");
        let changed = conn.execute(
            "UPDATE icons SET display_name = ?2 WHERE app_id = ?1 AND display_name <> ?2",
            params![app_id.as_ref(), display_name],
        )?;
        Ok(changed > 0)
    }

    /// Mark an id as gone (soft delete: keeps the row so a re-add reuses the
    /// id, but validity will fail until re-extraction succeeds).
    pub fn forget(&self, app_id: &AppId) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("cache mutex poisoned");
        conn.execute(
            "DELETE FROM icons WHERE app_id = ?1",
            params![app_id.as_ref()],
        )?;
        Ok(())
    }

    /// Delete every cached icon row. Used by the manual reset (R key / CLI
    /// `--reset-cache`) to force a full re-extraction. Returns the number of
    /// rows deleted.
    pub fn clear_all(&self) -> rusqlite::Result<usize> {
        let conn = self.conn.lock().expect("cache mutex poisoned");
        let n = conn.execute("DELETE FROM icons", [])?;
        Ok(n)
    }

    /// Bump `last_seen_at` for a set of ids that are still present, and delete
    /// any cached row whose id isn't in `present` (used after a rescan to GC
    /// stale rows). Runs in one transaction.
    ///
    /// `present` is expected to be small (hundreds of shortcuts); to stay under
    /// SQLite's host-parameter limit we delete in 500-id chunks, each chunk
    /// guarded by a `NOT IN` over a temp values-list built safely.
    pub fn retain_and_touch(&self, present: &[AppId]) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("cache mutex poisoned");
        let tx = conn.unchecked_transaction()?;
        let now = now_unix() as i64;
        for id in present {
            tx.execute(
                "UPDATE icons SET last_seen_at = ?1 WHERE app_id = ?2",
                params![now, id.as_ref()],
            )?;
        }

        // Build the full NOT-IN list once. SQLite's default limit is 999
        // variables; we chunk to 500 to be safe across builds.
        if present.is_empty() {
            tx.execute("DELETE FROM icons", [])?;
        } else {
            // Temp table approach: robust for any size.
            tx.execute(
                "CREATE TEMP TABLE IF NOT EXISTS retain(id TEXT PRIMARY KEY)",
                [],
            )?;
            tx.execute("DELETE FROM retain", [])?;
            for chunk in present.chunks(500) {
                // Each value tuple must be parenthesized: VALUES (?1),(?2),…
                let placeholders = (0..chunk.len())
                    .map(|i| format!("(?{})", i + 1))
                    .collect::<Vec<_>>()
                    .join(",");
                let params: Vec<&str> = chunk.iter().map(|id| id.as_ref()).collect();
                let sql = format!("INSERT OR IGNORE INTO retain(id) VALUES {placeholders}");
                let mut stmt = tx.prepare(&sql)?;
                stmt.execute(rusqlite::params_from_iter(params.iter()))?;
                drop(stmt);
            }
            tx.execute(
                "DELETE FROM icons WHERE app_id NOT IN (SELECT id FROM retain)",
                [],
            )?;
            tx.execute("DELETE FROM retain", [])?;
        }
        tx.commit()
    }

    /// Read persisted launcher settings. Missing or corrupt settings fall back
    /// to defaults so a bad preferences blob never blocks startup.
    pub fn get_settings(&self) -> Settings {
        self.kv_get(SETTINGS_KEY)
            .and_then(|b| serde_json::from_slice::<Settings>(&b).ok())
            .unwrap_or_default()
    }

    /// Persist launcher settings as a JSON blob in the generic kv table.
    pub fn put_settings(&self, settings: &Settings) -> rusqlite::Result<()> {
        let bytes = serde_json::to_vec(settings)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        self.kv_put(SETTINGS_KEY, &bytes)
    }

    /// Read the saved layout JSON. Strict validation is handled by load_layout.
    pub fn get_launcher_state(&self) -> Option<LauncherState> {
        self.kv_get(LAUNCHER_STATE_KEY)
            .and_then(|b| serde_json::from_slice::<LauncherState>(&b).ok())
    }

    /// Persist the current layout without touching settings or identity aliases.
    pub fn put_launcher_state(&self, state: &LauncherState) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().expect("cache mutex poisoned");
        let tx = conn.transaction()?;
        write_launcher_state(&tx, state)?;
        tx.commit()
    }

    #[cfg(windows)]
    pub fn load_identity_aliases(
        &self,
    ) -> rusqlite::Result<crate::domain::app_catalog::IdentityAliases> {
        let conn = self.conn.lock().expect("cache mutex poisoned");
        read_blob(&conn, IDENTITY_ALIASES_KEY)?
            .map(|bytes| serde_json::from_slice(&bytes).map_err(json_error))
            .transpose()
            .map(Option::unwrap_or_default)
    }

    /// Identity reconciliation and its corresponding placement commit together.
    #[cfg(windows)]
    pub fn put_layout_and_aliases(
        &self,
        state: &LauncherState,
        aliases: &crate::domain::app_catalog::IdentityAliases,
    ) -> rusqlite::Result<()> {
        let bytes = serde_json::to_vec(aliases).map_err(json_error)?;
        let mut conn = self.conn.lock().expect("cache mutex poisoned");
        let tx = conn.transaction()?;
        write_launcher_state(&tx, state)?;
        tx.execute(
            "INSERT INTO kv(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![IDENTITY_ALIASES_KEY, bytes],
        )?;
        tx.commit()
    }

    /// A settings reset commits the default preferences and regenerated layout together.
    pub fn put_settings_and_layout(
        &self,
        settings: &Settings,
        state: &LauncherState,
    ) -> rusqlite::Result<()> {
        let bytes = serde_json::to_vec(settings).map_err(json_error)?;
        let mut conn = self.conn.lock().expect("cache mutex poisoned");
        let tx = conn.transaction()?;
        tx.execute("INSERT INTO kv(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![SETTINGS_KEY, bytes])?;
        write_launcher_state(&tx, state)?;
        tx.commit()
    }

    /// Missing, corrupt and unreadable layouts are deliberately distinct.
    pub fn load_layout(&self) -> LayoutLoad {
        let conn = self.conn.lock().expect("cache mutex poisoned");
        load_layout(&conn, self.new_data_area).unwrap_or_else(|e| {
            eprintln!("layout: preserving unreadable data: {e}");
            LayoutLoad::unavailable()
        })
    }

    /// Runs on the discovery worker. Save before publishing a usable grid, in
    /// one transaction with the initialization/undo record inside launcher_state.
    pub fn prepare_initial_layout(
        &self,
        snapshot: &std::collections::BTreeMap<AppId, crate::domain::app_diff::SnapshotEntry>,
        _complete: bool,
        platform: Platform,
        japanese: bool,
    ) -> LayoutLoad {
        let mut conn = self.conn.lock().expect("cache mutex poisoned");
        let result = (|| -> rusqlite::Result<LayoutLoad> {
            let tx = conn.transaction()?;
            let mut loaded = load_layout(&tx, self.new_data_area)?;
            let settings = read_blob(&tx, SETTINGS_KEY)?
                .map(|b| serde_json::from_slice::<Settings>(&b))
                .transpose()
                .map_err(json_error)?
                .unwrap_or_default();
            let generation = format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            );
            let catalog = crate::catalog_sync::current(std::env::var("SystemRoot").ok());
            if loaded.fresh {
                loaded.state = initial_layout::generate(
                    snapshot, &settings, platform, japanese, generation, &catalog,
                )
                .state;
            } else {
                if loaded.state.initial_layout.is_none() {
                    let status = InitializationStatus::PreservedExisting;
                    loaded.state.initial_layout =
                        Some(InitialLayoutRecord::new(status, generation));
                }
                if !loaded.state.items.is_empty() || !loaded.state.folders.is_empty() {
                    loaded.state.customized = true;
                }
                let before = loaded.state.clone();
                let discovered = snapshot.keys().cloned().collect();
                loaded.state.integrate_discovered_apps(&discovered, |id| {
                    snapshot.get(id).map(|e| e.name.clone())
                });
                loaded.state.normalize(&discovered, false);
                let membership_changed = loaded
                    .state
                    .initial_layout
                    .as_ref()
                    .and_then(|r| r.undo.as_ref())
                    .is_some_and(|u| u.discovered != initial_layout::discovery_signature(snapshot));
                if before.items != loaded.state.items
                    || before.folders != loaded.state.folders
                    || membership_changed
                {
                    loaded.state.layout_changed();
                }
                loaded.state.customized = true;
            }
            write_launcher_state(&tx, &loaded.state)?;
            tx.commit()?;
            loaded.fresh = false;
            Ok(loaded)
        })();
        result.unwrap_or_else(|e| {
            eprintln!("layout: initial save skipped: {e}");
            // Never publish the grouped proposal after a failed save. Recover
            // the pre-existing layout if readable, and preserve the original DB.
            let mut fallback =
                load_layout(&conn, false).unwrap_or_else(|_| LayoutLoad::unavailable());
            fallback.writable = false;
            fallback.state.customized = true;
            fallback
        })
    }

    /// Generic single-blob read from the `kv` table. Returns `None` when the key
    /// is absent or the row can't be decoded.
    fn kv_get(&self, key: &str) -> Option<Vec<u8>> {
        let conn = self.conn.lock().expect("cache mutex poisoned");
        conn.query_row("SELECT value FROM kv WHERE key = ?1", params![key], |r| {
            r.get::<_, Vec<u8>>(0)
        })
        .optional()
        .ok()
        .flatten()
    }

    /// Generic single-blob upsert into the `kv` table.
    fn kv_put(&self, key: &str, value: &[u8]) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("cache mutex poisoned");
        conn.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Total cached icon count (for startup logging).
    pub fn count(&self) -> usize {
        let conn = self.conn.lock().expect("cache mutex poisoned");
        conn.query_row("SELECT COUNT(*) FROM icons", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as usize
    }

    /// Path of the DB file (for docs/logging).
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Is `cached` still fresh given the shortcut's current `probe`?
///
/// Mirrors the invalidation rules in `docs/ICON_CACHE.md`. Any single mismatch
/// → re-extract. We deliberately don't partially reuse (e.g. a target-only
/// change): icons are small and extraction is the expensive part anyway.
pub fn is_cache_valid(probe: &CacheProbe<'_>, cached: &CachedIcon) -> bool {
    cached.image.w == CACHED_ICON_SIZE
        && cached.image.h == CACHED_ICON_SIZE
        && cached.extracted_at_version == EXTRACTION_VERSION
        && cached.source_modified_at == probe.source_modified_at
        && cached.target_path == probe.target_path
        && cached.target_modified_at == probe.target_modified_at
        && cached.icon_location == probe.icon_location
        && cached.icon_index == probe.icon_index
        && cached.provider_icon.as_ref() == probe.provider_icon
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

const SETTINGS_KEY: &str = "settings";
const LAUNCHER_STATE_KEY: &str = "launcher_state";
#[cfg(windows)]
const IDENTITY_ALIASES_KEY: &str = "identity_aliases";

#[derive(Debug, Clone)]
pub struct LayoutLoad {
    pub state: LauncherState,
    pub fresh: bool,
    pub writable: bool,
}

impl LayoutLoad {
    fn unavailable() -> Self {
        let mut state = LauncherState::new();
        state.customized = true;
        Self {
            state,
            fresh: false,
            writable: false,
        }
    }
}

fn json_error(e: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(e))
}

fn read_blob(conn: &Connection, key: &str) -> rusqlite::Result<Option<Vec<u8>>> {
    conn.query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0))
        .optional()
}

fn load_layout(conn: &Connection, new_data_area: bool) -> rusqlite::Result<LayoutLoad> {
    if let Some(bytes) = read_blob(conn, LAUNCHER_STATE_KEY)? {
        let state: LauncherState = serde_json::from_slice(&bytes).map_err(json_error)?;
        let mut normalized = state.clone();
        normalized.normalize(&Default::default(), false);
        if normalized != state {
            return Err(rusqlite::Error::InvalidQuery);
        }
        return Ok(LayoutLoad {
            state,
            fresh: false,
            writable: true,
        });
    }
    let state = LauncherState::new();
    let has_keys: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM kv)", [], |r| r.get(0))?;
    let has_icons: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM icons)", [], |r| r.get(0))?;
    Ok(LayoutLoad {
        state,
        fresh: new_data_area && !has_keys && !has_icons,
        writable: true,
    })
}

fn write_launcher_state(conn: &Connection, state: &LauncherState) -> rusqlite::Result<()> {
    let bytes = serde_json::to_vec(state).map_err(json_error)?;
    conn.execute("INSERT INTO kv(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![LAUNCHER_STATE_KEY, bytes])?;
    Ok(())
}

fn open_at(path: &Path) -> Result<IconCache, String> {
    let new_data_area =
        path == Path::new(":memory:") || !path.try_exists().map_err(|e| e.to_string())?;
    let mut conn = Connection::open(path).map_err(|e| e.to_string())?;
    // Reject a too-new schema before making persistent PRAGMA changes.
    crate::database::migrate(&mut conn).map_err(|e| e.to_string())?;
    conn.busy_timeout(std::time::Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "synchronous", "FULL")
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "temp_store", "MEMORY")
        .map_err(|e| e.to_string())?;
    Ok(IconCache {
        conn: Mutex::new(conn),
        path: path.to_path_buf(),
        new_data_area,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_icon(c: [u8; 4]) -> DecodedIcon {
        let n = (CACHED_ICON_SIZE as usize).pow(2) * 4;
        DecodedIcon {
            rgba: [c[0], c[1], c[2], c[3]].repeat(n / 4),
            w: CACHED_ICON_SIZE,
            h: CACHED_ICON_SIZE,
        }
    }

    fn cache() -> IconCache {
        open_at(Path::new(":memory:")).expect("open in-memory cache")
    }

    pub(super) fn entry(app_id: &str, modified_at: u64) -> CachedIcon {
        CachedIcon {
            app_id: AppId::from_normalized(app_id.to_string()),
            link_path: format!("C:\\{app_id}.lnk"),
            display_name: app_id.to_string(),
            source_modified_at: modified_at,
            target_path: String::new(),
            target_modified_at: 0,
            icon_location: String::new(),
            icon_index: 0,
            provider_icon: None,
            image: fake_icon([1, 2, 3, 255]),
            extracted_at_version: EXTRACTION_VERSION,
            category: IconCategory::FullBleed,
            scale: 1.0,
        }
    }

    fn probe<'a>(app_id: &'a AppId, modified_at: u64) -> CacheProbe<'a> {
        CacheProbe {
            app_id,
            source_modified_at: modified_at,
            target_path: "",
            target_modified_at: 0,
            icon_location: "",
            icon_index: 0,
            provider_icon: None,
        }
    }

    fn id(s: &str) -> AppId {
        AppId::from_normalized(s.to_string())
    }

    #[test]
    fn put_then_get_if_valid_returns_entry() {
        let c = cache();
        let e = entry("app1", 10);
        c.put(&e).unwrap();
        let app = id("app1");
        let got = c.get_if_valid(&probe(&app, 10)).unwrap();
        assert!(got.is_some());
        assert_eq!(got.unwrap().display_name, "app1");
    }

    #[test]
    fn provider_artwork_roundtrips_and_invalidates_on_add_change_or_removal() {
        let c = cache();
        let mut e = entry("app1", 10);
        let app = id("app1");
        c.put(&e).unwrap();
        let source = IconSource {
            path: "library/game.exe".into(),
            modified_at: 20,
        };
        let mut p = probe(&app, 10);
        p.provider_icon = Some(&source);
        assert!(c.get_if_valid(&p).unwrap().is_none());
        e.provider_icon = Some(source.clone());
        c.put(&e).unwrap();
        assert_eq!(
            c.get_if_valid(&p).unwrap().unwrap().provider_icon,
            e.provider_icon
        );
        let updated = IconSource {
            modified_at: 21,
            ..source.clone()
        };
        p.provider_icon = Some(&updated);
        assert!(c.get_if_valid(&p).unwrap().is_none());
        let moved = IconSource {
            path: "library/new/game.exe".into(),
            ..source
        };
        p.provider_icon = Some(&moved);
        assert!(c.get_if_valid(&p).unwrap().is_none());
        p.provider_icon = None;
        assert!(c.get_if_valid(&p).unwrap().is_none());
    }

    #[test]
    fn missing_entry_returns_none() {
        let c = cache();
        let app = id("nope");
        assert!(c.get_if_valid(&probe(&app, 1)).unwrap().is_none());
    }

    #[test]
    fn modified_at_change_invalidates() {
        let c = cache();
        c.put(&entry("app1", 10)).unwrap();
        let app = id("app1");
        assert!(c.get_if_valid(&probe(&app, 11)).unwrap().is_none());
    }

    #[test]
    fn target_path_change_invalidates() {
        let c = cache();
        let mut e = entry("app1", 10);
        e.target_path = "C:\\old.exe".to_string();
        c.put(&e).unwrap();
        let app = id("app1");
        let mut p = probe(&app, 10);
        p.target_path = "C:\\new.exe";
        assert!(c.get_if_valid(&p).unwrap().is_none());
    }

    #[test]
    fn icon_location_change_invalidates() {
        let c = cache();
        let mut e = entry("app1", 10);
        e.icon_location = "old".to_string();
        c.put(&e).unwrap();
        let app = id("app1");
        let mut p = probe(&app, 10);
        p.icon_location = "new";
        assert!(c.get_if_valid(&p).unwrap().is_none());
    }

    #[test]
    fn wrong_extraction_version_invalidates() {
        let c = cache();
        let mut e = entry("app1", 10);
        e.extracted_at_version = EXTRACTION_VERSION + 1;
        c.put(&e).unwrap();
        let app = id("app1");
        // Stored version differs from current EXTRACTION_VERSION → invalid.
        assert!(c.get_if_valid(&probe(&app, 10)).unwrap().is_none());
    }

    #[test]
    fn wrong_image_size_invalidates() {
        let c = cache();
        let mut e = entry("app1", 10);
        e.image.w = CACHED_ICON_SIZE + 1;
        e.image.h = CACHED_ICON_SIZE + 1;
        c.put(&e).unwrap();
        let app = id("app1");
        assert!(c.get_if_valid(&probe(&app, 10)).unwrap().is_none());
    }

    #[test]
    fn put_is_upsert() {
        let c = cache();
        c.put(&entry("app1", 10)).unwrap();
        let mut e2 = entry("app1", 20);
        e2.display_name = "renamed".to_string();
        c.put(&e2).unwrap();
        let app = id("app1");
        let got = c.get_if_valid(&probe(&app, 20)).unwrap().unwrap();
        assert_eq!(got.display_name, "renamed");
        assert_eq!(c.count(), 1);
    }

    #[test]
    fn display_name_updates_without_replacing_icon_pixels() {
        let c = cache();
        c.put(&entry("app1", 10)).unwrap();
        let app = id("app1");

        assert!(c.update_display_name(&app, "localized name").unwrap());
        let got = c.get_if_valid(&probe(&app, 10)).unwrap().unwrap();
        assert_eq!(got.display_name, "localized name");
        assert_eq!(got.image.rgba, fake_icon([1, 2, 3, 255]).rgba);
        assert!(!c.update_display_name(&app, "localized name").unwrap());
    }

    #[test]
    fn forget_removes_row() {
        let c = cache();
        c.put(&entry("app1", 10)).unwrap();
        c.put(&entry("app2", 10)).unwrap();
        c.forget(&id("app1")).unwrap();
        assert_eq!(c.count(), 1);
    }

    #[test]
    fn retain_and_touch_keeps_present_and_drops_others() {
        // Regression guard: the multi-row VALUES syntax must be valid SQL
        // (earlier version emitted `VALUES ?1,?2,…` → syntax error).
        let c = cache();
        c.put(&entry("keep1", 10)).unwrap();
        c.put(&entry("keep2", 10)).unwrap();
        c.put(&entry("gone", 10)).unwrap();
        assert_eq!(c.count(), 3);

        c.retain_and_touch(&[id("keep1"), id("keep2")])
            .expect("retain_and_touch must not error");

        assert_eq!(c.count(), 2, "absent id should be GC'd");
        assert!(c.get_if_valid(&probe(&id("keep1"), 10)).unwrap().is_some());
        assert!(c.get_if_valid(&probe(&id("gone"), 10)).unwrap().is_none());
    }

    #[test]
    fn retain_and_touch_handles_large_sets() {
        // Exercise the 500-id chunking with a set bigger than one chunk.
        let c = cache();
        let mut present = Vec::new();
        for i in 0..750 {
            let e = entry(&format!("app{i}"), 1);
            c.put(&e).unwrap();
            present.push(e.app_id.clone());
        }
        // Add some rows that should be dropped.
        c.put(&entry("extra1", 1)).unwrap();
        c.put(&entry("extra2", 1)).unwrap();
        assert_eq!(c.count(), 752);

        c.retain_and_touch(&present)
            .expect("no SQL error across chunks");
        assert_eq!(c.count(), 750, "extras GC'd; all present kept");
    }

    #[test]
    fn is_cache_valid_logic_directly() {
        let e = entry("app1", 10);
        let app = id("app1");
        let p = probe(&app, 10);
        assert!(is_cache_valid(&p, &e));
    }

    #[test]
    fn corrupt_database_is_preserved_and_open_fails() {
        let tmp = std::env::temp_dir().join(format!(
            "launchpad-test-corrupt-{}.sqlite3",
            std::process::id()
        ));
        std::fs::write(&tmp, b"not a sqlite file").unwrap();
        assert!(open_at(&tmp).is_err());
        assert_eq!(std::fs::read(&tmp).unwrap(), b"not a sqlite file");
        std::fs::remove_file(&tmp).unwrap();
    }
}

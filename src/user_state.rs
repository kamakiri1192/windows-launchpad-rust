//! User-owned layout and identity aliases. Never rebuilt with the icon cache.
use crate::domain::{app_catalog::IdentityAliases, launcher_state::LauncherState};
use rusqlite::{Connection, OptionalExtension};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UserState {
    pub version: u32,
    pub layout: LauncherState,
    pub aliases: IdentityAliases,
}
impl UserState {
    pub fn new(layout: LauncherState, aliases: IdentityAliases) -> Self {
        Self {
            version: 1,
            layout,
            aliases,
        }
    }
}
pub fn load(path: &Path) -> Result<Option<UserState>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let conn = Connection::open(path).map_err(|e| e.to_string())?;
    let json: Option<String> = conn
        .query_row(
            "SELECT value FROM user_state WHERE key='current'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    json.map(|json| {
        let state: UserState = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        if state.version != 1 {
            return Err(format!("Unsupported user-state version {}", state.version));
        }
        Ok(state)
    })
    .transpose()
}
/// Layout and identity commit together. Preserve the pre-migration layout and
/// previous transaction for recovery; never delete a damaged durable database.
pub fn save(
    path: &Path,
    state: &UserState,
    migration_layout: &LauncherState,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut conn = Connection::open(path).map_err(|e| e.to_string())?;
    conn.busy_timeout(std::time::Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS user_state(key TEXT PRIMARY KEY, value TEXT NOT NULL);").map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let json = serde_json::to_string(state).map_err(|e| e.to_string())?;
    let current: Option<String> = tx
        .query_row(
            "SELECT value FROM user_state WHERE key='current'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if current.as_deref() == Some(&json) {
        return Ok(());
    }
    let backup = serde_json::to_string(migration_layout).map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT OR IGNORE INTO user_state VALUES('migration_layout', ?1)",
        [backup],
    )
    .map_err(|e| e.to_string())?;
    tx.execute("INSERT OR REPLACE INTO user_state SELECT 'previous', value FROM user_state WHERE key='current'", []).map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT OR REPLACE INTO user_state VALUES('current', ?1)",
        [json],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{app_id::AppId, launcher_item::LauncherItem};
    #[test]
    fn durable_layout_and_aliases_survive_icon_cache_deletion_and_bad_store_is_preserved() {
        let directory = std::env::temp_dir().join(format!(
            "launchpad-durable-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("user-state.sqlite3");
        let mut layout =
            LauncherState::from_legacy(vec![AppId::from_normalized("existing-link")], vec![]);
        let mut aliases = IdentityAliases::default();
        let entry = crate::domain::app_diff::SnapshotEntry {
            app_id: AppId::from_normalized("existing-link"),
            name: "Example".into(),
            link_path: "existing-link".into(),
            target_path: "v1/app.exe".into(),
            link_mtime: 0,
            target_mtime: 0,
            icon_location: String::new(),
            icon_index: 0,
            provider_icon: None,
            version: String::new(),
            publisher: String::new(),
            identifier: String::new(),
        };
        let candidate = crate::domain::app_catalog::Candidate {
            entry,
            identity: Some("aumid:Example".into()),
            native_key: None,
            exact_key: None,
            source: crate::domain::app_catalog::Source::Shell,
            normal: true,
            valid: true,
        };
        aliases.reconcile(
            crate::domain::app_catalog::group(vec![candidate]),
            &mut layout,
        );
        let first = UserState::new(layout.clone(), aliases);
        save(&path, &first, &layout).unwrap();
        let cache = directory.join("cache.sqlite3");
        std::fs::write(&cache, b"rebuildable icons").unwrap();
        std::fs::remove_file(cache).unwrap();
        assert_eq!(load(&path).unwrap(), Some(first.clone()));
        let mut second = first.clone();
        second
            .layout
            .items
            .push(LauncherItem::App(AppId::from_normalized("another")));
        save(&path, &second, &layout).unwrap();
        // Polling an unchanged catalog must not overwrite the recovery record.
        save(&path, &second, &layout).unwrap();
        assert_eq!(load(&path).unwrap(), Some(second));
        let conn = Connection::open(&path).unwrap();
        let backup: String = conn
            .query_row(
                "SELECT value FROM user_state WHERE key='migration_layout'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<LauncherState>(&backup).unwrap(),
            layout
        );
        let previous: String = conn
            .query_row(
                "SELECT value FROM user_state WHERE key='previous'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(serde_json::from_str::<UserState>(&previous).unwrap(), first);
        drop(conn);
        std::fs::write(&path, b"damaged user state").unwrap();
        assert!(load(&path).is_err());
        assert!(save(&path, &first, &layout).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"damaged user state");
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}

# Icon cache

The launcher caches normalized icon bitmaps on disk so that the **second and
later launches** never block first paint on platform icon extraction. This
document covers the storage location, schema, validity rules, and recovery
behavior.

See also: [STARTUP_PERFORMANCE.md](STARTUP_PERFORMANCE.md) for how the cache
fits into the launch pipeline, and [APP_REFRESH.md](APP_REFRESH.md) for how
cache invalidation interacts with live Start Menu changes.

## Storage location

The cache is a single SQLite file at the platform data directory:

```text
Windows: %LOCALAPPDATA%\Launchpad\launchpad.sqlite3
macOS:   ~/Library/Application Support/Launchpad/launchpad.sqlite3
```

These are the usual non-sandboxed paths. Windows resolves LocalAppData with
`SHGetKnownFolderPath`; macOS uses `NSFileManager`'s user Application Support
directory (including the sandbox container when applicable).
There is no executable-directory or current-directory fallback. The directory
is created on demand; failures stop startup.

SQLite is bundled on Windows; macOS uses the system SQLite library.

## Schema

The canonical schema is [001_initial.sql](../migrations/001_initial.sql):

- `icons`: rebuildable normalized RGBA images and extraction metadata.
- `kv`: user-owned JSON blobs (`settings`, `launcher_state`, and Windows
  `identity_aliases`). Initial-layout undo is part of `launcher_state`.

Settings and layout resets commit in one transaction; identity reconciliation
commits its aliases and layout together. Icon reset only deletes `icons` rows.

The icon image is stored as a **straight-alpha RGBA8 blob** (`TARGET × TARGET × 4`
bytes), not PNG. This trades a little disk space for zero encode/decode cost on
the hot startup path. Alpha is straight (not premultiplied); the icon shader
re-premultiplies at sample time.

PRAGMAs applied on open: `journal_mode=WAL`, `synchronous=FULL`,
`temp_store=MEMORY` — the same connection stores user data and icons.

## Versioning

Two independent version numbers guard against stale data:

- **Schema migrations**: `rusqlite_migration` 1.3.1 (compatible with Rust 1.89
  and `rusqlite` 0.32) manages SQLite's `user_version`. Numbered SQL files in
  `migrations/` are embedded and registered in order in `src/database.rs`.
  `to_latest` runs before normal DB access. Append new migrations; never edit
  one that has been released. There is no custom schema version table or
  column-repair logic.
- **`EXTRACTION_VERSION`** (`icon_cache::EXTRACTION_VERSION`, currently `6`
  on Windows and `9` on macOS) —
  the *extraction algorithm itself* (normalization target size, alpha handling,
  fallback strategy). Stored per-row in `extraction_version`; an entry whose
  stored version differs from the current one is treated as invalid even if the
  source is byte-identical. macOS version 5 resolves icons through Launch
  Services, covering asset-catalog-only apps and ICNS encodings that the
  portable decoder cannot read. macOS Steam entries prefer the installed
  game's `.app` bundle, avoiding the low-resolution artwork in
  `appcache/librarycache`.

Append a migration for schema/data changes. Bump `EXTRACTION_VERSION` only
when extraction changes. Schema updates do not automatically wipe icons.

## Cache invalidation

A cached entry is served only if **every** salient field still matches the
shortcut's current state. The check is `icon_cache::is_cache_valid`:

```rust
cached.image.w == CACHED_ICON_SIZE
    && cached.image.h == CACHED_ICON_SIZE
    && cached.extracted_at_version == EXTRACTION_VERSION
    && cached.link_mtime    == probe.link_mtime
    && cached.target_path   == probe.target_path
    && cached.target_mtime  == probe.target_mtime
    && cached.icon_location == probe.icon_location
    && cached.icon_index    == probe.icon_index
    && cached.provider_icon.as_ref() == probe.provider_icon
```

Provider artwork has its own path and mtime, independent of the selected launch
route and its target metadata. Adding a provider source invalidates only the
affected app\'s icon.

A single mismatch → re-extract. Concretely, the cache is invalidated when:

| Condition                              | Why                                            |
| -------------------------------------- | ---------------------------------------------- |
| `.lnk` path is new (no row)            | never extracted                                |
| `.lnk` mtime changed                   | shortcut file was rewritten                    |
| resolved target path changed           | shortcut now points elsewhere                  |
| target mtime changed                   | target exe/dll was updated                     |
| `IconLocation` string changed          | shortcut's explicit icon was re-pointed        |
| provider artwork added, removed, moved, or updated | preferred artwork changed |
| icon index changed                     | icon within the location changed               |
| `EXTRACTION_VERSION` changed           | extraction algorithm changed (re-extract all)  |
| normalized icon size (`CACHED_ICON_SIZE`) changed | atlas cell size changed             |

On startup, valid cache entries are applied **immediately** (no worker round
trip); only invalid/missing entries are queued for extraction. See
`App::ingest_snapshot` in `main.rs`.

## Lifecycle / garbage collection

- **Upsert**: `IconCache::put` writes one fully-extracted icon
  (`INSERT … ON CONFLICT(app_id) DO UPDATE`).
- **Forget**: `IconCache::forget` deletes a single row (used when an app is
  removed from the Start Menu).
- **Retain + touch**: `IconCache::retain_and_touch` bumps `last_seen_at` for
  every still-present id and deletes rows no longer referenced. Called after a
  refresh diff so the cache doesn't accumulate icons for uninstalled apps.
  Uses a SQLite temp table to safely handle arbitrarily large id sets.

`last_seen_at` and `deleted_at` are present for future tombstone-style GC; the
current implementation does eager deletion on retain.

## Startup failures

`IconCache::open` resolves the path, opens the shared DB and runs migrations.
Path resolution, open or migration failure exits with a nonzero status before
normal startup. Migration rollback is handled by the library's transaction.
An old schema cannot be used by the new executable just because rollback
succeeded. The next launch attempts the pending migrations normally.

No new user-facing message, dialog, automatic deletion, initialization after
failure, repair, retry loop, alternate file or in-memory fallback is added.
A DB newer than the executable's migrations is rejected without downgrading.

The old `cache.sqlite3` and `user-state.sqlite3` are not read by the app.
Pre-release developer data is moved once as a separate maintenance operation,
not as an application startup feature.

## Testing

`database::tests` covers initial creation, repeated migration, upgrade,
transaction rollback and future-schema rejection.
`icon_cache::tests` and `initial_layout_tests` cover icon validity, user-data
persistence, atomic saves, initial-layout undo and corruption preservation.
Tests use memory databases or isolated temporary paths, never the user's DB.

## Known limitations

- No partial reuse: a single field change re-extracts the whole icon. Icons are
  small and extraction is the expensive part, so this is acceptable.
- WAL/SHM sidecars may live next to the `.sqlite3`. Do not manually delete
  them: the WAL may contain committed data not yet checkpointed.
- The cache key intentionally avoids `fs::canonicalize` (which would resolve
  Start Menu junctions differently across machines); normalization is purely
  lexical, matching `app_id` normalization.

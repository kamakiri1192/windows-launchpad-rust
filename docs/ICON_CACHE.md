# Icon cache

The launcher caches normalized icon bitmaps on disk so that the **second and
later launches** never block first paint on Shell/GDI extraction. This document
covers the storage location, schema, validity rules, and recovery behavior.

See also: [STARTUP_PERFORMANCE.md](STARTUP_PERFORMANCE.md) for how the cache
fits into the launch pipeline, and [APP_REFRESH.md](APP_REFRESH.md) for how
cache invalidation interacts with live Start Menu changes.

## Storage location

The cache is a single SQLite file at:

```text
%LOCALAPPDATA%\Launchpad\cache.sqlite3
```

If `LOCALAPPDATA` is unset (unusual on a normal Windows session), the launcher
falls back to `launchpad-cache.sqlite3` next to the executable. The directory is
created on demand. Implemented in `icon_cache::default_db_path`.

We use `rusqlite` with the `bundled` feature, so no system SQLite dependency is
required — the C library is compiled into the binary.

## Schema

Two tables plus a `schema_meta` version store.

```sql
CREATE TABLE schema_meta (
    key   TEXT PRIMARY KEY,
    value INTEGER NOT NULL
);
-- Always holds: ('schema_version', N) and ('extraction_version', N).

CREATE TABLE icons (
    app_id             TEXT PRIMARY KEY,   -- normalized .lnk path (see app_id.rs)
    link_path          TEXT NOT NULL,      -- raw .lnk path
    display_name       TEXT NOT NULL,      -- file stem, for labels
    link_mtime         INTEGER NOT NULL,   -- Windows file time of the .lnk
    target_path        TEXT NOT NULL,      -- resolved + env-expanded target
    target_mtime       INTEGER NOT NULL,   -- mtime of the target file
    icon_location      TEXT NOT NULL,      -- Shell IconLocation (may be "")
    icon_index         INTEGER NOT NULL,   -- icon index inside icon_location/target
    image_w            INTEGER NOT NULL,   -- normalized icon width (== TARGET)
    image_h            INTEGER NOT NULL,   -- normalized icon height (== TARGET)
    image_rgba         BLOB NOT NULL,      -- straight-alpha RGBA8, w*h*4 bytes
    extraction_version INTEGER NOT NULL,   -- EXTRACTION_VERSION at extract time
    last_seen_at       INTEGER NOT NULL,   -- unix seconds of last scan that saw it
    deleted_at         INTEGER             -- reserved for tombstoning
);
CREATE INDEX icons_last_seen ON icons(last_seen_at);

CREATE TABLE kv (
    key   TEXT PRIMARY KEY,
    value BLOB NOT NULL
);
```

The icon image is stored as a **straight-alpha RGBA8 blob** (`TARGET × TARGET × 4`
bytes), not PNG. This trades a little disk space for zero encode/decode cost on
the hot startup path. Alpha is straight (not premultiplied); the icon shader
re-premultiplies at sample time.

PRAGMAs applied on open: `journal_mode=WAL`, `synchronous=NORMAL`,
`temp_store=MEMORY` — good durability/crash-safety for a rebuildable cache at
low write latency.

## Versioning

Two independent version numbers guard against stale data:

- **`SCHEMA_VERSION`** (`icon_cache::SCHEMA_VERSION`, currently `1`) — the
  on-disk table layout. A mismatch on open wipes the `icons` table so every
  entry is re-extracted against the new schema.
- **`EXTRACTION_VERSION`** (`icon_cache::EXTRACTION_VERSION`, currently `1`) —
  the *extraction algorithm itself* (normalization target size, alpha handling,
  fallback strategy). Stored per-row in `extraction_version`; an entry whose
  stored version differs from the current one is treated as invalid even if the
  `.lnk` is byte-identical.

Bump either constant when you change the corresponding thing; no manual
migration code is needed.

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
```

A single mismatch → re-extract. Concretely, the cache is invalidated when:

| Condition                              | Why                                            |
| -------------------------------------- | ---------------------------------------------- |
| `.lnk` path is new (no row)            | never extracted                                |
| `.lnk` mtime changed                   | shortcut file was rewritten                    |
| resolved target path changed           | shortcut now points elsewhere                  |
| target mtime changed                   | target exe/dll was updated                     |
| `IconLocation` string changed          | shortcut's explicit icon was re-pointed        |
| icon index changed                     | icon within the location changed               |
| `EXTRACTION_VERSION` changed           | extraction algorithm changed (re-extract all)  |
| `SCHEMA_VERSION` changed               | schema layout changed (table wiped on open)    |
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

## Recovery from corruption

`IconCache::open_or_rebuild` is the entry point. If opening the DB fails (corrupt
file, unreadable, schema migration error), the launcher:

1. deletes the DB file,
2. tries to open it again (now creating a fresh empty DB),
3. if *that* also fails, falls back to an in-memory SQLite database so the app
   keeps running uncached rather than crashing.

A corrupt cache therefore costs at most one cold extraction pass; it never
blocks startup.

## Testing

`icon_cache::tests` covers: put/get round-trip, missing entry, each
invalidation condition (mtime, target path, icon location, extraction version,
image size), upsert semantics, `forget`, the default-path heuristic, and
corrupt-file recovery. The DB is opened `:memory:` in tests so nothing touches
disk.

## Known limitations

- No partial reuse: a single field change re-extracts the whole icon. Icons are
  small and extraction is the expensive part, so this is acceptable.
- WAL/SHM sidecar files live next to the `.sqlite3`; they're recreated
  automatically and are safe to delete when the launcher isn't running.
- The cache key intentionally avoids `fs::canonicalize` (which would resolve
  Start Menu junctions differently across machines); normalization is purely
  lexical, matching `app_id` normalization.

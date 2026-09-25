# App refresh — live Start Menu changes

The launcher reflects Start Menu changes that happen **while it is running**:
new shortcuts appear, removed shortcuts disappear, and updated shortcuts get
their icons re-extracted. This document describes the design and how it stays
off the UI thread.

See also: [STARTUP_PERFORMANCE.md](STARTUP_PERFORMANCE.md) for the overall
launch pipeline, and [ICON_CACHE.md](ICON_CACHE.md) for the cache invalidation
that drives re-extraction.

## Goals

- A `.lnk` added to the Start Menu while the launcher is open appears in the
  grid (with its icon loaded in the background).
- A `.lnk` removed from the Start Menu disappears from the grid.
- A `.lnk` whose mtime / target / `IconLocation` / target-mtime changed gets its
  icon re-extracted and replaced.
- All of the above happens without freezing scrolling, clicks, or exit.
- While an icon is missing or being re-extracted, the placeholder color tile +
  label remain visible and the app stays launchable.

## Snapshot model

The source of truth for discovered Start Menu and Steam apps is a snapshot:

```text
BTreeMap<AppId, SnapshotEntry>
```

where `SnapshotEntry` (`app_diff::SnapshotEntry`) carries the fields that matter
for both identity and cache invalidation:

- `app_id` (durable catalog ID, legacy path ID or stable `steam:<appid>` id)
- `name`, `link_path`
- `link_mtime`, `target_path`, `target_mtime`
- `icon_location`, `icon_index`

On Windows, `app_scan::scan_catalog` collects all AppsFolder entries, Start Menu
links, desktop links and Steam manifests, with source completion status. The UI
reconciles identity aliases and user placement before producing the snapshot.
See [APP_DISCOVERY.md](APP_DISCOVERY.md) for matching and durable migration.
Other platforms continue to use `scan_start_menu`. Scanning reads metadata and
mtimes without extracting icon pixels.

Steam discovery remains active when the user disables **Show Steam apps** in
Settings. The display filter hides Steam entries from both the grid and search,
while keeping the latest registry state ready for an immediate toggle back on.

## Diffing

Comparing two snapshots is pure logic in `app_diff::diff_snapshots`:

```text
added   = ids in new but not old
removed = ids in old but not new
updated = ids in both whose entries differ
```

`SnapshotEntry::icon_relevant_diff` further classifies an `updated` entry as
icon-relevant (mtime / target / icon-location / icon-index changed) vs.
name-only (the icon is unaffected, only the label refreshes).

## Detection strategy (Phase 4: polling)

`refresh_watcher::spawn` runs a background thread that:

1. sleeps an `initial_delay` (default 2s, so first paint + cached-icon fan-out
   happen first),
2. on Windows, sends `RefreshMessage::Catalog` with candidates and source status,
3. every `poll_interval` (default 10s), repeats that scan. The UI reconciles
   durable IDs before diffing and skips rendering work for an unchanged catalog.

Other platforms send `RefreshMessage::Initial` followed by nonempty
`RefreshMessage::Diff` messages as before.

The UI thread's `App::drain_inbox` turns reconciled snapshots into `ingest_snapshot` (initial)
or `apply_diff` (subsequent) calls. Polling is deliberately simple and robust;
it has none of the edge cases of directory-change notifications (buffer sizing,
junction loops, overwritten-file semantics).

## Applying a diff

`App::apply_diff` runs on the UI thread and does the minimum work needed:

1. **Removals** → `registry.remove(id)`, `atlas.clear_slot(slot)` (the cell goes
   transparent), and `cache.forget(id)`. Other apps' slots/UVs are untouched.
2. **Added + updated** → ensure the app exists in the registry (new apps get a
   fresh slot), then re-probe the cache:
   - valid cache → apply immediately (`apply_cached_icon`);
   - invalid/missing → mark `Loading` and queue an `IconRequest` to the worker.
3. **Relayout once** so tile/label/icon-instance buffers reflect the new set.
4. **GC the cache** via `retain_and_touch` so removed apps don't linger on disk.

Because slots are stable, an app keeps its slot and UV across updates; only its
pixels are re-blitted when the new icon arrives. Adding or removing other apps
never shifts an existing app's UV.

## Stable identity and click safety

Clicks resolve through **stable `AppId`s, not positional indices**. The flow in
`App::resolve_clicked_app`:

```text
hit-test  →  display index  →  registry.apps()[index].app_id  →  AppLaunchInfo
```

The `AppLaunchInfo` (name + link_path) is **cloned** before the launcher
dismisses, so even a concurrent rescan that mutated the registry between the
pick and the `ShellExecuteExW` invocation retains the selected locator. The pre-existing
"dismiss first, launch second" behavior is preserved.

## Background-safety

Regular discovery and icon extraction live off the UI thread:

| Work                                 | Where it runs               |
| ------------------------------------ | --------------------------- |
| Start Menu recursive scan            | refresh watcher thread      |
| `.lnk` target/icon-location resolve  | refresh watcher thread      |
| `GetFileAttributesExW` (mtimes)      | refresh watcher thread      |
| icon extraction (Shell/GDI/COM)      | icon worker thread          |
| icon-cache writes                    | icon worker thread (via Arc)|
| Windows ID reconciliation and diff  | UI thread                   |
| durable layout/alias transaction    | UI thread                   |
| other-platform diff computation     | refresh watcher thread      |
| registry mutation, atlas blit, redraw| UI thread (cheap)           |

The UI commits layout and aliases together before applying a Windows migration.
Unchanged state skips the write and redraw. A launch parsing failure can run one
synchronous recovery scan after dismissing the window; only a changed locator for
the same durable app is retried. Normal launch and rejected/cancelled invocations
do not take this recovery path.

## Future work

- **Event-driven watching**: replace polling with `ReadDirectoryChangesW`
  (overlapped, per-directory handles for both Start Menu roots), with the
  polling watcher kept as a fallback when change notifications fail. The
  `RefreshMessage` channel is already the right seam — only `refresh_watcher`
  internals change.
- **Per-user + all-users as separate watches**: today both roots are scanned in
  one pass; separate watches would localize change notifications.
- **Debounce**: coalesce rapid bursts of changes (e.g. an installer writing many
  shortcuts) before diffing.
- **Slot compaction**: reclaim slots from removed apps after sustained churn,
  to keep the atlas from growing unbounded.

## Testing

- `app_diff::tests` covers added / removed / updated (mtime + icon-location),
  name-only changes (updated but not icon-relevant), and the all-three-at-once
  case.
- `app_scan::tests` verifies a scan returns a well-formed map without panicking
  on an empty/real Start Menu.
- `app_registry::tests` covers slot allocation, sorted insertion, rename
  re-sort, slot stability across updates/removals, and click-snapshot ownership.

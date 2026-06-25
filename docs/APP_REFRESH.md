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

The source of truth for "what's in the Start Menu" is a snapshot:

```text
BTreeMap<AppId, SnapshotEntry>
```

where `SnapshotEntry` (`app_diff::SnapshotEntry`) carries the fields that matter
for both identity and cache invalidation:

- `app_id` (normalized `.lnk` path — see `app_id.rs`)
- `name`, `link_path`
- `link_mtime`, `target_path`, `target_mtime`
- `icon_location`, `icon_index`

Snapshots are taken by `app_scan::scan_start_menu`, which walks both Start Menu
roots (per-user + all-users), resolves each `.lnk`'s target + icon location, and
reads mtimes — but does **not** touch Shell/GDI for pixels. A scan is cheap
enough to repeat periodically.

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
2. takes the first snapshot and sends `RefreshMessage::Initial` to the UI,
3. every `poll_interval` (default 10s), rescans, diffs, and sends
   `RefreshMessage::Diff(diff)` for any non-empty diff.

The UI thread's `App::drain_inbox` turns these into `ingest_snapshot` (initial)
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
pick and the `ShellExecuteW` can't launch the wrong app. The pre-existing
"dismiss first, launch second" behavior is preserved.

## Background-safety

Everything expensive lives off the UI thread:

| Work                                 | Where it runs               |
| ------------------------------------ | --------------------------- |
| Start Menu recursive scan            | refresh watcher thread      |
| `.lnk` target/icon-location resolve  | refresh watcher thread      |
| `GetFileAttributesExW` (mtimes)      | refresh watcher thread      |
| icon extraction (Shell/GDI/COM)      | icon worker thread          |
| SQLite writes                        | icon worker thread (via Arc)|
| diff computation                     | refresh watcher thread      |
| registry mutation, atlas blit, redraw| UI thread (cheap)           |

The UI thread only ever: receives results, mutates the registry, blits one cell
to the GPU, and requests a redraw.

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

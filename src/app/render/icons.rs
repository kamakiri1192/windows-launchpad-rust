//! Icon cache / worker / diff render adapter methods.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::domain::app_diff::{AppDiff, SnapshotEntry};
use crate::domain::app_id::AppId;
use crate::domain::app_registry::{AppRecord, IconState};
use crate::grid;
use crate::icon_cache::{CacheProbe, CachedIcon};
use crate::icons::normalize::DecodedIcon;
use crate::startup_timer::prefix;
use crate::workers::icon_worker::{IconReason, IconRequest};

use crate::app::state::App;

impl App {
    pub(crate) fn ensure_atlas_uploaded(&mut self) -> bool {
        let needed = self.registry.slot_count().max(1);
        let grew = self.atlas.ensure_capacity(needed);
        if grew {
            self.resync_registry_uvs();
        }
        let Some(r) = self.renderer.as_mut() else {
            return grew;
        };
        let (gw, gh) = (self.atlas.width(), self.atlas.height());
        let (cur_w, cur_h) = r.icon_atlas_size();

        if !self.atlas_uploaded || grew || (cur_w, cur_h) != (gw, gh) {
            // (Re)allocate + full upload.
            r.upload_icon_atlas(self.atlas.rgba(), gw, gh);
            self.atlas_uploaded = true;
            self.timer
                .mark(prefix::STARTUP, "atlas + GPU texture upload");
        }
        grew
    }

    /// Apply one freshly-extracted (or cached) icon: write it into the slot,
    /// update the registry UV + state, push the cell to the GPU, and refresh
    /// the icon instance buffer so the new UV is picked up.
    pub(crate) fn apply_icon(&mut self, app_id: &AppId, image: DecodedIcon, from_cache: bool) {
        let Some(slot) = self.registry.get(app_id).map(|r| r.slot) else {
            return;
        };
        // `write_icon` may grow the CPU atlas if the slot is beyond the current
        // capacity. A grow recomputes *every* slot's UV (the cell grid gains
        // columns), so any app whose UV we cached before the grow now points at
        // the wrong cell → overlapping icons. When that happens we must
        // re-sync every app's UV from the new atlas before redrawing.
        let (gpu_w, gpu_h) = self
            .renderer
            .as_ref()
            .map(|r| r.icon_atlas_size())
            .unwrap_or((0, 0));
        let (x, y, this_uv) = self.atlas.write_icon(slot, &image);
        let atlas_grew = (self.atlas.width(), self.atlas.height()) != (gpu_w, gpu_h) || gpu_w == 0;

        if atlas_grew {
            // Full re-upload at the new dimensions, then re-sync every app's
            // UV from the freshly-grown atlas so none sample a stale cell.
            if let Some(r) = self.renderer.as_mut() {
                r.upload_icon_atlas(self.atlas.rgba(), self.atlas.width(), self.atlas.height());
            }
            self.resync_registry_uvs();
        } else if let Some(r) = self.renderer.as_ref() {
            // Same dimensions → cheap single-cell update.
            r.write_icon_cell(&image.rgba, x, y, image.w, image.h);
        }
        // Optional diagnostic: dump the atlas after every icon apply so we can
        // inspect the final packed layout for overlapping cells. Enabled by
        // env var. We always overwrite the same file → final state wins.
        if std::env::var_os("LAUNCHPAD_DUMP_ATLAS").is_some() {
            crate::dump_atlas_png(&self.atlas);
        }

        let state = if from_cache {
            IconState::Cached
        } else {
            IconState::Loaded
        };
        // Use the post-grow UV (resync already updated the registry; for the
        // non-grow path this is just the UV write_icon returned).
        let new_uv = self.atlas.uv(slot).unwrap_or(this_uv);
        self.registry.update(app_id, |rec| {
            rec.uv = Some(new_uv);
            rec.icon_state = state;
        });
        // Refresh icon instances so the new UVs are uploaded. Cheap (hundreds
        // of f32s) and keeps the draw call in sync with the atlas.
        self.rebuild_icon_instances();
        self.request_redraw();
    }

    /// After an atlas grow, rewrite every app's cached UV from the new atlas
    /// layout. Without this, apps loaded before the grow keep stale UVs and
    /// sample the wrong cell, producing overlapping/garbled icons.
    pub(crate) fn resync_registry_uvs(&mut self) {
        // Collect (id, slot) first to avoid borrowing self.registry twice.
        let entries: Vec<(AppId, u32)> = self
            .registry
            .apps()
            .iter()
            .map(|r| (r.app_id.clone(), r.slot))
            .collect();
        for (id, slot) in entries {
            if let Some(uv) = self.atlas.uv(slot) {
                self.registry.update(&id, |rec| {
                    rec.uv = Some(uv);
                });
            }
        }
    }

    /// Rebuild the icon-instance buffer from the current registry + layout and
    /// push it to the GPU. Called whenever any app's UV may have changed.
    pub(crate) fn rebuild_icon_instances(&mut self) {
        let owned = self.grid_apps_owned();
        let apps: Vec<grid::GridApp<'_>> = owned
            .iter()
            .map(|(id, name, uv)| grid::GridApp {
                id: id.as_str(),
                name: name.as_str(),
                uv: *uv,
            })
            .collect();
        let (w, _h) = self.viewport_phys();
        let visible_ids = self.visible_app_ids();
        let anim = self.edit_anim(&visible_ids);
        let mut icon_instances = self.layout.build_icon_instances(w as f32, &apps, &anim);
        self.apply_spring_positions(&visible_ids, &mut icon_instances);
        self.render_model.icons = Some(icon_instances);
    }

    /// Mark an app's icon as failed (placeholder stays), logging the error.
    pub(crate) fn fail_icon(&mut self, app_id: &AppId, error: String) {
        eprintln!("icon-worker: failed app_id={app_id}: {error}");
        self.registry.update(app_id, |rec| {
            rec.icon_state = IconState::Failed;
        });
    }

    /// Manually reset the icon cache + re-extract every icon live (R key).
    ///
    /// Wipes the on-disk SQLite cache, clears every atlas cell back to
    /// transparent, resets each app's icon state to `Missing`, then re-queues
    /// the whole app set against the worker — so the visible page refills
    /// progressively without restarting the launcher. The current snapshot
    /// (names/targets/mtimes) is reused, so no Start Menu rescan is needed.
    pub(crate) fn reset_icons(&mut self) {
        eprintln!("icon-cache: manual reset requested — clearing cache + re-extracting");

        // (1) Wipe the on-disk cache so the next probe is always a miss.
        match self.cache.clear_all() {
            Ok(n) => eprintln!("icon-cache: cleared {n} cached rows"),
            Err(e) => eprintln!("icon-cache: clear_all failed: {e}"),
        }

        // (2) Clear every atlas slot and push the blanked atlas to the GPU so
        // stale icons vanish immediately.
        let max_slot = self.registry.max_slot();
        for slot in 0..=max_slot {
            self.atlas.clear_slot(slot);
        }
        if let Some(r) = self.renderer.as_mut() {
            r.upload_icon_atlas(self.atlas.rgba(), self.atlas.width(), self.atlas.height());
        }

        // (3) Reset every app's icon state + drop its UV so placeholders show.
        let ids: Vec<AppId> = self
            .registry
            .apps()
            .iter()
            .map(|r| r.app_id.clone())
            .collect();
        for id in &ids {
            self.registry.update(id, |rec| {
                rec.uv = None;
                rec.icon_state = IconState::Missing;
            });
        }
        self.rebuild_icon_instances();
        self.request_redraw();

        // (4) Re-queue extraction requests in display order (first page first)
        // using the cached snapshot, so the worker re-extracts everything.
        if let Some(handle) = self._worker.as_ref() {
            let mut queued = 0usize;
            for id in &ids {
                let Some(entry) = self.snapshot.get(id) else {
                    continue;
                };
                self.registry.update(id, |rec| {
                    rec.icon_state = IconState::Loading;
                });
                let req = IconRequest {
                    app_id: id.clone(),
                    name: entry.name.clone(),
                    link_path: PathBuf::from(&entry.link_path),
                    link_mtime: entry.link_mtime,
                    target_path: entry.target_path.clone(),
                    target_mtime: entry.target_mtime,
                    icon_location: entry.icon_location.clone(),
                    icon_index: entry.icon_index,
                    reason: IconReason::Fresh,
                };
                if handle.requests.send(req).is_err() {
                    eprintln!("icon-worker: request channel closed during reset");
                    break;
                }
                queued += 1;
            }
            eprintln!("icon-cache: re-queued {queued} icons for extraction");
        }
    }

    /// Populate the registry from a snapshot, applying cached icons where
    /// valid and queueing extraction requests for the rest. Called on the
    /// initial scan and on each refresh diff.
    pub(crate) fn ingest_snapshot(
        &mut self,
        new_snapshot: BTreeMap<AppId, SnapshotEntry>,
        is_initial: bool,
    ) {
        if is_initial {
            self.timer.mark_with(
                prefix::STARTUP,
                "app list enumeration",
                format!("({} apps)", new_snapshot.len()),
            );
        }
        self.snapshot = new_snapshot.clone();

        let mut requests: Vec<IconRequest> = Vec::new();
        let mut cached_applied = 0usize;

        // Insert every app from the snapshot (the registry dedupes; updates
        // happen via the diff path on subsequent scans).
        for (id, entry) in &new_snapshot {
            let exists = self.registry.get(id).is_some();
            if !exists {
                let slot = self.registry.alloc_slot();
                let rec = AppRecord {
                    app_id: id.clone(),
                    name: entry.name.clone(),
                    link_path: PathBuf::from(&entry.link_path),
                    resolved_target: PathBuf::from(&entry.target_path),
                    slot,
                    icon_state: IconState::Missing,
                    uv: None,
                };
                self.registry.insert(rec);
            }
        }

        // Display order = sorted by display name (matches the registry's sort).
        // Process in this order so cache hits for the FIRST PAGE land first —
        // the user sees the visible page's icons populate before off-screen
        // pages. This is the cache-read prioritization: we don't load all icons
        // at once in arbitrary map order; the visible page wins.
        let per_page = self.layout.cols * self.layout.rows;
        let mut display_order: Vec<&AppId> = new_snapshot.keys().collect();
        display_order.sort_by(|a, b| {
            let na = new_snapshot[*a].name.to_lowercase();
            let nb = new_snapshot[*b].name.to_lowercase();
            na.cmp(&nb)
        });

        // Pass 1: apply cached icons for the first page first.
        for id in display_order.iter().take(per_page) {
            cached_applied += self.apply_cache_if_available(id, &new_snapshot[id]);
        }
        // Pass 2: apply cached icons for the remaining pages.
        for id in display_order.iter().skip(per_page) {
            cached_applied += self.apply_cache_if_available(id, &new_snapshot[id]);
        }

        // Pass 3: queue extraction requests in display order (first-page misses
        // are extracted before off-screen misses, so the visible page fills in
        // first).
        for id in display_order {
            let entry = &new_snapshot[id];
            let already_loading = matches!(
                self.registry.get(id).map(|r| r.icon_state),
                Some(IconState::Loading | IconState::Loaded | IconState::Cached)
            );
            if already_loading {
                continue;
            }
            self.registry.update(id, |rec| {
                rec.icon_state = IconState::Loading;
            });
            requests.push(IconRequest {
                app_id: id.clone(),
                name: entry.name.clone(),
                link_path: PathBuf::from(&entry.link_path),
                link_mtime: entry.link_mtime,
                target_path: entry.target_path.clone(),
                target_mtime: entry.target_mtime,
                icon_location: entry.icon_location.clone(),
                icon_index: entry.icon_index,
                reason: if is_initial {
                    IconReason::Fresh
                } else {
                    IconReason::Updated
                },
            });
        }

        if cached_applied > 0 {
            self.timer.mark_with(
                prefix::ICON_CACHE,
                "cached icon apply",
                format!("({cached_applied} icons)"),
            );
        }

        // Relayout once so new tiles + cached icons show up immediately.
        self.relayout();
        self.request_redraw();

        // Dispatch extraction requests to the worker (first page already
        // first, thanks to the display-order sort above).
        if !requests.is_empty() {
            self.timer.mark_with(
                prefix::ICON_WORKER,
                "queue extraction",
                format!("({} icons)", requests.len()),
            );
            if let Some(handle) = self._worker.as_ref() {
                for req in requests {
                    if handle.requests.send(req).is_err() {
                        eprintln!("icon-worker: request channel closed");
                        break;
                    }
                }
            }
        }
    }

    /// Apply a cached icon for `id` if the cache holds a valid entry and the
    /// app isn't already loaded. Returns 1 if applied, 0 otherwise.
    pub(crate) fn apply_cache_if_available(&mut self, id: &AppId, entry: &SnapshotEntry) -> usize {
        let probe = CacheProbe {
            app_id: id,
            link_mtime: entry.link_mtime,
            target_path: &entry.target_path,
            target_mtime: entry.target_mtime,
            icon_location: &entry.icon_location,
            icon_index: entry.icon_index,
        };
        let rec_state = self.registry.get(id).map(|r| r.icon_state);
        if matches!(rec_state, Some(IconState::Loaded | IconState::Cached)) {
            return 0;
        }
        match self.cache.get_if_valid(&probe) {
            Ok(Some(cached)) => {
                self.apply_cached_icon(id, cached);
                1
            }
            _ => 0,
        }
    }

    /// Apply a cache-served icon without going through the worker.
    pub(crate) fn apply_cached_icon(&mut self, app_id: &AppId, cached: CachedIcon) {
        self.apply_icon(app_id, cached.image, true);
    }

    /// Apply a refresh diff: add new apps, update changed ones (re-extracting
    /// icons whose cache key moved), and remove gone apps.
    pub(crate) fn apply_diff(&mut self, diff: AppDiff) {
        self.timer.mark_with(
            prefix::APP_REFRESH,
            "app list refresh",
            format!(
                "(added={} updated={} removed={})",
                diff.added.len(),
                diff.updated.len(),
                diff.removed.len()
            ),
        );

        // Removals.
        for id in &diff.removed {
            let slot = self.registry.get(id).map(|r| r.slot);
            self.registry.remove(id);
            if let Some(s) = slot {
                self.atlas.clear_slot(s);
                if let Some(r) = self.renderer.as_mut() {
                    r.upload_icon_atlas(self.atlas.rgba(), self.atlas.width(), self.atlas.height());
                }
            }
            let _ = self.cache.forget(id);
        }

        // Merge the added/updated entries into a mini-snapshot so we can reuse
        // the cache-probe + extract logic. We rebuild against the *current*
        // snapshot so probes reflect the latest fields.
        for entry in diff.added.iter().chain(diff.updated.iter()) {
            self.snapshot.insert(entry.app_id.clone(), entry.clone());
        }
        // Re-probe only the changed ids.
        let changed_ids: Vec<AppId> = diff
            .added
            .iter()
            .chain(diff.updated.iter())
            .map(|e| e.app_id.clone())
            .collect();

        // Ensure new apps exist in the registry with a slot.
        for entry in diff.added.iter().chain(diff.updated.iter()) {
            if self.registry.get(&entry.app_id).is_none() {
                let slot = self.registry.alloc_slot();
                self.registry.insert(AppRecord {
                    app_id: entry.app_id.clone(),
                    name: entry.name.clone(),
                    link_path: PathBuf::from(&entry.link_path),
                    resolved_target: PathBuf::from(&entry.target_path),
                    slot,
                    icon_state: IconState::Missing,
                    uv: None,
                });
            } else {
                // Existing app: update mutable fields.
                self.registry.update(&entry.app_id, |rec| {
                    rec.name = entry.name.clone();
                    rec.link_path = PathBuf::from(&entry.link_path);
                    rec.resolved_target = PathBuf::from(&entry.target_path);
                });
            }
        }

        // Re-extract (or serve from cache) the changed icons.
        let mut requests = Vec::new();
        for id in &changed_ids {
            let Some(entry) = self.snapshot.get(id) else {
                continue;
            };
            let probe = CacheProbe {
                app_id: id,
                link_mtime: entry.link_mtime,
                target_path: &entry.target_path,
                target_mtime: entry.target_mtime,
                icon_location: &entry.icon_location,
                icon_index: entry.icon_index,
            };
            match self.cache.get_if_valid(&probe) {
                Ok(Some(cached)) => {
                    self.apply_cached_icon(id, cached);
                }
                _ => {
                    self.registry.update(id, |rec| {
                        rec.icon_state = IconState::Loading;
                    });
                    requests.push(IconRequest {
                        app_id: id.clone(),
                        name: entry.name.clone(),
                        link_path: PathBuf::from(&entry.link_path),
                        link_mtime: entry.link_mtime,
                        target_path: entry.target_path.clone(),
                        target_mtime: entry.target_mtime,
                        icon_location: entry.icon_location.clone(),
                        icon_index: entry.icon_index,
                        reason: IconReason::Updated,
                    });
                }
            }
        }

        self.relayout();
        self.request_redraw();

        if let Some(handle) = self._worker.as_ref() {
            for req in requests {
                let _ = handle.requests.send(req);
            }
        }

        // GC cache rows we no longer reference.
        let present: Vec<AppId> = self.snapshot.keys().cloned().collect();
        if let Err(e) = self.cache.retain_and_touch(&present) {
            eprintln!("icon-cache: retain_and_touch failed: {e}");
        }
    }
}

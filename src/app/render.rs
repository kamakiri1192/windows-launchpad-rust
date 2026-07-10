//! Renderer/text/GPU-facing adapter code.
//!
//! Phase 6 will split the renderer facade; this module adapts the layout-layer
//! `LayoutResult` back into the existing renderer upload path.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::bottom_control;
use crate::domain::app_diff::{AppDiff, SnapshotEntry};
use crate::domain::app_id::AppId;
use crate::domain::app_registry::{AppRecord, IconState};
use crate::domain::settings::{Settings, SettingsCategory, SortOrder};
use crate::grid;
use crate::icon_cache::{CacheProbe, CachedIcon};
use crate::icon_pipeline;
use crate::icons::normalize::DecodedIcon;
use crate::layout;
use crate::scroll::{self, Phase};
use crate::startup_timer::prefix;
use crate::text;
use crate::ui_model;
use crate::workers::icon_worker::{IconReason, IconRequest};

use super::state::App;

impl App {
    /// Lay out and upload the bottom control's glass capsule + overlay shapes
    /// and text for the current frame. Call this once per redraw, after the
    /// control has been ticked.
    pub(crate) fn render_bottom_control(
        &mut self,
    ) -> Option<crate::liquid_glass::geometry::GlassShape> {
        // Gather all the immutable data first (avoid overlapping borrows with
        // the mutable renderer/text borrows below).
        let scale = self.scale_factor;
        // Measure the query width exactly via cosmic-text shaping (same pass
        // as drawing), so the caret and IME anchor line up with the glyphs
        // regardless of ASCII/CJK widths. Cached for the frame so
        // `measure_query_width` can read it back under `&self`.
        if let Some(m) = self.text.as_mut() {
            self.cached_query_width = {
                let mut measure = |s: &str| -> f32 {
                    if s.is_empty() {
                        return 0.0;
                    }
                    let spec = text::CenteredLineSpec {
                        text: s,
                        font_size: QUERY_LABEL_SIZE,
                        line_height: QUERY_LABEL_LINE,
                        family: QUERY_LABEL_FONT,
                        color: [1.0, 1.0, 1.0, 1.0],
                        center: (0.0, 0.0),
                        scale_factor: scale,
                    };
                    m.measure_text(&spec)
                };
                // The caret sits after *all visible text*: the committed query
                // plus the in-flight IME preedit. Without the preedit width,
                // the caret stays put while the user types Japanese.
                let w = measure(&self.control.query) + measure(&self.control.preedit);
                Some(w)
            };
            let spec = text::CenteredLineSpec {
                text: DONE_LABEL,
                font_size: QUERY_LABEL_SIZE,
                line_height: QUERY_LABEL_LINE,
                family: QUERY_LABEL_FONT,
                color: [1.0, 1.0, 1.0, 1.0],
                center: (0.0, 0.0),
                scale_factor: scale,
            };
            self.cached_done_width = Some(m.measure_text(&spec));
        }
        let (geom, layers) = self.resolve_control()?;
        let query_width = self.measure_query_width();
        let caret_blink = caret_visibility(&self.control);
        let edit_visual_progress = self.edit_visual_progress();

        // 1) Procedural overlay instances (magnifier, dots, caret, close).
        // While the Done-width morph is active, keep normal pill contents hidden
        // so they don't overflow the narrower capsule.
        let instances = if edit_visual_progress > 0.0 {
            Vec::new()
        } else {
            bottom_control::build_overlay_instances(&geom, &layers, query_width, caret_blink)
        };

        // 2) Text glyphs (label / query / placeholder). Built via the shared
        // text renderer so they share the glyph atlas. Done before touching the
        // renderer so the atlas upload + dirty clear happen in one place.
        let (quads, atlas_dirty) = if let Some(t) = self.text.as_mut() {
            let q = self_layout_control_text(
                t,
                &geom,
                &layers,
                scale,
                &self.control,
                edit_visual_progress,
            );
            (q, t.atlas_dirty)
        } else {
            (Vec::new(), false)
        };
        if atlas_dirty {
            if let Some(t) = self.text.as_mut() {
                t.atlas_dirty = false;
            }
        }

        // 3) Upload the control ink/text and return its glass shape to the
        // caller. `tick_frame` immediately passes it to `render_gear`, keeping
        // the transient GPU-facing value out of persistent app state.
        let control_shape = bottom_control::glass_shape(&geom);
        self.upload_control_overlay(atlas_dirty, &instances, &quads);
        control_shape
    }

    /// Upload the control ink/text. Glass submission waits until
    /// [`render_gear`] has resolved both members of the overlay lane.
    fn upload_control_overlay(
        &mut self,
        atlas_dirty: bool,
        instances: &[bottom_control::ControlInstance],
        quads: &[text::GlyphQuad],
    ) {
        let Some(r) = self.renderer.as_mut() else {
            return;
        };
        if atlas_dirty {
            if let Some(t) = self.text.as_ref() {
                r.upload_atlas(t.atlas_rgba());
            }
        }
        r.set_control_instances(instances);
        r.set_control_text_instances(quads);
    }

    /// Lay out and upload the edit-mode settings gear capsule (the second
    /// capsule shown beside the Done button in edit mode). Hidden at all other
    /// times. See `bottom_control::edit_gear_geometry`. Submits the gear glass
    /// together with the control shape via `set_overlay_glass` (the
    /// Liquid Glass overlay lane), so the control + gear SDF field rebuilds
    /// once per frame.
    pub(crate) fn render_gear(
        &mut self,
        control_shape: Option<crate::liquid_glass::geometry::GlassShape>,
    ) {
        // The gear only appears in edit mode, alongside the Done capsule.
        let edit_progress = self.edit_visual_progress();
        let show = self.visible && edit_progress > 0.0 && !self.settings_panel_active();
        // Resolve the gear geometry once (it yields both the glass shape and
        // the ink instance).
        let gear_geom = if show {
            let viewport = self.viewport_phys();
            let frame_bottom = self.frame_bottom_y();
            let scale = self.scale_factor;
            let done_hw = self
                .cached_done_width
                .map(|w| bottom_control::done_half_width(w, scale))
                .unwrap_or_else(|| bottom_control::done_half_width(0.0, scale));
            bottom_control::edit_gear_geometry(
                viewport,
                frame_bottom,
                scale,
                done_hw,
                edit_progress,
            )
        } else {
            None
        };
        let gear_shape = gear_geom.map(|(geom, _)| bottom_control::edit_gear_glass_shape(&geom));
        let gear_instance =
            gear_geom.map(|(geom, alpha)| bottom_control::edit_gear_instance(&geom, alpha));
        if let Some(r) = self.renderer.as_mut() {
            // Submit the overlay lane in one call: the control capsule and the
            // gear share a Liquid Glass SDF field, so they must be submitted
            // together to composite correctly (merge / separate).
            r.set_overlay_glass(control_shape, gear_shape);
            if let Some(inst) = gear_instance {
                r.set_gear_instances(&[inst]);
            } else {
                r.set_gear_instances(&[]);
            }
        }
    }

    /// Lay out and upload the settings overlay panel (glass + title text +
    /// close ×) for the current frame. No-op when the overlay is closed.
    pub(crate) fn render_settings_panel(&mut self) {
        if !self.settings_panel_active() {
            if let Some(r) = self.renderer.as_mut() {
                // An empty model clears the modal glass lane (prepare detects
                // the signature went non-empty -> empty and submits None), and
                // the ink/text lists are cleared directly. This is the
                // "settings closed → modal glass/controls/text don't linger"
                // path.
                r.prepare(&ui_model::render_model::RenderModel::new());
                r.set_settings_instances(&[]);
                r.set_settings_text_instances(&[]);
            }
            return;
        }

        let scale = self.scale_factor;
        let hidden_count = self.registry.hidden().len();
        let hidden_count_label = format!("{hidden_count} 件");
        let copy = settings_panel_copy(&hidden_count_label);
        let model = layout::settings_panel::build_with_copy(
            layout::settings_panel::SettingsPanelInput {
                viewport: self.viewport_phys(),
                scale_factor: scale,
                category: settings_category_id(self.settings_category),
                sort_order: sort_order_id(self.settings.sort_order),
                frequent_apps_enabled: self.settings.frequent_apps_enabled,
                search_includes_hidden: self.settings.search_includes_hidden,
                hidden_count,
                progress: self.settings_panel_progress,
            },
            &copy,
        );
        let layout = model.layout;
        let visual_scale = model.visual_scale;
        let visual_alpha = model.visual_alpha;

        // Close × glyph at the top-right inset.
        let btn_r = layout::settings_panel::CLOSE_HALF * scale;
        let close = control_icon(
            layout.left + layout.hw * 2.0 - btn_r * 2.0,
            layout.top + btn_r * 2.0,
            btn_r,
            bottom_control::KIND_CLOSE,
            layout::settings_panel::INK,
        );

        let mut instances = Vec::new();
        let mut quads = Vec::new();
        let current_settings = self.settings.clone();
        let current_category = self.settings_category;

        build_settings_panel_instances(
            &layout,
            scale,
            current_category,
            &current_settings,
            hidden_count,
            &mut instances,
        );
        instances.push(close);

        if let Some(t) = self.text.as_mut() {
            build_settings_panel_text_views(t, &model.result.render.text, scale, &mut quads);
        }

        transform_settings_instances(
            &mut instances,
            [layout.cx, layout.cy],
            visual_scale,
            visual_alpha,
        );
        transform_settings_quads(
            &mut quads,
            [layout.cx, layout.cy],
            visual_scale,
            visual_alpha,
        );

        if let Some(r) = self.renderer.as_mut() {
            // The panel glass surface is produced by the layout layer as a
            // renderer-neutral `GlassSurface` (visual_scale already applied).
            // Route it through the facade's `prepare` instead of recomputing
            // the shader shape here; `prepare` dirty-tracks it so a frame
            // whose glass didn't move (settled settings panel) re-submits
            // nothing.
            r.prepare(&model.result.render);
            r.set_settings_instances(&instances);
            // Upload the atlas if the title added any glyphs this frame.
            if let Some(t) = self.text.as_ref() {
                if t.atlas_dirty {
                    r.upload_atlas(t.atlas_rgba());
                }
            }
            r.set_settings_text_instances(&quads);
        }
        if let Some(t) = self.text.as_mut() {
            t.atlas_dirty = false;
        }
    }

    pub(crate) fn step_edit_control_width(&mut self, dt: f32) -> bool {
        let target = if self.editing { 1.0 } else { 0.0 };
        let duration = if self.editing {
            bottom_control::EXPAND_DURATION
        } else {
            bottom_control::COLLAPSE_DURATION
        };
        let before = self.edit_control_progress;
        self.edit_control_progress =
            advance_unit_toward(self.edit_control_progress, target, dt, duration);
        (self.edit_control_progress - before).abs() > 0.0001
            || (self.edit_control_progress - target).abs() > 0.0001
    }

    pub(crate) fn step_settings_panel(&mut self, dt: f32) -> bool {
        let target = if self.settings_open { 1.0 } else { 0.0 };
        let duration = if self.settings_open {
            layout::settings_panel::OPEN_DURATION
        } else {
            layout::settings_panel::CLOSE_DURATION
        };
        let before = self.settings_panel_progress;
        self.settings_panel_progress =
            advance_unit_toward(self.settings_panel_progress, target, dt, duration);
        if !self.settings_open && self.settings_panel_progress < 0.001 {
            self.settings_panel_progress = 0.0;
        }
        (self.settings_panel_progress - before).abs() > 0.0001
            || (self.settings_panel_progress - target).abs() > 0.0001
    }

    /// Rebuild visible search results and redraw immediately after any input
    /// mutation. Keeps text input, IME composition, tiles, labels, click
    /// resolution, and scroll bounds in one state transition.
    pub(crate) fn search_input_changed(&mut self) {
        self.relayout();
        let (w, _h) = self.viewport_phys();
        let bounds = self.layout.bounds(w as f32);
        if let Some(s) = self.scroller.as_mut() {
            s.position = bounds.snap_target(s.position);
            s.velocity = 0.0;
            s.phase = Phase::Idle;
        }
        self.last_page = self.current_page() as i32;
        self.request_redraw();
    }

    /// Recompute layout/bounds for the current window size and push tile +
    /// label + icon instance buffers to the GPU.
    pub(crate) fn relayout(&mut self) {
        let (w, _h) = self.viewport_phys();
        let owned = self.grid_apps_owned();
        // Size pages to the current visible app count so every filtered app is
        // reachable and blank trailing pages disappear during search.
        self.layout = grid::GridLayout::for_app_count(owned.len())
            .with_scale_factor(self.scale_factor)
            .centered(w as f32);
        let bounds = self.layout.bounds(w as f32);
        if let Some(s) = self.scroller.as_mut() {
            s.set_bounds(bounds);
        }

        let apps: Vec<grid::GridApp<'_>> = owned
            .iter()
            .map(|(name, uv)| grid::GridApp {
                name: name.as_str(),
                uv: *uv,
            })
            .collect();

        // Text labels.
        let scale = self.scale_factor;
        let dirty = if let Some(t) = self.text.as_mut() {
            let labels = self.layout.build_labels(w as f32, &apps);
            let quads = t.layout_labels(&labels, scale);
            let dirty = t.atlas_dirty;
            if let Some(r) = self.renderer.as_mut() {
                r.set_text_instances(&quads);
                if dirty {
                    r.upload_atlas(t.atlas_rgba());
                }
            }
            dirty
        } else {
            false
        };
        if dirty {
            if let Some(t) = self.text.as_mut() {
                t.atlas_dirty = false;
            }
        }

        let visible_ids = self.visible_app_ids();
        let anim = self.edit_anim(&visible_ids);
        // Update the per-tile position springs to the new home cells (keeping
        // each spring's current value so tiles glide from where they were).
        self.update_tile_springs(&visible_ids, w as f32);
        // Build the instances and override each tile's position with its spring
        // value so a reorder (or relayout) animates the icons sliding into place
        // rather than snapping. Done before the renderer borrow so we can read
        // the springs under &self.
        let mut tile_instances = self.layout.build_instances(w as f32, &apps, &anim);
        self.apply_spring_positions(&visible_ids, &mut tile_instances);
        let mut icon_instances = self.layout.build_icon_instances(w as f32, &apps, &anim);
        self.apply_spring_positions(&visible_ids, &mut icon_instances);
        // While dragging, lift the dragged app off the grid: remove it from the
        // normal instance list and append a pointer-following copy at the end so
        // it draws on top of everything else.
        self.lift_dragged_instances(&mut tile_instances, &mut icon_instances, &visible_ids);
        if let Some(r) = self.renderer.as_mut() {
            // The liquid-glass shape rebuild uses the resting positions (the
            // glass doesn't need to follow the slide); the tile/icon instance
            // buffers carry the spring-adjusted positions.
            r.rebuild_instances(&self.layout, &apps, &anim);
            r.set_tile_instances(&tile_instances);
            r.set_icon_instances(&icon_instances);
        }

        let atlas_grew = self.ensure_atlas_uploaded();
        if atlas_grew {
            // Growing the atlas changes UVs for icons that were already cached
            // before this relayout, so refresh the icon instance buffer once
            // more after the registry has been re-synced.
            self.rebuild_icon_instances();
        }
    }

    /// Upload (or grow) the GPU icon atlas to match the CPU atlas, then push
    /// the full pixel buffer once after the first allocation. Subsequent
    /// per-icon updates go through [`apply_icon`][Self::apply_icon].
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
            .map(|(name, uv)| grid::GridApp {
                name: name.as_str(),
                uv: *uv,
            })
            .collect();
        let (w, _h) = self.viewport_phys();
        let visible_ids = self.visible_app_ids();
        let anim = self.edit_anim(&visible_ids);
        let mut icon_instances = self.layout.build_icon_instances(w as f32, &apps, &anim);
        self.apply_spring_positions(&visible_ids, &mut icon_instances);
        if let Some(r) = self.renderer.as_mut() {
            r.set_icon_instances(&icon_instances);
        }
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

    /// Build the per-app edit-mode animation parameters. Each visible app gets
    /// a `TileAnim`; outside edit mode they're all `IDLE`.
    ///
    /// - In edit mode, every app wiggles (FLAG_WIGGLE + a per-app phase offset
    ///   so they don't all swing in lockstep).
    /// - The app being dragged (if any) is lifted (`lift`), enlarged (`scale`),
    ///   and flagged `FLAG_DRAG` so the shader bypasses the frame clip and
    ///   follows the pointer instead of its home cell.
    pub(crate) fn edit_anim(&self, visible_ids: &[AppId]) -> Vec<grid::TileAnim> {
        if !self.editing {
            return Vec::new();
        }
        let drag_id = self.drag_app.as_ref();
        visible_ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let is_drag = drag_id.map(|d| d == id).unwrap_or(false);
                if is_drag {
                    grid::TileAnim {
                        phase: self.wiggle_phase + i as f32 * 0.37,
                        lift: 24.0 * self.scale_factor.max(1.0),
                        scale: 1.15,
                        flags: grid::TileAnim::FLAG_WIGGLE | grid::TileAnim::FLAG_DRAG,
                    }
                } else {
                    grid::TileAnim {
                        phase: self.wiggle_phase + i as f32 * 0.37,
                        lift: 0.0,
                        scale: 1.0,
                        flags: grid::TileAnim::FLAG_WIGGLE,
                    }
                }
            })
            .collect()
    }

    /// Realign `tile_springs` with the current visible app set. Existing
    /// springs are matched by `AppId`, not position, so a reordered app keeps
    /// its previous cell as the spring value and glides to its new home cell.
    pub(crate) fn update_tile_springs(&mut self, visible_ids: &[AppId], viewport_w: f32) {
        let mut old = std::mem::take(&mut self.tile_springs);
        self.tile_springs.reserve(visible_ids.len());
        for (i, id) in visible_ids.iter().enumerate() {
            let (x, y) = self.layout.tile_position(viewport_w, i);
            if let Some(pos) = old.iter().position(|(spring_id, _)| spring_id == id) {
                let (_, mut spring) = old.swap_remove(pos);
                spring.glide_to(x, y);
                self.tile_springs.push((id.clone(), spring));
            } else {
                self.tile_springs
                    .push((id.clone(), scroll::Spring2::at(x, y)));
            }
        }
    }

    /// Override each instance's position with its spring value, so the tile
    /// slides from where it was toward its home cell. Works for both
    /// `TileInstance` and `IconInstance` via the [`SpringPos`] trait.
    pub(crate) fn apply_spring_positions<T: SpringPos>(
        &self,
        visible_ids: &[AppId],
        instances: &mut [T],
    ) {
        for (id, inst) in visible_ids.iter().zip(instances.iter_mut()) {
            if let Some((_, spring)) = self
                .tile_springs
                .iter()
                .find(|(spring_id, _)| spring_id == id)
            {
                inst.set_pos(spring.x.value, spring.y.value);
            }
        }
    }

    /// While an edit-mode drag is in flight, move the dragged app's tile + icon
    /// to the end of the instance lists so it draws on top of everything else —
    /// but keep it as the *same* instance, not a duplicate. The shader uses
    /// `drag_pos` to make that trailing instance follow the pointer.
    pub(crate) fn lift_dragged_instances(
        &self,
        tile_instances: &mut Vec<grid::TileInstance>,
        icon_instances: &mut Vec<crate::icon_pipeline::IconInstance>,
        _visible_ids: &[AppId],
    ) {
        let is_drag = |flags: f32| (flags as u32 & grid::TileAnim::FLAG_DRAG) != 0;

        if let Some(pos) = tile_instances.iter().position(|t| is_drag(t.extra[3])) {
            let item = tile_instances.swap_remove(pos);
            tile_instances.push(item);
        }
        if let Some(pos) = icon_instances.iter().position(|i| is_drag(i.extra[3])) {
            let item = icon_instances.swap_remove(pos);
            icon_instances.push(item);
        }
    }

    /// Advance every tile position spring by `dt`. Returns `true` while any
    /// spring is still animating (so the caller keeps redrawing).
    pub(crate) fn step_tile_springs(&mut self, dt: f32) -> bool {
        let cfg = self.scroller.as_ref().map(|s| s.cfg).unwrap_or_default();
        let mut any = false;
        for (_, s) in &mut self.tile_springs {
            if s.step(dt, &cfg) {
                any = true;
            }
        }
        any
    }

    /// Rebuild + re-push the tile/icon instance buffers using the current
    /// spring positions, without recomputing the layout. Called every frame
    /// while the springs are animating so the slide is visible.
    pub(crate) fn refresh_spring_instances(&mut self) {
        let owned = self.grid_apps_owned();
        let apps: Vec<grid::GridApp<'_>> = owned
            .iter()
            .map(|(name, uv)| grid::GridApp {
                name: name.as_str(),
                uv: *uv,
            })
            .collect();
        let (w, _h) = self.viewport_phys();
        let visible_ids = self.visible_app_ids();
        let anim = self.edit_anim(&visible_ids);
        let mut tile_instances = self.layout.build_instances(w as f32, &apps, &anim);
        self.apply_spring_positions(&visible_ids, &mut tile_instances);
        let mut icon_instances = self.layout.build_icon_instances(w as f32, &apps, &anim);
        self.apply_spring_positions(&visible_ids, &mut icon_instances);
        self.lift_dragged_instances(&mut tile_instances, &mut icon_instances, &visible_ids);
        if let Some(r) = self.renderer.as_mut() {
            r.set_tile_instances(&tile_instances);
            r.set_icon_instances(&icon_instances);
        }
    }

    /// The center Y of the control capsule (for hit-testing the close button).
    pub(crate) fn frame_control_cy(&self) -> f32 {
        self.resolve_control()
            .map(|(geom, _)| geom.center.1)
            .unwrap_or(0.0)
    }

    /// Keep the OS IME in sync with the search field: enable it (and point the
    /// composition window at the caret) while the field is focused, disable it
    /// otherwise. Called every frame; `set_ime_allowed` is cheap.
    pub(crate) fn update_ime_state(&self) {
        let Some(r) = self.renderer.as_ref() else {
            return;
        };
        let want_ime = self.control.wants_keyboard();
        r.window.set_ime_allowed(want_ime);
        if want_ime {
            // Park the IME composition window at the caret so Japanese/IME
            // candidates appear right next to the typed text.
            let scale = self.scale_factor;
            let caret_x = self.control_caret_screen_x();
            let caret_y = self.frame_control_cy();
            r.window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(caret_x as f64, caret_y as f64),
                winit::dpi::PhysicalSize::new(1.0, (16.0 * scale) as f64),
            );
        }
    }

    /// Screen-space X of the text caret inside the search field (physical px),
    /// used to anchor the IME composition window.
    pub(crate) fn control_caret_screen_x(&self) -> f32 {
        let Some((geom, _)) = self.resolve_control() else {
            return 0.0;
        };
        let origin = bottom_control::field_text_origin_x(&geom);
        origin + self.measure_query_width()
    }
}

// ---- settings helper free functions ---------------------------------------

pub(crate) fn settings_category_id(
    category: SettingsCategory,
) -> layout::settings_panel::SettingsCategoryId {
    match category {
        SettingsCategory::Apps => layout::settings_panel::SettingsCategoryId::Apps,
        SettingsCategory::Search => layout::settings_panel::SettingsCategoryId::Search,
        SettingsCategory::System => layout::settings_panel::SettingsCategoryId::System,
        SettingsCategory::About => layout::settings_panel::SettingsCategoryId::About,
    }
}

pub(crate) fn settings_category_from_id(
    category: layout::settings_panel::SettingsCategoryId,
) -> SettingsCategory {
    match category {
        layout::settings_panel::SettingsCategoryId::Apps => SettingsCategory::Apps,
        layout::settings_panel::SettingsCategoryId::Search => SettingsCategory::Search,
        layout::settings_panel::SettingsCategoryId::System => SettingsCategory::System,
        layout::settings_panel::SettingsCategoryId::About => SettingsCategory::About,
    }
}

pub(crate) fn sort_order_id(order: SortOrder) -> layout::settings_panel::SortOrderId {
    match order {
        SortOrder::Name => layout::settings_panel::SortOrderId::Name,
        SortOrder::Manual => layout::settings_panel::SortOrderId::Manual,
        SortOrder::Recent => layout::settings_panel::SortOrderId::Recent,
        SortOrder::Frequent => layout::settings_panel::SortOrderId::Frequent,
    }
}

pub(crate) fn sort_order_from_id(order: layout::settings_panel::SortOrderId) -> SortOrder {
    match order {
        layout::settings_panel::SortOrderId::Name => SortOrder::Name,
        layout::settings_panel::SortOrderId::Manual => SortOrder::Manual,
        layout::settings_panel::SortOrderId::Recent => SortOrder::Recent,
        layout::settings_panel::SortOrderId::Frequent => SortOrder::Frequent,
    }
}

fn settings_panel_copy<'a>(
    hidden_count_label: &'a str,
) -> layout::settings_panel::SettingsPanelCopy<'a> {
    layout::settings_panel::SettingsPanelCopy {
        title: SETTINGS_TITLE,
        categories: [
            (
                layout::settings_panel::SettingsCategoryId::Apps,
                SettingsCategory::Apps.label(),
            ),
            (
                layout::settings_panel::SettingsCategoryId::Search,
                SettingsCategory::Search.label(),
            ),
            (
                layout::settings_panel::SettingsCategoryId::System,
                SettingsCategory::System.label(),
            ),
            (
                layout::settings_panel::SettingsCategoryId::About,
                SettingsCategory::About.label(),
            ),
        ],
        sort_orders: [
            (
                layout::settings_panel::SortOrderId::Name,
                SortOrder::Name.label(),
            ),
            (
                layout::settings_panel::SortOrderId::Manual,
                SortOrder::Manual.label(),
            ),
            (
                layout::settings_panel::SortOrderId::Recent,
                SortOrder::Recent.label(),
            ),
            (
                layout::settings_panel::SortOrderId::Frequent,
                SortOrder::Frequent.label(),
            ),
        ],
        sort_label: "並び順",
        frequent_apps_label: "よく使うアプリ",
        frequent_apps_detail: "ホーム画面に表示するための準備設定",
        hidden_apps_label: "非表示アプリ",
        hidden_count_label,
        search_hidden_label: "検索時に非表示アプリを含める",
        search_hidden_detail: "検索中だけ、隠したアプリも結果に表示します",
        reset_cache_label: "キャッシュをリセット",
        reset_cache_detail: "アイコンを再抽出します",
        reset_settings_label: "設定をリセット",
        reset_settings_detail: "並び順、非表示、設定値を初期状態に戻します",
        version_label: "バージョン",
        version_value: env!("CARGO_PKG_VERSION"),
    }
}

pub(crate) fn settings_press_target_from_layout_hit(
    hit: layout::settings_panel::SettingsPanelHit,
) -> super::state::SettingsPressTarget {
    match hit {
        layout::settings_panel::SettingsPanelHit::Close => super::state::SettingsPressTarget::Close,
        layout::settings_panel::SettingsPanelHit::Category(category) => {
            super::state::SettingsPressTarget::Category(settings_category_from_id(category))
        }
        layout::settings_panel::SettingsPanelHit::Sort(order) => {
            super::state::SettingsPressTarget::Sort(sort_order_from_id(order))
        }
        layout::settings_panel::SettingsPanelHit::FrequentToggle => {
            super::state::SettingsPressTarget::FrequentToggle
        }
        layout::settings_panel::SettingsPanelHit::SearchHiddenToggle => {
            super::state::SettingsPressTarget::SearchHiddenToggle
        }
        layout::settings_panel::SettingsPanelHit::ResetCache => {
            super::state::SettingsPressTarget::ResetCache
        }
        layout::settings_panel::SettingsPanelHit::ResetSettings => {
            super::state::SettingsPressTarget::ResetSettings
        }
        layout::settings_panel::SettingsPanelHit::Inside => {
            super::state::SettingsPressTarget::Inside
        }
        layout::settings_panel::SettingsPanelHit::Outside => {
            super::state::SettingsPressTarget::Outside
        }
    }
}

/// Scale a color's alpha by `a` (used to cross-fade control text layers).
fn mul_alpha(mut c: [f32; 4], a: f32) -> [f32; 4] {
    c[3] *= a.clamp(0.0, 1.0);
    c
}

/// Trait for instance types that carry an `(x, y)` position we can rewrite in
/// place — used by the reorder animation to override a tile/icon's home cell
/// with its spring value.
pub(crate) trait SpringPos {
    fn set_pos(&mut self, x: f32, y: f32);
}

impl SpringPos for grid::TileInstance {
    fn set_pos(&mut self, x: f32, y: f32) {
        self.x = x;
        self.y = y;
    }
}

impl SpringPos for icon_pipeline::IconInstance {
    fn set_pos(&mut self, x: f32, y: f32) {
        self.x = x;
        self.y = y;
    }
}

// Shared font metrics for the bottom-control text (label / query / placeholder
// / preedit), so measuring and drawing use identical shaping parameters.
const QUERY_LABEL_FONT: &str = "Yu Gothic UI";
const QUERY_LABEL_SIZE: f32 = 13.0;
const QUERY_LABEL_LINE: f32 = 18.0;
const DONE_LABEL: &str = "完了";

// ---- settings overlay (placeholder panel) ----------------------------------

/// Title shown in the placeholder settings panel.
const SETTINGS_TITLE: &str = "設定";
/// Title font for the settings panel.
const SETTINGS_TITLE_FONT: &str = "Yu Gothic UI";
const SETTINGS_SIDEBAR_W: f32 = 210.0;
const SETTINGS_SIDEBAR_TOP: f32 = 78.0;
const SETTINGS_SIDEBAR_ROW_H: f32 = 38.0;
const SETTINGS_SIDEBAR_STEP: f32 = 44.0;
const SETTINGS_CONTENT_PAD: f32 = 34.0;
const SETTINGS_CONTENT_TOP: f32 = 92.0;
const SETTINGS_ROW_H: f32 = 46.0;
const SETTINGS_ROW_STEP: f32 = 62.0;
const SETTINGS_SEGMENT_H: f32 = 32.0;
const SETTINGS_SEGMENT_GAP: f32 = 8.0;
const SETTINGS_INK: [f32; 4] = [1.0, 1.0, 1.0, 0.92];
const SETTINGS_MUTED: [f32; 4] = [1.0, 1.0, 1.0, 0.58];
const SETTINGS_DIM: [f32; 4] = [1.0, 1.0, 1.0, 0.34];
const SETTINGS_ACCENT: [f32; 4] = [0.35, 0.68, 1.0, 0.42];
const SETTINGS_GREEN: [f32; 4] = [0.28, 0.82, 0.48, 0.78];

fn transform_settings_instances(
    instances: &mut [bottom_control::ControlInstance],
    origin: [f32; 2],
    scale: f32,
    alpha: f32,
) {
    for instance in instances {
        instance.center[0] = origin[0] + (instance.center[0] - origin[0]) * scale;
        instance.center[1] = origin[1] + (instance.center[1] - origin[1]) * scale;
        instance.params[0] *= scale;
        instance.params[2] *= scale;
        instance.params[3] *= scale;
        instance.params[1] *= alpha;
        instance.color[3] *= alpha;
    }
}

fn transform_settings_quads(
    quads: &mut [text::GlyphQuad],
    origin: [f32; 2],
    scale: f32,
    alpha: f32,
) {
    for quad in quads {
        quad.x = origin[0] + (quad.x - origin[0]) * scale;
        quad.y = origin[1] + (quad.y - origin[1]) * scale;
        quad.w *= scale;
        quad.h *= scale;
        quad.color[3] *= alpha;
    }
}

fn control_icon(
    x: f32,
    y: f32,
    radius: f32,
    kind: f32,
    color: [f32; 4],
) -> bottom_control::ControlInstance {
    bottom_control::ControlInstance {
        center: [x, y],
        params: [radius, color[3], 1.6, 0.0],
        color,
        kind: [kind, 0.0, 0.0, 0.0],
    }
}

fn round_rect_instance(
    center: [f32; 2],
    half_width: f32,
    half_height: f32,
    radius: f32,
    color: [f32; 4],
) -> bottom_control::ControlInstance {
    bottom_control::ControlInstance {
        center,
        params: [half_height, color[3], half_width, radius],
        color,
        kind: [bottom_control::KIND_ROUND_RECT, 0.0, 0.0, 0.0],
    }
}

fn divider_instance(
    center: [f32; 2],
    half_width: f32,
    half_height: f32,
) -> bottom_control::ControlInstance {
    round_rect_instance(center, half_width, half_height, half_height, SETTINGS_DIM)
}

fn toggle_instances(
    center: [f32; 2],
    enabled: bool,
    scale: f32,
    instances: &mut Vec<bottom_control::ControlInstance>,
) {
    let track_hw = 22.0 * scale;
    let track_hh = 11.0 * scale;
    let track_color = if enabled {
        SETTINGS_GREEN
    } else {
        [1.0, 1.0, 1.0, 0.14]
    };
    instances.push(round_rect_instance(
        center,
        track_hw,
        track_hh,
        track_hh,
        track_color,
    ));
    if enabled {
        instances.push(control_icon(
            center[0] + 10.0 * scale,
            center[1],
            6.0 * scale,
            bottom_control::KIND_DOT,
            [1.0, 1.0, 1.0, 0.78],
        ));
    }
}

fn build_settings_panel_instances(
    layout: &crate::layout::settings_panel::SettingsPanelLayout,
    scale: f32,
    category: SettingsCategory,
    settings: &Settings,
    _hidden_count: usize,
    instances: &mut Vec<bottom_control::ControlInstance>,
) {
    let panel_right = layout.left + layout.hw * 2.0;
    let panel_bottom = layout.top + layout.hh * 2.0;
    instances.push(divider_instance(
        [layout.right_left, layout.cy],
        0.55 * scale,
        layout.hh - 28.0 * scale,
    ));

    for (i, item) in SettingsCategory::ALL.iter().copied().enumerate() {
        if item == category {
            let row_top = layout.top
                + SETTINGS_SIDEBAR_TOP * scale
                + i as f32 * SETTINGS_SIDEBAR_STEP * scale;
            instances.push(round_rect_instance(
                [
                    layout.left + layout.sidebar_w * 0.5,
                    row_top + SETTINGS_SIDEBAR_ROW_H * scale * 0.5,
                ],
                layout.sidebar_w * 0.5 - 14.0 * scale,
                SETTINGS_SIDEBAR_ROW_H * scale * 0.5,
                10.0 * scale,
                SETTINGS_ACCENT,
            ));
        }
    }

    let content_left = layout.right_left + SETTINGS_CONTENT_PAD * scale;
    let content_right = panel_right - SETTINGS_CONTENT_PAD * scale;
    let row_w = content_right - content_left;
    let row_h = SETTINGS_ROW_H * scale;
    let first_top = layout.top + SETTINGS_CONTENT_TOP * scale;

    match category {
        SettingsCategory::Apps => {
            let segment_top = first_top + 44.0 * scale;
            let gap = SETTINGS_SEGMENT_GAP * scale;
            let each_w = (row_w - gap * 3.0) / 4.0;
            for (i, order) in SortOrder::ALL.iter().copied().enumerate() {
                let left = content_left + i as f32 * (each_w + gap);
                let selected = settings.sort_order == order;
                instances.push(round_rect_instance(
                    [
                        left + each_w * 0.5,
                        segment_top + SETTINGS_SEGMENT_H * scale * 0.5,
                    ],
                    each_w * 0.5,
                    SETTINGS_SEGMENT_H * scale * 0.5,
                    10.0 * scale,
                    if selected {
                        SETTINGS_ACCENT
                    } else {
                        [1.0, 1.0, 1.0, 0.14]
                    },
                ));
                if selected {
                    instances.push(control_icon(
                        left + 15.0 * scale,
                        segment_top + SETTINGS_SEGMENT_H * scale * 0.5,
                        8.0 * scale,
                        bottom_control::KIND_CHECK,
                        SETTINGS_INK,
                    ));
                }
            }
            settings_row_backgrounds(
                content_left,
                first_top + SETTINGS_ROW_STEP * scale,
                row_w,
                row_h,
                scale,
                instances,
                2,
            );
            toggle_instances(
                [
                    content_right - 28.0 * scale,
                    first_top + SETTINGS_ROW_STEP * scale + row_h * 0.5,
                ],
                settings.frequent_apps_enabled,
                scale,
                instances,
            );
            instances.push(control_icon(
                content_right - 14.0 * scale,
                first_top + SETTINGS_ROW_STEP * 2.0 * scale + row_h * 0.5,
                9.0 * scale,
                bottom_control::KIND_CHEVRON,
                SETTINGS_MUTED,
            ));
        }
        SettingsCategory::Search => {
            settings_row_backgrounds(content_left, first_top, row_w, row_h, scale, instances, 1);
            toggle_instances(
                [content_right - 28.0 * scale, first_top + row_h * 0.5],
                settings.search_includes_hidden,
                scale,
                instances,
            );
        }
        SettingsCategory::System => {
            settings_row_backgrounds(content_left, first_top, row_w, row_h, scale, instances, 2);
            for i in 0..2 {
                instances.push(control_icon(
                    content_right - 14.0 * scale,
                    first_top + i as f32 * SETTINGS_ROW_STEP * scale + row_h * 0.5,
                    9.0 * scale,
                    bottom_control::KIND_CHEVRON,
                    SETTINGS_MUTED,
                ));
            }
        }
        SettingsCategory::About => {
            settings_row_backgrounds(content_left, first_top, row_w, row_h, scale, instances, 1);
        }
    }

    instances.push(divider_instance(
        [layout.cx, panel_bottom - 56.0 * scale],
        layout.hw - 26.0 * scale,
        0.45 * scale,
    ));
}

fn settings_row_backgrounds(
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    scale: f32,
    instances: &mut Vec<bottom_control::ControlInstance>,
    count: usize,
) {
    for i in 0..count {
        instances.push(round_rect_instance(
            [
                left + width * 0.5,
                top + i as f32 * SETTINGS_ROW_STEP * scale + height * 0.5,
            ],
            width * 0.5,
            height * 0.5,
            12.0 * scale,
            [1.0, 1.0, 1.0, 0.12],
        ));
    }
}

fn build_settings_panel_text_views(
    t: &mut text::TextRenderer,
    views: &[ui_model::text::TextView],
    scale: f32,
    quads: &mut Vec<text::GlyphQuad>,
) {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    for view in views {
        let color = [
            view.style.color.r,
            view.style.color.g,
            view.style.color.b,
            view.style.color.a,
        ];
        let center_y = view.rect.center().y;
        let line_height = view.rect.height / scale;
        match view.style.align {
            ui_model::text::TextAlign::Start => push_text_left(
                t,
                quads,
                &view.text,
                view.rect.x,
                center_y,
                view.style.size,
                line_height,
                color,
                scale,
            ),
            ui_model::text::TextAlign::End => push_text_right(
                t,
                quads,
                &view.text,
                view.rect.x,
                center_y,
                view.style.size,
                line_height,
                color,
                scale,
            ),
            ui_model::text::TextAlign::Center => {
                quads.append(&mut t.layout_centered_line(&text::CenteredLineSpec {
                    text: &view.text,
                    font_size: view.style.size,
                    line_height,
                    family: SETTINGS_TITLE_FONT,
                    color,
                    center: (view.rect.center().x, center_y),
                    scale_factor: scale,
                }));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_text_left(
    t: &mut text::TextRenderer,
    quads: &mut Vec<text::GlyphQuad>,
    value: &str,
    left: f32,
    center_y: f32,
    font_size: f32,
    line_height: f32,
    color: [f32; 4],
    scale: f32,
) {
    let width = t.measure_text(&text::CenteredLineSpec {
        text: value,
        font_size,
        line_height,
        family: SETTINGS_TITLE_FONT,
        color,
        center: (0.0, 0.0),
        scale_factor: scale,
    });
    quads.append(&mut t.layout_centered_line(&text::CenteredLineSpec {
        text: value,
        font_size,
        line_height,
        family: SETTINGS_TITLE_FONT,
        color,
        center: (left + width * 0.5, center_y),
        scale_factor: scale,
    }));
}

#[allow(clippy::too_many_arguments)]
fn push_text_right(
    t: &mut text::TextRenderer,
    quads: &mut Vec<text::GlyphQuad>,
    value: &str,
    right: f32,
    center_y: f32,
    font_size: f32,
    line_height: f32,
    color: [f32; 4],
    scale: f32,
) {
    let width = t.measure_text(&text::CenteredLineSpec {
        text: value,
        font_size,
        line_height,
        family: SETTINGS_TITLE_FONT,
        color,
        center: (0.0, 0.0),
        scale_factor: scale,
    });
    quads.append(&mut t.layout_centered_line(&text::CenteredLineSpec {
        text: value,
        font_size,
        line_height,
        family: SETTINGS_TITLE_FONT,
        color,
        center: (right - width * 0.5, center_y),
        scale_factor: scale,
    }));
}

/// Build the text glyph quads for the control's active layers. A free function
/// (not a method) so it can borrow `&mut TextRenderer` and `&BottomControl`
/// without colliding with the renderer borrow in `render_bottom_control`.
fn self_layout_control_text(
    t: &mut text::TextRenderer,
    geom: &bottom_control::ControlGeometry,
    layers: &[bottom_control::ControlLayer],
    scale: f32,
    control: &bottom_control::BottomControl,
    edit_visual_progress: f32,
) -> Vec<text::GlyphQuad> {
    let mut quads = Vec::new();
    const INK: [f32; 4] = [1.0, 1.0, 1.0, 0.92];
    const PLACEHOLDER: [f32; 4] = [1.0, 1.0, 1.0, 0.45];
    /// Preedit (in-flight IME composition) is shown slightly dimmer to hint
    /// it isn't committed yet.
    const PREEDIT_INK: [f32; 4] = [0.85, 0.92, 1.0, 0.88];

    // While edit-mode width is morphing, keep the Done label centered and skip
    // normal pill/indicator/field content so it cannot overflow the capsule.
    if edit_visual_progress > 0.0 {
        let mut q = t.layout_centered_line(&text::CenteredLineSpec {
            text: DONE_LABEL,
            font_size: QUERY_LABEL_SIZE,
            line_height: QUERY_LABEL_LINE,
            family: QUERY_LABEL_FONT,
            color: mul_alpha(INK, edit_visual_progress.clamp(0.0, 1.0)),
            center: (geom.center.0, geom.center.1),
            scale_factor: scale,
        });
        quads.append(&mut q);
        return quads;
    }

    for layer in layers {
        let a = layer.alpha;
        if a <= 0.01 {
            continue;
        }
        match layer.visual {
            bottom_control::Visual::SearchPill => {
                // "検索" label to the right of the magnifier.
                let (_, label_center_x) = bottom_control::search_pill_content_centers(geom);
                let mut q = t.layout_centered_line(&text::CenteredLineSpec {
                    text: "検索",
                    font_size: QUERY_LABEL_SIZE,
                    line_height: QUERY_LABEL_LINE,
                    family: QUERY_LABEL_FONT,
                    color: mul_alpha(INK, a),
                    center: (label_center_x, geom.center.1),
                    scale_factor: scale,
                });
                quads.append(&mut q);
            }
            bottom_control::Visual::PageIndicator => {
                // No text.
            }
            bottom_control::Visual::SearchField => {
                let origin_x = bottom_control::field_text_origin_x(geom);
                if control.query.is_empty() && control.preedit.is_empty() {
                    let mut q = t.layout_centered_line(&text::CenteredLineSpec {
                        text: "検索",
                        font_size: QUERY_LABEL_SIZE,
                        line_height: QUERY_LABEL_LINE,
                        family: QUERY_LABEL_FONT,
                        color: mul_alpha(PLACEHOLDER, a),
                        center: (origin_x + 14.0 * scale, geom.center.1),
                        scale_factor: scale,
                    });
                    quads.append(&mut q);
                } else {
                    // Render the committed query plus the in-flight IME
                    // preedit inline. The preedit is shown with an
                    // underline tint so the user can tell it's not yet
                    // committed. Widths are measured exactly (same shaping as
                    // drawing) so the caret / preedit line up with the glyphs.
                    let query_w = if control.query.is_empty() {
                        0.0
                    } else {
                        t.measure_text(&text::CenteredLineSpec {
                            text: &control.query,
                            font_size: QUERY_LABEL_SIZE,
                            line_height: QUERY_LABEL_LINE,
                            family: QUERY_LABEL_FONT,
                            color: INK,
                            center: (0.0, 0.0),
                            scale_factor: scale,
                        })
                    };
                    let preedit_w = if control.preedit.is_empty() {
                        0.0
                    } else {
                        t.measure_text(&text::CenteredLineSpec {
                            text: &control.preedit,
                            font_size: QUERY_LABEL_SIZE,
                            line_height: QUERY_LABEL_LINE,
                            family: QUERY_LABEL_FONT,
                            color: PREEDIT_INK,
                            center: (0.0, 0.0),
                            scale_factor: scale,
                        })
                    };
                    // Committed text: left-anchored at origin_x, so center on
                    // its own half-width.
                    if query_w > 0.0 {
                        let mut q = t.layout_centered_line(&text::CenteredLineSpec {
                            text: &control.query,
                            font_size: QUERY_LABEL_SIZE,
                            line_height: QUERY_LABEL_LINE,
                            family: QUERY_LABEL_FONT,
                            color: mul_alpha(INK, a),
                            center: (origin_x + query_w * 0.5, geom.center.1),
                            scale_factor: scale,
                        });
                        quads.append(&mut q);
                    }
                    // Preedit, starting right after the committed query.
                    if preedit_w > 0.0 {
                        let preedit_origin = origin_x + query_w;
                        let mut q = t.layout_centered_line(&text::CenteredLineSpec {
                            text: &control.preedit,
                            font_size: QUERY_LABEL_SIZE,
                            line_height: QUERY_LABEL_LINE,
                            family: QUERY_LABEL_FONT,
                            color: mul_alpha(PREEDIT_INK, a),
                            center: (preedit_origin + preedit_w * 0.5, geom.center.1),
                            scale_factor: scale,
                        });
                        quads.append(&mut q);
                    }
                }
            }
        }
    }
    quads
}

/// Caret blink visibility for this frame. The caret shows ~57% of a ~1.06s
/// cycle, in sync with the control's `caret_phase`.
fn caret_visibility(control: &bottom_control::BottomControl) -> f32 {
    // Only blink when the field is the focus.
    if !matches!(control.mode, bottom_control::Mode::Field) {
        return 1.0;
    }
    let phase = control.caret_phase % 1.06;
    if phase < 0.6 {
        1.0
    } else {
        0.0
    }
}

fn advance_unit_toward(v: f32, target: f32, dt: f32, duration: f32) -> f32 {
    if duration <= 0.0 {
        return target;
    }
    let dir = if target >= v { 1.0 } else { -1.0 };
    let next = v + dir * dt.max(0.0) / duration;
    if dir > 0.0 {
        next.min(target)
    } else {
        next.max(target)
    }
}

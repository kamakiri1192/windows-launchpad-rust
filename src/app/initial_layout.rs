//! Settings reset and initial-arrangement undo.
use super::state::App;

impl App {
    pub(crate) fn reset_settings_and_layout(&mut self) {
        use crate::domain::{
            initial_layout::{
                self,
                catalog::{Catalog, Platform},
            },
            settings::Settings,
        };
        if !self.layout_writable || !self.initial_layout_store_writable() {
            return;
        }
        let settings = Settings::default();
        let state = initial_layout::reset(
            &self.snapshot,
            Platform::current(),
            initial_layout::japanese_locale(sys_locale::get_locale().as_deref()),
            self.launcher_state.layout_revision,
            &Catalog::bundled(std::env::var("SystemRoot").ok()),
        );
        if !self.qa_enabled() {
            if let Err(error) = self.save_initial_layout_candidate(&state, Some(&settings)) {
                eprintln!("layout: settings reset could not be saved: {error}");
                return;
            }
        }
        self.settings = settings;
        self.launcher_state = state;
        self.layout_checkpoint = self.launcher_state.clone();
        self.layout_session_changed = true;
        // If reset was clicked during startup, apply the same explicit intent
        // once the first detected app set arrives, before its first frame.
        self.settings_reset_pending = !self.initial_snapshot_ready;
        self.apply_persisted_liquid_glass_to_renderer();
        self.close_folder();
        self.tile_springs.clear();
        self.relayout();
        self.request_redraw();
    }

    pub(crate) fn undo_initial_layout(&mut self) {
        if !self.layout_writable || !self.initial_layout_store_writable() {
            return;
        }
        let mut candidate = self.launcher_state.clone();
        if !candidate.undo_initial_layout() {
            return;
        }
        if !self.qa_enabled() {
            if let Err(e) = self.save_initial_layout_candidate(&candidate, None) {
                eprintln!("layout: undo could not be saved: {e}");
                return;
            }
        }
        self.launcher_state = candidate;
        self.layout_checkpoint = self.launcher_state.clone();
        self.layout_session_changed = true;
        self.close_folder();
        // Undo restores a complete saved arrangement. Start at those cells so
        // newly restored apps cannot overlap tiles from the grouped layout.
        self.tile_springs.clear();
        self.relayout();
        self.request_redraw();
    }
}

impl App {
    fn initial_layout_store_writable(&self) -> bool {
        #[cfg(windows)]
        {
            self.catalog_store_writable
        }
        #[cfg(not(windows))]
        {
            true
        }
    }

    /// Reset and undo use the same shared DB transaction on both platforms.
    fn save_initial_layout_candidate(
        &self,
        candidate: &crate::domain::launcher_state::LauncherState,
        settings: Option<&crate::domain::settings::Settings>,
    ) -> Result<(), String> {
        let result = match settings {
            Some(settings) => self.cache.put_settings_and_layout(settings, candidate),
            None => self.cache.put_launcher_state(candidate),
        };
        result.map_err(|error| error.to_string())
    }
}

//! Debug-only performance counters for the renderer.
//!
//! These exist so tests (and, eventually, a debug overlay) can assert that
//! the persistent-buffer refactor did not regress allocation behavior:
//!
//! - idle frames perform zero static-buffer creations,
//! - scroll-only frames do not rebuild the tile/icon/text scene,
//! - settings-close frames clear only the settings batch,
//! - capacity-internal updates reuse the buffer (no grow),
//! - only QA captures read GPU memory back.
//!
//! Counters are **debug-only**: the whole struct and its recording methods are
//! behind `#[cfg(debug_assertions)]` so they add zero cost to release builds
//! and cannot introduce allocation/lock contention on the production hot path.
//! In release builds, `BufferCounters` is a zero-sized no-op stub so all call
//! sites compile unchanged.

/// Per-category buffer-creation tally. Index = [`Category`] discriminant.
#[cfg(debug_assertions)]
type CountVec = Vec<(&'static str, u64)>;

/// A category of GPU resource whose creation/growth/upload we track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::renderer) enum Category {
    Tile,
    Icon,
    TextLabel,
    Control,
    Gear,
    Settings,
    SettingsText,
    ControlText,
    BadgeForeground,
}

impl Category {
    #[cfg(debug_assertions)]
    const ALL: [Self; 9] = [
        Self::Tile,
        Self::Icon,
        Self::TextLabel,
        Self::Control,
        Self::Gear,
        Self::Settings,
        Self::SettingsText,
        Self::ControlText,
        Self::BadgeForeground,
    ];

    #[cfg(debug_assertions)]
    const fn label(self) -> &'static str {
        match self {
            Self::Tile => "tile",
            Self::Icon => "icon",
            Self::TextLabel => "text_label",
            Self::Control => "control",
            Self::Gear => "gear",
            Self::Settings => "settings",
            Self::SettingsText => "settings_text",
            Self::ControlText => "control_text",
            Self::BadgeForeground => "badge_foreground",
        }
    }
}

/// Debug counters. Zero-cost in release.
#[derive(Debug, Clone, Default)]
pub(super) struct BufferCounters {
    #[cfg(debug_assertions)]
    creations: std::collections::HashMap<Category, u64>,
    #[cfg(debug_assertions)]
    grows: std::collections::HashMap<Category, u64>,
    #[cfg(debug_assertions)]
    prepare_calls: u64,
    #[cfg(debug_assertions)]
    full_scene_rebuilds: u64,
    #[cfg(debug_assertions)]
    atlas_rebinds: u64,
    /// GPU readbacks excluding QA captures. Should always be 0.
    #[cfg(debug_assertions)]
    non_qa_readbacks: u64,
}

impl BufferCounters {
    /// Record a buffer creation (first allocation) for `cat`.
    #[allow(unused_variables)]
    pub(super) fn record_creation(&mut self, cat: Category) {
        #[cfg(debug_assertions)]
        {
            *self.creations.entry(cat).or_insert(0) += 1;
        }
    }

    /// Record a buffer growth (capacity overflow reallocation) for `cat`.
    #[allow(unused_variables)]
    pub(super) fn record_growth(&mut self, cat: Category) {
        #[cfg(debug_assertions)]
        {
            *self.grows.entry(cat).or_insert(0) += 1;
        }
    }

    /// Record a `prepare(&RenderModel)` call.
    #[allow(unused_variables)]
    pub(super) fn record_prepare(&mut self) {
        #[cfg(debug_assertions)]
        {
            self.prepare_calls += 1;
        }
    }

    /// Record a full static-scene rebuild (tile+icon+text relayout).
    #[allow(unused_variables)]
    pub(super) fn record_full_scene_rebuild(&mut self) {
        #[cfg(debug_assertions)]
        {
            self.full_scene_rebuilds += 1;
        }
    }

    /// Record an icon-atlas rebind (texture reallocation).
    #[allow(unused_variables)]
    pub(super) fn record_atlas_rebind(&mut self) {
        #[cfg(debug_assertions)]
        {
            self.atlas_rebinds += 1;
        }
    }

    /// Record a GPU→CPU readback that is NOT a QA capture. Always a bug if >0.
    #[allow(unused_variables)]
    pub(super) fn record_non_qa_readback(&mut self) {
        #[cfg(debug_assertions)]
        {
            self.non_qa_readbacks += 1;
        }
    }

    /// Number of creations recorded for `cat` (debug only; 0 in release).
    pub(super) fn creations(&self, cat: Category) -> u64 {
        #[cfg(debug_assertions)]
        {
            *self.creations.get(&cat).unwrap_or(&0)
        }
        #[cfg(not(debug_assertions))]
        {
            let _ = cat;
            0
        }
    }

    /// Number of capacity-growth reallocations recorded for `cat`.
    pub(super) fn grows(&self, cat: Category) -> u64 {
        #[cfg(debug_assertions)]
        {
            *self.grows.get(&cat).unwrap_or(&0)
        }
        #[cfg(not(debug_assertions))]
        {
            let _ = cat;
            0
        }
    }

    /// Total `prepare` calls since the counters were last reset.
    pub(super) fn prepare_calls(&self) -> u64 {
        #[cfg(debug_assertions)]
        {
            self.prepare_calls
        }
        #[cfg(not(debug_assertions))]
        {
            0
        }
    }

    /// Total full-scene rebuilds.
    pub(super) fn full_scene_rebuilds(&self) -> u64 {
        #[cfg(debug_assertions)]
        {
            self.full_scene_rebuilds
        }
        #[cfg(not(debug_assertions))]
        {
            0
        }
    }

    /// Total icon-atlas rebinds.
    pub(super) fn atlas_rebinds(&self) -> u64 {
        #[cfg(debug_assertions)]
        {
            self.atlas_rebinds
        }
        #[cfg(not(debug_assertions))]
        {
            0
        }
    }

    /// Total non-QA GPU readbacks. Must stay 0.
    pub(super) fn non_qa_readbacks(&self) -> u64 {
        #[cfg(debug_assertions)]
        {
            self.non_qa_readbacks
        }
        #[cfg(not(debug_assertions))]
        {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_accumulate_per_category() {
        let mut c = BufferCounters::default();
        assert_eq!(c.creations(Category::Control), 0);
        c.record_creation(Category::Control);
        c.record_creation(Category::Control);
        c.record_creation(Category::Settings);
        assert_eq!(c.creations(Category::Control), 2);
        assert_eq!(c.creations(Category::Settings), 1);
        assert_eq!(c.creations(Category::Gear), 0);
    }

    #[test]
    fn growth_is_distinct_from_creation() {
        let mut c = BufferCounters::default();
        c.record_creation(Category::Tile);
        c.record_growth(Category::Tile);
        c.record_growth(Category::Tile);
        assert_eq!(c.creations(Category::Tile), 1);
        assert_eq!(c.grows(Category::Tile), 2);
    }

    #[test]
    fn prepare_and_full_scene_counters_are_independent() {
        let mut c = BufferCounters::default();
        c.record_prepare();
        c.record_prepare();
        c.record_full_scene_rebuild();
        assert_eq!(c.prepare_calls(), 2);
        assert_eq!(c.full_scene_rebuilds(), 1);
    }
}

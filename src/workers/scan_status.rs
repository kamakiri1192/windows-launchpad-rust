//! Completion of each local discovery source, kept separate from an empty result.
#[derive(Debug, Clone, Default)]
pub struct ScanStatus {
    pub sources: Vec<SourceStatus>,
}
#[derive(Debug, Clone)]
pub struct SourceStatus {
    pub source: &'static str,
    pub complete: bool,
}
impl ScanStatus {
    pub fn record(&mut self, source: &'static str, complete: bool) {
        self.sources.push(SourceStatus { source, complete });
    }
    pub fn complete(&self) -> bool {
        !self.sources.is_empty() && self.sources.iter().all(|s| s.complete)
    }
}

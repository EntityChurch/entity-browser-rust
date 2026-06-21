//! `ContentStreamOutput` — snapshot DTO consumed by the DOM renderer.

#[derive(Debug, Clone)]
pub struct ContentStreamOutput {
    pub routing_active: bool,
    pub counts: crate::views::path_tap::model::VariantCounts,
    pub rows: Vec<BindingRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    Put,
    Remove,
    Snapshot,
    CacheInvalidate,
}

#[derive(Debug, Clone)]
pub struct BindingRow {
    pub kind: BindingKind,
    pub path: String,
    pub entity_type: Option<String>,
    /// Full content hash. Renderer truncates to first 12 chars for
    /// display; the full value stays in the model so future drill-down
    /// (e.g. clicking the hash to open the entity) has it.
    pub content_hash: Option<String>,
    /// True iff this `Put` created a new entity (vs. modified existing).
    /// Ignored for non-`Put` kinds.
    pub is_new: bool,
}

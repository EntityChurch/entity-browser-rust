//! Renderer-neutral output for the Query Console window.

#![allow(dead_code)]

use crate::views::EventEntry;
use crate::window::WindowId;

#[derive(Debug, Clone)]
pub struct QueryConsoleOutput {
    pub window_id: WindowId,
    /// The window's bound local peer — query/count run against this
    /// peer's SDK, not the primary's (a console can be palette-bound
    /// to a non-primary backend peer).
    pub peer_id: String,
    pub fields: QueryFields,
    /// Result log (shared event log, pre-classified).
    pub events: Vec<EventEntry>,
}

/// Initial values for each query field. The DOM inputs own the live
/// values after creation; these seed the controls and provide
/// fallbacks for the click handler when the live values can't be read.
#[derive(Debug, Clone)]
pub struct QueryFields {
    pub type_filter: String,
    pub path_prefix: String,
    pub ref_filter: String,
    pub path_filter: String,
    pub limit: String,
    pub include_entities: bool,
}

//! `PathTapOutput` — snapshot DTO consumed by the DOM renderer.
//!
//! Pure presentation type — no `WorkerProxy`, no SDK refs. Built by
//! [`super::model::PathTapModel::render_output`] from the ring buffer.

#[derive(Debug, Clone)]
pub struct PathTapOutput {
    /// Whether inspect routing was successfully wired at window
    /// creation. `false` means install_inspect_sink failed (unknown
    /// peer, SDK not built with `.with_inspect_routing()`); the empty-
    /// state pane explains.
    pub routing_active: bool,
    /// Cumulative per-variant fact counter — visible in the renderer
    /// diagnostic strip so users can see "facts are arriving but no
    /// dispatches" vs "nothing at all is arriving."
    pub counts: crate::views::path_tap::model::VariantCounts,
    /// Recent dispatch facts, newest-first, capped at the ring
    /// capacity. Each row is one `InspectFact::Dispatch` exit event.
    pub rows: Vec<DispatchRow>,
}

#[derive(Debug, Clone)]
pub struct DispatchRow {
    pub request_id: String,
    pub handler_uri: String,
    pub operation: String,
    pub status: u32,
}

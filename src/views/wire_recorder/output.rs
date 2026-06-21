//! `WireRecorderOutput` — snapshot DTO consumed by the DOM renderer.
//!
//! Pure presentation type — no `WorkerProxy`, no SDK refs. Built by
//! [`super::model::WireRecorderModel::render_output`] from the ring buffer.

#[derive(Debug, Clone)]
pub struct WireRecorderOutput {
    /// Whether inspect routing was successfully wired at window
    /// creation. `false` means install_inspect_sink failed (unknown
    /// peer, SDK not built with `.with_inspect_routing()`); the empty-
    /// state pane explains.
    pub routing_active: bool,
    /// Cumulative per-variant fact counter — same strip as Path Tap /
    /// Content Stream so users can compare "wire facts arriving" vs
    /// "dispatches arriving" vs "bindings arriving."
    pub counts: crate::views::path_tap::model::VariantCounts,
    /// Recent wire facts, newest-first, capped at the ring capacity.
    pub rows: Vec<WireRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireDirection {
    Inbound,
    Outbound,
}

#[derive(Debug, Clone)]
pub struct WireRow {
    pub direction: WireDirection,
    /// Remote peer id when known (cross-peer traffic). `None` for
    /// frames whose remote couldn't be resolved at marshal time.
    pub peer_remote: Option<String>,
    pub frame_kind: String,
    pub bytes: u32,
    pub request_id: Option<String>,
}

//! Renderer-neutral output for the Chain Trace window.

#![allow(dead_code)]

use crate::window::WindowId;

#[derive(Debug, Clone)]
pub struct ChainTraceOutput {
    pub window_id: WindowId,
    pub peer_id: String,
    /// User-entered chain_id (empty when no chain selected).
    pub chain_id: String,
    /// Whether any continuation / marker has been observed for the
    /// entered chain_id. Lets the renderer distinguish "no chain by
    /// that id" from "chain exists but trace empty."
    pub chain_known: bool,
    /// Continuation entries in this chain. Ordered lexicographically
    /// by path (one entry per continuation entity).
    pub continuations: Vec<TraceEntry>,
    /// Chain-error markers attributed to this chain. Ordered
    /// lexicographically by path (step ordinal first, so render
    /// order ≈ chain step order).
    pub markers: Vec<TraceEntry>,
}

/// One entry in the trace — either a continuation or an error marker.
/// Renderer consults [`crate::render_policy::RenderPolicy`] to decide
/// what to surface in the body cell.
#[derive(Debug, Clone)]
pub struct TraceEntry {
    pub path: String,
    pub entity_type: String,
    /// Whether the renderer has the entity body available; if false,
    /// the cache hadn't decoded it yet and the renderer should display
    /// `<unread>` rather than fetch synchronously.
    pub body_available: bool,
    /// Decoded body for display, already redacted per `RenderPolicy`.
    /// `None` when policy denies rendering or body is unavailable.
    pub body_display: Option<String>,
    /// Short label per marker kind ("lost" / "rejected") derived from
    /// the path, or empty for continuations.
    pub kind_label: String,
    /// Reason segment from the marker path, or empty for continuations.
    pub reason_label: String,
}

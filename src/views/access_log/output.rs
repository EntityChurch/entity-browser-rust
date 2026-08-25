//! Access Log output — the user-facing "who did what, where, with what result"
//! projection of the live dispatch stream. Pure presentation (no SDK refs);
//! built by [`super::model::AccessLogModel::render_output`].
//!
//! This is the *visible* half of the capability-audit direction
//! (`RESEARCH-CAPABILITY-MANAGEMENT-UX §4 Step 1`): every row is one operation
//! that actually crossed the dispatch boundary — the handler (where), the
//! operation (how), and the result (allowed / denied). The set of rows is also
//! the raw material for the internal minimal-permission map.

/// The result of one dispatched operation, classified from its status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessOutcome {
    /// A 2xx — the operation was permitted and succeeded.
    Allowed,
    /// A 401/403 — a capability refusal (the enforcement signal). While
    /// `debug_open_grants` is on this is rare, but it's the exact edge the
    /// cutover cares about, surfaced honestly.
    Denied,
    /// Any other non-2xx — a real error, not an authorization decision.
    Error,
    /// Entry phase (status 0) — dispatched, not yet resolved. Kept out of the
    /// log (see the model), listed here for completeness of the classifier.
    Pending,
}

/// One access-log entry: a completed operation and its outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessEntry {
    /// Target peer id, from an `entity://{peer}/...` handler URI; `None` for a
    /// local/self dispatch (a bare handler path like `system/tree`).
    pub peer: Option<String>,
    /// The handler invoked — the "where" (e.g. `local/files`, `system/tree`).
    pub handler: String,
    /// The operation verb — the "how" (e.g. `list`, `read`, `write`).
    pub operation: String,
    /// The classified result.
    pub outcome: AccessOutcome,
    /// The raw status, kept for the detail/tooltip.
    pub status: u32,
}

/// The whole window's render input.
pub struct AccessLogOutput {
    /// Whether the inspect sink attached at window creation. `false` → the
    /// empty state explains that routing isn't wired (unknown peer / SDK built
    /// without inspect routing).
    pub routing_active: bool,
    /// Completed accesses, newest first.
    pub entries: Vec<AccessEntry>,
}

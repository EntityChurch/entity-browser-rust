//! Access Log output — the user-facing "who did what, where, with what result"
//! projection. Pure presentation (no SDK refs); built by
//! [`super::model::AccessLogModel::render_output`] from the app-tier
//! [`crate::access_log_store`].
//!
//! This is the *visible* half of the capability-audit direction
//! (`RESEARCH-CAPABILITY-MANAGEMENT-UX §4 Step 1`): every row is one operation
//! that actually crossed the dispatch boundary — the **actor** (who), the
//! target and handler (where), the operation (how), and the result (allowed /
//! denied). The set of rows is also the raw material for the internal
//! minimal-permission map.

// The entry + outcome types live with the store that produces them; re-export so
// the renderer and callers keep importing them from the view module.
pub use crate::access_log_store::{AccessDirection, AccessEntry, AccessOutcome};

/// Which directions the log is showing — the operator's dropdown selection.
/// `All` is the default; the rest narrow to one boundary crossing so the single
/// unified log can stand in for the old per-direction windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DirectionFilter {
    #[default]
    All,
    Outbound,
    Inbound,
    Local,
}

impl DirectionFilter {
    /// The stable value used on the `<option>` + parsed back from the change
    /// event (kept short + ascii — it round-trips through the DOM).
    pub fn as_value(self) -> &'static str {
        match self {
            DirectionFilter::All => "all",
            DirectionFilter::Outbound => "out",
            DirectionFilter::Inbound => "in",
            DirectionFilter::Local => "local",
        }
    }

    /// Parse the dropdown value back into a filter; unknown → `All` (safe default).
    pub fn from_value(v: &str) -> Self {
        match v {
            "out" => DirectionFilter::Outbound,
            "in" => DirectionFilter::Inbound,
            "local" => DirectionFilter::Local,
            _ => DirectionFilter::All,
        }
    }

    /// Human label for the option.
    pub fn label(self) -> &'static str {
        match self {
            DirectionFilter::All => "All",
            DirectionFilter::Outbound => "→ Outbound (you called a peer)",
            DirectionFilter::Inbound => "← Inbound (a peer called this device)",
            DirectionFilter::Local => "· Local (this app's own peer)",
        }
    }

    /// Whether an entry passes this filter.
    pub fn matches(self, dir: AccessDirection) -> bool {
        match self {
            DirectionFilter::All => true,
            DirectionFilter::Outbound => dir == AccessDirection::Outbound,
            DirectionFilter::Inbound => dir == AccessDirection::Inbound,
            DirectionFilter::Local => dir == AccessDirection::Local,
        }
    }

    /// The options, in display order.
    pub const ALL: [DirectionFilter; 4] = [
        DirectionFilter::All,
        DirectionFilter::Outbound,
        DirectionFilter::Inbound,
        DirectionFilter::Local,
    ];
}

/// One selectable peer in the Peer dropdown — the *subject* of an access log
/// (whose log the row belongs to), with a friendly label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerOption {
    /// Stable key: a peer id for a local actor, or the native-backend key for
    /// inbound rows. Round-trips through the dropdown value.
    pub key: String,
    /// Human label — "System peer", "System backend", etc. (canonical names, see
    /// docs/architecture/specs/TERMINOLOGY-AND-WINDOWS.md).
    pub label: String,
}

/// The subject peer of an access row — *whose* access log it belongs to. This is
/// the axis the operator manages by: the System peer (its own dispatches out) vs
/// the System backend (who reached into it). Local/Outbound belong to the acting
/// peer; Inbound belongs to the System backend that was called.
pub fn subject_key(entry: &AccessEntry, backend_key: &str) -> String {
    match entry.direction {
        AccessDirection::Inbound => backend_key.to_string(),
        AccessDirection::Local | AccessDirection::Outbound => entry.actor.clone(),
    }
}

/// Which view the Access Log window is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccessView {
    /// The live stream of individual accesses (newest first).
    #[default]
    Activity,
    /// The observed-capability map: per peer, the distinct grants it exercised.
    Capabilities,
}

impl AccessView {
    pub fn as_value(self) -> &'static str {
        match self {
            AccessView::Activity => "activity",
            AccessView::Capabilities => "capabilities",
        }
    }
    pub fn from_value(v: &str) -> Self {
        match v {
            "capabilities" => AccessView::Capabilities,
            _ => AccessView::Activity,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            AccessView::Activity => "Activity (live log)",
            AccessView::Capabilities => "Observed capabilities",
        }
    }
    pub const ALL: [AccessView; 2] = [AccessView::Activity, AccessView::Capabilities];
}

/// One distinct capability an acting peer was observed exercising — the dedup
/// unit of the observed-capability map. Aggregated from the raw access stream:
/// each `(target, handler, operation, resource)` tuple collapses to one row with
/// a use count. This *is* the minimal grant that op would require under
/// enforcement — the raw material for "observed vs. authored".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedGrant {
    /// Friendly label of the target peer the op ran against; `None` = a local
    /// dispatch on the actor's own peer (no cross-peer grant needed).
    pub target_label: Option<String>,
    pub handler: String,
    pub operation: String,
    /// The resource path, when captured. `None` today on local + inbound (a known
    /// capture gap — see PLAN-OF-RECORD-capability-enforcement.md §3).
    pub resource: Option<String>,
    /// How many times this exact tuple was seen (in the retained window).
    pub count: usize,
    /// Whether any occurrence was denied (a capability refusal) — visible even
    /// though enforcement is currently open, so a future denial stands out.
    pub any_denied: bool,
}

/// A peer's **authored** grant on the System backend — the profile it was
/// granted, expanded to bits. Shown beside the observed grants so the
/// observed-vs-authorized gap is visible (the input to the enforcement decision).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthoredGrant {
    /// Friendly profile name, e.g. "File transfer (pull only)".
    pub profile_label: String,
    /// Plain-English scope summary.
    pub summary: String,
    pub handlers: Vec<String>,
    pub resources: Vec<String>,
    pub operations: Vec<String>,
}

/// The observed capabilities of one acting peer — "what this peer actually does,
/// i.e. the minimal grant it would need under enforcement" — paired with what it
/// is *authorized* for, when a grant has been recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerCapabilities {
    /// The acting peer's id (the grantee).
    pub actor_key: String,
    /// Friendly label (System peer / System backend / descriptor + short id).
    pub actor_label: String,
    /// The distinct grants it exercised, sorted for a stable read.
    pub grants: Vec<ObservedGrant>,
    /// What this peer is *authorized* for on the System backend, if a grant has
    /// been recorded. `None` = no explicit grant (our own system peers; or a
    /// device not yet authorized). The observed-vs-authorized comparison.
    pub authorized: Option<AuthoredGrant>,
}

/// The capability-map view's render input — the aggregation over all retained
/// accesses, grouped by acting peer.
pub struct CapabilityMapOutput {
    pub peers: Vec<PeerCapabilities>,
}

/// The whole window's render input: completed accesses (newest first, already
/// narrowed to the active filters) plus the state the two dropdowns reflect.
pub struct AccessLogOutput {
    pub entries: Vec<AccessEntry>,
    /// Active direction filter (→/←/·).
    pub direction: DirectionFilter,
    /// Active peer filter — a `PeerOption.key`, or empty for "all peers".
    pub peer_filter: String,
    /// The distinct subject peers present in the (unfiltered) log, for the
    /// dropdown — the System peer, the System backend, any others.
    pub peer_options: Vec<PeerOption>,
    /// The subject key inbound rows are attributed to (the System backend peer id
    /// when known, else a stable sentinel) — the renderer's Peer column uses it.
    pub backend_key: String,
    /// Subject key → friendly label, for the Peer column (superset of
    /// `peer_options`, so a row always resolves even mid-filter).
    pub labels: std::collections::HashMap<String, String>,
}

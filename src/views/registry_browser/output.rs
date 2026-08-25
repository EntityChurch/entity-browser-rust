//! Renderer-neutral output for the Registry Browser.
//!
//! The model builds this; `dom::registry_browser` is a pure consumer. Keeping
//! the shape here (rather than formatting in the renderer) is what lets the
//! honesty rules below be **unit-tested natively** — every one of them is a
//! statement about what a user is told, and none of them is testable through a
//! DOM.

/// Where the registry in force came from. A name resolving through a registry
/// the user never chose must never be silent [AP25].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinOrigin {
    /// Typed here with `name pin` (or this window). Tab-scoped.
    User,
    /// Seeded by `/entity-deployment.json`'s `name_registry_pin`.
    Deployment,
}

/// The registry this window is pointed at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedRegistry {
    pub peer_id: String,
    pub origin: String,
    pub source: PinOrigin,
}

/// The name listing recovered from a registry's **signed root**.
///
/// **`complete` is not decoration.** A bounded walk that returns fewer keys than
/// the registry holds is indistinguishable from a small registry unless the
/// surface says so, which is the same defect the walk itself is built to prevent
/// (a shortened list that does not announce itself). The renderer must show the
/// truncation notice whenever this is `false`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameListing {
    pub names: Vec<String>,
    /// `true` when the walk visited every node the signed root declares.
    pub complete: bool,
    /// Interior nodes fetched — the cost, and the honest answer to "why is this
    /// big registry slow".
    pub nodes_walked: usize,
}

/// What a resolve established, and what it checked to establish it.
///
/// Every field here is *evidence*, not a claim: `NameEvidence` records which
/// checks actually ran, and nothing in it is a boolean a caller can set. A
/// resolved name and an unchecked one must not look alike.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedName {
    pub name: String,
    pub peer_id: String,
    /// Where the binding says that peer publishes. `None` when the binding
    /// carried no consumable `http-poll` transport profile — resolved WHO but
    /// not WHERE, which is a different failure from "no such name" and is
    /// reported as one.
    pub origin: Option<String>,
    pub association_committed: bool,
    pub name_checked: bool,
    pub revocation_checked: bool,
    pub expires_at_ms: u64,
    /// Set when OUR resolver ceiling shortened the lifetime the registry issued.
    /// Announced rather than applied silently [AP25]: a silent clamp makes a
    /// correctly-issued binding look like a registry that mis-set its TTL, and
    /// the operator is the one person who can tell the difference.
    pub clamped: Option<(u64, u64)>,
}

/// One in-flight or finished operation, so the surface can distinguish "nothing
/// asked yet" from "asked and empty" — which are the same pixels otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase<T> {
    /// Nothing asked yet. **Not** an empty result.
    Idle,
    Running,
    Done(T),
    Failed(String),
}

/// Everything the Registry Browser renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryBrowserOutput {
    /// `None` = this deployment seeds no pin and the user typed none, so
    /// resolution fails closed. That is a state to *state*, not a blank panel.
    pub pinned: Option<PinnedRegistry>,
    pub listing: Phase<NameListing>,
    pub resolved: Phase<ResolvedName>,
    /// How many publishers this tab holds a `seq` floor for.
    pub sessions: usize,
    /// True on native builds, where the fetch has no implementation. Saying so
    /// beats an empty panel that reads as a broken registry.
    pub browser_only: bool,
}

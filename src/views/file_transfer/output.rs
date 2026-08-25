//! Renderer-neutral output for the File Transfer window.
//!
//! Slice 0 (DESIGN-CROSS-DEVICE-FILE-TRANSFER §7): the window dispatches
//! `local/files` list/read against a connected remote peer's exposed
//! `local/files/shared/` root and surfaces the results. Dynamic browsing,
//! byte-level Save-As, and the reverse (push) direction are later phases.

#![allow(dead_code)]

use crate::views::EventEntry;

#[derive(Debug, Clone)]
pub struct FileTransferOutput {
    /// The bound local peer the execute dispatches originate from. A
    /// remote target is reached as `entity://{target}/local/files` through
    /// this peer's connection pool — never via the local peer registry
    /// (remote peers aren't registered locally).
    pub peer_id: String,
    /// **Remembered** remote peers that can be transfer targets, pre-built with
    /// the `selected` flag. Empty until this app has connected to something at
    /// least once.
    ///
    /// This used to say "connected", which was wrong and quietly load-bearing:
    /// the source is `connections::read_connections`, the petname/authz registry
    /// whose presence means *"we have connected to this peer at least once"* —
    /// never that it is up now (`MODEL-REMOTE-PEER-FACTS` §1). Offering a
    /// long-offline device as a target is correct (you may pick it, then
    /// connect); calling it connected was not. [`target_reach`] is the axis that
    /// answers the other question.
    ///
    /// [`target_reach`]: FileTransferOutput::target_reach
    pub target_options: Vec<TargetOption>,
    /// The currently-selected target peer id (empty when none connected).
    pub selected_target: String,
    /// Tree prefix the peer exposes its share at (matches the backend's
    /// `SHARE_PREFIX`). Rendered read-only so the demo is legible.
    pub share_prefix: String,
    /// Initial value of the filename input (defaults to the seeded file).
    pub filename_initial: String,
    /// The share browsed as a **tree** (`DESIGN-CROSS-DEVICE-FILE-TRANSFER`) —
    /// flattened rows from the shared `TreeNode` machinery, one per visible
    /// file/folder. Empty until the root has been listed.
    pub tree_rows: Vec<FileRow>,
    /// True once the share root has been listed (drives "Browse" vs the tree).
    pub root_listed: bool,
    /// True while the root list is in flight.
    pub root_loading: bool,
    /// Full tree path of the currently-selected file (the Pull target), if any.
    pub selected_full_path: Option<String>,
    /// **How** to pull the selected file — resolved in the model, carried
    /// verbatim by the DOM. `Share` for a file in a native peer's `local/files`
    /// mount, `Offer` for one a browser peer published as hash-addressed
    /// content. The window renders one Pull button either way, and knows
    /// nothing about which kind of peer it is talking to.
    pub selected_pull: Option<crate::action::PullPlan>,
    /// Last browse error, surfaced loudly (D13).
    pub browse_error: Option<String>,
    /// True when at least one remote peer is **remembered** — drives the
    /// role-aware hint ("connect a backend peer first" vs. the controls). Same
    /// correction as [`target_options`]: the registry is an ever-connected set,
    /// not a liveness signal.
    ///
    /// [`target_options`]: FileTransferOutput::target_options
    pub has_target: bool,
    /// Can we reach the **effective target** right now? The kernel read-model,
    /// in the app's one connection vocabulary — the same `conn_display`
    /// resolution Peer Connections and Chat render, so the three cannot
    /// disagree about the same link.
    ///
    /// **Independent of [`access`], and that is the point.** Authorization and
    /// reachability are orthogonal facts about a target, and only one of them
    /// used to be shown. `classify_target_access` deliberately skips transport
    /// errors (never manufacture a denial from silence — correct), so an
    /// unreachable peer landed in `TargetAccess::Unknown`, whose whole meaning
    /// is *"nothing tried yet — do not alarm"*. A device that cannot be reached
    /// at all therefore looked exactly like one you had not used yet. This is
    /// the axis that tells them apart. (Same shape as AP22: two independent
    /// facts where only one failed loudly.)
    ///
    /// [`access`]: FileTransferOutput::access
    pub target_reach: crate::peer_liveness::ConnDisplay,
    /// Access status of the **effective target** — what drives the status chip
    /// and the (only-when-real) authorize affordance. File Transfer is a pure
    /// *consumer* of authorization (`DESIGN-AUTHORIZE-GATE-INCREMENT-3 §2.1`);
    /// it never authors grants. The signal is **ground-truth, result-driven**:
    /// a real refusal (`403`) from an operation against the target, not a guess
    /// from the local mirror (which records grants *we* authored as a host, not
    /// whether the *remote* authorized us — the two only coincide under mutual
    /// pairing). See [`TargetAccess`].
    pub access: TargetAccess,
    /// Result log (shared event log, pre-classified) — where list/read
    /// responses surface.
    pub events: Vec<EventEntry>,
    /// What **this** peer is offering — the serving half of the window.
    ///
    /// Independent of everything above it, and rendered even with no target at
    /// all: an offer is published on our side and is not addressed to anyone, so
    /// gating it behind "is a peer selected / has it authorized us" would hide
    /// the only send a browser↔browser pair has behind a question it never asks.
    /// (Read from our own tree, so it needs the offers prefix subscribed — see
    /// `FileTransferWindow::window_type`.)
    pub own_offers: Vec<OwnOffer>,
    /// The stated ceiling on one offered file (`file_offer::MAX_OFFER_BYTES`),
    /// carried so the window can say it **before** a picker refuses.
    pub offer_limit: u64,
}

/// One file this peer is publishing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnOffer {
    /// Hex of the blob hash — the manifest's key, and what a withdrawal names.
    pub id: String,
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct TargetOption {
    pub value: String,
    pub label: String,
    pub selected: bool,
}

/// One visible row of the share tree — a flattened [`crate::views::entity_tree::
/// tree::VisibleRow`] enriched with file/dir metadata. Indentation comes from
/// `depth`; the toggle glyph from `is_dir`/`expanded`; a spinner from `loading`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    /// Relative-to-share path — the key for expand/select events.
    pub path: String,
    /// Leaf display name.
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    /// This directory is expanded but its listing hasn't arrived yet.
    pub loading: bool,
    pub selected: bool,
    pub size: Option<u64>,
    /// Full tree path (`local/files/shared/…`) for a file pull.
    pub full_path: String,
}

/// Access status of the effective transfer target — honest and evidence-based.
///
/// The gate for a *client* pull/push is the **remote's** grant to us, which we
/// cannot read cheaply while `debug_open_grants` confers kernel-level wildcards
/// invisible to the policy table. So the only reliable signal is what actually
/// happens when we talk to the target: a `403`/`401` refusal (`Denied`) or a
/// `2xx` success (`Authorized`). We never manufacture a "not authorized" claim
/// from an empty local mirror — that false-alarmed a working transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetAccess {
    /// A recent operation against the target succeeded (`2xx`), or the local
    /// authz mirror confirms a grant. Optional profile string when known.
    Authorized(Option<String>),
    /// A recent operation was **refused** (`403`/`401`) — the real signal to
    /// surface the "[Authorize in Peer Connections]" affordance.
    Denied,
    /// No evidence yet (nothing tried, no mirror entry). Neutral — do NOT
    /// alarm; with `debug_open_grants` on the peer is in fact allowed.
    Unknown,
}

//! Renderer-neutral output for the System Overview window.

use crate::peer_display::PeerDescriptor;

/// One system peer, rendered as an identity card: its id + the truthful
/// role·runtime·storage facts. (The native peer's live listen address / share
/// is shown by the Status section below the cards, not repeated here.)
#[derive(Clone, Debug)]
pub struct SystemPeerCard {
    /// Short, scannable id (the row/badge form).
    pub short_id: String,
    /// Full peer id (tooltip / copy target).
    pub full_id: String,
    /// The honest role·runtime·storage descriptor (glyph + chips).
    pub descriptor: PeerDescriptor,
}

/// The System Overview top section: the system peer(s) that own the control
/// plane, plus a terse posture summary. Deliberately NOT a link farm — it shows
/// the peers themselves; the native peer's live detail follows below it in the
/// same window (S2 — no drill-in).
#[derive(Clone, Debug)]
pub struct SystemPeersOutput {
    /// The in-app system peer — always present.
    pub in_app: SystemPeerCard,
    /// The native system peer — `Some` only in a desktop deployment once it's
    /// registered; `None` in a plain browser session.
    pub native: Option<SystemPeerCard>,
    /// Total hosted peers (system + user).
    pub total_peers: usize,
    /// Of those, how many the user created.
    pub user_peers: usize,
    /// Human summary of the boot surface (e.g. "Window chrome" / "Content site"
    /// / "Window: Peers").
    pub startup: String,
    /// Whether this deployment allows creating peers (capability posture).
    pub peer_creation: bool,
}

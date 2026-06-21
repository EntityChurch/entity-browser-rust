//! Application-layer display classification for managed peers.
//!
//! The SDK is classification-agnostic — a peer is just "a peer you built
//! with certain configuration options and extensions installed." From the
//! SDK's perspective there's no inherent Primary/Local/Remote distinction.
//!
//! These types live at the application layer because they encode
//! entity-browser UX policy:
//!   - which peer is the "primary" (the default peer)
//!   - which peers are directly accessible vs protocol-only
//!   - which peers the user is allowed to delete
//!
//! Views should classify peers through [`PeerDisplay::classify`] rather
//! than reading an SDK field.

use std::collections::HashMap;

use crate::peer_mode::PeerMode;
use crate::peers::Peers;

/// Display classification for a managed peer, derived from SDK facts
/// plus application policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerDisplay {
    /// The SDK's default peer. Always present, not user-deletable.
    Primary,
    /// A local peer with a PeerContext — direct tree access available.
    Local,
    /// A protocol-only peer (metadata-only; accessed via execute over a
    /// connection).
    Remote,
}

impl PeerDisplay {
    /// Derive the display classification for `peer_id` from Peers state.
    /// Routes the same query to Direct vs Worker arm via `Peers`.
    pub fn classify(peers: &Peers, peer_id: &str) -> Self {
        if peer_id == peers.default_peer_id() {
            Self::Primary
        } else if peers.is_backend_hosted(peer_id) {
            // Backend (Memory/OPFS) worker peer: it has a PeerContext
            // in its OWN dedicated SDK, so the `has_peer_context`
            // check below would wrongly call it a frontend ("Local")
            // peer. Classify as Remote → role_name/glyph render
            // "backend (...)". This is why every backend peer used to
            // show up as "frontend".
            Self::Remote
        } else if peers.has_peer_context(peer_id) {
            Self::Local
        } else {
            Self::Remote
        }
    }

    /// Short lowercase label (for CSS class names, logs, etc).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Local => "local",
            Self::Remote => "remote",
        }
    }

    /// Inverse of [`as_str`](Self::as_str). Unknown tags fall back to
    /// `Remote` (the conservative classification — no direct tree
    /// access assumed). Lets a consumer reconstruct the classification
    /// from a persisted registry record without re-querying `Peers`.
    pub fn from_tag(tag: &str) -> Self {
        match tag {
            "primary" => Self::Primary,
            "local" => Self::Local,
            _ => Self::Remote,
        }
    }

}

impl std::fmt::Display for PeerDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Resolve a hosted peer's display role truthfully.
///
/// Returns `(kind, glyph, role_name)`:
/// - `kind` — structural classification (Primary/Local/Remote), a
///   *runtime* fact from `classify` (which SDK hosts it). Correct as-is.
/// - `glyph` / `role_name` — the **mode**, resolved from the
///   authoritative `modes` map (`peer_id` → [`PeerMode`], built from
///   the persisted store via [`crate::persistence::peer_modes`]). This
///   replaces the old `persisted`-flag proxy, which misrepresented
///   every backend peer as "memory" and OPFS peers as memory.
///
/// Glyphs: `★` system · `●` frontend · `◆` backend (memory) ·
/// `◆⛁` backend (opfs).
///
/// A hosted peer not in `modes` is an in-session/ephemeral peer (not
/// persisted); fall back to the structural kind. Connected-remote
/// peers aren't in the hosted set this is called over.
pub fn resolve_role(
    peers: &Peers,
    peer_id: &str,
    modes: &HashMap<String, PeerMode>,
) -> (PeerDisplay, &'static str, &'static str) {
    let kind = PeerDisplay::classify(peers, peer_id);
    if kind == PeerDisplay::Primary {
        return (kind, "★", "system");
    }
    let (glyph, role) = match modes.get(peer_id) {
        Some(PeerMode::Frontend) => ("●", "frontend"),
        Some(PeerMode::BackendMemory) => ("◆", "backend (memory)"),
        Some(PeerMode::BackendOpfs) => ("◆⛁", "backend (opfs)"),
        None => match kind {
            PeerDisplay::Local => ("●", "frontend"),
            // Hosted, non-primary, not persisted, no local context:
            // an ephemeral backend peer (backends are normally
            // persisted, so this is the rare unsaved case).
            _ => ("◆", "backend (memory)"),
        },
    };
    (kind, glyph, role)
}

/// Whether the user interface should allow deleting this peer.
///
/// Application policy: the primary (default) peer is never user-deletable
/// because the SDK relies on it always being present.
///
/// Only called from the WASM DOM render path (`render_dom`), so native
/// builds would flag it as unused without the allow attribute.
#[allow(dead_code)]
pub fn is_user_deletable(peers: &Peers, peer_id: &str) -> bool {
    peer_id != peers.default_peer_id()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_peers() -> Peers {
        Peers::new_direct()
    }

    #[test]
    fn default_peer_classifies_as_primary() {
        let peers = make_peers();
        let id = peers.primary_peer_id().to_string();
        assert_eq!(PeerDisplay::classify(&peers, &id), PeerDisplay::Primary);
    }

    #[test]
    fn backend_peer_classifies_as_remote() {
        let mut peers = make_peers();
        let pid = "2KBackendDisplay123".to_string();
        peers.register_backend_peer_primary(pid.clone(), None, Vec::new());
        assert_eq!(PeerDisplay::classify(&peers, &pid), PeerDisplay::Remote);
    }

    #[test]
    fn unknown_peer_classifies_as_remote() {
        let peers = make_peers();
        assert_eq!(PeerDisplay::classify(&peers, "unknown-peer-id"), PeerDisplay::Remote);
    }

    #[test]
    fn default_peer_is_not_user_deletable() {
        let peers = make_peers();
        let id = peers.primary_peer_id().to_string();
        assert!(!is_user_deletable(&peers, &id));
    }

    #[test]
    fn other_peers_are_user_deletable() {
        let mut peers = make_peers();
        let pid = "2KOtherPeerId999".to_string();
        peers.register_backend_peer_primary(pid.clone(), None, Vec::new());
        assert!(is_user_deletable(&peers, &pid));
    }
}

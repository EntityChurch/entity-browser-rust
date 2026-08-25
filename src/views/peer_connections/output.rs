//! Renderer-neutral output for the Peer Connections window.

#![allow(dead_code)]

use crate::peer_display::PeerDisplay;
use crate::window::WindowId;

#[derive(Debug, Clone)]
pub struct PeerConnectionsOutput {
    pub window_id: WindowId,
    /// Info about this window's bound peer.
    pub bound_peer: BoundPeerInfo,
    /// Peers we've connected to before — the remembered-peer registry
    /// (`connections.rs`), enriched with a reconnect address. Since we have
    /// no disconnect signal yet (§D9) this is a *known-peers history*, not a
    /// live list — the honest framing from `DESIGN-CROSS-DEVICE-FILE-TRANSFER
    /// §13.2`. Supersedes the old plain "connected" id list.
    pub known_peers: Vec<KnownPeer>,
    /// Backend peers with at least one listen address — quick-connect targets.
    pub backend_peers: Vec<BackendPeer>,
    /// Initial value for the manual address input.
    pub address_input_initial: String,
    /// QR pairing payload — `{ws_addr}|{peer_id}` for a peer that can
    /// actually be reached here (this process's native listener, or the
    /// system's Tauri-managed backend listener). `None` when nothing
    /// here listens: the QR *display* is hidden (a `no-listener` QR is
    /// useless) while the *scanner* stays available. See the Peer
    /// Connections renderer's `render_qr_section`.
    pub qr_payload: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BoundPeerInfo {
    /// Full peer-id of this window's bound peer — the peer outbound
    /// connections are made *from* (`Action::ConnectPeer`). Not just
    /// `short_pid`, which is display-truncated.
    pub peer_id: String,
    pub short_pid: String,
    pub kind: PeerDisplay,
    /// Set only for the system peer with an active WS listener.
    pub ws_listen_addr: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BackendPeer {
    pub peer_id: String,
    pub display: String,
    /// Connect targets, with browser-side rewriting applied (loopback /
    /// wildcard hosts substituted with the page's hostname).
    pub connect_addresses: Vec<String>,
}

/// A remembered peer — one row in the "Known Peers" section. Reconnect
/// dials [`addr`](Self::addr) (empty for legacy pre-enrichment entries → no
/// reconnect button, just presence).
#[derive(Debug, Clone)]
pub struct KnownPeer {
    pub remote_pid: String,
    pub display: String,
    /// Saved reconnect address; empty for legacy entries.
    pub addr: String,
    /// Epoch-ms of the last successful connect (carried for a later
    /// "last seen" label; not rendered yet).
    pub last_seen: u64,
}

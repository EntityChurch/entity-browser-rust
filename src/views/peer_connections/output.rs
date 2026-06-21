//! Renderer-neutral output for the Peer Connections window.

#![allow(dead_code)]

use crate::peer_display::PeerDisplay;
use crate::window::WindowId;

#[derive(Debug, Clone)]
pub struct PeerConnectionsOutput {
    pub window_id: WindowId,
    /// Info about this window's bound peer.
    pub bound_peer: BoundPeerInfo,
    /// Currently connected remote peers.
    pub connected: Vec<String>,
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

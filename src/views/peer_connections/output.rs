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
    /// Outcome of the last manual Connect press — `(address, outcome)`, `None`
    /// when nothing has been tried this session. Rendered in the Connect card so
    /// the action reports itself; without this, both a failed dial and a
    /// successful one that produced no visible row looked identical to "the
    /// button did nothing" (`crate::connect_attempt`).
    pub last_attempt: Option<(String, crate::connect_attempt::ConnectOutcome)>,
    /// QR pairing payload — `{ws_addr}|{peer_id}` for a peer that can
    /// actually be reached here (this process's native listener, or the
    /// system's Tauri-managed backend listener). `None` when nothing
    /// here listens: the QR *display* is hidden (a `no-listener` QR is
    /// useless) while the *scanner* stays available. See the Peer
    /// Connections renderer's `render_qr_section`.
    pub qr_payload: Option<String>,
    /// The **connector registry** — signaling nodes this peer may rendezvous
    /// through, and which one is selected (`crate::connectors`). Same list the
    /// `connector` shell verb prints; this window is its localized surface.
    pub connectors: Vec<ConnectorRow>,
    /// The selection is not what this session is running on — reload to apply.
    ///
    /// Not a hint rendered next to every Use button: that would be permanent
    /// furniture, and the user cannot tell a standing caption from a live one.
    /// This is a *state*, true only while the choice is genuinely pending, and
    /// it clears itself on the reload that applies it.
    pub connector_reload_pending: bool,
    /// Result of the last `Check` press (`advertise()`), or `None`. Like
    /// [`Self::last_attempt`], this exists so the action reports itself —
    /// asking a node what it serves and showing nothing is the dead-button
    /// disease Phase 14.5 exists to catch.
    pub connector_notice: Option<ConnectorNotice>,
    /// **Meet at a name** — the `lobby` / `tag` / `secret` rendezvous modes
    /// (`crate::rendezvous`). The same operations the `meet` shell verb
    /// exposes; this window is their localized surface.
    pub meet: MeetPanel,
    /// Where the last *Find peers here* press has got to, or `None` before the
    /// first one. Rendered in the Connect card beside the button.
    pub find: Option<super::model::FindPeers>,
    // (Device authorizations moved to the System Backend window, Direction A.)
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
    /// Display status in the one §4c vocabulary — the KERNEL liveness read-model
    /// ([`crate::peer_liveness`], the reactive `system/peer/status` surface)
    /// resolved against the app-owned dialing transient. This is what fixes the
    /// stale-"Connected" lie: a real mid-session drop the old mirror missed now
    /// surfaces as Offline/Reconnecting.
    pub status: crate::peer_liveness::ConnDisplay,
    /// Whether "Forget" means anything for this row.
    ///
    /// `true` for a remembered-registry row — Forget drops the entry and the row
    /// goes away. `false` for a row derived from live peer metadata (the
    /// auto-provisioned system backend), where there is no registry entry to
    /// drop: Forget would remove nothing, the row would stay, and the button
    /// would be a visible no-op. A dead button is the same disease as the silent
    /// Connect, so the renderer omits it rather than offering it.
    pub forgettable: bool,
}

/// One row of the connector registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorRow {
    pub node_peer_id: String,
    /// Display-truncated id, matching every other peer row in this window.
    pub short_pid: String,
    pub node_addr: String,
    pub label: String,
    /// Is this the node provisioning will use? At most one row is selected —
    /// the selection is a single entity, not a per-row flag, so "exactly one"
    /// cannot drift.
    pub selected: bool,
    /// Where this row came from, which decides what may be done to it.
    pub source: ConnectorSource,
}

/// Where a listed node came from.
///
/// The distinction exists because **the node a session rendezvous through is
/// not always a row in the registry**: provisioning resolves URL query >
/// selected connector > build knob, and only the middle one writes anything
/// durable. So a browser that arrived by the link this desktop serves — the
/// ordinary case, and the whole point of the served URL — had a working
/// establisher, a working Meet, and a list that said *"no rendezvous nodes
/// yet"* directly underneath. A working feature beside an empty list of the
/// thing it supposedly requires reads as a bug in whichever half you look at
/// second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorSource {
    /// A durable row the user added. Selectable, removable.
    Registry,
    /// Synthesized from the provisioning in force ([`crate::connectors::node_in_force`]).
    ///
    /// Rendered, and deliberately offered **no** Use or Remove: there is no
    /// stored row to reselect or delete, so both buttons would be visible
    /// no-ops — the dead-button disease this window has fixed twice. `Check`
    /// stays, because asking a node what it serves is a question about the node
    /// and not about the row.
    Session,
}

/// The Meet section: the form's preconditions plus whatever search is running.
#[derive(Debug, Clone, Default)]
pub struct MeetPanel {
    /// Is a connector selected to meet through? Without one the form has
    /// nowhere to go, and saying so beats a button that fails.
    pub has_connector: bool,
    /// Display id of the node a meet would use.
    pub node_short: String,
    /// The search in progress or just finished — `None` before the first press.
    pub status: Option<MeetStatusRow>,
    /// A refusal from the last Meet press (an input the mode won't take, no
    /// connector). Its own slot rather than the connector notice: a message
    /// about the meet form belongs in the meet card, next to the button that
    /// caused it.
    pub notice: Option<ConnectorNotice>,
}

/// A running (or settled) meet, flattened for the renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetStatusRow {
    /// The upstream mode tag (`tag` / `secret` / `lobby`) — the renderer maps it
    /// to a **localized** word rather than showing the protocol's English one.
    pub mode_name: String,
    /// The user's input, already safe to display: empty for a `secret` (never
    /// echoed) and for `lobby` (its input belongs to the node).
    pub mode_input: String,
    /// `true` while still searching; the renderer shows progress rather than a
    /// result.
    pub searching: bool,
    /// Set when the meet stopped early, with the reason.
    pub error: Option<String>,
    pub polls: u32,
    pub max_polls: u32,
    pub found: Vec<MeetFoundRow>,
}

/// One peer a meet turned up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetFoundRow {
    pub peer_id: String,
    pub short_pid: String,
    /// Did the message carrying this id verify against its claimed signer? An
    /// unverified id is not a lie we can detect at rendezvous — it is checked at
    /// the handshake — but the user should see which is which.
    pub verified: bool,
    /// Already in the remembered-peer registry, so `Remember` would be a
    /// visible no-op and the renderer omits it.
    pub remembered: bool,
}

/// Outcome of a `Check` (`advertise()`) press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorNotice {
    pub text: String,
    pub is_error: bool,
}

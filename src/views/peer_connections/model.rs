//! Peer Connections model — mirrored shape.
//!
//! Per-window state: just the manual address input value (persisted
//! to the tree). Render data (connected peers, backend listings, WS
//! listen address, QR payload) is read from `Peers` on demand —
//! these change asynchronously from the window's perspective and
//! don't benefit from caching.

use std::sync::{Arc, Mutex};

use entity_entity::Entity;
use crate::peers::Peers;

use crate::peer_display::PeerDisplay;
use crate::window::WindowId;

use super::output::{
    AuthRowView, BackendAuthView, BackendPeer, BoundPeerInfo, KnownPeer, PeerConnectionsOutput,
};

/// Persisted per-window state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerConnectionsState {
    pub address: String,
}

impl Default for PeerConnectionsState {
    fn default() -> Self {
        Self {
            address: default_address(),
        }
    }
}

impl PeerConnectionsState {
    pub fn from_entity(entity: &Entity) -> Self {
        let value: ciborium::Value = match ciborium::from_reader(entity.data.as_slice()) {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };
        let map = match value.as_map() {
            Some(m) => m,
            None => return Self::default(),
        };
        let mut state = Self::default();
        for (k, v) in map {
            if k.as_text() == Some("address") {
                if let Some(s) = v.as_text() {
                    state.address = s.to_string();
                }
            }
        }
        state
    }

    pub fn to_entity(&self) -> Entity {
        let data = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
            "address" => entity_ecf::text(&self.address)
        });
        Entity::new("app/state/peer_connections", data).unwrap()
    }
}

/// In plain browser (not Tauri), suggest `ws://{page_host}:4041` —
/// covers the common case of serving WASM from the same machine that's
/// running a native peer with a WS listener.
#[cfg(target_arch = "wasm32")]
fn default_address() -> String {
    if crate::tauri_ipc::is_tauri() {
        return String::new();
    }
    web_sys::window()
        .and_then(|w| w.location().hostname().ok())
        .filter(|h| !h.is_empty())
        .map(|host| format!("ws://{}:4041", host))
        .unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
fn default_address() -> String {
    String::new()
}

#[derive(Debug)]
pub struct PeerConnectionsModel {
    window_id: WindowId,
    peer_id: String,
    inner: Arc<Mutex<PeerConnectionsState>>,
}

impl PeerConnectionsModel {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        Self {
            window_id,
            peer_id,
            inner: Arc::new(Mutex::new(PeerConnectionsState::default())),
        }
    }

    pub fn initialize(&mut self, peers: &Peers) {
        self.ensure_state_in_tree(peers);
        let state = self.read_window_state(peers);
        *self.inner.lock().unwrap() = state;
    }

    fn state_path(&self, _peers: &Peers) -> String {
        crate::app_paths::window_state_path(crate::app_paths::APP_ID, &self.peer_id, self.window_id)
    }

    fn ensure_state_in_tree(&self, peers: &Peers) {
        let path = self.state_path(peers);
        if peers.get_entity(&self.peer_id, &path).is_none() {
            peers.dispatch_write(&self.peer_id, path, PeerConnectionsState::default().to_entity());
        }
    }

    fn read_window_state(&self, peers: &Peers) -> PeerConnectionsState {
        let path = self.state_path(peers);
        peers
            .get_entity(&self.peer_id, &path)
            .map(|e| PeerConnectionsState::from_entity(&e))
            .unwrap_or_default()
    }

    fn persist_state(&self, peers: &Peers) {
        let entity = self.inner.lock().unwrap().to_entity();
        let path = self.state_path(peers);
        peers.dispatch_write(&self.peer_id, path, entity);
    }

    // -- Action methods --

    pub fn set_address(&self, value: &str) {
        self.inner.lock().unwrap().address = value.to_string();
    }

    pub fn clear_address(&self) {
        self.inner.lock().unwrap().address.clear();
    }

    pub fn save_state(&self, peers: &Peers) {
        self.persist_state(peers);
    }

    // -- Pure read API --

    #[allow(dead_code)] // called from WASM render path
    pub fn render_output(&self, peers: &Peers) -> PeerConnectionsOutput {
        let kind = PeerDisplay::classify(peers, &self.peer_id);
        let ws_addr = crate::listener_state::read_address(peers);
        let bound_peer = BoundPeerInfo {
            peer_id: self.peer_id.clone(),
            short_pid: crate::views::display_name(peers, &self.peer_id),
            kind,
            ws_listen_addr: if kind == PeerDisplay::Primary { ws_addr.clone() } else { None },
        };

        // Known peers = the remembered-peer registry, enriched with a
        // reconnect address (§13.2). Display name resolved per entry.
        let known_peers: Vec<KnownPeer> = crate::connections::read_connections(peers)
            .into_iter()
            .map(|r| KnownPeer {
                display: crate::views::display_name(peers, &r.remote_pid),
                remote_pid: r.remote_pid,
                addr: r.addr,
                last_seen: r.last_seen,
            })
            .collect();

        // Authorize-gate observability (§3 Step 3-4): for each known backend,
        // project its locally-mirrored observation (written by the async remote
        // read) into a render-ready view. Absent mirror ⇒ `checked: false` (a
        // "Refresh to check" prompt), never a misleading empty list.
        let sys_pid = peers.system_peer_id().to_string();
        let backend_auth: Vec<BackendAuthView> = known_peers
            .iter()
            .map(|kp| {
                let path = crate::app_paths::backend_auth_entry_path(
                    crate::app_paths::APP_ID,
                    &sys_pid,
                    &kp.remote_pid,
                );
                let obs = peers
                    .get_entity(&sys_pid, &path)
                    .map(|e| crate::backend_auth::BackendAuthObservation::from_entity(&e));
                match obs {
                    Some(o) => BackendAuthView {
                        backend_pid: kp.remote_pid.clone(),
                        backend_display: kp.display.clone(),
                        checked: true,
                        error: o.error.clone(),
                        pending: o.pending().map(auth_row_view).collect(),
                        authorized: o.authorized().map(auth_row_view).collect(),
                    },
                    None => BackendAuthView {
                        backend_pid: kp.remote_pid.clone(),
                        backend_display: kp.display.clone(),
                        checked: false,
                        error: None,
                        pending: Vec::new(),
                        authorized: Vec::new(),
                    },
                }
            })
            .collect();

        let all_pids = peers.peer_ids();
        let mut backend_peers = Vec::new();
        for p in &all_pids {
            let Some(meta) = peers.peer_metadata(p) else { continue };
            if PeerDisplay::classify(peers, p) != PeerDisplay::Remote {
                continue;
            }
            if meta.listen_addresses.is_empty() {
                continue;
            }
            let display = crate::views::display_name(peers, p);
            let connect_addresses = meta
                .listen_addresses
                .iter()
                .map(|a| rewrite_for_browser(a))
                .collect();
            backend_peers.push(BackendPeer {
                peer_id: (*p).to_string(),
                display,
                connect_addresses,
            });
        }

        // QR pairing advertises an address a phone / another device dials
        // to reach a peer *here*. Prefer this process's own native WS
        // listener (the `make native` path, published via listener_state);
        // in Tauri the WebView frontend never binds, so fall back to the
        // system's Tauri-managed backend peer, whose listen address (now
        // the host LAN IP — see src-tauri connectable_addr) arrives via
        // IPC into peer metadata and is surfaced in `backend_peers`.
        //
        // When nothing here listens (e.g. a plain browser with no backend
        // started) there is no useful QR — leave it `None` so the display
        // is hidden. Scanning a QR still works from any peer.
        let qr_payload = ws_addr
            .as_ref()
            .map(|addr| (addr.clone(), self.peer_id.clone()))
            .or_else(|| {
                backend_peers.iter().find_map(|bp| {
                    bp.connect_addresses
                        .first()
                        .map(|a| (a.clone(), bp.peer_id.clone()))
                })
            })
            .map(|(addr, pid)| format!("{}|{}", addr, pid));

        PeerConnectionsOutput {
            window_id: self.window_id,
            bound_peer,
            known_peers,
            backend_peers,
            address_input_initial: self.inner.lock().unwrap().address.clone(),
            qr_payload,
            backend_auth,
        }
    }

    #[cfg(test)]
    pub fn state_snapshot(&self) -> PeerConnectionsState {
        self.inner.lock().unwrap().clone()
    }
}

/// Project a derived auth row into its render view. The peer key is an
/// identity-hash hex string (how the backend reports sessions); we keep the
/// full id for the authorize target and show a truncated form.
fn auth_row_view(row: &crate::peer_auth::PeerAuthRow) -> AuthRowView {
    let display = if row.peer_id.len() > 14 {
        format!("{}…", &row.peer_id[..14])
    } else {
        row.peer_id.clone()
    };
    AuthRowView {
        peer_id: row.peer_id.clone(),
        display,
    }
}

/// Rewrite a peer-reported address into one that's actually connectable
/// from the browser's network position. Substitutes loopback / wildcard
/// hosts with `window.location.hostname`. Pure passthrough on native.
#[cfg(target_arch = "wasm32")]
fn rewrite_for_browser(addr: &str) -> String {
    let scheme_end = match addr.find("://") {
        Some(idx) => idx + 3,
        None => return addr.to_string(),
    };
    let host_start = scheme_end;
    let after_host = addr[scheme_end..]
        .find(|c: char| c == ':' || c == '/')
        .map(|i| host_start + i)
        .unwrap_or(addr.len());
    let host = &addr[host_start..after_host];

    let needs_substitute = matches!(
        host,
        "0.0.0.0" | "[::]" | "localhost" | "127.0.0.1" | "[::1]"
    );
    if !needs_substitute {
        return addr.to_string();
    }

    let browser_host = match web_sys::window()
        .and_then(|w| w.location().hostname().ok())
        .filter(|h| !h.is_empty())
    {
        Some(h) => h,
        None => return addr.to_string(),
    };

    let mut result = String::with_capacity(addr.len() + browser_host.len());
    result.push_str(&addr[..host_start]);
    result.push_str(&browser_host);
    result.push_str(&addr[after_host..]);
    result
}

#[cfg(not(target_arch = "wasm32"))]
fn rewrite_for_browser(addr: &str) -> String {
    addr.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::ConnectionsWriter;

    #[test]
    fn known_peers_surface_the_remembered_registry_with_reconnect_addr() {
        let peers = Peers::new_direct();
        let pid = peers.system_peer_id().to_string();
        ConnectionsWriter::new(&peers).add("REMOTE_B", "ws://10.0.0.9:4041");

        let model = PeerConnectionsModel::new(7, pid);
        let out = model.render_output(&peers);

        assert_eq!(out.known_peers.len(), 1, "the remembered peer surfaces");
        assert_eq!(out.known_peers[0].remote_pid, "REMOTE_B");
        assert_eq!(
            out.known_peers[0].addr, "ws://10.0.0.9:4041",
            "reconnect address carried through for one-tap reconnect"
        );
    }

    #[test]
    fn known_backend_without_observation_reads_unchecked() {
        let peers = Peers::new_direct();
        let pid = peers.system_peer_id().to_string();
        ConnectionsWriter::new(&peers).add("REMOTE_B", "ws://10.0.0.9:4041");

        let model = PeerConnectionsModel::new(7, pid);
        let out = model.render_output(&peers);

        assert_eq!(out.backend_auth.len(), 1, "one view per known backend");
        let view = &out.backend_auth[0];
        assert_eq!(view.backend_pid, "REMOTE_B");
        assert!(!view.checked, "no observation yet → prompts a Check access");
        assert!(view.pending.is_empty() && view.authorized.is_empty());
    }

    #[test]
    fn observation_mirror_projects_into_pending_and_authorized_rows() {
        use crate::backend_auth::{BackendAuthObservation, BackendAuthWriter};
        use crate::peer_auth::{AuthState, PeerAuthRow};

        let peers = Peers::new_direct();
        let pid = peers.system_peer_id().to_string();
        ConnectionsWriter::new(&peers).add("REMOTE_B", "ws://10.0.0.9:4041");
        // The async remote read would write this mirror; do it directly here.
        BackendAuthWriter::new(&peers).record(&BackendAuthObservation::ok(
            "REMOTE_B",
            vec![
                PeerAuthRow { peer_id: "aaaa1111".into(), state: AuthState::Pending },
                PeerAuthRow { peer_id: "bbbb2222".into(), state: AuthState::Authorized },
            ],
        ));

        let model = PeerConnectionsModel::new(7, pid);
        let view = &model.render_output(&peers).backend_auth[0];

        assert!(view.checked, "mirror present → checked");
        assert_eq!(view.error, None);
        assert_eq!(view.pending.len(), 1);
        assert_eq!(view.pending[0].peer_id, "aaaa1111", "authorize target keeps the full id");
        assert_eq!(view.authorized.len(), 1);
        assert_eq!(view.authorized[0].peer_id, "bbbb2222");
    }

    #[test]
    fn observation_read_failure_surfaces_error() {
        use crate::backend_auth::{BackendAuthObservation, BackendAuthWriter};

        let peers = Peers::new_direct();
        let pid = peers.system_peer_id().to_string();
        ConnectionsWriter::new(&peers).add("REMOTE_B", "ws://10.0.0.9:4041");
        BackendAuthWriter::new(&peers)
            .record(&BackendAuthObservation::failed("REMOTE_B", "no manager capability"));

        let model = PeerConnectionsModel::new(7, pid);
        let view = &model.render_output(&peers).backend_auth[0];

        assert!(view.checked);
        assert_eq!(view.error.as_deref(), Some("no manager capability"));
        assert!(view.pending.is_empty(), "an error must not read as pending rows");
    }
}

/// Generate an SVG for the given QR payload. Pure utility — used by
/// the renderer.
#[allow(dead_code)] // called from WASM render path only
pub fn generate_qr_svg(payload: &str) -> String {
    match qrcode::QrCode::new(payload.as_bytes()) {
        Ok(code) => code
            .render::<qrcode::render::svg::Color>()
            .quiet_zone(true)
            .dark_color(qrcode::render::svg::Color("#000000"))
            .light_color(qrcode::render::svg::Color("#ffffff"))
            .build(),
        Err(_) => "<p>Failed to generate QR code</p>".into(),
    }
}

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

use super::output::{BackendPeer, BoundPeerInfo, PeerConnectionsOutput};

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

        let connected: Vec<String> = crate::connections::read_connected(peers);

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

        let qr_payload = match &ws_addr {
            Some(addr) => format!("{}|{}", addr, self.peer_id),
            None => format!("no-listener|{}", self.peer_id),
        };

        PeerConnectionsOutput {
            window_id: self.window_id,
            bound_peer,
            connected,
            backend_peers,
            address_input_initial: self.inner.lock().unwrap().address.clone(),
            qr_payload,
        }
    }

    #[cfg(test)]
    pub fn state_snapshot(&self) -> PeerConnectionsState {
        self.inner.lock().unwrap().clone()
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

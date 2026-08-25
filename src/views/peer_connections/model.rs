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

use super::output::{BackendPeer, BoundPeerInfo, KnownPeer, PeerConnectionsOutput};

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
        peers.seed_state_if_absent(
            &self.peer_id,
            path,
            PeerConnectionsState::default().to_entity(),
            "peer_connections",
        );
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
    // (No `set_address`: the in-progress address is a renderer-side draft —
    // `components::text_input` + `ctx.drafts` — not model state.)

    pub fn clear_address(&self) {
        self.inner.lock().unwrap().address.clear();
    }

    pub fn save_state(&self, peers: &Peers) {
        self.persist_state(peers);
    }

    // -- Pure read API --

    #[allow(dead_code)] // called from WASM render path
    pub fn render_output(
        &self,
        peers: &Peers,
        dials: &crate::dial_markers::DialMarkers,
        attempt: &crate::connect_attempt::ConnectAttempt,
    ) -> PeerConnectionsOutput {
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
        //
        // The system backend is **shown here like any other device**. It used
        // to be filtered out as "infrastructure managed elsewhere", which was
        // wrong in two independent ways. (a) Being auto-connected is not a
        // reason to be invisible — the user still needs to see whether the link
        // is actually up, and still needs a way to re-dial it when it isn't.
        // (b) Worse, the filter silently swallowed the *result of the user's own
        // action*: a manual connect to the backend's address succeeds, writes
        // its registry row here, and the row was then filtered straight back
        // out — a connect that fully worked was indistinguishable from one that
        // did nothing at all (D13; the reported bug).
        let mut known_peers: Vec<KnownPeer> = crate::connections::read_connections(peers)
            .into_iter()
            .map(|r| KnownPeer {
                display: crate::views::display_name(peers, &r.remote_pid),
                // Kernel read-model is authoritative for real liveness; the
                // in-memory dial marker contributes only the app-owned dial
                // transient (a dial in flight / gave-up-before-connecting) the
                // kernel does not model — and only while the kernel is silent.
                status: crate::peer_liveness::conn_display(
                    crate::peer_liveness::liveness_of(peers, &r.remote_pid),
                    dials.hint(&r.remote_pid),
                ),
                remote_pid: r.remote_pid,
                addr: r.addr,
                last_seen: r.last_seen,
                // A real registry entry — Forget drops it and the row goes.
                forgettable: true,
            })
            .collect();

        // (Device authorizations moved to the System Backend window, Direction
        // A — it binds to the system peer, the manager that holds B's grant.)

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

        // Surface every reachable backend (the auto-connected system backend
        // included) as a device row, even when it has no remembered-registry
        // entry. It usually has none: `connections::add` runs only on a MANUAL
        // connect (`handle_connect_peer`) or the shell verb, while the boot
        // auto-connect goes through `maintain-peer`, which writes no registry
        // row. So the registry alone renders an empty list on a machine whose
        // backend is up and connected — the user sees nothing while the link is
        // fine. Metadata + the kernel liveness read-model is what actually knows.
        //
        // Deduped by peer id, registry row wins: it carries the address the user
        // actually dialed and a real `last_seen`.
        for bp in &backend_peers {
            if known_peers.iter().any(|k| k.remote_pid == bp.peer_id) {
                continue;
            }
            known_peers.push(KnownPeer {
                remote_pid: bp.peer_id.clone(),
                display: bp.display.clone(),
                addr: bp.connect_addresses.first().cloned().unwrap_or_default(),
                // No successful *manual* connect is recorded for an
                // auto-connected peer, and inventing a timestamp would be a
                // lie in a field named `last_seen`.
                last_seen: 0,
                status: crate::peer_liveness::conn_display(
                    crate::peer_liveness::liveness_of(peers, &bp.peer_id),
                    dials.hint(&bp.peer_id),
                ),
                // Derived from live metadata, not the registry: there is nothing
                // for Forget to remove, and the row would survive the click.
                forgettable: false,
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
            last_attempt: attempt.read(),
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
pub(crate) fn rewrite_for_browser(addr: &str) -> String {
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
pub(crate) fn rewrite_for_browser(addr: &str) -> String {
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
        let out = model.render_output(
            &peers,
            &crate::dial_markers::DialMarkers::new(),
            &crate::connect_attempt::ConnectAttempt::new(),
        );

        assert_eq!(out.known_peers.len(), 1, "the remembered peer surfaces");
        assert_eq!(out.known_peers[0].remote_pid, "REMOTE_B");
        assert_eq!(
            out.known_peers[0].addr, "ws://10.0.0.9:4041",
            "reconnect address carried through for one-tap reconnect"
        );
    }

    /// The system backend must NOT be filtered out of the device list.
    ///
    /// It used to be, as "infrastructure managed elsewhere". The cost was that
    /// a manual connect to the backend's address succeeded, wrote its registry
    /// row, and had the row filtered straight back out — the user's own action
    /// produced no visible change anywhere, which is indistinguishable from a
    /// dead button. Being auto-connected is not a reason to be invisible.
    #[test]
    fn the_system_backend_is_shown_like_any_other_device() {
        let peers = Peers::new_direct();
        let pid = peers.system_peer_id().to_string();
        let backend = "REMOTE_SYSTEM_BACKEND";
        ConnectionsWriter::new(&peers).add(backend, "ws://192.168.68.55:4041");

        let model = PeerConnectionsModel::new(7, pid);
        let out = model.render_output(
            &peers,
            &crate::dial_markers::DialMarkers::new(),
            &crate::connect_attempt::ConnectAttempt::new(),
        );

        assert!(
            out.known_peers.iter().any(|k| k.remote_pid == backend),
            "the system backend must appear in the device list — it was filtered \
             out before, which silently swallowed the result of a manual connect \
             to it. Got: {:?}",
            out.known_peers.iter().map(|k| &k.remote_pid).collect::<Vec<_>>()
        );
    }

    /// A reachable backend shows up **without any remembered-registry row**.
    ///
    /// This is the path that actually matters on the desktop, and the one the
    /// old code could not produce. `connections::add` runs only on a MANUAL
    /// connect or the shell verb; the boot auto-connect goes through
    /// `maintain-peer`, which writes no registry row. So on a machine whose
    /// system backend is up and connected, the registry is empty and the device
    /// list rendered nothing at all — the user sees an empty window while the
    /// link is fine. Metadata + the kernel read-model is what knows.
    #[test]
    fn a_reachable_backend_appears_even_with_an_empty_registry() {
        let mut peers = Peers::new_direct();
        let pid = peers.system_peer_id().to_string();
        let backend = "2KBackendNoRegistryRow".to_string();
        // A backend peer as the Tauri IPC path registers it: its own SDK (so it
        // classifies Remote) and a listen address (so it is dialable).
        peers.register_backend_peer_primary(
            backend.clone(),
            Some(crate::views::system_overview::model::SYSTEM_BACKEND_LABEL.to_string()),
            vec!["ws://192.168.68.55:4041".to_string()],
        );

        let model = PeerConnectionsModel::new(7, pid);
        let out = model.render_output(
            &peers,
            &crate::dial_markers::DialMarkers::new(),
            &crate::connect_attempt::ConnectAttempt::new(),
        );

        let row = out
            .known_peers
            .iter()
            .find(|k| k.remote_pid == backend)
            .unwrap_or_else(|| {
                panic!(
                    "the system backend is up and reachable but rendered NO device row \
                     (the registry is empty because auto-connect writes none). Got: {:?}",
                    out.known_peers.iter().map(|k| &k.remote_pid).collect::<Vec<_>>()
                )
            });
        assert_eq!(
            row.addr, "ws://192.168.68.55:4041",
            "the row must carry a dialable address, or its Reconnect button can't work"
        );
        assert!(
            !row.forgettable,
            "a metadata-derived row has no registry entry, so Forget would remove \
             nothing and the row would survive the click — the renderer must not \
             offer it. A visible no-op is the same disease as the silent Connect."
        );
    }

    /// The connect outcome reaches the render output, so the window can report
    /// what the button did. Without this the action is silent in BOTH
    /// directions — the reported bug.
    #[test]
    fn the_last_connect_outcome_reaches_the_output() {
        use crate::connect_attempt::{ConnectAttempt, ConnectOutcome};

        let peers = Peers::new_direct();
        let pid = peers.system_peer_id().to_string();
        let model = PeerConnectionsModel::new(7, pid);
        let dials = crate::dial_markers::DialMarkers::new();

        // Nothing pressed yet ⇒ nothing claimed.
        let attempt = ConnectAttempt::new();
        assert_eq!(
            model.render_output(&peers, &dials, &attempt).last_attempt,
            None,
            "an untouched window must not assert an outcome it never had"
        );

        // A failure carries its reason all the way to the renderer.
        attempt.set_failed("ws://10.0.0.9:4041", "connection refused");
        assert_eq!(
            model.render_output(&peers, &dials, &attempt).last_attempt,
            Some((
                "ws://10.0.0.9:4041".to_string(),
                ConnectOutcome::Failed("connection refused".to_string())
            ))
        );

        // …and so does a success, which is the case that used to vanish.
        attempt.set_connected("ws://192.168.68.55:4041", "system-backend");
        assert_eq!(
            model.render_output(&peers, &dials, &attempt).last_attempt,
            Some((
                "ws://192.168.68.55:4041".to_string(),
                ConnectOutcome::Connected("system-backend".to_string())
            ))
        );
    }

    // (The device-authorization projection tests moved with the surface to
    // `views::system_overview::model`.)
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
        // Not escaped: the value is a catalog string, not user input, and this
        // module compiles on native too — `dom::util::escape_html` is wasm-only.
        Err(_) => format!("<p>{}</p>", crate::i18n::t("peerconn.qr_failed", &[])),
    }
}

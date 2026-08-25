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
    /// Outcome of the last connector `Check`. Deliberately **in-memory**, not
    /// tree-backed: it is the result of an action in progress, meaningless
    /// across a reload — the same reasoning that keeps `dial_markers` out of
    /// the tree.
    connector_notice: Arc<Mutex<Option<crate::views::peer_connections::output::ConnectorNotice>>>,
    /// The meet in progress (`crate::rendezvous`). In memory for the same
    /// reason as the notice above — a search is an action in progress, gone on
    /// reload, and a tree-backed one would come back as a search nobody started.
    meet: Arc<Mutex<Option<crate::rendezvous::MeetSession>>>,
    /// Why the last Meet press did nothing, when it did nothing.
    meet_notice: Arc<Mutex<Option<crate::views::peer_connections::output::ConnectorNotice>>>,
}

impl PeerConnectionsModel {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        Self {
            window_id,
            peer_id,
            inner: Arc::new(Mutex::new(PeerConnectionsState::default())),
            connector_notice: Arc::new(Mutex::new(None)),
            meet: Arc::new(Mutex::new(None)),
            meet_notice: Arc::new(Mutex::new(None)),
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
        connector_reload_pending: bool,
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

        // The connector registry lives on the SYSTEM peer (it is deployment
        // infrastructure, not per-window state), which is also the peer
        // `connectors::ConnectorRegistry` watches and mirrors from.
        let sys_pid = peers.system_peer_id().to_string();
        let selected = crate::connectors::selected_connector(peers, &sys_pid)
            .map(|c| c.node_peer_id);
        let connectors = crate::connectors::read_connectors(peers, &sys_pid)
            .into_iter()
            .map(|c| crate::views::peer_connections::output::ConnectorRow {
                short_pid: crate::views::short_pid(&c.node_peer_id),
                selected: Some(&c.node_peer_id) == selected.as_ref(),
                node_peer_id: c.node_peer_id,
                node_addr: c.node_addr,
                label: c.label,
            })
            .collect();

        // The Meet section. `remembered` is resolved against the rows above, so
        // a peer already in the registry is not offered a Remember that would
        // change nothing visible.
        let meet = {
            let selected_node = selected.clone();
            let status = self.meet.lock().ok().and_then(|s| s.as_ref().map(|s| s.status()));
            crate::views::peer_connections::output::MeetPanel {
                has_connector: selected_node.is_some(),
                node_short: selected_node
                    .as_deref()
                    .map(crate::views::short_pid)
                    .unwrap_or_default(),
                status: status.map(|st| {
                    crate::views::peer_connections::output::MeetStatusRow {
                        mode_name: st.mode_name.to_string(),
                        mode_input: st.mode_input,
                        searching: !st.phase.is_settled(),
                        error: match &st.phase {
                            crate::rendezvous::MeetPhase::Failed(e) => Some(e.clone()),
                            _ => None,
                        },
                        polls: st.polls,
                        max_polls: st.max_polls,
                        found: st
                            .found
                            .into_iter()
                            .map(|d| crate::views::peer_connections::output::MeetFoundRow {
                                short_pid: crate::views::short_pid(&d.peer_id),
                                remembered: known_peers.iter().any(|k| k.remote_pid == d.peer_id),
                                peer_id: d.peer_id,
                                verified: d.verified,
                            })
                            .collect(),
                    }
                }),
                notice: self.meet_notice.lock().ok().and_then(|n| n.clone()),
            }
        };

        PeerConnectionsOutput {
            window_id: self.window_id,
            bound_peer,
            known_peers,
            backend_peers,
            address_input_initial: self.inner.lock().unwrap().address.clone(),
            last_attempt: attempt.read(),
            qr_payload,
            connectors,
            connector_reload_pending,
            connector_notice: self.connector_notice.lock().unwrap().clone(),
            meet,
        }
    }

    // -- Meet (crate::rendezvous) --

    /// Start a search at `mode` through the selected connector. Replaces any
    /// previous one — a second press means "look for this instead".
    ///
    /// The connector comes from the **system** peer's registry (deployment
    /// infrastructure) while the search runs from this window's **bound** peer:
    /// that is the peer whose pool carries the calls, and whose id we announce,
    /// so it is the identity a counterpart comes away with.
    pub fn start_meet(&self, peers: &Peers, mode: crate::rendezvous::Mode) -> Result<(), String> {
        let sys = peers.system_peer_id().to_string();
        let node = crate::connectors::selected_connector(peers, &sys)
            .ok_or_else(|| crate::i18n::t("peerconn.meet_needs_connector", &[]))?;
        let session = crate::rendezvous::MeetSession::start(&self.peer_id, node, mode);
        if let Ok(mut slot) = self.meet.lock() {
            *slot = Some(session);
        }
        Ok(())
    }

    /// Publish (or clear) the meet form's refusal. `None` clears.
    pub fn set_meet_notice(&self, notice: Option<(String, bool)>) {
        if let Ok(mut slot) = self.meet_notice.lock() {
            *slot = notice.map(|(text, is_error)| {
                crate::views::peer_connections::output::ConnectorNotice { text, is_error }
            });
        }
    }

    /// End the search, keeping what it found.
    pub fn stop_meet(&self) {
        if let Ok(slot) = self.meet.lock() {
            if let Some(s) = slot.as_ref() {
                s.stop();
            }
        }
    }

    /// One frame of progress for a running meet. Called from the window's
    /// `tick` — the frame loop is where the `&Peers` a round trip needs exists.
    ///
    /// Returns `true` when the visible status changed, so the caller can mark
    /// the window dirty: a meet writes nothing to the tree, so nothing else
    /// would ever repaint it.
    pub fn pump_meet(&self, peers: &Peers) -> bool {
        let Ok(mut slot) = self.meet.lock() else { return false };
        let Some(session) = slot.as_mut() else { return false };
        session.pump(peers);
        // Ask the session, do NOT diff the status around the pump: nearly every
        // change lands in a spawned round trip *between* frames, so a diff sees
        // before == after and reports "nothing happened". That is precisely how
        // a meet whose dial had already failed kept rendering "Searching…"
        // (e2e Phase 14.7).
        session.take_changed()
    }

    /// Shared handle to the `Check` outcome, so the async `advertise` landing
    /// off-frame can publish into the next render.
    pub fn connector_notice_handle(
        &self,
    ) -> Arc<Mutex<Option<crate::views::peer_connections::output::ConnectorNotice>>> {
        self.connector_notice.clone()
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
        // A real PeerID: the reconnect address now resolves from the kernel's
        // route entity, whose path segment derives from the identity hash.
        let remote = entity_crypto::Keypair::generate().peer_id().to_string();
        ConnectionsWriter::new(&peers).add(&remote);
        crate::transport_profiles::publish_dialed(
            &peers.writer_handle().expect("Direct writer handle"),
            &pid,
            &remote,
            "ws://10.0.0.9:4041",
            None,
        );

        let model = PeerConnectionsModel::new(7, pid);
        let out = model.render_output(
            &peers,
            &crate::dial_markers::DialMarkers::new(),
            &crate::connect_attempt::ConnectAttempt::new(),
            false,
        );

        assert_eq!(out.known_peers.len(), 1, "the remembered peer surfaces");
        assert_eq!(out.known_peers[0].remote_pid, remote);
        assert_eq!(
            out.known_peers[0].addr, "ws://10.0.0.9:4041",
            "one-tap reconnect reads its address from the route, not the row"
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
        ConnectionsWriter::new(&peers).add(backend);

        let model = PeerConnectionsModel::new(7, pid);
        let out = model.render_output(
            &peers,
            &crate::dial_markers::DialMarkers::new(),
            &crate::connect_attempt::ConnectAttempt::new(),
            false,
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
            false,
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
            model.render_output(&peers, &dials, &attempt, false).last_attempt,
            None,
            "an untouched window must not assert an outcome it never had"
        );

        // A failure carries its reason all the way to the renderer.
        attempt.set_failed("ws://10.0.0.9:4041", "connection refused");
        assert_eq!(
            model.render_output(&peers, &dials, &attempt, false).last_attempt,
            Some((
                "ws://10.0.0.9:4041".to_string(),
                ConnectOutcome::Failed("connection refused".to_string())
            ))
        );

        // …and so does a success, which is the case that used to vanish.
        attempt.set_connected("ws://192.168.68.55:4041", "system-backend");
        assert_eq!(
            model.render_output(&peers, &dials, &attempt, false).last_attempt,
            Some((
                "ws://192.168.68.55:4041".to_string(),
                ConnectOutcome::Connected("system-backend".to_string())
            ))
        );
    }

    /// A meet with no connector selected refuses **where the user can see it**,
    /// rather than starting a search that can never reach a node.
    #[test]
    fn meet_without_a_connector_is_refused_not_started() {
        let peers = Peers::new_direct();
        let pid = peers.system_peer_id().to_string();
        let model = PeerConnectionsModel::new(7, pid);

        let refused = model.start_meet(&peers, crate::rendezvous::Mode::Tag("chess".into()));
        assert!(refused.is_err(), "no connector ⇒ no search");

        let out = model.render_output(
            &peers,
            &crate::dial_markers::DialMarkers::new(),
            &crate::connect_attempt::ConnectAttempt::new(),
            false,
        );
        assert!(!out.meet.has_connector, "the form says why it can't run");
        assert!(out.meet.status.is_none(), "and no search is claimed to be running");
    }

    /// **A meet change that lands between frames still wakes the window.**
    ///
    /// A meet writes nothing to the tree, so the only thing that repaints this
    /// window is `pump_meet` returning `true`. Nearly every change lands in a
    /// *spawned* round trip — between pumps — so a `pump_meet` that diffed the
    /// status before and after its own `pump` call saw them equal and reported
    /// "nothing happened". The window then kept rendering "Searching…" for a
    /// meet whose dial had already failed, indefinitely. e2e Phase 14.7 caught
    /// it; this is the native gate that keeps it caught.
    ///
    /// Written as: on the frame the search settles, `pump_meet` must have said
    /// so. Revert to the diff and it fails on exactly that.
    #[tokio::test]
    async fn a_meet_outcome_that_lands_between_frames_still_wakes_the_window() {
        use crate::connectors::{self, Connector};
        use crate::rendezvous::Mode;
        use entity_peer::transport::{MemoryConnector, MemoryTransportRegistry};

        let registry = MemoryTransportRegistry::new();
        let peers =
            Peers::new_direct_with_connector(std::sync::Arc::new(MemoryConnector::new(registry)));
        let pid = peers.primary_peer_id().to_string();
        let sys = peers.system_peer_id().to_string();
        // A node nobody is listening on: the dial fails, and it fails inside a
        // spawned future — which is the whole point.
        let ghost = Connector {
            node_peer_id: "2KNobodyHome".to_string(),
            node_addr: "memory://2KNobodyHome".to_string(),
            label: String::new(),
        };
        connectors::add_connector(&peers, &sys, &ghost).expect("add");
        // Both halves need waiting on: `select_connector` refuses until the
        // row it names is readable, and the selection it then writes is itself
        // a dispatched write. Polling for the readable end covers both.
        for _ in 0..400 {
            let _ = connectors::select_connector(&peers, &sys, "2KNobodyHome");
            if connectors::selected_connector(&peers, &sys).is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        let model = PeerConnectionsModel::new(7, pid);
        model.start_meet(&peers, Mode::Tag("chess".into())).expect("a connector is selected");

        let dials = crate::dial_markers::DialMarkers::new();
        let attempt = crate::connect_attempt::ConnectAttempt::new();
        let mut said_so = None;
        for _ in 0..2000 {
            let changed = model.pump_meet(&peers);
            let settled = model
                .render_output(&peers, &dials, &attempt, false)
                .meet
                .status
                .is_some_and(|s| !s.searching);
            if settled {
                said_so = Some(changed);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }

        assert_eq!(
            said_so,
            Some(true),
            "the meet settled and the pump reported no change — nothing would have \
             repainted the window, and it would still be showing 'Searching…'"
        );
    }

    /// The panel reports the peer a real meet found, and knows whether it is
    /// already remembered — the flag that decides whether `Remember` is offered
    /// at all. A button that would change nothing visible is a dead button.
    #[tokio::test]
    async fn the_meet_panel_reports_what_the_search_found() {
        use crate::connectors::{self, Connector};
        use crate::rendezvous::{MeetSession, Mode};
        use entity_peer::transport::{MemoryConnector, MemoryTransportRegistry};

        let registry = MemoryTransportRegistry::new();
        let (node_pid, node) =
            connectors::tests::spawn_signaling_node(registry.clone(), "pool-seven");
        let row = Connector {
            node_peer_id: node_pid.clone(),
            node_addr: format!("memory://{node_pid}"),
            label: String::new(),
        };

        let peers = Peers::new_direct_with_connector(std::sync::Arc::new(MemoryConnector::new(
            registry.clone(),
        )));
        let pid = peers.primary_peer_id().to_string();
        let sys = peers.system_peer_id().to_string();
        connectors::add_connector(&peers, &sys, &row).expect("add");
        for _ in 0..400 {
            let _ = connectors::select_connector(&peers, &sys, &node_pid);
            if connectors::selected_connector(&peers, &sys).is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        let other = Peers::new_direct_with_connector(std::sync::Arc::new(MemoryConnector::new(
            registry.clone(),
        )));
        let other_pid = other.primary_peer_id().to_string();
        let mut other_session =
            MeetSession::start(&other_pid, row.clone(), Mode::Tag("chess".into()));
        tokio::task::yield_now().await;

        let model = PeerConnectionsModel::new(7, pid);
        model.start_meet(&peers, Mode::Tag("chess".into())).expect("a connector is selected");

        let dials = crate::dial_markers::DialMarkers::new();
        let attempt = crate::connect_attempt::ConnectAttempt::new();
        let render = || model.render_output(&peers, &dials, &attempt, false);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut row_seen = false;
        while std::time::Instant::now() < deadline && !row_seen {
            for _ in 0..40 {
                model.pump_meet(&peers);
                other_session.pump(&other);
            }
            row_seen = render()
                .meet
                .status
                .is_some_and(|s| s.found.iter().any(|f| f.peer_id == other_pid));
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(row_seen, "the panel never showed the peer the meet found");

        let out = render();
        let found = out.meet.status.unwrap();
        let peer = found.found.iter().find(|f| f.peer_id == other_pid).unwrap();
        assert!(
            !peer.remembered,
            "a meet remembers nothing on its own — the user decides who to keep"
        );

        // Once remembered, the row must stop offering Remember.
        crate::connections::ConnectionsWriter::new(&peers).add(&other_pid);
        let mut flipped = false;
        for _ in 0..200 {
            if render()
                .meet
                .status
                .is_some_and(|s| s.found.iter().any(|f| f.peer_id == other_pid && f.remembered))
            {
                flipped = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(flipped, "a remembered peer's row still offered Remember");

        node.abort();
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

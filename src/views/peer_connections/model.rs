//! Peer Connections model — mirrored shape.
//!
//! Per-window state: just the manual address input value (persisted
//! to the tree). Render data (connected peers, backend listings, WS
//! listen address, QR payload) is read from `Peers` on demand —
//! these change asynchronously from the window's perspective and
//! don't benefit from caching.

use std::sync::{Arc, Mutex};

use entity_entity::Entity;
use std::sync::atomic::{AtomicBool, Ordering};

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

/// Entity type of this window's persisted state. Window ids are reused across
/// a reload, so a foreign type's state can sit at this path — see
/// [`crate::views::entity_tree::model::STATE_TYPE`] for the full reason.
pub const STATE_TYPE: &str = "app/state/peer_connections";

impl PeerConnectionsState {
    pub fn from_entity(entity: &Entity) -> Self {
        if entity.entity_type != STATE_TYPE {
            return Self::default();
        }
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
        Entity::new(STATE_TYPE, data).unwrap()
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
    /// Did `initialize`'s synchronous read actually answer? On the Direct arm
    /// the in-process store is authoritative, so a surface hydrated there needs
    /// no round-trip (AP41).
    hydrated: Arc<AtomicBool>,
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
    /// Where the last *Find peers here* press has got to. In memory, like the
    /// meet it starts.
    find: Arc<Mutex<Option<FindPeers>>>,
}

/// **Where a *Find peers here* press has got to.**
///
/// The press is three steps the user used to perform by hand, in three cards:
/// dial the address, add the same address as a rendezvous node (and select it),
/// then open *Meet at a name* and start a lobby meet. Each step reports here, in
/// the card that has the button, because the whole point is that the user no
/// longer has to go looking for which card moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindPeers {
    /// Dialing the address; the rendezvous row is written when it answers.
    Adding { addr: String },
    /// The node answered and its row is written. Waiting for it to be the node
    /// in force before the meet starts — the write is dispatched, and a meet
    /// started a frame early would run at whatever node was in force before.
    Switching { addr: String, node: String, frames_left: u32 },
    /// A lobby meet is running at the node.
    Meeting { addr: String, node: String },
    /// Stopped, with the reason in the words of the step that failed.
    Failed { addr: String, reason: String },
}

/// How many frames [`FindPeers::Switching`] waits for the selection to land
/// before it gives up. About ten seconds at 60 fps; the Direct arm lands it on
/// the next frame, the Worker arm on the next mirror event.
pub const FIND_SWITCH_FRAMES: u32 = 600;

/// One frame of [`FindPeers::Switching`], decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindStep {
    /// The node we added is not in force yet; keep waiting.
    Wait,
    /// It is in force — start the lobby meet now.
    StartMeet,
    /// It never became the node in force. Saying so beats a meet at a
    /// different node, which is the silent version of the same failure.
    GiveUp,
}

/// Decide a [`FindPeers::Switching`] frame. Pure, so the ordering — *in force*
/// outranks *out of time* — is gated natively.
pub fn find_step(node: &str, in_force: Option<&str>, frames_left: u32) -> FindStep {
    if in_force == Some(node) {
        return FindStep::StartMeet;
    }
    if frames_left == 0 {
        return FindStep::GiveUp;
    }
    FindStep::Wait
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
            find: Arc::new(Mutex::new(None)),
            hydrated: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn initialize(&mut self, peers: &Peers) {
        self.ensure_state_in_tree(peers);
        // Best-effort: on the Worker arm this reads nothing, and on Direct-IDB
        // it races the store filling from IndexedDB (AP41).
        // `hydrate_durable` is the authoritative correction.
        let persisted = self.read_window_state_opt(peers);
        self.hydrated.store(persisted.is_some(), Ordering::SeqCst);
        *self.inner.lock().unwrap() = persisted.unwrap_or_default();
    }

    /// AP41's correction, through the shared machinery
    /// (`crate::window_hydration`), which owns the three traps.
    ///
    /// **Assign, not merge:** every field of `PeerConnectionsState` appears in `to_entity`, so a
    /// decoded state is a complete state and there is no session-only half to
    /// preserve. Verified field-by-field 2026-08-31; if a non-persisted field is
    /// ever added to this struct, this becomes a merge.
    fn hydration_job(
        &self,
        peers: &Peers,
    ) -> Option<impl std::future::Future<Output = crate::window::Hydration> + 'static> {
        let inner = self.inner.clone();
        let w = self.inner.clone();
        crate::window_hydration::durable_hydration_job(
            peers,
            &self.peer_id,
            self.state_path(peers),
            STATE_TYPE,
            self.hydrated.clone(),
            move || w.lock().unwrap().to_entity().data,
            move |e| *inner.lock().unwrap() = PeerConnectionsState::from_entity(e),
        )
    }

    #[cfg(test)]
    pub async fn hydrate_durable(&self, peers: &Peers) -> crate::window::Hydration {
        match self.hydration_job(peers) {
            Some(job) => job.await,
            None => crate::window::Hydration::AlreadyResolved,
        }
    }

    /// [`hydration_job`](Self::hydration_job), driven in the background — the
    /// window factory runs inside the synchronous frame loop and cannot await.
    #[cfg(target_arch = "wasm32")]
    pub fn spawn_hydrate_durable(&self, peers: &Peers) {
        crate::window_hydration::spawn_hydration(self.hydration_job(peers));
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

    /// The persisted state, and **whether the tree actually held ours**.
    ///
    /// `None` covers two cases that are one fact here: nothing at this path, or
    /// another window type's entity in our reused slot (AP42). The type check
    /// has to be here rather than left to `from_entity`, because that reports a
    /// mismatch as `Default` — indistinguishable from a successful read of a
    /// default-valued state, which would make `hydrated` claim an answer it
    /// never got.
    fn read_window_state_opt(&self, peers: &Peers) -> Option<PeerConnectionsState> {
        peers
            .get_entity(&self.peer_id, &self.state_path(peers))
            .filter(|e| e.entity_type == STATE_TYPE)
            .map(|e| PeerConnectionsState::from_entity(&e))
    }

    fn read_window_state(&self, peers: &Peers) -> PeerConnectionsState {
        self.read_window_state_opt(peers).unwrap_or_default()
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
        // The node the Meet panel can actually use. NOT the same question as
        // which ROW is selected: a session provisioned by URL (the link this
        // desktop serves) or by the build knob (`make pair-serve`) has a working
        // establisher and an EMPTY registry, and gating Meet on a row told those
        // users "add a connector" while they were already rendezvousing through
        // one. The row list above keeps using `selected` — a synthesized node is
        // not a row, and marking it as one would offer a Remove that removes
        // nothing.
        let in_force = crate::connectors::node_in_force(peers, &sys_pid);
        let node_in_force = in_force.as_ref().map(|c| c.node_peer_id.clone());
        let connectors = connector_rows(
            crate::connectors::read_connectors(peers, &sys_pid),
            selected.as_deref(),
            in_force.as_ref(),
        );

        // The Meet section. `remembered` is resolved against the rows above, so
        // a peer already in the registry is not offered a Remember that would
        // change nothing visible.
        let meet = {
            let selected_node = node_in_force.clone();
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
                        time_left:
                            crate::views::peer_connections::output::TimeLeft::from_remaining_ms(
                                st.remaining_ms,
                                !st.phase.is_settled(),
                            ),
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
            find: self.find.lock().unwrap().clone(),
        }
    }

    // -- Find peers here --

    /// Shared handle to the find state, so the async add can move it on.
    pub fn find_handle(&self) -> Arc<Mutex<Option<FindPeers>>> {
        self.find.clone()
    }

    /// One frame of a *Find peers here* press: once the node it added is the one
    /// in force, start a lobby meet there. Returns `true` when the visible state
    /// moved.
    pub fn pump_find(&self, peers: &Peers) -> bool {
        let (addr, node, frames_left) = match self.find.lock().ok().and_then(|f| f.clone()) {
            Some(FindPeers::Switching { addr, node, frames_left }) => (addr, node, frames_left),
            _ => return false,
        };
        let sys = peers.system_peer_id().to_string();
        let in_force = crate::connectors::node_in_force(peers, &sys).map(|c| c.node_peer_id);
        let next = match find_step(&node, in_force.as_deref(), frames_left) {
            FindStep::Wait => {
                if let Ok(mut f) = self.find.lock() {
                    *f = Some(FindPeers::Switching { addr, node, frames_left: frames_left - 1 });
                }
                return false;
            }
            FindStep::GiveUp => FindPeers::Failed {
                reason: crate::i18n::t("peerconn.find_not_in_use", &[("addr", &addr)]),
                addr,
            },
            FindStep::StartMeet => match self.start_meet(peers, crate::rendezvous::Mode::Lobby) {
                Ok(()) => {
                    // The same reachability warning a Meet press gives, in the
                    // same slot — a lobby meet from this button is a meet.
                    self.set_meet_notice(
                        meet_reach_for(peers, &self.peer_id)
                            .message_key()
                            .map(|k| (crate::i18n::t(k, &[]), true)),
                    );
                    FindPeers::Meeting { addr, node }
                }
                Err(reason) => FindPeers::Failed { addr, reason },
            },
        };
        if let Ok(mut f) = self.find.lock() {
            *f = Some(next);
        }
        true
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
        // `node_in_force`, not `selected_connector` — see `render_output`'s note:
        // a URL- or build-provisioned session has a working establisher and no
        // registry row, and asking for a row refused the meet.
        let node = crate::connectors::node_in_force(peers, &sys)
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
        // **Meeting someone IS the intent to talk to them**, so it is where the
        // reach keeper learns its targets — and this window is where a user
        // actually meets, the Shell verb being the developer surface.
        //
        // This was in the Shell's identical loop, with a comment explaining it
        // is load-bearing, and NOT here. The cost is invisible and asymmetric:
        // a browser peer has no listener, so being reachable is an activity it
        // performs. A side that only *serves* — offers a file and waits —
        // dispatches nothing, is therefore never present at the rendezvous, and
        // the puller's offer deposits meet zero collects. So meeting through
        // the GUI left you unreachable while meeting through the Shell did not,
        // and the file gates all drive the Shell.
        //
        // `reach_keeper` is idempotent per `(local, remote)`, so re-registering
        // every pump costs an enum compare once the peer reads `Connected`.
        for found in &session.status().found {
            crate::reach_keeper::global().want(&self.peer_id, &found.peer_id);
        }
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

/// The rendezvous-node list: the registry's rows, plus the node this session is
/// actually using when that is not one of them.
///
/// # Why this is a free function
///
/// The interesting case is **unreachable from `make test` through the model**:
/// `node_in_force` synthesizes a node from URL/build provisioning only on
/// wasm32, and its native shadow is exactly `selected_connector` — so on native
/// `in_force` is always already a row and the insertion below never runs. Pure
/// and separate, the rule is testable everywhere; the same native-shadow split
/// `WebRtcProvisioning` uses against the worker wire types.
///
/// What can be wrong here is the *position*, the *flags*, and whether a node
/// gets listed twice — none of it observable from inside a render.
pub(crate) fn connector_rows(
    registry: Vec<crate::connectors::Connector>,
    selected: Option<&str>,
    in_force: Option<&crate::connectors::Connector>,
) -> Vec<super::output::ConnectorRow> {
    use super::output::{ConnectorRow, ConnectorSource};
    let mut rows: Vec<ConnectorRow> = registry
        .into_iter()
        .map(|c| ConnectorRow {
            short_pid: crate::views::short_pid(&c.node_peer_id),
            selected: Some(c.node_peer_id.as_str()) == selected,
            node_peer_id: c.node_peer_id,
            node_addr: c.node_addr,
            label: c.label,
            source: ConnectorSource::Registry,
        })
        .collect();
    // **The node in force is listed even when it is not a row.** A session
    // provisioned by the served link (or by `make pair-serve`) rendezvous
    // through a node the registry has never heard of, so this list used to
    // render its empty state beside a Meet card that was working — a feature
    // sitting next to an empty list of the thing it supposedly requires.
    let Some(c) = in_force else { return rows };
    if rows.iter().any(|r| r.node_peer_id == c.node_peer_id) {
        // Already a row. Listing it twice would be the opposite bug: the same
        // node offering Use and Remove in one line and not in the next.
        return rows;
    }
    rows.insert(
        0,
        ConnectorRow {
            short_pid: crate::views::short_pid(&c.node_peer_id),
            // In force IS in use. `selected` asks which node this session
            // rendezvous through, and for this row that is true by construction
            // — it is where the answer came from.
            selected: true,
            node_peer_id: c.node_peer_id.clone(),
            node_addr: c.node_addr.clone(),
            label: c.label.clone(),
            source: ConnectorSource::Session,
        },
    );
    rows
}

#[cfg(test)]
mod connector_row_tests {
    use super::connector_rows;
    use crate::connectors::Connector;
    use crate::views::peer_connections::output::ConnectorSource;

    fn node(id: &str, addr: &str) -> Connector {
        Connector {
            node_peer_id: id.to_string(),
            node_addr: addr.to_string(),
            label: String::new(),
            ice: String::new(),
            ice_advertised: String::new(),
            relay: String::new(),
            relay_username: String::new(),
            relay_credential: String::new(),
        }
    }

    /// **A working Meet must not sit above an empty list of the thing it uses.**
    ///
    /// A browser that arrived by the link this desktop serves is provisioned
    /// before boot and never writes a registry row — the ordinary case, and the
    /// whole point of the served URL. The list rendered "no rendezvous nodes
    /// yet" underneath a Meet card that was rendezvousing fine.
    #[test]
    fn the_node_this_session_uses_is_listed_even_with_an_empty_registry() {
        let live = node("2KFromTheLink", "ws://192.168.1.10:4041");
        let rows = connector_rows(Vec::new(), None, Some(&live));
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].node_peer_id, "2KFromTheLink");
        assert_eq!(rows[0].source, ConnectorSource::Session);
        assert!(rows[0].selected, "in force IS in use");
    }

    /// It leads, because it is the one being used — and the registry rows keep
    /// their own selection flags around it.
    #[test]
    fn the_session_node_comes_first_and_does_not_disturb_the_rows_below() {
        let live = node("2KFromTheLink", "ws://a:1");
        let rows = connector_rows(
            vec![node("2KAlice", "ws://b:2"), node("2KBob", "ws://c:3")],
            Some("2KBob"),
            Some(&live),
        );
        assert_eq!(
            rows.iter().map(|r| r.node_peer_id.as_str()).collect::<Vec<_>>(),
            ["2KFromTheLink", "2KAlice", "2KBob"],
        );
        assert!(!rows[1].selected);
        assert!(rows[2].selected, "the registry's own selection survives");
        assert_eq!(rows[1].source, ConnectorSource::Registry);
    }

    /// **The opposite bug, and the reason the guard is a membership test rather
    /// than "did we synthesize".** A node the user added AND is using is one
    /// row, not two — the duplicate would offer Use and Remove on one line and
    /// withhold both on the next, for the same node.
    #[test]
    fn a_node_that_is_both_stored_and_in_force_is_listed_once() {
        let live = node("2KAlice", "ws://b:2");
        let rows = connector_rows(vec![node("2KAlice", "ws://b:2")], Some("2KAlice"), Some(&live));
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].source, ConnectorSource::Registry, "removable, as it should be");
        assert!(rows[0].selected);
    }

    /// No provisioning at all is the honest empty list — the empty state must
    /// still be reachable, or the card can never say "add one".
    #[test]
    fn nothing_in_force_and_nothing_stored_is_an_empty_list() {
        assert!(connector_rows(Vec::new(), None, None).is_empty());
    }
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
            ice: String::new(),
            ice_advertised: String::new(),
            relay: String::new(),
            relay_username: String::new(),
            relay_credential: String::new(),
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
            ice: String::new(),
            ice_advertised: String::new(),
            relay: String::new(),
            relay_username: String::new(),
            relay_credential: String::new(),
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

        // **Meeting through the GUI must make us reachable, exactly as meeting
        // through the Shell does.** A browser peer has no listener, so presence
        // is an activity it performs; a side that only serves (offers a file
        // and waits) dispatches nothing and is never at the rendezvous, so the
        // other side's offer deposits meet zero collects. This window is where
        // a user actually meets — the Shell verb is the developer surface, and
        // it was the only one registering the intent.
        //
        // Asserted on the keeper rather than on a rendered row because the
        // symptom has no pixels: everything *looks* met, and the failure only
        // appears later as a transfer that never starts.
        assert!(
            crate::reach_keeper::global().targets().contains(&other_pid),
            "a meet through the window registered no reach intent, so this peer \
             is discoverable and unreachable — the asymmetry that made offers \
             work from the Shell and hang from the GUI. targets: {:?}",
            crate::reach_keeper::global().targets()
        );

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

/// **Why a peer you meet may not be able to reach back.**
///
/// A meet hands a stranger this window's peer id. Discovery runs over the
/// websocket to the rendezvous node and succeeds without a §6.5 establisher;
/// the *connect back* cannot even be attempted without one. So a meet with no
/// establisher leaves the counterpart holding an id that silently never
/// connects — worth saying at the moment of meeting.
///
/// **There are four outcomes and the surface used to state one of them for all
/// of them (AP40).** The message said *"switch this window to your main peer"*,
/// which is the right advice for exactly one cause and actively misdirecting for
/// the other two — a fresh profile (a private window, a first visit) has no
/// rendezvous node at boot, so it is already on its main peer and is told to
/// move to it.
///
/// **Why `NeedsReload` is a state at all, rather than something we just fix.**
/// The establisher is a **constructor argument** — `Peers::new_direct_idb_with_establish`
/// takes the seam because it must be captured before the peer's `PeerShared`
/// clones do, and there is no `&mut Peer` on this arm. So a rendezvous node
/// chosen *after* the tab loaded cannot be installed into the running peer, and
/// the honest thing is to say the reload is what applies it. That is also why
/// this matters more than it looks: `provisioning` resolves from URL → the
/// localStorage selection mirror → the build knob, all read at boot, so **every
/// first session on a fresh profile is unreachable until one reload.**
///
/// Pure and native-tested; the caller gathers the three facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetReach {
    /// A §6.5 establisher is installed on this window's peer — meets are
    /// two-way and nothing needs saying.
    Reachable,
    /// **This engine has no `RTCPeerConnection`.** Outranks everything below,
    /// because no amount of rendezvous configuration can help: the Linux Tauri
    /// WebView (WebKitGTK) ships without the bindings compiled in — measured on
    /// Debian 2.50.6 and Fedora 43 2.50.5, both with `MediaStream` present and
    /// `RTCPeerConnection` undefined — so a desktop window is a rendezvous
    /// *node* and a websocket peer, never a WebRTC peer.
    ///
    /// It is listed first for the reason `src-tauri`'s own note gives about why
    /// this cost three sessions to find: *"nothing fails loudly, and the half
    /// that keeps working is the half you look at"* — `meet` is an ordinary
    /// websocket call, so two devices pair and appear fine, then every
    /// establishment fails in both directions for a reason no surface mentioned.
    /// `readiness`'s `webrtc-api` row was right the whole time and nobody was
    /// pointed at it.
    NoWebRtcApi,
    /// This window is bound to a peer that is not the primary. The establisher
    /// is primary-only, so no amount of rendezvous configuration helps here;
    /// the fix really is to meet from the main peer.
    NotThisPeer,
    /// The primary, with a rendezvous node available **now** but not at boot —
    /// the user selected a connector during this session. One reload applies it.
    NeedsReload,
    /// The primary, and no rendezvous node is configured at all. Nothing to
    /// reload into; a connector has to be added first.
    NoNode,
}

impl MeetReach {
    /// Does the user need to be told anything?
    pub fn warrants_notice(self) -> bool {
        !matches!(self, MeetReach::Reachable)
    }

    /// The catalog key for this outcome, or `None` when there is nothing to say.
    ///
    /// A key per outcome rather than one string with a suffix: two outcomes
    /// rendered alike lose the distinction they exist to carry, and the whole
    /// defect here was one sentence standing in for three situations.
    pub fn message_key(self) -> Option<&'static str> {
        match self {
            MeetReach::Reachable => None,
            MeetReach::NoWebRtcApi => Some("peerconn.meet_no_webrtc_api"),
            MeetReach::NotThisPeer => Some("peerconn.meet_no_establisher"),
            MeetReach::NeedsReload => Some("peerconn.meet_needs_reload"),
            MeetReach::NoNode => Some("peerconn.meet_no_node"),
        }
    }
}

/// Gather the three facts and classify — the one expression both meet surfaces
/// use (the Peer Connections window and the Shell's `meet` verb).
///
/// Shared rather than duplicated because it had already been written twice, in
/// two files, with the same single-cause mistake in both — which is the shape
/// AP44 names: a rule stated as *"and also warn here"* is right the day it lands
/// and decays at the next call site.
///
/// **`node_available` is answered differently per arm, deliberately.** On wasm it
/// is *what a reload would resolve* — `resolve_provisioning_quietly`, the one
/// expression of URL > selection mirror > build knob — so a session booted with
/// `?webrtc_node=…` is never told to reload for a selection a reload will keep
/// ignoring. On native there is no URL and no mirror, so it is the durable
/// selection, which is the same question minus the two sources native cannot
/// have.
pub fn meet_reach_for(peers: &Peers, window_peer: &str) -> MeetReach {
    #[cfg(target_arch = "wasm32")]
    let node_available =
        crate::connectors::resolve_provisioning_quietly(&crate::app::webrtc_url_query()).is_some();
    #[cfg(not(target_arch = "wasm32"))]
    let node_available = {
        let sys = peers.system_peer_id().to_string();
        crate::connectors::selected_connector(peers, &sys).is_some()
    };

    // Feature-detect the constructor rather than construct one — the same probe
    // and the same reason as `readiness`: a construction attempt throws in one
    // engine and returns a crippled object in another, and the question is only
    // whether the API is there. Native has no window; the arm is covered by
    // `meet_reach`'s own tests, which is why the decision is pure.
    #[cfg(target_arch = "wasm32")]
    let engine_has_webrtc = web_sys::window()
        .map(|w| {
            js_sys::Reflect::get(
                w.as_ref(),
                &wasm_bindgen::JsValue::from_str("RTCPeerConnection"),
            )
            .map(|v| v.is_function())
            .unwrap_or(false)
        })
        .unwrap_or(true);
    #[cfg(not(target_arch = "wasm32"))]
    let engine_has_webrtc = true;

    meet_reach_with_engine(
        engine_has_webrtc,
        peers.peer_has_webrtc(window_peer),
        window_peer == peers.primary_peer_id(),
        node_available,
    )
}

/// [`meet_reach`] plus the engine question, which outranks everything.
///
/// Split so the three-argument form stays the shape the existing tests and
/// callers use, and so the engine arm is gated on its own.
pub fn meet_reach_with_engine(
    engine_has_webrtc: bool,
    has_establisher: bool,
    is_primary: bool,
    node_available: bool,
) -> MeetReach {
    if !engine_has_webrtc {
        return MeetReach::NoWebRtcApi;
    }
    meet_reach(has_establisher, is_primary, node_available)
}

/// Classify a meet's reachability. See [`MeetReach`].
///
/// Order matters and is the whole content: an installed establisher settles it;
/// otherwise a non-primary peer cannot be helped by configuration, so that
/// outcome outranks both node cases.
pub fn meet_reach(has_establisher: bool, is_primary: bool, node_available: bool) -> MeetReach {
    if has_establisher {
        return MeetReach::Reachable;
    }
    if !is_primary {
        return MeetReach::NotThisPeer;
    }
    if node_available {
        return MeetReach::NeedsReload;
    }
    MeetReach::NoNode
}

#[cfg(test)]
mod meet_reach_tests {
    use super::*;

    /// An installed establisher settles it whatever else is true — otherwise a
    /// working session could be told to reload.
    #[test]
    fn an_installed_establisher_is_reachable_however_it_got_there() {
        for is_primary in [true, false] {
            for node in [true, false] {
                assert_eq!(meet_reach(true, is_primary, node), MeetReach::Reachable);
            }
        }
    }

    /// **The regression this exists to prevent.** A fresh profile — a private
    /// window, a first visit — is on its main peer with no node at boot, and was
    /// told to switch to the peer it is already on.
    #[test]
    fn a_fresh_profile_on_its_main_peer_is_not_told_to_switch_peers() {
        assert_eq!(meet_reach(false, true, false), MeetReach::NoNode);
        assert_eq!(meet_reach(false, true, true), MeetReach::NeedsReload);
        assert_ne!(
            meet_reach(false, true, false),
            MeetReach::NotThisPeer,
            "the primary peer must never be told to switch to the primary peer"
        );
    }

    /// A node chosen after boot and no node at all are different situations
    /// with different next actions — reload versus add one (AP40).
    #[test]
    fn a_node_selected_this_session_is_a_reload_not_a_missing_node() {
        assert_eq!(meet_reach(false, true, true), MeetReach::NeedsReload);
        assert_eq!(meet_reach(false, true, false), MeetReach::NoNode);
    }

    /// A non-primary peer cannot be repaired by configuration, so it outranks
    /// both node cases — telling that user to reload would be a promise the
    /// app cannot keep.
    #[test]
    fn a_non_primary_peer_is_not_offered_a_reload_that_would_not_help_it() {
        assert_eq!(meet_reach(false, false, true), MeetReach::NotThisPeer);
        assert_eq!(meet_reach(false, false, false), MeetReach::NotThisPeer);
    }

    /// Every outcome has its own word, and the count is asserted so a fifth
    /// cannot quietly reuse one.
    #[test]
    fn every_outcome_has_its_own_message_and_only_one_is_silent() {
        let all = [
            MeetReach::Reachable,
            MeetReach::NoWebRtcApi,
            MeetReach::NotThisPeer,
            MeetReach::NeedsReload,
            MeetReach::NoNode,
        ];
        let keys: std::collections::BTreeSet<&str> =
            all.iter().filter_map(|r| r.message_key()).collect();
        assert_eq!(keys.len(), 4, "four distinct messages, one per non-clear outcome");
        assert_eq!(
            all.iter().filter(|r| r.warrants_notice()).count(),
            4,
            "only Reachable is silent"
        );
    }

    /// **An engine with no `RTCPeerConnection` outranks every other cause**, and
    /// this is the arm that matters on the Linux desktop: WebKitGTK ships
    /// without the bindings, so a Tauri window can hold a perfectly good
    /// rendezvous node, report `peer_has_webrtc`, and still never establish
    /// anything. Telling that user to add a connector or reload sends them to
    /// fix something that is not broken — the misdirection this whole enum
    /// exists to stop, one row further out.
    #[test]
    fn an_engine_without_webrtc_outranks_every_configuration_answer() {
        for has_est in [true, false] {
            for is_primary in [true, false] {
                for node in [true, false] {
                    assert_eq!(
                        meet_reach_with_engine(false, has_est, is_primary, node),
                        MeetReach::NoWebRtcApi,
                        "no engine support is not fixable by configuration"
                    );
                }
            }
        }
    }

    /// …and it must not fire on an engine that HAS the API, or every browser
    /// gets a desktop-only message. One bit apart from the test above.
    #[test]
    fn an_engine_with_webrtc_falls_through_to_the_configuration_answers() {
        assert_eq!(meet_reach_with_engine(true, true, true, true), MeetReach::Reachable);
        assert_eq!(meet_reach_with_engine(true, false, true, false), MeetReach::NoNode);
        assert_eq!(meet_reach_with_engine(true, false, true, true), MeetReach::NeedsReload);
        assert_eq!(meet_reach_with_engine(true, false, false, true), MeetReach::NotThisPeer);
    }
}

/// *Find peers here* — the one decision in the flow that is not somebody
/// else's function: when to start the meet.
#[cfg(test)]
mod find_peers_tests {
    use super::{find_step, FindStep};

    /// The node we added is in force: start. This must win even on the last
    /// frame, or a selection that lands exactly at the deadline is reported as
    /// a failure the user can see working.
    #[test]
    fn the_node_in_force_starts_the_meet_even_on_the_last_frame() {
        assert_eq!(find_step("2KNode", Some("2KNode"), 5), FindStep::StartMeet);
        assert_eq!(find_step("2KNode", Some("2KNode"), 0), FindStep::StartMeet);
    }

    /// **A different node in force is not a reason to start.** Starting would
    /// run the lobby meet at a node the user did not type — the silent version
    /// of the failure this button exists to remove.
    #[test]
    fn a_different_node_in_force_waits_and_then_gives_up() {
        assert_eq!(find_step("2KNode", Some("2KOther"), 5), FindStep::Wait);
        assert_eq!(find_step("2KNode", None, 5), FindStep::Wait);
        assert_eq!(find_step("2KNode", Some("2KOther"), 0), FindStep::GiveUp);
        assert_eq!(find_step("2KNode", None, 0), FindStep::GiveUp);
    }
}

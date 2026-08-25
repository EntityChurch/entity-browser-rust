//! Peer Connections window — connect, QR pairing, peer status.

pub mod model;
pub mod output;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use model::PeerConnectionsModel;

use crate::window_watch::WindowWatch;

pub struct PeerConnectionsWindow {
    window_id: WindowId,
    peer_id: String,
    model: PeerConnectionsModel,
    watch: WindowWatch,
}

impl PeerConnectionsWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        let model = PeerConnectionsModel::new(window_id, peer_id.clone());
        Self {
            window_id,
            peer_id,
            model,
            watch: WindowWatch::new(),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Peer Connections", // i18n-ignore — identity key; display via window.peer_connections
            description: "Manage peer network connections and pairing", // i18n-ignore — dead_code, never rendered
            scope: crate::window::WindowScope::Peer,
            create: |id, peer_id, pm| {
                let mut window = PeerConnectionsWindow::new(id, peer_id.to_string());
                window.model.initialize(pm);
                let sys_pid = pm.system_peer_id().to_string();
                // Window state on the bound peer.
                pm.watch_prefix(
                    &mut window.watch,
                    &window.peer_id,
                    crate::app_paths::window_state_path(crate::app_paths::APP_ID, &window.peer_id, window.window_id),
                );
                // Tree-backed inputs all live on the system peer.
                pm.watch_prefix(
                    &mut window.watch,
                    &sys_pid,
                    crate::app_paths::connections_prefix(crate::app_paths::APP_ID, &sys_pid),
                );
                // `read_connections` resolves each peer's address from the
                // KERNEL's route entity — the single durable home of an address
                // (`MODEL-REMOTE-PEER-FACTS` §1). On the Worker arm a tree read
                // hits a cache mirror populated only for subscribed prefixes, so
                // without this watch every address reads empty for peers that
                // are perfectly reachable.
                pm.watch_prefix(
                    &mut window.watch,
                    &sys_pid,
                    crate::transport_profiles::routes_prefix(&sys_pid),
                );
                // `render_output` reads `read_connections`, which joins the
                // sibling `authz` mirror — so watch it too, or an authorize
                // write wouldn't wake this window in the Worker arm (§4,
                // `feedback_worker_cache_get_needs_subscription`).
                pm.watch_prefix(
                    &mut window.watch,
                    &sys_pid,
                    crate::app_paths::authz_prefix(crate::app_paths::APP_ID, &sys_pid),
                );
                pm.watch_prefix(
                    &mut window.watch,
                    &sys_pid,
                    crate::app_paths::listener_state_path(crate::app_paths::APP_ID, &sys_pid),
                );
                // (Device authorizations moved to the System Backend window,
                // Direction A — the backend-auth mirror is watched there now.)
                // Wake on roster changes via the tree-backed registry
                // instead of the content-free signal.
                pm.watch_prefix(
                    &mut window.watch,
                    &sys_pid,
                    crate::app_paths::peers_registry_prefix(crate::app_paths::APP_ID, &sys_pid),
                );
                // The connector registry + its selection, both on the system
                // peer. Without these the Worker-arm cache mirror never carries
                // this prefix and the section renders permanently empty for a
                // registry that is perfectly well populated — the same rule the
                // route/authz watches above exist for.
                pm.watch_prefix(
                    &mut window.watch,
                    &sys_pid,
                    crate::app_paths::connectors_prefix(crate::app_paths::APP_ID, &sys_pid),
                );
                pm.watch_prefix(
                    &mut window.watch,
                    &sys_pid,
                    crate::app_paths::connector_selection_path(crate::app_paths::APP_ID, &sys_pid),
                );
                // The KERNEL liveness surface the known-device rows render from
                // — the authoritative read-model. See the helper for why the
                // subscription is load-bearing and why it is every vantage.
                crate::peer_liveness::watch_all_vantages(pm, &mut window.watch);
                Box::new(window)
            },
        }
    }
}

impl PeerConnectionsWindow {
    /// Publish (or clear) the connector notice and repaint. `None` clears.
    fn set_connector_notice(&self, notice: Option<(String, bool)>) {
        if let Ok(mut slot) = self.model.connector_notice_handle().lock() {
            *slot = notice.map(|(text, is_error)| {
                crate::views::peer_connections::output::ConnectorNotice { text, is_error }
            });
        }
        // A notice that changes no tree state still has to reach the screen —
        // nothing else will dirty this window for it.
        self.watch.mark_dirty();
    }

    /// `Check` — ask the node what it actually serves (`advertise()`).
    ///
    /// Async, so the result lands off-frame into the shared notice slot and
    /// marks the window dirty; there is no tree write to wake it otherwise.
    fn check_connector(&self, peers: &Peers, node_peer_id: &str) {
        let sys = peers.system_peer_id().to_string();
        // Dial first when nothing has: a connector the user just added has no
        // route at all, and the EXECUTE would fail with "no transport profile"
        // — which reads as "the node is down" when we simply never called it
        // (`connectors::reach_node`). The address is in the registry row, which
        // is why this resolves the row rather than trusting the id alone.
        let Some(row) = crate::connectors::read_connectors(peers, &sys)
            .into_iter()
            .find(|c| c.node_peer_id == node_peer_id)
        else {
            self.set_connector_notice(Some((
                format!("no connector with peer-id {node_peer_id}"), // i18n-ignore — unreachable via the UI (the button rides a row)
                true,
            )));
            return;
        };
        let reach = crate::connectors::reach_node(peers, &sys, &row);
        let fut = crate::connectors::advertise(peers, &sys, node_peer_id);
        let slot = self.model.connector_notice_handle();
        let dirty = self.watch.flag();
        let short = crate::views::short_pid(node_peer_id);
        // §4.5.1's automatic half: a `Check` is exactly the round trip that
        // learns the node's own reflectors, so record them while we have them.
        let writer = peers.writer_handle();
        let sys_for_write = sys.clone();
        crate::views::peer_connections::spawn_check(async move {
            let outcome = match reach.await {
                Ok(()) => fut.await,
                Err(e) => Err(e),
            };
            if let (Ok(ad), Some(w)) = (&outcome, writer.as_ref()) {
                crate::connectors::record_advertised_reflectors(
                    w,
                    &sys_for_write,
                    &row,
                    &ad.reflection_endpoints,
                );
            }
            let notice = match outcome {
                Ok(ad) => crate::views::peer_connections::output::ConnectorNotice {
                    text: crate::i18n::t(
                        "peerconn.connector_serves",
                        &[
                            ("node", &short),
                            ("endpoint", &ad.endpoint),
                            ("lobby", crate::connectors::lobby_constant_for(&ad)),
                        ],
                    ),
                    is_error: false,
                },
                Err(e) => crate::views::peer_connections::output::ConnectorNotice {
                    text: e,
                    is_error: true,
                },
            };
            if let Ok(mut s) = slot.lock() {
                *s = Some(notice);
            }
            dirty.mark();
        });
    }
}

/// Spawn the `Check` round-trip on whichever runtime we are on.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_check<F: std::future::Future<Output = ()> + Send + 'static>(f: F) {
    tokio::spawn(f);
}

#[cfg(target_arch = "wasm32")]
fn spawn_check<F: std::future::Future<Output = ()> + 'static>(f: F) {
    wasm_bindgen_futures::spawn_local(f);
}

impl WindowView for PeerConnectionsWindow {
    fn title(&self) -> String {
        crate::i18n::window_title("Peer Connections") // i18n-ignore — lookup key, resolves via catalog
    }

    fn type_name(&self) -> &'static str {
        "Peer Connections" // i18n-ignore — stable type identifier, not UI text
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, action: &Action, peers: &Peers) {
        let state_changed = match action {
            Action::WindowEvent { window_id, event, value } if *window_id == self.window_id => {
                let _ = value;
                match event.as_str() {
                    // The in-progress address is a renderer-side DRAFT
                    // (`ctx.drafts` via `components::text_input`), not model
                    // state — typing survives unrelated repaints without any
                    // per-keystroke event (`BUGLOG-2026-07-14` B3/B4). The model
                    // only hears about the one-shot transitions: a successful
                    // Connect clears the persisted suggestion so the field
                    // doesn't re-offer the just-dialed address.
                    "clear_address" => {
                        self.model.clear_address();
                        true
                    }
                    // Connector registry — the same four operations the
                    // `connector` shell verb exposes (add / use / rm / check),
                    // driving the same `crate::connectors` functions. One
                    // model, two surfaces.
                    //
                    // These write the SYSTEM peer's registry, not this window's
                    // bound peer: a connector is deployment infrastructure, and
                    // provisioning reads it from the system peer.
                    "connector_add" => {
                        // Packed
                        // "{peer_id}\x1f{addr}\x1f{label}\x1f{ice}\x1f{relay}\x1f{user}\x1f{cred}"
                        // — the app's multi-field convention, so one event
                        // carries the form. `splitn(7, ..)` so a credential
                        // containing the separator would be preserved whole
                        // rather than truncated (it cannot contain `\x1f` from a
                        // text input, but the last field is the right place for
                        // the remainder either way).
                        let mut parts = value.splitn(7, '\x1f');
                        let c = crate::connectors::Connector {
                            node_peer_id: parts.next().unwrap_or("").to_string(),
                            node_addr: parts.next().unwrap_or("").to_string(),
                            label: parts.next().unwrap_or("").to_string(),
                            ice: parts.next().unwrap_or("").to_string(),
                            // Ignored by `add_connector` — a node's own advertisement is
                                // learned, never typed.
                                ice_advertised: String::new(),
                            relay: parts.next().unwrap_or("").to_string(),
                            relay_username: parts.next().unwrap_or("").to_string(),
                            relay_credential: parts.next().unwrap_or("").to_string(),
                            };
                        let sys = peers.system_peer_id().to_string();
                        self.set_connector_notice(
                            match crate::connectors::add_connector(peers, &sys, &c) {
                                // `AddOutcome.selected` needs no notice of its
                                // own here: the row list marks the selection
                                // with the same glyph it always has, and
                                // selecting the first node is exactly what
                                // makes `connector_reload_pending` fire — so
                                // the one thing the user must now do is already
                                // on screen, in a string that is translated.
                                Ok(_) => {
                                    // Learn what this node serves (§4.5.1) —
                                    // adding it is the moment to ask, and the
                                    // user should not have to press Check to
                                    // get the reflectors it publishes.
                                    crate::connectors::learn_node_reflectors(peers, &sys, &c);
                                    None
                                }
                                // A refusal (missing half, unsafe id) must be
                                // sayable — otherwise Add is a dead button.
                                Err(e) => Some((e, true)),
                            },
                        );
                        false
                    }
                    "connector_use" => {
                        let sys = peers.system_peer_id().to_string();
                        self.set_connector_notice(
                            match crate::connectors::select_connector(peers, &sys, value) {
                                Ok(()) => None,
                                Err(e) => Some((e, true)),
                            },
                        );
                        false
                    }
                    "connector_rm" => {
                        let sys = peers.system_peer_id().to_string();
                        crate::connectors::remove_connector(peers, &sys, value);
                        self.set_connector_notice(None);
                        false
                    }
                    "connector_check" => {
                        self.check_connector(peers, value);
                        false
                    }
                    // Meet at a name (`crate::rendezvous`) — the same three
                    // modes the `meet` shell verb takes, over the same session
                    // type. Packed "{mode}\x1f{input}", the app's multi-field
                    // convention, so one event carries the form.
                    "meet_start" => {
                        let (mode, input) = value.split_once('\u{1f}').unwrap_or((value, ""));
                        let notice = match crate::rendezvous::Mode::parse(mode, input)
                            .and_then(|m| self.model.start_meet(peers, m))
                        {
                            // Started — but a meet hands strangers THIS peer's
                            // id, and the §6.5 establisher is primary-only while
                            // this window binds the *user-selected* peer. Without
                            // one, discovery succeeds and the connect back can
                            // never be attempted, so the counterpart is left
                            // holding an unreachable id with nothing said. Warn
                            // rather than refuse: the meet itself is legitimate,
                            // and the user may be introducing two other peers.
                            Ok(()) if !peers.peer_has_webrtc(&self.peer_id) => Some((
                                crate::i18n::t("peerconn.meet_no_establisher", &[]),
                                true,
                            )),
                            Ok(()) => None,
                            // A refusal (no connector selected, a mode with
                            // nothing to meet at) must be sayable, or Meet is a
                            // dead button. It lands in the meet card's own slot,
                            // beside the button that caused it.
                            Err(e) => Some((e, true)),
                        };
                        self.model.set_meet_notice(notice);
                        self.watch.mark_dirty();
                        false
                    }
                    "meet_stop" => {
                        self.model.stop_meet();
                        self.watch.mark_dirty();
                        false
                    }
                    // Keep a peer we met. Deliberately a **user action**: a
                    // public `tag` bucket is guessable by design, so anyone
                    // posting in one would otherwise write rows into this
                    // registry. Discovery reports; the user decides who to keep.
                    "meet_remember" => {
                        crate::connections::ConnectionsWriter::new(peers).add(value);
                        self.watch.mark_dirty();
                        false
                    }
                    _ => false,
                }
            }
            _ => false,
        };
        if state_changed {
            self.model.save_state(peers);
        }
    }

    /// Drive a running meet. A meet writes nothing to the tree, so nothing else
    /// would ever repaint this window for it — the pump reports whether the
    /// visible status moved and we mark dirty on that. Free when idle.
    fn tick(&mut self, peers: &Peers) {
        if self.model.pump_meet(peers) {
            self.watch.mark_dirty();
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        peers: &Peers,
        ctx: &crate::dom::DomCtx,
    ) {
        let output = self.model.render_output(
            peers,
            &ctx.dial_markers,
            &ctx.connect_attempt,
            ctx.provisioning_drifted,
        );
        crate::dom::peer_connections::render(container, &output, ctx);
    }
}

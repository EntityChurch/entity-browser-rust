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
                // Connection-health mirror: known-device rows show live
                // Connected/Unreachable, so watch it to repaint on a health change.
                pm.watch_prefix(
                    &mut window.watch,
                    &sys_pid,
                    crate::app_paths::connection_health_prefix(crate::app_paths::APP_ID, &sys_pid),
                );
                // Wake on roster changes via the tree-backed registry
                // instead of the content-free signal.
                pm.watch_prefix(
                    &mut window.watch,
                    &sys_pid,
                    crate::app_paths::peers_registry_prefix(crate::app_paths::APP_ID, &sys_pid),
                );
                // Piece A (P2.0): subscribe the KERNEL liveness surface
                // (`system/peer/status`) for every local vantage, so we can
                // shadow it against the `connection_health` mirror before
                // migrating consumers off the mirror (Piece B). Watching seeds
                // the Worker-arm cache and wakes the window on a kernel
                // connect/keepalive-miss/disconnect transition.
                for vantage in pm.peer_ids() {
                    pm.watch_prefix(
                        &mut window.watch,
                        &vantage,
                        crate::peer_liveness::peer_status_prefix(&vantage),
                    );
                }
                Box::new(window)
            },
        }
    }
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
                    _ => false,
                }
            }
            _ => false,
        };
        if state_changed {
            self.model.save_state(peers);
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        peers: &Peers,
        ctx: &crate::dom::DomCtx,
    ) {
        // Piece A (P2.0) shadow-parity: log the kernel liveness surface vs the
        // connection_health mirror on each rebuild, so convergence is
        // observable in the live build / e2e before Piece B migrates the render
        // off the mirror. Read-only, no behaviour.
        crate::peer_liveness::log_shadow_parity(peers);
        let output = self.model.render_output(peers);
        crate::dom::peer_connections::render(container, &output, ctx);
    }
}

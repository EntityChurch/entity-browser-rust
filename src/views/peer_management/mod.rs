//! Peer Management window — list, create, and delete local peers.

pub mod model;
pub mod output;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use model::PeerManagementModel;

use crate::window_watch::WindowWatch;

pub struct PeerManagementWindow {
    peer_id: String,
    // Used only on the WASM render path; native sees it as unused.
    #[allow(dead_code)]
    model: PeerManagementModel,
    watch: WindowWatch,
}

impl PeerManagementWindow {
    pub fn new(peer_id: String) -> Self {
        let model = PeerManagementModel::new(peer_id.clone());
        Self {
            peer_id,
            model,
            watch: WindowWatch::new(),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Peers",
            description: "Manage local peers — create, delete, inspect",
            scope: crate::window::WindowScope::System,
            create: |_id, peer_id, pm| {
                let mut window = PeerManagementWindow::new(peer_id.to_string());
                // The hosted-peer roster lives in the tree under the
                // registry prefix.
                // Subscribe to it like any other entity-backed window;
                // EntityApp reconciles it on every lifecycle change.
                let sys_pid = pm.system_peer_id().to_string();
                pm.watch_prefix(
                    &mut window.watch,
                    &sys_pid,
                    crate::app_paths::peers_registry_prefix(crate::app_paths::APP_ID, &sys_pid),
                );
                Box::new(window)
            },
        }
    }
}

impl WindowView for PeerManagementWindow {
    fn title(&self) -> String {
        "Peers".to_string()
    }

    fn type_name(&self) -> &'static str {
        "Peers"
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, _action: &Action, _peers: &Peers) {
        // Peer registry actions (CreatePeerWithMode, DeletePeer,
        // SpawnWindow, StartBackendPeer, StopBackendPeer) are handled
        // globally in app.rs::process_actions. No per-window state.
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        peers: &Peers,
        ctx: &crate::dom::DomCtx,
    ) {
        let output = self.model.render_output(peers);
        crate::dom::peer_management::render(container, &output, ctx);
    }
}

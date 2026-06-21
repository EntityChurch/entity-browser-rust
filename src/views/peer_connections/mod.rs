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
            name: "Peer Connections",
            description: "Manage peer network connections and pairing",
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
                pm.watch_prefix(
                    &mut window.watch,
                    &sys_pid,
                    crate::app_paths::listener_state_path(crate::app_paths::APP_ID, &sys_pid),
                );
                // Wake on roster changes via the tree-backed registry
                // instead of the content-free signal.
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

impl WindowView for PeerConnectionsWindow {
    fn title(&self) -> String {
        "Peer Connections".to_string()
    }

    fn type_name(&self) -> &'static str {
        "Peer Connections"
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
                match event.as_str() {
                    "set_address" => {
                        self.model.set_address(value);
                        true
                    }
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
        let output = self.model.render_output(peers);
        crate::dom::peer_connections::render(container, &output, ctx);
    }
}

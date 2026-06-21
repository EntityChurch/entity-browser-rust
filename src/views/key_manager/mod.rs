//! Key Manager window — hosted-peer public identities and roles.
//!
//! A conforming Pass-through window over the tree-backed peer registry:
//! entity-backed, no struct
//! state, subscription-driven — same shape as the Peers window.

pub mod model;
pub mod output;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use model::KeyManagerModel;

use crate::window_watch::WindowWatch;

pub struct KeyManagerWindow {
    peer_id: String,
    // Used only on the WASM render path; native sees it as unused.
    #[allow(dead_code)]
    model: KeyManagerModel,
    watch: WindowWatch,
}

impl KeyManagerWindow {
    pub fn new(peer_id: String) -> Self {
        let model = KeyManagerModel::new(peer_id.clone());
        Self {
            peer_id,
            model,
            watch: WindowWatch::new(),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Key Manager",
            description: "Hosted-peer public identities and roles",
            scope: crate::window::WindowScope::System,
            create: |_id, peer_id, pm| {
                let mut window = KeyManagerWindow::new(peer_id.to_string());
                // Public identities live in the tree-backed registry on
                // the system peer; subscribe like any entity-backed
                // window. EntityApp reconciles it on every lifecycle
                // change.
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

impl WindowView for KeyManagerWindow {
    fn title(&self) -> String {
        "Key Manager".to_string()
    }

    fn type_name(&self) -> &'static str {
        "Key Manager"
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, _action: &Action, _peers: &Peers) {
        // No per-window state — the roster is global, reconciled in
        // app.rs. Matches the Peers window.
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        peers: &Peers,
        _ctx: &crate::dom::DomCtx,
    ) {
        let output = self.model.render_output(peers);
        crate::dom::key_manager::render(container, &output);
    }
}

//! Execute Console window — handler-aware EXECUTE interface.

pub mod model;
pub mod output;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use model::ExecuteConsoleModel;

use crate::window_watch::WindowWatch;

pub struct ExecuteConsoleWindow {
    window_id: WindowId,
    peer_id: String,
    model: ExecuteConsoleModel,
    watch: WindowWatch,
}

impl ExecuteConsoleWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        let model = ExecuteConsoleModel::new(window_id, peer_id.clone());
        Self {
            window_id,
            peer_id,
            model,
            watch: WindowWatch::new(),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Execute Console",
            description: "Execute handler operations on local or remote peers",
            scope: crate::window::WindowScope::Peer,
            create: |id, peer_id, pm| {
                let mut window = ExecuteConsoleWindow::new(id, peer_id.to_string());
                window.model.initialize(pm);
                window.model.refresh_handlers(pm, window.watch.flag());
                let sys_pid = pm.system_peer_id().to_string();
                pm.watch_prefix(
                    &mut window.watch,
                    &window.peer_id,
                    crate::app_paths::window_state_path(crate::app_paths::APP_ID, &window.peer_id, window.window_id),
                );
                // Stage C: per-event subscription on
                // the event log drives the model's local cache.
                let event_log_inner = window.model.event_log_cache().inner_arc();
                pm.observe_with_events(
                    &mut window.watch,
                    &sys_pid,
                    crate::app_paths::event_log_prefix(crate::app_paths::APP_ID, &sys_pid),
                    move |op| crate::event_log_cache::apply_change(&event_log_inner, op),
                );
                pm.watch_prefix(
                    &mut window.watch,
                    &sys_pid,
                    crate::app_paths::connections_prefix(crate::app_paths::APP_ID, &sys_pid),
                );
                // Wake on roster changes via the tree-backed registry.
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

impl WindowView for ExecuteConsoleWindow {
    fn title(&self) -> String {
        "Execute Console".into()
    }

    fn type_name(&self) -> &'static str {
        "Execute Console"
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
                    "set_mode" => { self.model.set_mode(value); true }
                    "select_peer" => {
                        self.model.select_peer(value);
                        self.model.refresh_handlers(peers, self.watch.flag());
                        true
                    }
                    "select_handler" => { self.model.select_handler(value); true }
                    "select_operation" => { self.model.select_operation(value); true }
                    "set_resource" => { self.model.set_resource(value); true }
                    "set_raw_uri" => { self.model.set_raw_uri(value); true }
                    "set_raw_operation" => { self.model.set_raw_operation(value); true }
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
        crate::dom::execute_console::render(container, &output, ctx);
    }
}

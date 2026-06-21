//! Query Console window — find/count entities by type, path, ref, link.

pub mod model;
pub mod output;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use model::QueryConsoleModel;

use crate::window_watch::WindowWatch;

pub struct QueryConsoleWindow {
    window_id: WindowId,
    peer_id: String,
    model: QueryConsoleModel,
    watch: WindowWatch,
}

impl QueryConsoleWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        let model = QueryConsoleModel::new(window_id, peer_id.clone());
        Self {
            window_id,
            peer_id,
            model,
            watch: WindowWatch::new(),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Query Console",
            description: "Find and count entities by type, path, and reference filters",
            scope: crate::window::WindowScope::Peer,
            create: |id, peer_id, pm| {
                let mut window = QueryConsoleWindow::new(id, peer_id.to_string());
                window.model.initialize(pm);
                let sys_pid = pm.system_peer_id().to_string();
                // Window-state watch: notify-only, the controller
                // writes here on action handling.
                pm.watch_prefix(
                    &mut window.watch,
                    &window.peer_id,
                    crate::app_paths::window_state_path(crate::app_paths::APP_ID, &window.peer_id, window.window_id),
                );
                // Stage C: per-event subscription on
                // the system peer's event log. Drives the model's
                // local cache so render reads in-memory.
                let inner = window.model.event_log_cache().inner_arc();
                pm.observe_with_events(
                    &mut window.watch,
                    &sys_pid,
                    crate::app_paths::event_log_prefix(crate::app_paths::APP_ID, &sys_pid),
                    move |op| crate::event_log_cache::apply_change(&inner, op),
                );
                Box::new(window)
            },
        }
    }
}

impl WindowView for QueryConsoleWindow {
    fn title(&self) -> String {
        "Query Console".into()
    }

    fn type_name(&self) -> &'static str {
        "Query Console"
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
                    "set_type_filter" => { self.model.set_type_filter(value); true }
                    "set_path_prefix" => { self.model.set_path_prefix(value); true }
                    "set_ref_filter" => { self.model.set_ref_filter(value); true }
                    "set_path_filter" => { self.model.set_path_filter(value); true }
                    "set_limit" => { self.model.set_limit(value); true }
                    "toggle_include_entities" => { self.model.toggle_include_entities(); true }
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
        crate::dom::query_console::render(container, &output, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_peers() -> Peers {
        Peers::new_direct()
    }

    async fn flush_writes() {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    #[tokio::test]
    async fn window_type_writes_initial_state() {
        let peers = make_peers();
        let wt = QueryConsoleWindow::window_type();
        let _view = (wt.create)(1, peers.primary_peer_id(), &peers);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, peers.primary_peer_id(), 1);
        let entity = peers.get_entity(peers.primary_peer_id(), &path);
        assert!(entity.is_some());
        assert_eq!(entity.unwrap().entity_type, "app/state/query_console");
    }

    #[tokio::test]
    async fn handle_set_type_filter_writes_to_tree() {
        let peers = make_peers();
        let wt = QueryConsoleWindow::window_type();
        let mut view = (wt.create)(1, peers.primary_peer_id(), &peers);
        view.handle_action(
            &Action::WindowEvent {
                window_id: 1,
                event: "set_type_filter".into(),
                value: "app/user".into(),
            },
            &peers,
        );
        flush_writes().await;
        let path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, peers.primary_peer_id(), 1);
        let entity = peers.get_entity(peers.primary_peer_id(), &path).unwrap();
        let state = model::QueryState::from_entity(&entity);
        assert_eq!(state.type_filter, "app/user");
    }

    #[tokio::test]
    async fn handle_toggle_include_entities() {
        let peers = make_peers();
        let wt = QueryConsoleWindow::window_type();
        let mut view = (wt.create)(1, peers.primary_peer_id(), &peers);
        view.handle_action(
            &Action::WindowEvent {
                window_id: 1,
                event: "toggle_include_entities".into(),
                value: String::new(),
            },
            &peers,
        );
        flush_writes().await;
        let path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, peers.primary_peer_id(), 1);
        let entity = peers.get_entity(peers.primary_peer_id(), &path).unwrap();
        assert!(model::QueryState::from_entity(&entity).include_entities);
    }

    #[test]
    fn spawnable_from_window_type() {
        let peers = make_peers();
        let wt = QueryConsoleWindow::window_type();
        let view = (wt.create)(1, peers.primary_peer_id(), &peers);
        assert_eq!(view.type_name(), "Query Console");
    }
}

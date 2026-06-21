//! Entity Tree window — tree navigation + document + inspector panels.
//!
//! Architecture:
//!
//! - The **model** (`model.rs`) is long-lived. It holds persisted
//!   state (`current_path`) under `Arc<Mutex<_>>` and produces a
//!   renderer-neutral [`EntityTreeOutput`](output::EntityTreeOutput)
//!   on demand.
//! - This **window** is a thin controller — translates `Action`s into
//!   model method calls and asks the model to persist after state-
//!   changing actions.
//! - The **DOM renderer** (`crate::dom::entity_tree`) consumes the
//!   pure output and never touches `Peers`.
//!
//! Per-window state lives in the tree at
//! `{peer_id}/app/entity-browser/workspace/windows/{id}/state`.
//! Multiple instances can be open at different paths or different
//! peers.

pub mod model;
pub mod output;
pub mod tree;

use crate::selection::{clear_entity_selection, publish_entity_selection};

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use model::EntityTreeModel;

use crate::window_watch::WindowWatch;

/// An Entity Tree window instance.
pub struct EntityTreeWindow {
    window_id: WindowId,
    peer_id: String,
    model: EntityTreeModel,
    watch: WindowWatch,
}

impl EntityTreeWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        let model = EntityTreeModel::new(window_id, peer_id.clone());
        Self {
            window_id,
            peer_id,
            model,
            watch: WindowWatch::new(),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Entity Tree",
            description: "Navigate entity tree, inspect content and metadata",
            scope: crate::window::WindowScope::Peer,
            create: |id, peer_id, pm| {
                let mut window = EntityTreeWindow::new(id, peer_id.to_string());
                window.model.initialize(pm);
                // Stage A.1: per-event subscription with
                // seed. Direct uses `on_prefix_change_seeded`; Worker
                // uses the new `observe_with_events` proxy primitive.
                // Both deliver normalized `ChangeOp`s to apply_change,
                // which mutates the local mirror in O(depth) per event.
                // No more O(N) refresh_mirror per render.
                let prefix = format!("/{}/", window.peer_id);
                let inner = window.model.inner_arc();
                pm.observe_with_events(
                    &mut window.watch,
                    &window.peer_id,
                    prefix,
                    move |op| model::apply_change(&inner, op),
                );
                Box::new(window)
            },
        }
    }
}

impl WindowView for EntityTreeWindow {
    fn title(&self) -> String {
        "Entity Tree".to_string()
    }

    fn type_name(&self) -> &'static str {
        "Entity Tree"
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, action: &Action, peers: &Peers) {
        // Stage B: on Navigate, publish the new path to
        // the per-panel + app-aggregate selection slots so other
        // panels can co-orient. ToggleExpand / SetSearch are
        // panel-local (action_event::Propagation::Panel) — no slot
        // writes.
        let state_changed = match action {
            Action::Navigate(id, path) if *id == self.window_id => {
                self.model.navigate(path);
                publish_entity_selection(peers, &self.peer_id, self.window_id, path);
                true
            }
            Action::NavigateUp(id) if *id == self.window_id => {
                self.model.navigate_up();
                // Publish the (possibly new) current_path. None
                // means the user navigated above the peer root —
                // clear both slots via dispatch_remove.
                if let Some(path) = self.model.current_path() {
                    publish_entity_selection(peers, &self.peer_id, self.window_id, &path);
                } else {
                    clear_entity_selection(peers, &self.peer_id, self.window_id);
                }
                true
            }
            Action::EntityTreeToggleExpand(id, path) if *id == self.window_id => {
                self.model.toggle_expand(path);
                true
            }
            Action::EntityTreeSetSearch(id, query) if *id == self.window_id => {
                self.model.set_search(query);
                true
            }
            Action::SetSelectionSource(id, wire) if *id == self.window_id => {
                self.model.set_selection_source(wire);
                true
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
        crate::dom::entity_tree::render(container, &output, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use entity_ecf::{text, to_ecf};
    use entity_entity::Entity;

    fn make_peers() -> Peers {
        Peers::new_direct()
    }

    fn make_peers_with_entity() -> (Peers, String) {
        let pm = Peers::new_direct();
        let pid = pm.primary_peer_id().to_string();
        let data = to_ecf(&text("hello"));
        let entity = Entity::new("test/type", data).unwrap();
        let path = format!("/{}/docs/arch/overview", pid);
        pm.put_entity(&pid, &path, entity);
        (pm, pid)
    }

    async fn flush_writes() {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    #[tokio::test]
    async fn factory_writes_initial_state() {
        let peers = make_peers();
        let wt = EntityTreeWindow::window_type();
        let _view = (wt.create)(1, peers.primary_peer_id(), &peers);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, peers.primary_peer_id(), 1);
        let entity = peers.get_entity(peers.primary_peer_id(), &path);
        assert!(entity.is_some(), "initial state should be in tree");
        assert_eq!(entity.unwrap().entity_type, "app/state/entity_tree");
    }

    #[tokio::test]
    async fn handle_navigate_writes_to_tree() {
        let peers = make_peers();
        let wt = EntityTreeWindow::window_type();
        let mut view = (wt.create)(1, peers.primary_peer_id(), &peers);

        view.handle_action(&Action::Navigate(1, "docs/test".into()), &peers);
        flush_writes().await;

        let path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, peers.primary_peer_id(), 1);
        let entity = peers.get_entity(peers.primary_peer_id(), &path).unwrap();
        let state = model::EntityTreeState::from_entity(&entity);
        assert_eq!(state.current_path.as_deref(), Some("docs/test"));
    }

    #[tokio::test]
    async fn handle_navigate_publishes_panel_and_app_selection_slots() {
        let peers = make_peers();
        let pid = peers.primary_peer_id().to_string();
        let wt = EntityTreeWindow::window_type();
        let mut view = (wt.create)(7, &pid, &peers);

        let target = format!("/{}/docs/test", pid);
        view.handle_action(&Action::Navigate(7, target.clone()), &peers);
        flush_writes().await;

        // Per-panel slot.
        let panel_path = crate::app_paths::panel_selection_path(
            crate::app_paths::APP_ID,
            &pid,
            7,
        );
        let panel_entity = peers
            .get_entity(&pid, &panel_path)
            .expect("per-panel selection slot should exist after Navigate");
        assert_eq!(panel_entity.entity_type, "app/state/selection");
        let panel_sel = crate::selection::Selection::from_entity(&panel_entity);
        assert_eq!(panel_sel.path, target);
        assert_eq!(panel_sel.type_.as_deref(), Some("entity"));

        // App-aggregate slot.
        let app_path = crate::app_paths::app_selection_path(
            crate::app_paths::APP_ID,
            &pid,
        );
        let app_entity = peers
            .get_entity(&pid, &app_path)
            .expect("app-aggregate selection slot should exist after Navigate");
        let app_sel = crate::selection::Selection::from_entity(&app_entity);
        assert_eq!(app_sel.path, target);
    }

    #[tokio::test]
    async fn handle_navigate_up_writes_to_tree() {
        let peers = make_peers();
        let wt = EntityTreeWindow::window_type();
        let mut view = (wt.create)(1, peers.primary_peer_id(), &peers);

        view.handle_action(&Action::Navigate(1, "a/b/c".into()), &peers);
        flush_writes().await;
        view.handle_action(&Action::NavigateUp(1), &peers);
        flush_writes().await;

        let path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, peers.primary_peer_id(), 1);
        let entity = peers.get_entity(peers.primary_peer_id(), &path).unwrap();
        let state = model::EntityTreeState::from_entity(&entity);
        assert_eq!(state.current_path.as_deref(), Some("a/b"));
    }

    #[tokio::test]
    async fn independent_instances_persist_separately() {
        let peers = make_peers();
        let wt = EntityTreeWindow::window_type();
        let mut v1 = (wt.create)(1, peers.primary_peer_id(), &peers);
        let mut v2 = (wt.create)(2, peers.primary_peer_id(), &peers);

        v1.handle_action(&Action::Navigate(1, "docs/a".into()), &peers);
        v2.handle_action(&Action::Navigate(2, "system/b".into()), &peers);
        flush_writes().await;

        let pid = peers.primary_peer_id();
        let e1 = peers
            .get_entity(pid, &crate::app_paths::window_state_path(crate::app_paths::APP_ID, pid, 1))
            .unwrap();
        let e2 = peers
            .get_entity(pid, &crate::app_paths::window_state_path(crate::app_paths::APP_ID, pid, 2))
            .unwrap();
        assert_eq!(
            model::EntityTreeState::from_entity(&e1).current_path.as_deref(),
            Some("docs/a")
        );
        assert_eq!(
            model::EntityTreeState::from_entity(&e2).current_path.as_deref(),
            Some("system/b")
        );
    }

    #[tokio::test]
    async fn navigate_to_existing_path_resolves_in_render_output() {
        let (pm, pid) = make_peers_with_entity();
        let wt = EntityTreeWindow::window_type();
        let mut view = (wt.create)(1, pm.primary_peer_id(), &pm);

        let target = format!("/{}/docs/arch/overview", pid);
        view.handle_action(&Action::Navigate(1, target.clone()), &pm);
        flush_writes().await;

        // Reach into the model via downcast — only the controller has it,
        // but render_output's correctness was already covered in
        // model.rs tests. Here we just confirm the action plumbing
        // landed by reading the persisted state.
        let state_path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, &pid, 1);
        let state_entity = pm.get_entity(&pid, &state_path).unwrap();
        let state = model::EntityTreeState::from_entity(&state_entity);
        assert_eq!(state.current_path.as_deref(), Some(target.as_str()));
    }
}

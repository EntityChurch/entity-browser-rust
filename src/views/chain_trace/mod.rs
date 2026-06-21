//! Chain Trace window — 11th window family member, peer-scoped.
//!
//! User enters a `chain_id`; window renders the trace timeline by
//! reading the continuation entity at
//! `/{peer_id}/system/continuation/{chain_id}` and the chain-error
//! markers at `/{peer_id}/system/runtime/chain-errors/**` filtered to
//! the matching chain. Both are static substrate-read primitives per
//! GUIDE-INSPECTABILITY v1.2 §2.4 projection table; no L1 hook
//! required for the v0 surface.
//!
//! Renderer respects `RenderPolicy` per the cross-peer observability
//! audit: continuation + chain-error marker entities are `CapControlled`, so
//! body content surfaces in operator mode (our app's default today)
//! and is suppressed in normal mode.

pub mod model;
pub mod output;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use crate::window_watch::WindowWatch;
use model::ChainTraceModel;

pub struct ChainTraceWindow {
    window_id: WindowId,
    peer_id: String,
    #[allow(dead_code)]
    model: ChainTraceModel,
    watch: WindowWatch,
}

impl ChainTraceWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        let model = ChainTraceModel::new(window_id, peer_id.clone());
        Self {
            window_id,
            peer_id,
            model,
            watch: WindowWatch::new(),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Chain Trace",
            description: "Inspect: walk a continuation chain by chain_id",
            scope: crate::window::WindowScope::Peer,
            create: |id, peer_id, pm| {
                let mut window = ChainTraceWindow::new(id, peer_id.to_string());
                window.model.initialize(pm);

                // Window-state watch — input-field re-render trigger.
                pm.watch_prefix(
                    &mut window.watch,
                    &window.peer_id,
                    crate::app_paths::window_state_path(
                        crate::app_paths::APP_ID,
                        &window.peer_id,
                        window.window_id,
                    ),
                );

                // Subscribe to the two trace-relevant prefixes on the
                // bound peer. Each Put fires the corresponding
                // `apply_*_change` into the cache, flipping the dirty
                // flag for re-render.
                let continuation_prefix = format!("/{}/system/continuation/", window.peer_id);
                let chain_errors_prefix =
                    format!("/{}/system/runtime/chain-errors/", window.peer_id);

                let cont_inner = window.model.cache().inner_arc();
                pm.observe_with_events(
                    &mut window.watch,
                    &window.peer_id,
                    continuation_prefix,
                    move |op| crate::chain_trace_cache::apply_continuation_change(&cont_inner, op),
                );
                let marker_inner = window.model.cache().inner_arc();
                pm.observe_with_events(
                    &mut window.watch,
                    &window.peer_id,
                    chain_errors_prefix,
                    move |op| crate::chain_trace_cache::apply_marker_change(&marker_inner, op),
                );

                Box::new(window)
            },
        }
    }
}

impl WindowView for ChainTraceWindow {
    fn title(&self) -> String {
        "Chain Trace".into()
    }

    fn type_name(&self) -> &'static str {
        "Chain Trace"
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, action: &Action, peers: &Peers) {
        if let Action::WindowEvent { window_id, event, value } = action {
            if *window_id == self.window_id && event == "set_chain_id" {
                self.model.set_chain_id(value);
                self.model.save_state(peers);
            }
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
        crate::dom::chain_trace::render(container, &output, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_type_is_peer_scoped() {
        let wt = ChainTraceWindow::window_type();
        assert_eq!(wt.name, "Chain Trace");
        assert!(matches!(wt.scope, crate::window::WindowScope::Peer));
    }

    #[tokio::test]
    async fn spawnable_from_window_type_writes_initial_state() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id();
        let wt = ChainTraceWindow::window_type();
        let view = (wt.create)(1, pid, &peers);
        assert_eq!(view.type_name(), "Chain Trace");
        assert_eq!(view.peer_id(), pid);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let state_path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, pid, 1);
        let entity = peers.get_entity(pid, &state_path);
        assert!(entity.is_some());
        assert_eq!(entity.unwrap().entity_type, "app/state/chain_trace");
    }

    #[tokio::test]
    async fn set_chain_id_via_action_persists() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id();
        let wt = ChainTraceWindow::window_type();
        let mut view = (wt.create)(1, pid, &peers);
        view.handle_action(
            &Action::WindowEvent {
                window_id: 1,
                event: "set_chain_id".into(),
                value: "CHAIN_X".into(),
            },
            &peers,
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let state_path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, pid, 1);
        let entity = peers.get_entity(pid, &state_path).unwrap();
        assert_eq!(
            crate::views::chain_trace::model::ChainTraceState::from_entity(&entity).chain_id,
            "CHAIN_X",
        );
    }
}

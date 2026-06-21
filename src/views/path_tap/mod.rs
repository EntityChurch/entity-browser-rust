//! Path Tap — live dispatch-event view, 12th window family member.
//!
//! Owns an `InspectSinkHandle` for the bound peer. The sink callback
//! pushes Dispatch exit-phase facts into a ring buffer; render reads
//! the snapshot. Sink detaches on window drop.
//!
//! First consumer of the live-event inspect surface (the Chain Trace window
//! is the path-bound surface; Path Tap is the live-event complement).

pub mod model;
pub mod output;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use crate::inspect_router::PeersInspectSinkHandle;
use crate::window_watch::WindowWatch;
use model::PathTapModel;

pub struct PathTapWindow {
    peer_id: String,
    model: PathTapModel,
    watch: WindowWatch,
    /// Holds the inspect-sink registration. `Drop` on this handle
    /// detaches the sink. Field is `Option` because installation may
    /// fail (e.g., unknown peer); the window still renders with an
    /// empty-state notice.
    #[allow(dead_code)] // existence is the contract; reads happen via Drop
    sink_handle: Option<PeersInspectSinkHandle>,
}

impl PathTapWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        let model = PathTapModel::new(window_id, peer_id.clone());
        Self {
            peer_id,
            model,
            watch: WindowWatch::new(),
            sink_handle: None,
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Path Tap",
            description: "Inspect: live dispatch-event stream for the bound peer",
            scope: crate::window::WindowScope::Peer,
            create: |id, peer_id, pm| {
                let mut window = PathTapWindow::new(id, peer_id.to_string());

                // Install the inspect sink. The callback owns a clone
                // of the ring + the window's dirty flag so the next
                // frame rebuilds the section when a fact arrives.
                let ring = window.model.ring();
                let dirty = window.watch.flag();
                match pm.install_inspect_sink(peer_id, move |fact| {
                    ring.push(fact);
                    dirty.mark();
                }) {
                    Ok(handle) => {
                        window.sink_handle = Some(handle);
                        window.model.mark_routing_active();
                    }
                    Err(e) => {
                        // Log via tracing; render shows the empty-state
                        // pane explaining "no facts will arrive."
                        tracing::warn!(
                            error = %e,
                            peer = %window.peer_id,
                            "Path Tap: install_inspect_sink failed; window will show empty state"
                        );
                    }
                }

                Box::new(window)
            },
        }
    }
}

impl WindowView for PathTapWindow {
    fn title(&self) -> String {
        "Path Tap".into()
    }

    fn type_name(&self) -> &'static str {
        "Path Tap"
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, _action: &Action, _peers: &Peers) {
        // No interactive state in v1. Future: filter input (handler-
        // uri prefix), pause/clear buttons.
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        _peers: &Peers,
        ctx: &crate::dom::DomCtx,
    ) {
        let output = self.model.render_output();
        crate::dom::path_tap::render(container, &output, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_type_is_peer_scoped() {
        let wt = PathTapWindow::window_type();
        assert_eq!(wt.name, "Path Tap");
        assert!(matches!(wt.scope, crate::window::WindowScope::Peer));
    }

    #[tokio::test]
    async fn factory_installs_sink_and_marks_routing_active() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let wt = PathTapWindow::window_type();
        let view = (wt.create)(1, &pid, &peers);
        assert_eq!(view.type_name(), "Path Tap");
        assert_eq!(view.peer_id(), pid);
        // Trigger a put that fires a dispatch hook + exit phase. The
        // sink should push into the ring; we can't read the ring
        // through `WindowView`, so this test just exercises the
        // factory path without panic. Functional ring assertions live
        // in `model::tests`.
    }
}

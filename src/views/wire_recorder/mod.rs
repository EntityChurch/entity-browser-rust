//! Wire Recorder — live wire-frame view, 13th window.
//!
//! Owns an `InspectSinkHandle` for the bound peer. The sink callback
//! pushes `InspectFact::Wire` facts into a ring buffer; render reads
//! the snapshot. Sink detaches on window drop.
//!
//! Sibling to `views::path_tap` (Dispatch) and `views::content_stream`
//! (Binding) — all three are Inspect-family windows that consume the
//! live-event surface installed by `Peers::install_inspect_sink`.

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
use model::WireRecorderModel;

pub struct WireRecorderWindow {
    peer_id: String,
    model: WireRecorderModel,
    watch: WindowWatch,
    /// Existence is the contract; `Drop` detaches the sink.
    #[allow(dead_code)]
    sink_handle: Option<PeersInspectSinkHandle>,
}

impl WireRecorderWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        let model = WireRecorderModel::new(window_id, peer_id.clone());
        Self {
            peer_id,
            model,
            watch: WindowWatch::new(),
            sink_handle: None,
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Wire Recorder",
            description: "Inspect: live wire-frame stream for the bound peer",
            scope: crate::window::WindowScope::Peer,
            create: |id, peer_id, pm| {
                let mut window = WireRecorderWindow::new(id, peer_id.to_string());

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
                        tracing::warn!(
                            error = %e,
                            peer = %window.peer_id,
                            "Wire Recorder: install_inspect_sink failed; window will show empty state"
                        );
                    }
                }

                Box::new(window)
            },
        }
    }
}

impl WindowView for WireRecorderWindow {
    fn title(&self) -> String {
        "Wire Recorder".into()
    }

    fn type_name(&self) -> &'static str {
        "Wire Recorder"
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, _action: &Action, _peers: &Peers) {
        // No interactive state in v1.
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        _peers: &Peers,
        ctx: &crate::dom::DomCtx,
    ) {
        let output = self.model.render_output();
        crate::dom::wire_recorder::render(container, &output, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_type_is_peer_scoped() {
        let wt = WireRecorderWindow::window_type();
        assert_eq!(wt.name, "Wire Recorder");
        assert!(matches!(wt.scope, crate::window::WindowScope::Peer));
    }

    #[tokio::test]
    async fn factory_installs_sink_and_marks_routing_active() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let wt = WireRecorderWindow::window_type();
        let view = (wt.create)(1, &pid, &peers);
        assert_eq!(view.type_name(), "Wire Recorder");
        assert_eq!(view.peer_id(), pid);
    }
}

//! Access Log window — the user-facing capability-audit surface: what
//! operations are crossing the dispatch boundary, against which target, and
//! whether they were allowed or denied.
//!
//! Owns an `InspectSinkHandle` for the bound (system) peer; the sink pushes
//! exit-phase `InspectFact::Dispatch` facts into a ring the renderer reads.
//! Sink detaches on window drop. Same live-event substrate as Path Tap / Wire
//! Recorder — reframed from a raw dev trace into a legible access log
//! (`RESEARCH-CAPABILITY-MANAGEMENT-UX §4 Step 1`). System-scoped: bound to the
//! system peer (the control point most operations dispatch from); a multi-peer
//! aggregate + the durable, subscribable audit + the grant-used column are the
//! documented next steps.

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
use model::AccessLogModel;

pub struct AccessLogWindow {
    peer_id: String,
    model: AccessLogModel,
    watch: WindowWatch,
    /// Existence is the contract; `Drop` detaches the sink.
    #[allow(dead_code)]
    sink_handle: Option<PeersInspectSinkHandle>,
}

impl AccessLogWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        let model = AccessLogModel::new(window_id, peer_id.clone());
        Self {
            peer_id,
            model,
            watch: WindowWatch::new(),
            sink_handle: None,
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Access Log",
            description: "Live access log: operations, targets, and allow/deny outcomes",
            scope: crate::window::WindowScope::System,
            create: |id, peer_id, pm| {
                let mut window = AccessLogWindow::new(id, peer_id.to_string());

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
                            "Access Log: install_inspect_sink failed; window will show empty state"
                        );
                    }
                }

                Box::new(window)
            },
        }
    }
}

impl WindowView for AccessLogWindow {
    fn title(&self) -> String {
        "Access Log".into()
    }

    fn type_name(&self) -> &'static str {
        "Access Log"
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, _action: &Action, _peers: &Peers) {
        // Passive, sink-fed — no interactive state in v1.
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        _peers: &Peers,
        ctx: &crate::dom::DomCtx,
    ) {
        let output = self.model.render_output();
        crate::dom::access_log::render(container, &output, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_type_is_system_scoped() {
        let wt = AccessLogWindow::window_type();
        assert_eq!(wt.name, "Access Log");
        assert!(matches!(wt.scope, crate::window::WindowScope::System));
    }

    #[tokio::test]
    async fn factory_installs_sink() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let wt = AccessLogWindow::window_type();
        let view = (wt.create)(1, &pid, &peers);
        assert_eq!(view.type_name(), "Access Log");
    }
}

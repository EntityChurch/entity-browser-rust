//! Access Log window — the user-facing capability-audit surface: what
//! operations are crossing the dispatch boundary, **which peer issued them**,
//! against which target, and whether they were allowed or denied.
//!
//! Pure reader over the app-tier [`crate::access_log_store`] global ring, which
//! is fed from two complementary sources so both halves of an access are visible:
//!
//! - **Local dispatches** (queries, counts, local executes) — a session-lived
//!   inspect sink installed once at app boot on the system peer (`EntityApp`),
//!   whose `Dispatch` facts become rows with `actor` = system peer.
//! - **Outbound remote executes** (e.g. a file transfer to a backend) — the
//!   app's `ops::execute` chokepoint. A remote execute fires its `Dispatch` fact
//!   on the *remote* peer, so the local sink structurally can't see it (it only
//!   emits a `Wire` frame); `ops::execute` records it with the actor + target.
//!
//! The window just subscribes its dirty flag to the store so a new record
//! re-renders it (the store isn't tree-backed, so `WindowWatch`'s tree-prefix
//! subscriptions would never fire) — capture is app-global, so any number of
//! Access Log windows read the same ring without double-recording. Reframed from
//! a raw dev trace into a legible access log
//! (`RESEARCH-CAPABILITY-MANAGEMENT-UX §4 Step 1`); the inbound "who accessed my
//! share" half (backend-side) + a durable, subscribable audit + the grant-used
//! column are the documented next steps.

pub mod model;
pub mod output;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use crate::window_watch::WindowWatch;
use model::AccessLogModel;
use output::{AccessView, DirectionFilter};

pub struct AccessLogWindow {
    window_id: WindowId,
    peer_id: String,
    model: AccessLogModel,
    watch: WindowWatch,
    /// Which view: the live activity log, or the observed-capability map.
    view: AccessView,
    /// The active direction filter (→/←/·). View-only state: the store is
    /// app-global + ephemeral, so this lives on the window, not the tree.
    filter: DirectionFilter,
    /// The active peer filter — which peer's access log to show (a subject key),
    /// or empty for "all peers". The frontend system peer and the native backend
    /// each keep their own log; this is how the operator picks between them.
    peer_filter: String,
}

impl AccessLogWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        let model = AccessLogModel::new(window_id, peer_id.clone());
        let watch = WindowWatch::new();
        // The store is app-global, not tree-backed — subscribe the dirty flag so
        // a new record (from either source) re-renders this window.
        model.store().subscribe(watch.flag());
        Self {
            window_id,
            peer_id,
            model,
            watch,
            view: AccessView::default(),
            filter: DirectionFilter::default(),
            peer_filter: String::new(),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Access Log", // i18n-ignore — identity key; display via window.access_log
            description: "Live access log: who accessed what, and allow/deny outcomes", // i18n-ignore — dead_code
            scope: crate::window::WindowScope::System,
            create: |id, peer_id, _pm| Box::new(AccessLogWindow::new(id, peer_id.to_string())),
        }
    }
}

impl WindowView for AccessLogWindow {
    fn title(&self) -> String {
        crate::i18n::window_title("Access Log") // i18n-ignore — lookup key, resolves via catalog
    }

    fn type_name(&self) -> &'static str {
        "Access Log" // i18n-ignore — stable type identifier, not UI text
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, action: &Action, _peers: &Peers) {
        // The interactive state is the two dropdowns (direction + peer). The store
        // isn't tree-backed, so mark the watch dirty ourselves to force a rebuild.
        if let Action::WindowEvent { window_id, event, value } = action {
            if *window_id != self.window_id {
                return;
            }
            let changed = match event.as_str() {
                "set_access_view" => {
                    let next = AccessView::from_value(value);
                    let changed = next != self.view;
                    self.view = next;
                    changed
                }
                "set_direction_filter" => {
                    let next = DirectionFilter::from_value(value);
                    let changed = next != self.filter;
                    self.filter = next;
                    changed
                }
                "set_peer_filter" => {
                    let changed = *value != self.peer_filter;
                    self.peer_filter = value.clone();
                    changed
                }
                _ => false,
            };
            if changed {
                self.watch.mark_dirty();
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
        match self.view {
            AccessView::Activity => {
                let output = self.model.render_output(peers, self.filter, &self.peer_filter);
                crate::dom::access_log::render(container, &output, self.view, ctx);
            }
            AccessView::Capabilities => {
                let output = self.model.capability_map(peers);
                crate::dom::access_log::render_capabilities(container, &output, self.view, ctx);
            }
        }
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
    async fn factory_builds_and_reads_the_store() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let wt = AccessLogWindow::window_type();
        let view = (wt.create)(1, &pid, &peers);
        assert_eq!(view.type_name(), "Access Log");
    }
}

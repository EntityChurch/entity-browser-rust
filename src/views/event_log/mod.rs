//! Event Log window — read-only view over the shared event log.

pub mod model;
pub mod output;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use model::EventLogModel;

use crate::window_watch::WindowWatch;

pub struct EventLogWindow {
    // Used only on the WASM render path; native sees it as unused.
    #[allow(dead_code)]
    model: EventLogModel,
    watch: WindowWatch,
    /// The peer this window was spawned against.
    ///
    /// ⭐ **Held only so it can be REPORTED.** Every read here goes through the
    /// system peer the factory resolves, so the struct had no use for the
    /// argument and dropped it — and then `WindowView::peer_id` answered `""`,
    /// which is what makes a window invisible to `find_open` and therefore
    /// impossible to focus instead of duplicating. The binding is the window
    /// manager's fact about this window, not this window's fact about itself:
    /// answering with what we were handed is what keeps the spawn, the reuse and
    /// the close talking about the same window.
    peer_id: String,
}

impl EventLogWindow {
    pub fn new(peer_id: &str) -> Self {
        Self {
            model: EventLogModel::new(),
            watch: WindowWatch::new(),
            peer_id: peer_id.to_string(),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Event Log", // i18n-ignore — identity key; display via window.event_log
            description: "Connection events, execute results, and errors", // i18n-ignore — dead_code
            scope: crate::window::WindowScope::System,
            create: |_id, peer_id, pm| {
                let mut window = EventLogWindow::new(peer_id);
                // Stage C: per-event subscription drives
                // the in-memory `EventLogCache`. apply_change updates
                // the cache mirror; render reads in-memory only.
                let pid = pm.system_peer_id().to_string();
                let prefix = crate::app_paths::event_log_prefix(crate::app_paths::APP_ID, &pid);
                let inner = window.model.cache().inner_arc();
                pm.observe_with_events(
                    &mut window.watch,
                    &pid,
                    prefix,
                    move |op| crate::event_log_cache::apply_change(&inner, op),
                );
                Box::new(window)
            },
        }
    }
}



impl WindowView for EventLogWindow {
    fn title(&self) -> String {
        crate::i18n::window_title("Event Log") // i18n-ignore — lookup key
    }

    fn type_name(&self) -> &'static str {
        "Event Log" // i18n-ignore — stable type identifier, not UI text
    }

    /// What the window manager bound this to — see the field.
    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, _action: &Action, _peers: &Peers) {}

    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        peers: &Peers,
        ctx: &crate::dom::DomCtx,
    ) {
        let output = self.model.render_output(peers);
        crate::dom::event_log::render(container, &output, ctx);
    }
}

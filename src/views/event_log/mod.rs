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
}

impl EventLogWindow {
    pub fn new() -> Self {
        Self {
            model: EventLogModel::new(),
            watch: WindowWatch::new(),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Event Log",
            description: "Connection events, execute results, and errors",
            scope: crate::window::WindowScope::System,
            create: |_id, _peer_id, pm| {
                let mut window = EventLogWindow::new();
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

impl Default for EventLogWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowView for EventLogWindow {
    fn title(&self) -> String {
        "Event Log".into()
    }

    fn type_name(&self) -> &'static str {
        "Event Log"
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

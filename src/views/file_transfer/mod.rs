//! File Transfer window — move files between this device and a paired peer.
//!
//! Slice 0 of DESIGN-CROSS-DEVICE-FILE-TRANSFER: list + pull a file from a
//! connected peer's exposed `local/files/shared/` root over `entity://`.
//! Purpose-built (not the generic Execute Console) so it can grow into a
//! real file-management surface — but it reuses the same proven dispatch
//! path (`Action::Execute`) and event-log result plumbing.

pub mod model;
pub mod output;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use model::FileTransferModel;

use crate::window_watch::WindowWatch;

pub struct FileTransferWindow {
    window_id: WindowId,
    peer_id: String,
    model: FileTransferModel,
    watch: WindowWatch,
}

impl FileTransferWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        let model = FileTransferModel::new(window_id, peer_id.clone());
        Self {
            window_id,
            peer_id,
            model,
            watch: WindowWatch::new(),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "File Transfer",
            description: "Move files between this device and a paired peer",
            scope: crate::window::WindowScope::System,
            create: |id, peer_id, pm| {
                let mut window = FileTransferWindow::new(id, peer_id.to_string());
                let sys_pid = pm.system_peer_id().to_string();
                // Results surface via the shared event log — subscribe the
                // model's cache to it (same pattern as Execute Console).
                let event_log_inner = window.model.event_log_cache().inner_arc();
                pm.observe_with_events(
                    &mut window.watch,
                    &sys_pid,
                    crate::app_paths::event_log_prefix(crate::app_paths::APP_ID, &sys_pid),
                    move |op| crate::event_log_cache::apply_change(&event_log_inner, op),
                );
                // Wake the target selector when connections change.
                pm.watch_prefix(
                    &mut window.watch,
                    &sys_pid,
                    crate::app_paths::connections_prefix(crate::app_paths::APP_ID, &sys_pid),
                );
                Box::new(window)
            },
        }
    }
}

impl WindowView for FileTransferWindow {
    fn title(&self) -> String {
        "File Transfer".into()
    }

    fn type_name(&self) -> &'static str {
        "File Transfer"
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, action: &Action, _peers: &Peers) {
        if let Action::WindowEvent { window_id, event, value } = action {
            if *window_id == self.window_id {
                match event.as_str() {
                    "select_target" => self.model.select_target(value),
                    "set_filename" => self.model.set_filename(value),
                    _ => {}
                }
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
        crate::dom::file_transfer::render(container, &output, ctx);
    }
}

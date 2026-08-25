//! Theme Editor window — create, edit, and delete user-defined themes.
//!
//! Architecture (settings-window pattern):
//! - The **model** (`model.rs`) is a thin typed accessor over the editor's
//!   per-window state entity; the themes themselves live in the runtime
//!   registry (`theme_tokens`) + the tree (`user_themes`).
//! - This **window** translates `Action`s into model calls.
//! - The **DOM renderer** (`crate::dom::theme_editor`) consumes a pure
//!   [`ThemeEditorOutput`](model::ThemeEditorOutput); edit buffers live in
//!   DOM inputs, read back at preview/save time.

pub mod model;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use model::ThemeEditorModel;

use crate::window_watch::WindowWatch;

pub struct ThemeEditorWindow {
    #[allow(dead_code)]
    window_id: WindowId,
    peer_id: String,
    model: ThemeEditorModel,
    watch: WindowWatch,
}

impl ThemeEditorWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        let model = ThemeEditorModel::new(window_id, peer_id.clone());
        Self { window_id, peer_id, model, watch: WindowWatch::new() }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Theme Editor",
            description: "Create and edit user-defined color themes",
            scope: crate::window::WindowScope::System,
            create: |id, peer_id, pm| {
                let mut window = ThemeEditorWindow::new(id, peer_id.to_string());
                // Re-render on: our own state transitions (editing/status/
                // revision) …
                pm.watch_prefix(&mut window.watch, &window.peer_id, window.model.state_path());
                // … any theme change (another editor window, another tab,
                // the boot sync landing) — the dropdowns and rows read the
                // registry, which is synced from this prefix …
                pm.watch_prefix(
                    &mut window.watch,
                    &window.peer_id,
                    crate::app_paths::user_themes_prefix(crate::app_paths::APP_ID, &window.peer_id),
                );
                // … and the settings entity (the in-use flag on Delete
                // follows the current chrome theme / site override).
                pm.watch_prefix(
                    &mut window.watch,
                    &window.peer_id,
                    crate::app_paths::settings_path(
                        crate::app_paths::APP_ID,
                        &window.peer_id,
                        crate::views::settings::model::SETTINGS_PATH,
                    ),
                );
                Box::new(window)
            },
        }
    }
}

impl WindowView for ThemeEditorWindow {
    fn title(&self) -> String {
        "Theme Editor".into()
    }

    fn type_name(&self) -> &'static str {
        "Theme Editor"
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, action: &Action, peers: &Peers) {
        if let Action::WindowEvent { event, value, .. } = action {
            match event.as_str() {
                "load_theme" => self.model.load_theme(value, peers),
                "create_theme" => self.model.create_theme(value, peers),
                "save_theme" => self.model.save_theme(value, peers),
                "revert_theme" => self.model.revert(peers),
                "delete_theme" => self.model.delete_theme(peers),
                _ => {}
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(&self, container: &web_sys::Element, peers: &Peers, ctx: &crate::dom::DomCtx) {
        let output = self.model.render_output(peers);
        crate::dom::theme_editor::render(container, &output, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn flush() {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }

    #[tokio::test]
    async fn window_actions_drive_the_model() {
        let peers = Peers::new_direct();
        let wt = ThemeEditorWindow::window_type();
        assert_eq!(wt.name, "Theme Editor");
        let mut view = (wt.create)(1, peers.system_peer_id(), &peers);

        view.handle_action(
            &Action::WindowEvent {
                window_id: 1,
                event: "create_theme".into(),
                value: "dark\x1fwindow-made".into(),
            },
            &peers,
        );
        flush().await;
        assert!(crate::theme_tokens::registered("window-made").is_some());

        view.handle_action(
            &Action::WindowEvent { window_id: 1, event: "delete_theme".into(), value: String::new() },
            &peers,
        );
        flush().await;
        assert!(crate::theme_tokens::registered("window-made").is_none());
    }
}

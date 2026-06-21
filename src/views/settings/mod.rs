//! Settings window — application configuration backed by entity state.
//!
//! Architecture:
//!
//! - The **model** (`model.rs`) is a thin typed accessor over the
//!   global settings entity (`app/entity-browser/settings/ui`). It
//!   holds **no in-memory state** because settings is shared across
//!   all Settings window instances — an in-memory mirror would go
//!   stale when another window writes the same path.
//! - This **window** is a thin controller that translates `Action`s
//!   into model method calls.
//! - The **DOM renderer** (`crate::dom::settings`) consumes a pure
//!   [`SettingsOutput`](output::SettingsOutput) and never touches
//!   `Peers`.

pub mod model;
pub mod output;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use model::{SettingsModel, SETTINGS_PATH};

use crate::window_watch::WindowWatch;

pub struct SettingsWindow {
    #[allow(dead_code)]
    window_id: WindowId,
    peer_id: String,
    model: SettingsModel,
    watch: WindowWatch,
}

impl SettingsWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        let model = SettingsModel::new(window_id, peer_id.clone());
        Self {
            window_id,
            peer_id,
            model,
            watch: WindowWatch::new(),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Settings",
            description: "Application settings and preferences",
            scope: crate::window::WindowScope::System,
            create: |id, peer_id, pm| {
                let mut window = SettingsWindow::new(id, peer_id.to_string());
                window.model.ensure_state(pm);
                // Subscribe to the global settings entity. Any settings
                // write — from this window, another instance, or any
                // other writer — flips the dirty flag. Routes to
                // ctx.store().subscribe in Direct mode and proxy.observe
                // in Worker mode via the Peers facade.
                let path = crate::app_paths::settings_path(crate::app_paths::APP_ID, &window.peer_id, SETTINGS_PATH);
                pm.watch_prefix(&mut window.watch, &window.peer_id, path);
                // Also watch the session config entity (Site & Surface
                // section) so a change here — or from boot / the status-bar
                // toggle — re-renders. On the Worker arm the cache mirror only
                // feeds subscribed prefixes, so this observe is what makes the
                // section reflect external writes.
                pm.watch_prefix(
                    &mut window.watch,
                    &window.peer_id,
                    crate::session_config::state_path(&window.peer_id),
                );
                // Site-target dropdown reads `{peer}/sites/` via L0
                // `.list`; on the Worker arm the cache mirror only feeds
                // *subscribed* prefixes, so watch the system peer's sites here
                // (the default target peer). Other peers' sites get watched
                // dynamically when the peer dropdown changes (`set_boot_peer`).
                pm.watch_prefix(
                    &mut window.watch,
                    &window.peer_id,
                    crate::content_site::paths::sites_prefix(&window.peer_id),
                );
                // Peer dropdown must REACT to the roster: a peer added/deleted
                // re-renders the control. The roster lives in the tree under the
                // peers-registry prefix (the single peer-membership reactivity
                // mechanism) — observe it. On delete the config self-heal also
                // writes the session config (subscribed above), so the selection
                // re-validates too.
                pm.watch_prefix(
                    &mut window.watch,
                    &window.peer_id,
                    crate::app_paths::peers_registry_prefix(
                        crate::app_paths::APP_ID,
                        &window.peer_id,
                    ),
                );
                // The Site boot-target picker lists every site in my store —
                // owned + cached foreign — via the derived site index
                // (`discovery::read_site_index`). Observe the index path so the
                // picker re-renders when an async refresh populates it, then
                // kick that refresh: "call it, let it populate, pick it up via
                // the subscription." The index is a materialized
                // `list_all_sites` type-query (the L1 query can't be awaited in
                // the sync render path).
                pm.watch_prefix(
                    &mut window.watch,
                    &window.peer_id,
                    crate::app_paths::site_index_path(crate::app_paths::APP_ID, &window.peer_id),
                );
                crate::content_site::discovery::refresh_site_index(pm, &window.peer_id);
                Box::new(window)
            },
        }
    }
}

impl WindowView for SettingsWindow {
    fn title(&self) -> String {
        "Settings".into()
    }

    fn type_name(&self) -> &'static str {
        "Settings"
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
                "set_theme" => self.model.set_theme(value, peers),
                "set_site_appearance" => self.model.set_site_appearance(value, peers),
                "toggle_inspector" => self.model.toggle_inspector(peers),
                "toggle_autoconnect" => self.model.toggle_autoconnect(peers),
                "toggle_singleton_windows" => self.model.toggle_singleton_windows(peers),
                // Site & Surface — the startup-surface (peer, kind, target).
                "set_profile" => self.model.set_profile(value, peers),
                "set_boot_kind" => self.model.set_boot_kind(value, peers),
                "set_boot_peer" => {
                    self.model.set_boot_peer(value, peers);
                    // The newly-selected peer's site list reads through the
                    // cache mirror on the Worker arm — observe its sites prefix
                    // so the Site target dropdown populates (the cross-peer half
                    // of the Worker-cache gotcha). `&mut self` here gives us the
                    // watch; idempotent re-subscribes are bounded by peer count.
                    if !value.is_empty() {
                        peers.watch_prefix(
                            &mut self.watch,
                            value,
                            crate::content_site::paths::sites_prefix(value),
                        );
                    }
                }
                "set_boot_target" => self.model.set_boot_target(value, peers),
                "toggle_show_toggle" => self.model.toggle_show_toggle(peers),
                "toggle_fast_paint" => self.model.toggle_fast_paint(peers),
                _ => {}
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
        crate::dom::settings::render(container, &output, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::{SettingsState, SETTINGS_PATH};

    fn make_peers() -> Peers {
        Peers::new_direct()
    }

    fn settings_path(peers: &Peers) -> String {
        let pid = peers.primary_peer_id();
        crate::app_paths::settings_path(crate::app_paths::APP_ID, pid, SETTINGS_PATH)
    }

    async fn flush_writes() {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    #[tokio::test]
    async fn factory_creates_with_initial_state() {
        let peers = make_peers();
        let wt = SettingsWindow::window_type();
        let _view = (wt.create)(1, peers.primary_peer_id(), &peers);
        // factory's ensure_state path is now dispatch_write (async).
        // Wait for the put to propagate.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let path = settings_path(&peers);
        let entity = peers.get_entity(peers.primary_peer_id(), &path);
        assert!(entity.is_some(), "initial state should be written to tree");
        assert_eq!(entity.unwrap().entity_type, "app/state/setting");
    }

    #[tokio::test]
    async fn handle_action_writes_to_global_path() {
        let peers = make_peers();
        let wt = SettingsWindow::window_type();
        let mut view = (wt.create)(1, peers.primary_peer_id(), &peers);

        view.handle_action(
            &Action::WindowEvent {
                window_id: 1,
                event: "set_theme".into(),
                value: "light".into(),
            },
            &peers,
        );
        flush_writes().await;

        let entity = peers.get_entity(peers.primary_peer_id(), &settings_path(&peers)).unwrap();
        assert_eq!(SettingsState::from_entity(&entity).theme, "light");
    }

    #[tokio::test]
    async fn multiple_windows_share_state() {
        let peers = make_peers();
        let wt = SettingsWindow::window_type();
        let mut v1 = (wt.create)(1, peers.primary_peer_id(), &peers);
        let mut v2 = (wt.create)(2, peers.primary_peer_id(), &peers);

        v1.handle_action(
            &Action::WindowEvent {
                window_id: 1,
                event: "set_theme".into(),
                value: "light".into(),
            },
            &peers,
        );
        flush_writes().await;

        let entity = peers.get_entity(peers.primary_peer_id(), &settings_path(&peers)).unwrap();
        assert_eq!(SettingsState::from_entity(&entity).theme, "light");

        v2.handle_action(
            &Action::WindowEvent {
                window_id: 2,
                event: "toggle_autoconnect".into(),
                value: String::new(),
            },
            &peers,
        );
        flush_writes().await;

        let entity = peers.get_entity(peers.primary_peer_id(), &settings_path(&peers)).unwrap();
        let state = SettingsState::from_entity(&entity);
        assert_eq!(state.theme, "light");
        assert!(state.auto_connect);
    }

    #[tokio::test]
    async fn second_window_does_not_overwrite_state() {
        let peers = make_peers();
        let wt = SettingsWindow::window_type();
        let mut v1 = (wt.create)(1, peers.primary_peer_id(), &peers);
        v1.handle_action(
            &Action::WindowEvent {
                window_id: 1,
                event: "set_theme".into(),
                value: "light".into(),
            },
            &peers,
        );
        flush_writes().await;

        let _v2 = (wt.create)(2, peers.primary_peer_id(), &peers);

        let entity = peers.get_entity(peers.primary_peer_id(), &settings_path(&peers)).unwrap();
        assert_eq!(SettingsState::from_entity(&entity).theme, "light");
    }
}

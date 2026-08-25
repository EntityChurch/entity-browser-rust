//! System Overview window — the one System governance window (title + type key
//! "System Overview"; see TERMINOLOGY-AND-WINDOWS.md). Renders the system-peer
//! cards + posture up top (via `system_peers`), then the **System backend**
//! peer's live detail: status (id, listen address, S↔B connection, share),
//! device authorizations, and a tail of its native `tracing` output streamed in
//! from `src-tauri` over IPC (`DESIGN-SYSTEM-BACKEND-PEER.md` §4).
//!
//! The backend detail is desktop-only in substance (the System backend and its
//! logs live in the Tauri native process). In a plain browser the window shows
//! the system-peer cards + a short "desktop app" note.

pub mod model;
pub mod output;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowType, WindowView};

use crate::window::WindowId;
use crate::window_watch::WindowWatch;
use model::SystemOverviewModel;

pub struct SystemOverviewWindow {
    // Used only on the WASM render path; native sees it as unused.
    #[allow(dead_code)]
    model: SystemOverviewModel,
    watch: WindowWatch,
    peer_id: String,
    /// This instance's id — window-local button events (`Clear`) carry it so
    /// `handle_action` can ignore events meant for other windows.
    window_id: WindowId,
}

impl SystemOverviewWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        Self {
            model: SystemOverviewModel::new(),
            watch: WindowWatch::new(),
            peer_id,
            window_id,
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            // `name` is the durable spawn/lookup key. Renamed from the legacy
            // "System Backend" to match the title; any old persisted boot-surface
            // reference still resolves via `window::canonical_window_type` (the
            // back-compat alias). Per-window state keys on the numeric id, not
            // this, so no state was stranded. See TERMINOLOGY-AND-WINDOWS.md.
            name: "System Overview", // i18n-ignore — identity key; display via window.system_overview
            description: "Govern the System peer and System backend: status, authorizations, share, live logs", // i18n-ignore — dead_code
            scope: crate::window::WindowScope::System,
            create: |id, _peer_id, pm| {
                let sys_pid = pm.system_peer_id().to_string();
                let mut window = SystemOverviewWindow::new(id, sys_pid.clone());
                // Watch the connections registry so the S↔B status repaints the
                // instant S connects to B (independent of the log poll cadence).
                pm.watch_prefix(
                    &mut window.watch,
                    &sys_pid,
                    crate::app_paths::connections_prefix(crate::app_paths::APP_ID, &sys_pid),
                );
                // `read_connections` resolves each peer's address from the
                // KERNEL's route entity — the single durable home of an address
                // (`MODEL-REMOTE-PEER-FACTS` §1). On the Worker arm a tree read
                // hits a cache mirror populated only for subscribed prefixes, so
                // without this watch every address reads empty for peers that
                // are perfectly reachable.
                pm.watch_prefix(
                    &mut window.watch,
                    &sys_pid,
                    crate::transport_profiles::routes_prefix(&sys_pid),
                );
                // The KERNEL liveness surface behind the S↔B link chip — the
                // authoritative `connected/suspect/disconnected` (subscribe,
                // don't poll). See the helper for why it is every vantage.
                crate::peer_liveness::watch_all_vantages(pm, &mut window.watch);
                // The app-owned `Dialing` transient (a dial in flight, before the
                // kernel writes any status) is an in-memory marker now
                // (`crate::dial_markers`), not a tree entity — nothing to watch.
                // Watch the backend-auth mirror so the device-authorizations
                // surface repaints when a Check/Refresh read lands — and (Worker
                // arm) so the synchronous `get_entity` read is seeded for this
                // prefix. `authz` mirrors our own grant decisions (a sibling
                // read on authorize); watch it too.
                pm.watch_prefix(
                    &mut window.watch,
                    &sys_pid,
                    crate::app_paths::backend_auth_prefix(crate::app_paths::APP_ID, &sys_pid),
                );
                pm.watch_prefix(
                    &mut window.watch,
                    &sys_pid,
                    crate::app_paths::authz_prefix(crate::app_paths::APP_ID, &sys_pid),
                );
                // Merged-in overview projection: watch the peer registry (roster
                // → the native peer appears/leaves, counts move) and the system
                // config (posture) so the peer cards + posture line up top react.
                // In Worker mode these also seed the sync cache the projection
                // reads (subscribe-don't-poll: read only what you watch).
                pm.watch_prefix(
                    &mut window.watch,
                    &sys_pid,
                    crate::app_paths::peers_registry_prefix(crate::app_paths::APP_ID, &sys_pid),
                );
                pm.watch_prefix(
                    &mut window.watch,
                    &sys_pid,
                    crate::session_config::state_path(&sys_pid),
                );
                // Kick the background poll loop (status + log tail). No-op in a
                // browser (guards on `is_tauri`); ends when the window closes.
                #[cfg(target_arch = "wasm32")]
                window.model.start_polling(window.watch.flag());
                Box::new(window)
            },
        }
    }
}

impl WindowView for SystemOverviewWindow {
    fn title(&self) -> String {
        crate::i18n::window_title("System Overview") // i18n-ignore — lookup key, resolves via catalog
    }

    fn type_name(&self) -> &'static str {
        // Durable key — must match `window_type().name`. See the note there.
        "System Overview" // i18n-ignore — stable type identifier, not UI text
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, action: &Action, _peers: &Peers) {
        let Action::WindowEvent { window_id, event, value } = action else {
            return;
        };
        if *window_id != self.window_id {
            return;
        }
        match event.as_str() {
            "sb_clear_logs" => {
                self.model.clear_logs();
                // Synchronous mutation — nothing else would repaint, so force the
                // rebuild that shows the cleared pane.
                self.watch.mark_dirty();
            }
            "sb_set_level" => {
                #[cfg(target_arch = "wasm32")]
                {
                    self.model.set_level(value);
                    // Reflect the new selection immediately.
                    self.watch.mark_dirty();
                }
                #[cfg(not(target_arch = "wasm32"))]
                let _ = value;
            }
            // Serve (or stop serving) §6.5 rendezvous for browsers that can
            // reach this desktop. `value` carries the backend peer-id and the
            // desired state, because the row that raises this is the only place
            // that knows which backend it is describing.
            "sb_set_signaling_node" => {
                #[cfg(target_arch = "wasm32")]
                {
                    if let Some((pid, want)) = value.split_once('\u{1f}') {
                        self.model
                            .set_signaling_node(pid, want == "1", self.watch.flag());
                    }
                }
                #[cfg(not(target_arch = "wasm32"))]
                let _ = value;
            }
            // Serve (or stop serving) the SPA over HTTP, so another device on
            // this network can load it. Same `\x1f`-packed shape as its two
            // neighbours and for the same reason — the row is the only place
            // that knows both which backend and which direction.
            "sb_set_app_server" => {
                #[cfg(target_arch = "wasm32")]
                {
                    if let Some((pid, want)) = value.split_once('\u{1f}') {
                        self.model.set_app_server(pid, want == "1", self.watch.flag());
                    }
                }
                #[cfg(not(target_arch = "wasm32"))]
                let _ = value;
            }
            "sb_set_port_mapping" => {
                #[cfg(target_arch = "wasm32")]
                {
                    if let Some((pid, want)) = value.split_once('\u{1f}') {
                        self.model
                            .set_port_mapping(pid, want == "1", self.watch.flag());
                    }
                }
                #[cfg(not(target_arch = "wasm32"))]
                let _ = value;
            }
            _ => {}
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        peers: &Peers,
        ctx: &crate::dom::DomCtx,
    ) {
        let output = self.model.render_output(peers, &ctx.dial_markers);
        // Merged overview projection (stateless): re-derived from Peers + the
        // watched registry/config prefixes each render.
        let overview =
            crate::views::system_peers::model::SystemPeersModel::new().render_output(peers);

        // Reactive device authorizations (no button): while connected and a
        // backend is present, auto-fire the backend-auth re-read on a throttle.
        // The read writes the subscribed mirror (which repaints) and dedupes, so
        // this is quiet when nothing changed. It's also the real S↔B liveness
        // probe — a stale link surfaces as the read failing, then self-heals on
        // reconnect. `frame()` drains render-time actions the same frame.
        if output.connected {
            if let Some(auth) = &output.authorizations {
                // Adaptive cadence: fast while a device awaits authorization,
                // backed off in the steady state (see `due_for_auth_refresh`).
                if self.model.due_for_auth_refresh(!auth.pending.is_empty()) {
                    ctx.actions.borrow_mut().push(Action::RefreshBackendAuth {
                        local_peer_id: auth.manager_pid.clone(),
                        backend_pid: auth.backend_pid.clone(),
                    });
                    // Pending actions drain at the top of the next frame; nudge
                    // one so the read fires now rather than on the next dirty.
                    // The throttle prevents this from re-pushing → no loop.
                    (ctx.repaint)();
                }
            }
        }

        crate::dom::system_overview::render(container, &output, &overview, ctx);
    }
}

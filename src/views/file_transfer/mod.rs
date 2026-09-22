//! File Transfer window — move files between this device and a paired peer.
//!
//! Slice 0 of DESIGN-CROSS-DEVICE-FILE-TRANSFER: list + pull a file from a
//! connected peer's exposed `local/files/shared/` root over `entity://`.
//! Purpose-built (not the generic Execute Console) so it can grow into a
//! real file-management surface — but it reuses the same proven dispatch
//! path (`Action::Execute`) and event-log result plumbing.

pub mod browse;
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

    /// Fetch a directory's listing from the remote share and fold it into the
    /// browse cache, then repaint via the window's [`DirtyFlag`]. `relpath` is
    /// the share-relative path (`""` = root); `force` re-lists an already-cached
    /// directory (Refresh). Idempotent — `begin_load` suppresses duplicate /
    /// in-flight fetches. The list op is exactly what the window already drives
    /// (`entity://{target}/local/files` `list`), just consumed structurally
    /// (the directory entity) instead of as an event-log string.
    #[cfg(target_arch = "wasm32")]
    fn load_dir(&self, peers: &Peers, target: &str, relpath: &str, force: bool) {
        if target.is_empty() {
            return;
        }
        let cache = self.model.browse().clone();
        if !cache.begin_load(relpath, force) {
            return; // already listed (and not forced) or in flight
        }
        let resource = if relpath.is_empty() {
            model::SHARE_PREFIX.to_string()
        } else {
            format!("{}{}/", model::SHARE_PREFIX, relpath)
        };
        let fut = crate::ops::execute(
            peers,
            crate::ops::ExecuteRequest {
                peer_id: self.peer_id.clone(),
                handler_uri: format!("entity://{}/local/files", target),
                operation: "list".into(),
                params: None,
                resource: Some(resource),
            },
        );
        let dirty = self.watch.flag();
        let relpath = relpath.to_string();
        wasm_bindgen_futures::spawn_local(async move {
            match fut.await {
                Ok(resp) if resp.result.status == entity_handler::STATUS_OK => {
                    let children = model::decode_listing(&resp.result.result.data);
                    cache.apply_listing(&relpath, children);
                }
                // A peer that serves no share answers 404 `handler_not_found`,
                // which is an ANSWER and not a fault — every browser peer gives
                // it, and rendering it as an error put a red banner beside
                // working transfers. `classify_share_failure` decides; this
                // site only decodes.
                Ok(resp) => {
                    let code = entity_handler::decode_error_entity(&resp.result.result)
                        .and_then(|(code, _msg)| code);
                    cache.fail_load(
                        &relpath,
                        browse::classify_share_failure(
                            resp.result.status,
                            code.as_deref(),
                            &resp.summary,
                        ),
                    );
                }
                // A transport failure is never "this peer has no share" — we
                // never heard an answer at all.
                Err(e) => cache.fail_load(&relpath, browse::ShareState::Failed(e)),
            }
            dirty.mark();
        });
    }

    /// Fetch what `target` is **offering** and fold it into the same root
    /// listing the share fills — the other half of "one window, two kinds of
    /// serving peer".
    ///
    /// Runs beside `load_dir`, never instead of it: a peer may have a mounted
    /// share *and* offered files, and neither answer is authoritative about the
    /// other. A failure is deliberately quiet — a native peer that offers
    /// nothing simply returns an empty list, and a peer that cannot answer at
    /// all already has its error from the share half. Loudness here would put a
    /// permanent red banner on the ordinary desktop case.
    #[cfg(target_arch = "wasm32")]
    fn load_offers(&self, peers: &Peers, target: &str) {
        if target.is_empty() {
            return;
        }
        let Some(dispatch) = peers.dispatch_handle(&self.peer_id) else {
            return;
        };
        // Being shown a peer's files is intent to reach that peer — and the
        // serving side has to be attempting too, or nothing establishes
        // (`reach_keeper`). Registering here covers the case the Shell verbs
        // do not: a window opened on a peer we merely remember.
        crate::reach_keeper::global().want(&self.peer_id, target);
        let cache = self.model.browse().clone();
        let dirty = self.watch.flag();
        let target = target.to_string();
        wasm_bindgen_futures::spawn_local(async move {
            match crate::file_offer::list_offers(&dispatch, &target).await {
                // An EMPTY answer is applied too, and that is the point: a peer
                // that withdrew an offer answers with a shorter list, and a
                // cache that only merges would keep showing the file it just
                // took down. Repaint only on an actual change — this runs on
                // every Refresh and every target switch.
                Ok(offers) => {
                    if cache.apply_offers(offers) {
                        dirty.mark();
                    }
                }
                Err(e) => tracing::debug!("file transfer: no offers from {target}: {e}"),
            }
        });
    }

    /// Native builds have no async runtime for this WASM-only UI path.
    #[cfg(not(target_arch = "wasm32"))]
    fn load_dir(&self, _peers: &Peers, _target: &str, _relpath: &str, _force: bool) {}

    #[cfg(not(target_arch = "wasm32"))]
    fn load_offers(&self, _peers: &Peers, _target: &str) {}

    pub fn window_type() -> WindowType {
        WindowType {
            name: "File Transfer", // i18n-ignore — identity key; display via window.file_transfer
            description: "Move files between this device and a paired peer", // i18n-ignore — dead_code
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
                // read_connections joins the sibling authz mirror; watch it too
                // so the Worker-arm get_entity cache is seeded for the join and
                // the affordance repaints when authorization changes
                // (`[[feedback_worker_cache_get_needs_subscription]]`).
                pm.watch_prefix(
                    &mut window.watch,
                    &sys_pid,
                    crate::app_paths::authz_prefix(crate::app_paths::APP_ID, &sys_pid),
                );
                // What WE are offering, on the peer that would serve it. Read
                // and watch must name the same peer — the offer is published by
                // the window's bound peer (`Action::OfferFile`), so a watch on
                // the system peer would be right only by coincidence. Without
                // this the Worker arm's read hits an unseeded cache mirror and
                // the list stays empty forever, with the offer perfectly
                // pullable by anyone else: the write works, only *our* view of
                // it is missing.
                //
                // Proven by `a_lone_file_transfer_window_lists_what_it_offers`,
                // which boots with ONLY this window open — and needs to. The
                // e2e monolith opens every window, two of which subscribe the
                // whole peer tree, and the Worker proxy's cache is a union over
                // every subscription's mirror; with those open, dropping this
                // line changes nothing observable. Reading through another
                // subscriber's mirror is exactly the theme-dropdown bug (AP20).
                let own_pid = window.peer_id.clone();
                pm.watch_prefix(
                    &mut window.watch,
                    &own_pid,
                    crate::app_paths::offers_prefix(crate::app_paths::APP_ID, &own_pid),
                );
                // Files apps kept privately, one prefix per app set.
                for set in crate::apps::paths::APP_SETS {
                    pm.watch_prefix(
                        &mut window.watch,
                        &own_pid,
                        crate::app_paths::app_files_prefix(crate::app_paths::APP_ID, &own_pid, set),
                    );
                }
                // Reachability of the target — the axis this window lacked. The
                // registry above answers "do we know this peer"; only the kernel
                // liveness surface answers "can we reach it right now", and the
                // two are independent.
                crate::peer_liveness::watch_all_vantages(pm, &mut window.watch);
                Box::new(window)
            },
        }
    }
}

impl WindowView for FileTransferWindow {
    fn title(&self) -> String {
        crate::i18n::window_title("File Transfer") // i18n-ignore — lookup key, resolves via catalog
    }

    fn type_name(&self) -> &'static str {
        "File Transfer" // i18n-ignore — stable type identifier, not UI text
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, action: &Action, peers: &Peers) {
        let Action::WindowEvent { window_id, event, value } = action else { return };
        if *window_id != self.window_id {
            return;
        }
        match event.as_str() {
            "select_target" => {
                self.model.select_target(value);
                // Load the newly-selected target's share root (a user action).
                let target = self.model.effective_target(peers);
                self.model.browse().set_expanded("");
                self.load_dir(peers, &target, "", false);
                self.load_offers(peers, &target);
            }
            "set_filename" => self.model.set_filename(value),
            // Re-list the share root (Browse / Refresh button).
            "ft_refresh" => {
                let target = self.model.effective_target(peers);
                self.model.browse().set_expanded("");
                self.load_dir(peers, &target, "", true);
                self.load_offers(peers, &target);
            }
            // Expand/collapse a directory; fetch its listing on first open.
            "ft_toggle" => {
                if self.model.browse().toggle_expand(value) {
                    let target = self.model.effective_target(peers);
                    self.load_dir(peers, &target, value, false);
                }
            }
            // Highlight a file (the Pull target).
            "ft_select" => self.model.browse().select(value),
            // A wake with no state change of its own. The file picker writes
            // the offer's progress into `offer_attempt`, which is in-memory by
            // design — so no tree write fires, no subscription fires, and a
            // bare repaint would schedule a frame that rebuilds nothing. This
            // event exists so that surface can reach the same `mark_dirty`
            // below that every other event here relies on.
            "ft_wake" => {}
            _ => {}
        }
        // Selection/expand toggles change only in-memory state (no tree write),
        // so nothing else would repaint — mark dirty to force the rebuild.
        self.watch.mark_dirty();
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        peers: &Peers,
        ctx: &crate::dom::DomCtx,
    ) {
        // Auto-load the share root the first time a target resolves (ROADMAP
        // 3d / S5) — no manual "Browse" click. Settle the cache's target first
        // (so a switch resets the one-shot guard), claim the load, then build
        // the output — which reports `root_loading` so the window shows
        // "Loading…" rather than the Browse button this frame.
        let target = self.model.effective_target(peers);
        if !target.is_empty() {
            self.model.browse().sync_target(&target);
            if self.model.browse().claim_auto_load() {
                self.load_dir(peers, &target, "", false);
                self.load_offers(peers, &target);
            }
        }
        let output = self.model.render_output(peers, &ctx.dial_markers);
        crate::dom::file_transfer::render(container, &output, ctx);
    }
}

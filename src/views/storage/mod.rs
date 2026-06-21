//! Storage window — a read-only overview of what each hosted peer is keeping
//! in its stores, and where the bloat is.
//!
//! Motivation: a peer's
//! tree is a **content-addressed, append-only** ContentStore (entity bodies,
//! never reaped by `put`) plus a mutable LocationIndex (`path → hash`).
//! Overwriting a path moves the pointer; the superseded blob stays. Save-state
//! churn (`dom::games` persists on every move) makes the blob count grow
//! unbounded while only the latest value is ever read. This window is the
//! visibility layer that confronts that — the dashboard the cleanup work
//! (coalesce writes → reclaim save blobs → kernel GC) reports into.
//!
//! Read-only by design: no reclamation here, so no risk. It surfaces, per
//! hosted peer, the content-store blob count, the live-path count, the
//! approximate orphan gap between them, a per-prefix breakdown of where the
//! live paths sit, and the save-state churn highlight; plus the origin-level
//! disk estimate (`navigator.storage.estimate()`, IndexedDB arm).

pub mod model;
pub mod output;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use crate::window_watch::WindowWatch;
use model::StorageModel;

#[cfg(target_arch = "wasm32")]
use output::OriginEstimate;

/// `WindowEvent` name the Refresh button emits — re-reads counts and re-probes
/// the origin disk estimate. Defined here so the native `handle_action`
/// matches it without the wasm-only `dom` module.
pub const REFRESH_EVENT: &str = "refresh_storage";

pub struct StorageWindow {
    #[allow(dead_code)] // bound peer (system); window enumerates all hosted peers
    peer_id: String,
    model: StorageModel,
    watch: WindowWatch,
    /// Origin disk estimate, filled in asynchronously (the `estimate()` API is
    /// promise-based). `None` until the first probe resolves.
    #[cfg(target_arch = "wasm32")]
    estimate: std::rc::Rc<std::cell::RefCell<Option<OriginEstimate>>>,
    /// Set when a fresh estimate probe should be kicked on the next render
    /// (boot + each Refresh). Guards against re-spawning on every frame.
    #[cfg(target_arch = "wasm32")]
    needs_estimate: std::rc::Rc<std::cell::Cell<bool>>,
}

impl StorageWindow {
    pub fn new(peer_id: String) -> Self {
        Self {
            peer_id,
            model: StorageModel::new(),
            watch: WindowWatch::new(),
            #[cfg(target_arch = "wasm32")]
            estimate: std::rc::Rc::new(std::cell::RefCell::new(None)),
            #[cfg(target_arch = "wasm32")]
            needs_estimate: std::rc::Rc::new(std::cell::Cell::new(true)),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Storage",
            description: "Per-peer content-store / tree usage and disk estimate",
            scope: crate::window::WindowScope::System,
            create: |_id, peer_id, pm| {
                let mut window = StorageWindow::new(peer_id.to_string());
                // Subscription-driven like every other window (the dirty flag
                // coalesces a write burst into one rebuild per frame). Watch
                // each hosted peer's whole tree so the counts/breakdown track
                // live writes — including the games save-state churn this
                // window exists to watch. The peer-registry prefix flips dirty
                // when a peer is added/removed so the peer list stays current
                // (a brand-new peer's *internal* writes need a re-open — same
                // open-time bound as the other windows; the Refresh button and
                // any registry change cover it). Refresh additionally re-probes
                // the async disk estimate, which has no tree signal.
                let sys = pm.system_peer_id().to_string();
                window.watch_all(pm, &sys);
                Box::new(window)
            },
        }
    }

    /// Subscribe the watch to every hosted peer's tree + the peer registry, so
    /// the dashboard rebuilds reactively on any storage write.
    fn watch_all(&mut self, pm: &Peers, system_peer_id: &str) {
        for pid in pm.peer_ids() {
            pm.watch_prefix(&mut self.watch, &pid, format!("/{pid}/"));
        }
        pm.watch_prefix(
            &mut self.watch,
            system_peer_id,
            crate::app_paths::peers_registry_prefix(crate::app_paths::APP_ID, system_peer_id),
        );
    }
}

impl WindowView for StorageWindow {
    fn title(&self) -> String {
        "Storage".into()
    }

    fn type_name(&self) -> &'static str {
        "Storage"
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, action: &Action, _peers: &Peers) {
        if let Action::WindowEvent { event, .. } = action {
            if event == REFRESH_EVENT {
                // The per-peer counts/breakdown track the tree via subscription
                // (no button needed). Refresh exists to re-probe the async
                // origin **disk estimate**, which has no tree signal.
                #[cfg(target_arch = "wasm32")]
                self.needs_estimate.set(true);
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
        // Kick a one-shot origin-estimate probe if one is pending. On resolve
        // it stashes the value and flips the watch dirty so the next frame
        // paints it.
        if self.needs_estimate.replace(false) {
            let est = self.estimate.clone();
            let flag = self.watch.flag();
            wasm_bindgen_futures::spawn_local(async move {
                if let Some(value) = fetch_origin_estimate().await {
                    *est.borrow_mut() = Some(value);
                    flag.mark();
                }
            });
        }

        let estimate = *self.estimate.borrow();
        let output = self.model.render_output(peers, estimate);
        crate::dom::storage::render(container, &output, ctx);
    }
}

/// Probe `navigator.storage.estimate()` (+ `persisted()`) for the origin-wide
/// disk figures. Returns `None` where the API is unavailable (older runtimes /
/// some automation contexts) — the renderer then simply omits the line.
#[cfg(target_arch = "wasm32")]
async fn fetch_origin_estimate() -> Option<OriginEstimate> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    let global = js_sys::global();
    let navigator = js_sys::Reflect::get(&global, &"navigator".into()).ok()?;
    let storage = js_sys::Reflect::get(&navigator, &"storage".into()).ok()?;
    if storage.is_undefined() || storage.is_null() {
        return None;
    }

    // estimate() → { usage, quota }
    let est_fn: js_sys::Function = js_sys::Reflect::get(&storage, &"estimate".into())
        .ok()?
        .dyn_into()
        .ok()?;
    let result = JsFuture::from(js_sys::Promise::from(est_fn.call0(&storage).ok()?))
        .await
        .ok()?;
    let usage_bytes = js_sys::Reflect::get(&result, &"usage".into())
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let quota_bytes = js_sys::Reflect::get(&result, &"quota".into())
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    // persisted() → bool (best-effort; absent on some engines)
    let persisted = match js_sys::Reflect::get(&storage, &"persisted".into())
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
    {
        Some(f) => match f.call0(&storage) {
            Ok(p) => JsFuture::from(js_sys::Promise::from(p))
                .await
                .ok()
                .and_then(|r| r.as_bool()),
            Err(_) => None,
        },
        None => None,
    };

    Some(OriginEstimate {
        usage_bytes,
        quota_bytes,
        persisted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_type_is_system_scoped() {
        let t = StorageWindow::window_type();
        assert_eq!(t.name, "Storage");
        assert!(matches!(t.scope, crate::window::WindowScope::System));
    }
}

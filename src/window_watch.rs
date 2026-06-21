//! Per-window subscription-driven reactivity.
//!
//! A window holding a [`WindowWatch`] subscribes to the tree prefixes it
//! reads on each render. Tree writes that match those prefixes flip the
//! watch's dirty flag through SDK L0 subscriptions. The DOM renderer
//! reads + clears the flag on each frame and rebuilds only the windows
//! that are dirty.
//!
//! This is the replacement for `WindowView::content_hash` / the global
//! `legacy_hash` fallback. During the migration both mechanisms coexist:
//! windows that expose a `WindowWatch` via [`WindowView::watch`] use the
//! dirty-flag path; the others fall back to the legacy hash.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use entity_sdk::PeerContext;
use entity_sdk::sdk::SubscriptionHandle;

#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;
#[cfg(target_arch = "wasm32")]
use std::rc::{Rc, Weak};
#[cfg(target_arch = "wasm32")]
use entity_wasm_worker_proxy::{SubHandle, WebTransport};

/// Cheaply-clonable handle to a window's dirty flag.
///
/// Cloned into long-lived consumers (subscription callbacks running on
/// background tasks, model setters that don't have access to the
/// `WindowWatch` itself) so they can mark the window dirty without
/// owning the watch.
#[derive(Clone)]
pub struct DirtyFlag {
    inner: Arc<AtomicBool>,
}

#[allow(dead_code)] // public surface; not used on native (dom is WASM-only)
impl DirtyFlag {
    /// Mark the watch dirty so the next frame rebuilds the window's section.
    pub fn mark(&self) {
        self.inner.store(true, Ordering::Relaxed);
    }
}

/// Subscription-driven dirty flag for one window.
///
/// Hold this on the window struct. On window-factory construction,
/// register the prefixes the window's render reads via
/// [`subscribe_prefix`](Self::subscribe_prefix). The DOM renderer calls
/// [`take_dirty`](Self::take_dirty) once per frame to decide whether to
/// rebuild the section.
pub struct WindowWatch {
    dirty: Arc<AtomicBool>,
    /// L0 subscription handles. Held to keep the subscriptions alive;
    /// dropped when the window closes (cancels them automatically).
    handles: Vec<SubscriptionHandle>,
    /// Worker-mode subscription handles, populated asynchronously by
    /// `watch_prefix` via the proxy's `observe()`. Wrapped in
    /// `Rc<RefCell<...>>` so the spawn_local bridging task can `push()`
    /// from outside the borrow on `WindowWatch` itself; the task holds
    /// only a `Weak` and silently drops the handle if the window has
    /// closed before observe completed.
    #[cfg(target_arch = "wasm32")]
    worker_subs: Rc<RefCell<Vec<SubHandle<WebTransport>>>>,
}

/// Slot returned by [`WindowWatch::worker_subs_slot`]. Holds a `Weak`
/// reference to the parent watch's worker-sub vector; if the window
/// drops before `attach` is called, the SubHandle is consumed and
/// dropped (cancelling the subscription) rather than leaked.
#[cfg(target_arch = "wasm32")]
pub struct WorkerSubsSlot {
    inner: Weak<RefCell<Vec<SubHandle<WebTransport>>>>,
}

#[cfg(target_arch = "wasm32")]
impl WorkerSubsSlot {
    /// Push the subscription handle into the watch's vector. If the
    /// watch has already been dropped, `handle` is consumed here and
    /// drops — canceling its subscription cleanly.
    pub fn attach(self, handle: SubHandle<WebTransport>) {
        if let Some(rc) = self.inner.upgrade() {
            rc.borrow_mut().push(handle);
        }
        // else: window closed; handle drops here.
    }
}

#[allow(dead_code)] // some methods only used on WASM (DOM renderer)
impl WindowWatch {
    /// Create a new watch. The dirty flag starts set so the first
    /// render call after spawn always builds the section.
    pub fn new() -> Self {
        Self {
            dirty: Arc::new(AtomicBool::new(true)),
            handles: Vec::new(),
            #[cfg(target_arch = "wasm32")]
            worker_subs: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// Returns a slot the bridging task can `attach` a worker SubHandle
    /// into. Used by `peers_worker::WorkerPeerStore::watch_prefix`.
    #[cfg(target_arch = "wasm32")]
    pub fn worker_subs_slot(&self) -> WorkerSubsSlot {
        WorkerSubsSlot {
            inner: Rc::downgrade(&self.worker_subs),
        }
    }

    /// Subscribe to a tree prefix on `ctx`. Any write whose path begins
    /// with `prefix` flips the dirty flag.
    pub fn subscribe_prefix(&mut self, ctx: &PeerContext, prefix: impl Into<String>) {
        let dirty = self.dirty.clone();
        let handle = ctx.store().on_prefix_change(prefix, move |_event| {
            dirty.store(true, Ordering::Relaxed);
        });
        self.handles.push(handle);
    }

    /// Park a pre-registered subscription handle on this watch so its
    /// lifetime is tied to the window. Used by
    /// [`crate::peers::Peers::observe_with_events`] when it registers
    /// the per-event seeded subscription directly on `PeerContext` and
    /// needs the handle to outlive the call site.
    pub fn push_handle(&mut self, handle: SubscriptionHandle) {
        self.handles.push(handle);
    }

    /// Cheap clonable handle for code paths that aren't tree subscriptions
    /// — model setters, action handlers — that need to mark dirty
    /// without owning the watch.
    pub fn flag(&self) -> DirtyFlag {
        DirtyFlag {
            inner: self.dirty.clone(),
        }
    }

    /// Mark the watch dirty.
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Atomically check + clear the dirty flag. Returns `true` when the
    /// renderer should rebuild this window's section.
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }
}

impl Default for WindowWatch {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for WindowWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowWatch")
            .field("dirty", &self.dirty.load(Ordering::Relaxed))
            .field("handles", &self.handles.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use entity_ecf::{cbor_map, text, to_ecf};
    use entity_entity::Entity;
    use crate::peers::Peers;

    fn entity(message: &str) -> Entity {
        let data = to_ecf(&cbor_map! { "message" => text(message) });
        Entity::new("test/note", data).unwrap()
    }

    #[test]
    fn new_starts_dirty() {
        let watch = WindowWatch::new();
        assert!(watch.take_dirty());
        assert!(!watch.take_dirty());
    }

    #[test]
    fn mark_dirty_sets_flag() {
        let watch = WindowWatch::new();
        watch.take_dirty(); // clear initial
        assert!(!watch.take_dirty());

        watch.mark_dirty();
        assert!(watch.take_dirty());
    }

    #[test]
    fn flag_clones_share_state() {
        let watch = WindowWatch::new();
        watch.take_dirty();

        let f = watch.flag();
        let f2 = f.clone();
        f2.mark();

        assert!(watch.take_dirty());
    }

    #[tokio::test]
    async fn subscribe_prefix_marks_dirty_on_matching_write() {
        let pm = Peers::new_direct();
        let pid = pm.primary_peer_id().to_string();
        let ctx = pm.test_seed_ctx(&pid);

        let mut watch = WindowWatch::new();
        watch.subscribe_prefix(ctx, format!("/{}/test/", pid));
        watch.take_dirty(); // clear initial

        // Write under the watched prefix.
        ctx.store()
            .put(&format!("/{}/test/note-1", pid), entity("hello"))
            .unwrap();

        // Subscription delivery is async — give it a tick.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        assert!(watch.take_dirty(), "watch should be dirty after matching write");
    }

    #[tokio::test]
    async fn subscribe_prefix_ignores_non_matching_writes() {
        let pm = Peers::new_direct();
        let pid = pm.primary_peer_id().to_string();
        let ctx = pm.test_seed_ctx(&pid);

        let mut watch = WindowWatch::new();
        watch.subscribe_prefix(ctx, format!("/{}/watched/", pid));
        watch.take_dirty();

        ctx.store()
            .put(&format!("/{}/elsewhere/x", pid), entity("ignore"))
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        assert!(!watch.take_dirty(), "non-matching write must not flip flag");
    }

    #[tokio::test]
    async fn dropping_watch_cancels_subscriptions() {
        let pm = Peers::new_direct();
        let pid = pm.primary_peer_id().to_string();
        let ctx = pm.test_seed_ctx(&pid);

        let dirty = {
            let mut watch = WindowWatch::new();
            watch.subscribe_prefix(ctx, format!("/{}/test/", pid));
            watch.take_dirty();
            watch.flag()
        }; // watch dropped here, subscriptions cancelled

        // Subsequent write — the dropped watch must not see it (its flag
        // handle still exists via `dirty`, but the callback is gone).
        ctx.store()
            .put(&format!("/{}/test/x", pid), entity("after-drop"))
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        assert!(
            !dirty.inner.load(Ordering::Relaxed),
            "subscription should be cancelled when watch dropped"
        );
    }
}

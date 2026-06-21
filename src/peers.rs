//! `Peers` — the enum-dispatch facade over PeerManager (Direct) and
//! WorkerPeerStore (Worker) backends.
//!
//! Phase 3.0 architecture decision: rather than building a `PeerStore`
//! trait + async-trait machinery, we use an enum with concrete impls.
//! Each method dispatches to whichever arm is active. The compile-time
//! `worker` feature flag gates which arms exist.
//!
//! Phase 3.0 covers only the methods Settings + Event Log pilots need.
//! Phase 3.1-3.3 extend this surface as each window migrates. When all
//! windows are migrated and the worker path is the only one used in
//! production, the `Direct` arm can be removed.
//!
//! Read patterns:
//! - **Direct**: synchronous L0 reads via `PeerManager.get_entity`,
//!   `tree_listing`, etc. — direct access to the in-process store.
//! - **Worker**: synchronous reads from `wasm-worker-proxy`'s cache
//!   mirror, primed by `observe()` calls at window-spawn time. The
//!   mirror is kept in sync by Change events streamed from the worker.
//!
//! Write patterns:
//! - **Direct**: `dispatch_write` (fire-and-forget L1 put via
//!   `ctx.put().await` spawned on the SDK's task pool).
//! - **Worker**: `dispatch_write` (fire-and-forget `proxy.put().await`
//!   spawned on `wasm_bindgen_futures`).
//!
//! Subscriptions:
//! - **Direct**: `ctx.store().subscribe(prefix, callback)` → returns
//!   `entity_sdk::SubscriptionHandle`; stored in `WindowWatch.handles`.
//! - **Worker**: `proxy.observe(prefix)` → returns
//!   `(SubHandle, NotifyChannel)`; the channel is bridged to the dirty
//!   flag via a background task; the handle is held in `WindowWatch`.

pub use entity_entity::Entity;
pub use entity_store::LocationEntry;

#[cfg(target_arch = "wasm32")]
pub use crate::peers_worker::WorkerPeerStore;

use crate::window_watch::WindowWatch;
use std::collections::HashMap;

/// Normalized tree-change event for [`Peers::observe_with_events`].
///
/// Both arms (Direct's `entity_store::TreeChangeEvent` and Worker's
/// `entity_wasm_worker_proxy::ChangeEvent`) collapse into this shape
/// at the Peers boundary so consumers have one match arm regardless of
/// deployment.
///
/// We deliberately drop hash detail — the model only needs to know
/// "this path now binds" / "this path no longer binds" / "we missed
/// events; resync." If a consumer needs hashes for content diffing,
/// add it later; YAGNI for the Entity Tree refactor.
#[derive(Debug, Clone)]
pub enum ChangeOp {
    /// Path binds an entity (covers Direct `Created` + `Modified` and
    /// Worker `Created` + `Updated`). Idempotent — applying twice is
    /// a no-op on consumer state.
    Put { path: String },
    /// Path no longer binds an entity.
    Remove { path: String },
    /// Worker arm lost events to channel overflow. The proxy mirror is
    /// still authoritative; consumer should drop incremental state and
    /// resync via `tree_listing(prefix)`.
    Resync,
}

/// Detached-future return types for `Peers` lifecycle ops (§4.1
/// uniform shape). Direct produces a `ready()`; Worker awaits the
/// proxy. Native variants are `Send`; wasm variants aren't
/// (`spawn_local` doesn't require it).
#[cfg(not(target_arch = "wasm32"))]
pub type CreatePeerFuture<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<(String, [u8; 32], entity_sdk::PeerMetadata), String>,
            > + Send
            + 'a,
    >,
>;

#[cfg(target_arch = "wasm32")]
pub type CreatePeerFuture<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<(String, [u8; 32], entity_sdk::PeerMetadata), String>,
            > + 'a,
    >,
>;

/// Future yielding the connected remote peer's id, or a stringified
/// error. Used by `Peers::connect_peer`.
#[cfg(not(target_arch = "wasm32"))]
pub type ConnectPeerFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>,
>;

#[cfg(target_arch = "wasm32")]
pub type ConnectPeerFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<String, String>> + 'a>,
>;

/// Why a Direct-arm L0 escape hatch (`direct_peer_context`) yielded no
/// context. The hatches expose main-thread `PeerContext` for L0 access
/// and exist **only** on the Direct arm — reaching for one on a
/// Worker-hosted peer is a routing mistake, not an absent value, so the
/// hatch returns this typed error (never a silent `None`/primary-default)
/// and the caller must acknowledge the wrong-arm case. Cross-arm code
/// goes through the `Peers` router (`get_entity` / `tree_listing` /
/// `dispatch_write` / `put_and_wait` / `execute` / `query` / `count`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectArmError {
    /// The peer lives on a Worker SDK — there is no main-thread
    /// `PeerContext`. Route through the `Peers` L1 methods instead.
    WorkerArm,
    /// No SDK hosts this peer id (genuinely unknown — §4.4 `sdk_for`
    /// already refuses the silent slot-0 fallback).
    UnknownPeer,
}

/// SDK-host enum — one variant per host location (Direct = main thread,
/// Worker = web worker, future: Remote = native backend over IPC).
/// Internal to the application; external callers go through [`Peers`].
///
/// Each `Sdk` instance hosts one or more peers; `Peers::peer_routes`
/// maps `peer_id → sdks[idx]` so per-peer ops land on the right SDK.
pub(crate) enum Sdk {
    Direct(entity_sdk::PeerManager),
    #[cfg(target_arch = "wasm32")]
    Worker(WorkerPeerStore),
}

impl Sdk {
    pub fn primary_peer_id(&self) -> &str {
        match self {
            Sdk::Direct(pm) => pm.primary_peer_id(),
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => w.primary_peer_id(),
        }
    }

    /// Synchronous read. Direct: hits the in-process store. Worker: hits
    /// the per-prefix mirror, which must be primed via `watch_prefix`.
    /// Returns `None` if the path isn't currently mirrored (Worker) or
    /// has no binding (Direct).
    pub fn get_entity(&self, peer_id: &str, path: &str) -> Option<Entity> {
        match self {
            Sdk::Direct(pm) => pm.get_entity(peer_id, path),
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => w.cache_get(path),
        }
    }

    /// Synchronous prefix-scan. Same Direct vs Worker pattern as
    /// `get_entity`.
    pub fn tree_listing(&self, peer_id: &str, prefix: &str) -> Vec<LocationEntry> {
        match self {
            Sdk::Direct(pm) => pm.tree_listing(peer_id, prefix),
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => {
                // peer_id is implicit in `prefix` (caller passes
                // `/{peer_id}/...`) and the cache mirror is keyed by
                // full path, so per-peer disambiguation happens
                // naturally. Audited (§3.8 closeout).
                let _ = peer_id;
                w.cache_list(prefix)
            }
        }
    }

    /// Total entity count for a peer. Worker: derived from cache.list()
    /// over the peer's qualified prefix — accurate only for the prefixes
    /// the consumer has subscribed to. Phase 3.3 may add a proper L1
    /// `entity_count` round-trip if needed.
    pub fn entity_count(&self, peer_id: &str) -> usize {
        match self {
            Sdk::Direct(pm) => pm.entity_count(peer_id),
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => w.entity_count_estimate(peer_id),
        }
    }

    /// Total path count for a peer. Same caveat as `entity_count`.
    pub fn path_count(&self, peer_id: &str) -> usize {
        match self {
            Sdk::Direct(pm) => pm.path_count(peer_id),
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => w.path_count_estimate(peer_id),
        }
    }

    /// Fire-and-forget write. Both arms spawn an async put internally and
    /// return immediately; consumers see the result of the write on the
    /// next subscription event flowing back into the cache (Worker) or
    /// the next L0 read (Direct).
    pub fn dispatch_write(&self, peer_id: &str, path: impl Into<String>, entity: Entity) {
        match self {
            Sdk::Direct(pm) => pm.dispatch_write(peer_id, path, entity),
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => w.dispatch_write(peer_id.to_string(), path.into(), entity),
        }
    }

    /// Awaitable write that resolves only after the cache reflects the
    /// new entity. Use when an action handler must transition view state
    /// to display the just-written entity — `dispatch_write` returns
    /// immediately and races the subscription-driven cache update,
    /// producing a brief "no longer available" flash. See WORKER-MODE
    /// living doc §3.2.
    ///
    /// Direct arm: `ctx.put(...)` IS the cache update (the in-process
    /// tree is the cache); resolves on success. Worker arm: delegates to
    /// `WorkerPeerStore::put_and_wait` → `proxy.put_and_wait_for_cache`.
    ///
    /// Returns an owning future. `timeout_ms` only applies to the worker
    /// arm; ignored on Direct.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn put_and_wait(
        &self,
        peer_id: &str,
        path: impl Into<String>,
        entity: Entity,
        _timeout_ms: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>> {
        let path: String = path.into();
        match self {
            // Direct: the in-process tree IS the cache, so a resolved
            // put is immediately consistent — `put_and_wait`'s
            // semantics. §4.1b: flat op owns its future + folds the
            // unknown-peer miss into SdkError.
            Sdk::Direct(pm) => {
                let fut = pm.sdk().put(peer_id, &path, entity);
                Box::pin(async move { fut.await.map(|_| ()).map_err(|e| e.to_string()) })
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn put_and_wait(
        &self,
        peer_id: &str,
        path: impl Into<String>,
        entity: Entity,
        timeout_ms: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>> {
        let path: String = path.into();
        match self {
            Sdk::Direct(pm) => {
                let _ = timeout_ms;
                let fut = pm.sdk().put(peer_id, &path, entity);
                Box::pin(async move { fut.await.map(|_| ()).map_err(|e| e.to_string()) })
            }
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => Box::pin(w.put_and_wait(
                peer_id.to_string(),
                path,
                entity,
                timeout_ms,
            )),
        }
    }

    /// Durable-authoritative seed — write `default` only if the path is
    /// absent in the **durable** store, awaited. See the [`Peers`]-level
    /// [`Peers::put_if_absent`] for the full contract; this is the per-arm
    /// dispatch. `Ok(true)` = seeded, `Ok(false)` = already present.
    ///
    /// Direct arm: the in-process store is authoritative — a sync
    /// `store().get` decides absence, `store().put` seeds; wrapped in a
    /// ready future for a uniform signature. Worker arm: delegates to
    /// [`WorkerPeerStore::put_if_absent`] (L1 durable get → `put_and_wait`).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn put_if_absent(
        &self,
        peer_id: &str,
        path: impl Into<String>,
        default: Entity,
        _timeout_ms: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send>> {
        let path: String = path.into();
        let r = match self {
            Sdk::Direct(pm) => Self::direct_put_if_absent(pm, peer_id, &path, default),
        };
        Box::pin(async move { r })
    }

    #[cfg(target_arch = "wasm32")]
    pub fn put_if_absent(
        &self,
        peer_id: &str,
        path: impl Into<String>,
        default: Entity,
        timeout_ms: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>>>> {
        let path: String = path.into();
        match self {
            Sdk::Direct(pm) => {
                let _ = timeout_ms;
                let r = Self::direct_put_if_absent(pm, peer_id, &path, default);
                Box::pin(async move { r })
            }
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => {
                Box::pin(w.put_if_absent(peer_id.to_string(), path, default, timeout_ms))
            }
        }
    }

    /// Direct-arm check-and-set against the in-process store (the
    /// authoritative tree on this arm). Shared by both `cfg` forms of
    /// [`Self::put_if_absent`]. Synchronous — the store IS the cache.
    fn direct_put_if_absent(
        pm: &entity_sdk::PeerManager,
        peer_id: &str,
        path: &str,
        default: Entity,
    ) -> Result<bool, String> {
        let ctx = pm
            .peer_context(peer_id)
            .ok_or_else(|| format!("put_if_absent: no Direct context for peer {peer_id}"))?;
        if ctx.store().get(path).is_some() {
            return Ok(false);
        }
        ctx.store()
            .put(path, default)
            .map(|_| true)
            .map_err(|e| e.to_string())
    }

    /// Fire-and-forget remove. Direct arm uses sync L0 `tree.remove`.
    /// Worker arm spawns an async `proxy.remove(...)`. Like
    /// `dispatch_write`, the consumer learns the outcome via the next
    /// subscription event / cache read, not from a return value.
    pub fn dispatch_remove(&self, peer_id: &str, path: impl Into<String>) {
        let path: String = path.into();
        match self {
            Sdk::Direct(pm) => {
                if let Some(shared) = pm.peer_shared(peer_id) {
                    shared.tree.remove(&path);
                }
            }
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => w.dispatch_remove(peer_id.to_string(), path),
        }
    }

    /// Subscribe a prefix into the window's dirty-flag pattern. Direct:
    /// `ctx.store().subscribe(prefix, callback)` storing the
    /// `SubscriptionHandle` on `WindowWatch.handles`. Worker:
    /// `proxy.observe(prefix)` + a spawn_local task bridging
    /// `NotifyChannel` → dirty flag; `SubHandle` is stored on
    /// `WindowWatch.worker_subs`.
    pub fn watch_prefix(
        &self,
        watch: &mut WindowWatch,
        peer_id: &str,
        prefix: impl Into<String>,
    ) {
        let prefix = prefix.into();
        match self {
            Sdk::Direct(pm) => {
                if let Some(ctx) = pm.peer_context(peer_id) {
                    watch.subscribe_prefix(ctx, prefix);
                }
            }
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => {
                w.watch_prefix(watch, peer_id.to_string(), prefix);
            }
        }
    }

    /// Per-event subscription with seed. Normalizes Direct
    /// `TreeChangeEvent` and Worker `ChangeEvent` into a single
    /// [`ChangeOp`] for the callback. Mirrors the dirty flag on the
    /// `WindowWatch` so renderers see the same "rebuild now" trigger
    /// as `watch_prefix` consumers do.
    ///
    /// Closes the Stage A `refresh_mirror` O(N) diff loop: consumers
    /// can maintain an incremental local mirror with O(depth) work
    /// per event instead of O(N) per dirty tick. See the upstream
    /// worker-observe-event-payload design for the proxy-side primitive
    /// this depends on.
    pub fn observe_with_events<F>(
        &self,
        watch: &mut WindowWatch,
        peer_id: &str,
        prefix: impl Into<String>,
        on_event: F,
    ) where
        F: Fn(ChangeOp) + Send + Sync + 'static,
    {
        let prefix = prefix.into();
        let on_event = std::sync::Arc::new(on_event);
        match self {
            Sdk::Direct(pm) => {
                if let Some(ctx) = pm.peer_context(peer_id) {
                    let dirty = watch.flag();
                    let cb = on_event.clone();
                    let handle = ctx.store().on_prefix_change_seeded(prefix, move |ev| {
                        let op = match ev.change_type {
                            entity_store::ChangeType::Created
                            | entity_store::ChangeType::Modified => {
                                ChangeOp::Put { path: ev.path.clone() }
                            }
                            entity_store::ChangeType::Deleted => {
                                ChangeOp::Remove { path: ev.path.clone() }
                            }
                        };
                        cb(op);
                        dirty.mark();
                    });
                    watch.push_handle(handle);
                }
            }
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => {
                w.observe_with_events(watch, peer_id.to_string(), prefix, on_event);
            }
        }
    }

    /// Direct-only — returns the underlying `PeerManager` when present,
    /// `None` on the Worker arm. Provided as an escape hatch for code
    /// paths that need `peer_context` / `peer_shared` for L0 access (e.g.
    /// `event_log_writer`, `listener_state`) and haven't yet been
    /// migrated. Used by wasm32 build paths (`build_wasm_app`,
    /// the `CreatePeerWithMode`/`DeletePeer` arm gates); appears
    /// unused on native (the deprecation-stub binary) since those
    /// paths are wasm32-gated.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub fn as_direct(&self) -> Option<&entity_sdk::PeerManager> {
        match self {
            Sdk::Direct(pm) => Some(pm),
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(_) => None,
        }
    }

    // ---------------------------------------------------------------
    // L0-escape-hatch delegations.
    //
    // Phase 3.0 ships these as Direct-only — they panic on the Worker
    // arm because the L0 surface (`PeerContext`, `PeerShared`, `EntitySDK`)
    // isn't available across the worker boundary. Phase 3.3 refactors
    // each caller to either:
    //   - use a proper L1 method on Peers (e.g. dispatch_write), OR
    //   - move the logic into the worker (handler-side), OR
    //   - subscribe via watch_prefix and read from the cache.
    //
    // Until then these compile through `--features worker` builds but
    // panic if invoked at runtime. The Settings/Event Log pilots don't
    // hit any of them.
    // ---------------------------------------------------------------

    /// Returns a `PeerContext` for `peer_id` on the Direct arm.
    /// The Worker arm has no main-thread `PeerContext` (the worker
    /// host owns dispatch internally) and returns `None`. Treat as a
    /// Direct-only escape hatch — `None` means either the peer is
    /// unknown or it lives on a Worker SDK. Callers that need
    /// Worker-arm execution must go through the L1 router methods
    /// on `Peers` (`execute`, `query`, etc.) instead.
    pub fn direct_peer_context(
        &self,
        peer_id: &str,
    ) -> Result<&entity_sdk::PeerContext, DirectArmError> {
        match self {
            Sdk::Direct(pm) => pm
                .peer_context(peer_id)
                .ok_or(DirectArmError::UnknownPeer),
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(_) => Err(DirectArmError::WorkerArm),
        }
    }

    // §L1: `peer_context_or_default` was DELETED here. Its
    // "fall back to the primary peer" semantics were anti-pattern AP2 by
    // definition (silent default-to-primary) and it panicked on the
    // Worker arm — a double footgun. There was no legitimate use; the one
    // prod caller already proved Direct via `primary_as_direct()` and
    // holds its own `PeerManager`. See the peer-arm escape-hatch
    // hardening review.

    /// Returns shared peer runtime state for the Direct arm.
    /// The Worker arm has no analogue (the worker host owns shared
    /// state in its own context) and returns `None`. Treat as a
    /// Direct-only escape hatch — `None` means either the peer is
    /// unknown or it lives on a Worker SDK.
    pub fn direct_peer_shared(
        &self,
        peer_id: &str,
    ) -> Option<std::sync::Arc<entity_peer::PeerShared>> {
        match self {
            Sdk::Direct(pm) => pm.peer_shared(peer_id),
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(_) => None,
        }
    }

    /// Direct-only: read-only access to the underlying SDK. Panics on Worker.
    pub fn sdk(&self) -> &entity_sdk::EntitySDK {
        match self {
            Sdk::Direct(pm) => pm.sdk(),
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(_) => panic!("Peers::sdk not supported on Worker arm"),
        }
    }

    /// Direct-only: mutable SDK access (for peer registration etc.).
    /// Panics on Worker.
    pub fn sdk_mut(&mut self) -> &mut entity_sdk::EntitySDK {
        match self {
            Sdk::Direct(pm) => pm.sdk_mut(),
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(_) => panic!("Peers::sdk_mut not supported on Worker arm"),
        }
    }

    // §4.1a: the `delete_peer` (sync/panic-on-Worker) +
    // `delete_peer_worker` (async/Err-on-Direct) twins were collapsed
    // into the single uniform `Peers::delete_peer`. The
    // `peer_host_is_worker` band-aid is gone with them.
    //
    // §4.1b: the `create_new_peer` + `connect_peer` twins
    // followed the same collapse pattern. `Sdk` no longer exposes
    // per-arm versions; `Peers::create_new_peer` /
    // `Peers::connect_peer` match the `Sdk` variant inline. There is
    // no caller-facing arm choice for any lifecycle op.

    /// Direct-only: bootstrap-time persisted-peer load. On Worker the
    /// equivalent happens inside the worker at `InitParams` time.
    /// Used by Direct WASM bootstrap (`EntityApp::new_wasm`); appears
    /// unused in worker-only builds.
    pub fn load_persisted(&mut self, persisted: Vec<entity_sdk::PersistedPeer>) {
        match self {
            Sdk::Direct(pm) => pm.load_persisted(persisted),
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(_) => panic!("Peers::load_persisted: worker arm loads peers via InitParams"),
        }
    }

    /// Direct-only: register a Tauri-backend peer's metadata. Returns
    /// true on success. Panics on Worker — the worker has its own
    /// `Request::RegisterBackendPeer`. Used by wasm32 Tauri-IPC
    /// backend-peer flow (`handle_create_backend_peer`) and by
    /// peer_display tests; appears unused on native (the
    /// deprecation-stub binary).
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub fn register_backend_peer(
        &mut self,
        peer_id: String,
        label: Option<String>,
        listen_addresses: Vec<String>,
    ) -> bool {
        match self {
            Sdk::Direct(pm) => pm.register_backend_peer(peer_id, label, listen_addresses),
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(_) => panic!("Peers::register_backend_peer not supported on Worker arm (use Request::RegisterBackendPeer)"),
        }
    }

    /// Direct-only: access an underlying entity-peer instance. Panics on
    /// Worker. Gated by `native-ws` to match PeerManager.
    #[cfg(feature = "native-ws")]
    pub fn peer(&self, peer_id: &str) -> Option<&entity_peer::Peer> {
        match self {
            Sdk::Direct(pm) => pm.peer(peer_id),
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(_) => panic!("Peers::peer not supported on Worker arm"),
        }
    }

    /// Direct-only: synchronous "put" via the in-process tree. Panics on
    /// Worker. Worker-mode equivalent is `dispatch_write` (async) or
    /// `put_and_wait` (awaitable). Test-only — runtime code goes
    /// through the L1 surface.
    #[cfg(test)]
    pub fn put_entity(&self, peer_id: &str, path: &str, entity: Entity) -> Option<entity_hash::Hash> {
        match self {
            Sdk::Direct(pm) => pm.put_entity(peer_id, path, entity),
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(_) => panic!("Peers::put_entity not supported on Worker arm; use dispatch_write"),
        }
    }

    /// Worker-arm proxy handle (cheap `Rc` clone) for components that
    /// need to dispatch their own writes outside the standard
    /// `dispatch_write` flow. Returns `None` on Direct or on builds
    /// without the `worker` feature.
    ///
    /// **Prefer [`writer_handle`](Self::writer_handle) for app-tier
    /// writers** — it bundles both arms into a single cloneable type,
    /// eliminating the per-writer dual-arm boilerplate. This raw
    /// proxy handle is kept for cases that genuinely need the
    /// underlying proxy (e.g. for non-tree operations).
    #[cfg(target_arch = "wasm32")]
    pub fn worker_proxy_handle(
        &self,
    ) -> Option<std::rc::Rc<entity_wasm_worker_proxy::WorkerProxy<entity_wasm_worker_proxy::WebTransport>>> {
        match self {
            Sdk::Direct(_) => None,
            Sdk::Worker(w) => Some(w.proxy_handle()),
        }
    }

    /// Cloneable, arm-agnostic write handle bound to the system peer.
    /// Use this for app-tier writers (event log, peer-registry signal,
    /// connections, etc.) that need to be moved into spawned futures.
    /// Returns `None` if no transport is wireable on this arm (Direct
    /// without a primary peer's `PeerShared`, or a non-worker non-wasm
    /// target).
    ///
    /// See [`crate::writer_handle::WriterHandle`].
    pub fn writer_handle(&self) -> Option<crate::writer_handle::WriterHandle> {
        match self {
            Sdk::Direct(pm) => {
                let pid = pm.primary_peer_id();
                pm.peer_shared(pid).map(crate::writer_handle::WriterHandle::Direct)
            }
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => Some(crate::writer_handle::WriterHandle::Worker {
                proxy: w.proxy_handle(),
                peer_id: w.primary_peer_id().to_string(),
            }),
        }
    }

    /// Like [`writer_handle`](Self::writer_handle) but bound to a SPECIFIC
    /// `peer_id` (which this SDK must host) rather than the SDK's primary.
    /// Use for **per-peer** writers — a write whose tree path is keyed to a
    /// non-system peer (e.g. the derived site-index at `/{me}/app/...`). The
    /// arm-split footgun is that the primary-bound `writer_handle` lands the
    /// write in the PRIMARY SDK's store; on the Worker arm a backend peer has
    /// its OWN store, so a `/{me}/...`-keyed read then misses what the primary
    /// store holds (the "No sites yet" divergence).
    fn writer_handle_for(&self, peer_id: &str) -> Option<crate::writer_handle::WriterHandle> {
        match self {
            Sdk::Direct(pm) => pm
                .peer_shared(peer_id)
                .map(crate::writer_handle::WriterHandle::Direct),
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => Some(crate::writer_handle::WriterHandle::Worker {
                proxy: w.proxy_handle(),
                peer_id: peer_id.to_string(),
            }),
        }
    }

    /// L1 execute, branched. Returns a `'static` future so callers can
    /// move it into `spawn_local` / `tokio::spawn` without holding a
    /// borrow on `Peers`. The `Send` bound is added only on non-WASM
    /// targets — native `tokio::spawn` requires it; WASM `spawn_local`
    /// does not and the Worker arm's `Rc`-backed proxy isn't Send.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn execute(
        &self,
        peer_id: &str,
        handler_uri: String,
        operation: String,
        params: Entity,
        opts: entity_handler::ExecuteOptions,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<entity_handler::HandlerResult, String>> + Send>>
    {
        match self {
            Sdk::Direct(pm) => {
                let fut = pm.sdk().execute(peer_id, handler_uri, operation, params, opts);
                Box::pin(async move { fut.await.map_err(|e| e.to_string()) })
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn execute(
        &self,
        peer_id: &str,
        handler_uri: String,
        operation: String,
        params: Entity,
        opts: entity_handler::ExecuteOptions,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<entity_handler::HandlerResult, String>>>>
    {
        match self {
            // §4.1b: flat EntitySDK op resolves the peer + owns its
            // future internally (SdkError::UnknownPeer on miss). The
            // hand-rolled peer_context lookup + None-bridge is gone;
            // only the SdkError→String map remains (correctly ours).
            Sdk::Direct(pm) => {
                let fut = pm.sdk().execute(peer_id, handler_uri, operation, params, opts);
                Box::pin(async move { fut.await.map_err(|e| e.to_string()) })
            }
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => Box::pin(w.execute(
                peer_id.to_string(),
                handler_uri,
                operation,
                params,
                opts,
            )),
        }
    }

    /// L1 query, branched. Worker arm returns `QueryResults` with
    /// `total = 0`, `cursor = None`, and empty `entity_type` per match
    /// until the wire protocol carries those fields. Documented in
    /// `WORKER-MODE-LIVING-DOC.md` §3.5.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn query(
        &self,
        peer_id: &str,
        expression: Entity,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<entity_sdk::QueryResults, String>> + Send>>
    {
        match self {
            Sdk::Direct(pm) => {
                let fut = pm.sdk().query(peer_id, expression);
                Box::pin(async move { fut.await.map_err(|e| e.to_string()) })
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn query(
        &self,
        peer_id: &str,
        expression: Entity,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<entity_sdk::QueryResults, String>>>>
    {
        match self {
            Sdk::Direct(pm) => {
                let fut = pm.sdk().query(peer_id, expression);
                Box::pin(async move { fut.await.map_err(|e| e.to_string()) })
            }
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => Box::pin(w.query(peer_id.to_string(), expression)),
        }
    }

    /// L1 count, branched. Full fidelity in both arms.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn count(
        &self,
        peer_id: &str,
        expression: Entity,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<u64, String>> + Send>> {
        match self {
            Sdk::Direct(pm) => {
                let fut = pm.sdk().count(peer_id, expression);
                Box::pin(async move { fut.await.map_err(|e| e.to_string()) })
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn count(
        &self,
        peer_id: &str,
        expression: Entity,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<u64, String>>>> {
        match self {
            Sdk::Direct(pm) => {
                let fut = pm.sdk().count(peer_id, expression);
                Box::pin(async move { fut.await.map_err(|e| e.to_string()) })
            }
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => Box::pin(w.count(peer_id.to_string(), expression)),
        }
    }

    /// On-demand async tree get, branched. Direct resolves synchronously
    /// from the store (wrapped in a ready future); Worker issues a `Get`
    /// round-trip. `Ok(None)` for a missing path. Unlike the sync mirror
    /// read `get_entity`, this works for paths the caller never
    /// subscribed to on the Worker arm — needed by `compute show`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn get_entity_async(
        &self,
        peer_id: &str,
        path: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<Entity>, String>> + Send>>
    {
        match self {
            Sdk::Direct(pm) => {
                let e = pm.peer_context(peer_id).and_then(|c| c.store().get(path));
                Box::pin(async move { Ok(e) })
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn get_entity_async(
        &self,
        peer_id: &str,
        path: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<Entity>, String>>>> {
        match self {
            Sdk::Direct(pm) => {
                let e = pm.peer_context(peer_id).and_then(|c| c.store().get(path));
                Box::pin(async move { Ok(e) })
            }
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => Box::pin(w.get_entity_async(peer_id.to_string(), path.to_string())),
        }
    }

    /// Authoritative async prefix-scan, branched. Direct resolves
    /// synchronously from the store (the full in-process tree, wrapped in a
    /// ready future); Worker issues a `List` round-trip. Unlike the sync
    /// `tree_listing` mirror read, this enumerates prefixes the caller never
    /// subscribed to on the Worker arm — needed by the boot-time roster read
    /// and the reconcile gate (the sync mirror returns silently empty there).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn tree_listing_async(
        &self,
        peer_id: &str,
        prefix: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<LocationEntry>, String>> + Send>>
    {
        match self {
            Sdk::Direct(pm) => {
                let v = pm.tree_listing(peer_id, prefix);
                Box::pin(async move { Ok(v) })
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn tree_listing_async(
        &self,
        peer_id: &str,
        prefix: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<LocationEntry>, String>>>> {
        match self {
            Sdk::Direct(pm) => {
                let v = pm.tree_listing(peer_id, prefix);
                Box::pin(async move { Ok(v) })
            }
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => Box::pin(w.list_async(peer_id.to_string(), prefix.to_string())),
        }
    }

    /// Async discover_handlers, branched. Direct mirrors the sync
    /// method; Worker rounds through the proxy. UIs that depend on
    /// handler-listing info should call this on init/peer-change and
    /// cache the result.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn discover_handlers_async(
        &self,
        peer_id: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<entity_sdk::HandlerInfo>, String>> + Send>>
    {
        match self {
            Sdk::Direct(pm) => {
                // §4.1b: flat op folds unknown-peer into
                // SdkError::UnknownPeer (consistent with §4.4) rather
                // than whatever the bare PeerManager call did on miss.
                let fut = pm.sdk().discover_handlers(peer_id);
                Box::pin(async move { fut.await.map_err(|e| e.to_string()) })
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn discover_handlers_async(
        &self,
        peer_id: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<entity_sdk::HandlerInfo>, String>>>>
    {
        match self {
            Sdk::Direct(pm) => {
                // §4.1b: flat op folds unknown-peer into
                // SdkError::UnknownPeer (consistent with §4.4) rather
                // than whatever the bare PeerManager call did on miss.
                let fut = pm.sdk().discover_handlers(peer_id);
                Box::pin(async move { fut.await.map_err(|e| e.to_string()) })
            }
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => Box::pin(w.discover_handlers(peer_id.to_string())),
        }
    }

    /// List every known peer id (local + backend). Owned strings so
    /// callers don't hold borrows across the worker boundary.
    pub fn peer_ids(&self) -> Vec<String> {
        match self {
            Sdk::Direct(pm) => pm.sdk().peer_ids().into_iter().map(String::from).collect(),
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => w.peer_ids(),
        }
    }

    /// Return the cached metadata for `peer_id` (label, listen addresses,
    /// etc.). Owned clone — see [`peer_ids`] re: the worker boundary.
    pub fn peer_metadata(&self, peer_id: &str) -> Option<entity_sdk::PeerMetadata> {
        match self {
            Sdk::Direct(pm) => pm.sdk().peer_metadata(peer_id).cloned(),
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => w.peer_metadata(peer_id),
        }
    }

    /// True if this peer has a local `PeerContext` — i.e., it lives in
    /// this process / worker rather than being a remote we connect to.
    /// Worker arm: true for peers loaded via `InitParams`.
    pub fn has_peer_context(&self, peer_id: &str) -> bool {
        match self {
            Sdk::Direct(pm) => pm.sdk().has_peer_context(peer_id),
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => w.has_local_peer(peer_id),
        }
    }

}

impl Sdk {
    /// Fresh Direct Sdk wrapping a new PeerManager. Helper used by
    /// the `Peers::new_direct()` constructor.
    pub(crate) fn new_direct_sdk() -> Self {
        Sdk::Direct(entity_sdk::PeerManager::new())
    }

    /// Direct Sdk whose primary peer uses a caller-supplied keypair —
    /// a **stable, reproducible** primary peer-id. Helper for
    /// [`Peers::new_direct_with_keypair`].
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn new_direct_sdk_with_keypair(keypair: entity_crypto::Keypair) -> Self {
        Sdk::Direct(entity_sdk::PeerManager::with_keypair(keypair))
    }
}

// =====================================================================
// Peers — public multi-SDK router (Stage 2A).
//
// Holds one or more `Sdk` instances and a `peer_id → sdks[idx]` route
// map. External callers see this as the single entry point; the `Sdk`
// enum and its match-on-variant logic are internal to this module.
//
// Stage 2A invariant: `sdks.len() == 1`. Every method routes to slot 0.
// Stage 2B will lazy-spawn additional SDKs as peers with different
// host configs are created, and `peer_routes` will start to carry
// per-peer indices.
// =====================================================================

/// A per-peer op was attempted against a peer that has no route in
/// `peer_routes`. Returned by [`Peers::sdk_for`] instead of the old
/// silent slot-0 fallback — see the §4.4 hardening in
/// the peer-SDK-arm architecture review. Making the miss
/// a typed value (not a silent default-to-primary) is what stops the
/// "default-to-primary" bug class at the type level.
#[derive(Debug, Clone)]
pub struct UnknownPeer(pub String);

impl std::fmt::Display for UnknownPeer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unrouted peer: {}", self.0)
    }
}

/// Multi-SDK router. Holds the SDK instances that host peers and
/// routes per-peer operations to the right one.
pub struct Peers {
    sdks: Vec<Sdk>,
    /// `peer_id → sdks[idx]`. A peer_id with no entry is **unrouted**
    /// — [`sdk_for`] returns `Err(UnknownPeer)` rather than silently
    /// falling back to slot 0 (the primary). Per-peer wrappers turn
    /// that into the semantically-correct miss (empty read / loud
    /// dropped write / `Err` future), never a silent primary hit.
    peer_routes: HashMap<String, usize>,
    /// The "primary" peer's id — the system-scoped default. Always
    /// present in `peer_routes` after construction (invariant relied
    /// on by `primary_sdk`/`primary_sdk_mut`).
    primary_peer_id: String,
}

impl Peers {
    // ---- Construction -----------------------------------------------

    /// Direct-mode constructor. Builds a fresh PeerManager (auto-
    /// generated primary keypair) and wraps it as the single SDK.
    pub fn new_direct() -> Self {
        let sdk = Sdk::new_direct_sdk();
        Self::new_direct_with_sdk(sdk)
    }

    /// Direct-mode constructor that builds the primary `PeerManager`
    /// with a caller-supplied transport `Connector`. Used by native
    /// integration tests (multi-peer sync over `MemoryConnector`) and
    /// by future in-process multi-peer scenarios. The connector
    /// applies to every peer created through this manager.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new_direct_with_connector(
        connector: std::sync::Arc<dyn entity_peer::transport::Connector>,
    ) -> Self {
        let pm = entity_sdk::PeerManager::with_connector(connector);
        Self::new_direct_with_sdk(Sdk::Direct(pm))
    }

    /// Direct-mode constructor whose primary peer uses a **caller-supplied
    /// keypair** instead of a freshly generated one — so the primary peer-id
    /// is stable across runs. Used by the headless `content_site::publish`
    /// path so a content publisher's static permalinks don't shift every run
    /// (the publisher peer-id is the address). Native-only (publish is native).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new_direct_with_keypair(keypair: entity_crypto::Keypair) -> Self {
        Self::new_direct_with_sdk(Sdk::new_direct_sdk_with_keypair(keypair))
    }

    fn new_direct_with_sdk(sdk: Sdk) -> Self {
        let primary_peer_id = sdk.primary_peer_id().to_string();
        let mut peers = Self {
            sdks: vec![sdk],
            peer_routes: HashMap::new(),
            primary_peer_id,
        };
        peers.refresh_routes_for_sdk(0);
        peers
    }

    /// Direct-mode constructor whose primary peer is backed by a durable,
    /// **main-thread IndexedDB** store, instead of the ephemeral in-memory
    /// store of [`new_direct`](Self::new_direct). Async because IDB open +
    /// the initial replay are request-based.
    ///
    /// `keypair` MUST be a stable seed-derived identity (durability depends
    /// on the same peer-id mapping to the same IDB database across reloads);
    /// `db_name` is the IndexedDB database name. This is the durable Direct
    /// arm — the building block the persistent system peer reuses (per the
    /// persistent-system-peer + durability-substrate design §4.2 row 2 /
    /// §5 Shape 1).
    #[cfg(target_arch = "wasm32")]
    pub async fn new_direct_idb(
        keypair: entity_crypto::Keypair,
        db_name: &str,
    ) -> Result<Self, entity_sdk::SdkError> {
        let pm = entity_sdk::PeerManager::with_keypair_idb(keypair, db_name).await?;
        Ok(Self::new_direct_with_sdk(Sdk::Direct(pm)))
    }

    /// Worker-mode constructor. Wraps an already-spawned WorkerPeerStore
    /// as the single SDK. The store's primary peer becomes Peers' primary.
    #[cfg(target_arch = "wasm32")]
    pub fn new_worker(store: WorkerPeerStore) -> Self {
        let primary_peer_id = store.primary_peer_id().to_string();
        let mut peers = Self {
            sdks: vec![Sdk::Worker(store)],
            peer_routes: HashMap::new(),
            primary_peer_id,
        };
        peers.refresh_routes_for_sdk(0);
        peers
    }

    // ---- Routing internals ------------------------------------------

    /// Look up the Sdk hosting `peer_id`.
    ///
    /// `peer_routes` is a **fast-path cache, not the authority**. On a
    /// cache miss we scan the SDKs for the one that actually hosts
    /// `peer_id` (covers the transient window between a peer being
    /// created/attached and `refresh_routes_for_sdk` running — e.g. a
    /// worker-created peer whose route is registered asynchronously).
    /// **Still no silent default-to-primary:** a peer that no SDK
    /// hosts is `Err(UnknownPeer)`, never slot 0 (§4.4 invariant). The
    /// scan only runs on a miss (rare) and is bounded by the small
    /// peer count.
    fn sdk_for(&self, peer_id: &str) -> Result<&Sdk, UnknownPeer> {
        if let Some(idx) = self.peer_routes.get(peer_id).copied() {
            return Ok(&self.sdks[idx]);
        }
        self.sdks
            .iter()
            .find(|s| s.peer_ids().iter().any(|p| p == peer_id))
            .ok_or_else(|| UnknownPeer(peer_id.to_string()))
    }

    fn sdk_for_mut(&mut self, peer_id: &str) -> Result<&mut Sdk, UnknownPeer> {
        if let Some(idx) = self.peer_routes.get(peer_id).copied() {
            return Ok(&mut self.sdks[idx]);
        }
        // Cache miss → authoritative scan; self-heal the route so
        // subsequent lookups hit the fast path.
        match self
            .sdks
            .iter()
            .position(|s| s.peer_ids().iter().any(|p| p == peer_id))
        {
            Some(idx) => {
                self.peer_routes.insert(peer_id.to_string(), idx);
                Ok(&mut self.sdks[idx])
            }
            None => Err(UnknownPeer(peer_id.to_string())),
        }
    }

    /// The primary peer's SDK — the one app-tier writers and bootstrap
    /// code target. The primary is inserted into `peer_routes` at
    /// construction and never removed, so resolution is infallible by
    /// invariant (panics only if that invariant is violated, which
    /// would be a construction bug, not a per-peer-routing bug).
    fn primary_sdk(&self) -> &Sdk {
        self.sdk_for(&self.primary_peer_id)
            .expect("primary peer must always be routed (construction invariant)")
    }

    fn primary_sdk_mut(&mut self) -> &mut Sdk {
        let pid = self.primary_peer_id.clone();
        self.sdk_for_mut(&pid)
            .expect("primary peer must always be routed (construction invariant)")
    }

    /// Repopulate route entries from an SDK's current peer-id list.
    /// Idempotent — entries that already point to `idx` stay; entries
    /// for peers no longer in the SDK are NOT removed (delete_peer
    /// handles removal). Call after `create_new_peer`, `load_persisted`,
    /// `register_backend_peer`, or any operation that grows the SDK's
    /// peer set.
    fn refresh_routes_for_sdk(&mut self, idx: usize) {
        let ids = self.sdks[idx].peer_ids();
        for pid in ids {
            self.peer_routes.insert(pid, idx);
        }
    }

    /// Clear and rebuild the entire `peer_routes` cache from the live
    /// `sdks` list. `peer_routes` maps `peer_id → sdks` index, so any
    /// structural change to `sdks` (notably [`remove_sdk`], which shifts
    /// every index after the removed slot) invalidates the cache. This
    /// re-derives it from scratch. Cheap (one pass over a handful of
    /// SDKs); `sdk_for`'s authoritative fallback scan makes a transient
    /// stale entry self-healing anyway, but a full rebuild keeps the
    /// fast path correct after teardown.
    #[cfg(target_arch = "wasm32")]
    fn rebuild_routes(&mut self) {
        self.peer_routes.clear();
        for idx in 0..self.sdks.len() {
            self.refresh_routes_for_sdk(idx);
        }
    }

    /// Tear down a non-primary (backend) SDK entirely: drop it from
    /// `sdks` and rebuild `peer_routes`. This is the correct teardown
    /// for a backend worker peer on delete — a backend peer is the
    /// **sole primary of its own dedicated Worker SDK**, so deleting "a
    /// peer within it" is refused by the worker (`Ok(false)`), and the
    /// only way to remove it is to drop the whole SDK.
    ///
    /// ⚠️ **Index-shift:** `Vec::remove(k)` shifts every index `> k`
    /// down by one, which would corrupt `peer_routes` — hence the
    /// `rebuild_routes` call. The boot/primary SDK is slot 0 and must
    /// never be removed here (a backend SDK is always slot ≥ 1); the
    /// debug-assert guards the invariant.
    ///
    /// Note: there is no upstream `worker.terminate()` (see
    /// `opfs_cleanup.rs`), so dropping the `Sdk::Worker` unroots the
    /// proxy/Worker rather than killing the thread. The OS worker is no
    /// longer routable and the Peers row vanishes immediately; any held
    /// OPFS sync handles are released at the next boot via the tombstone
    /// drain (`mark_opfs_for_cleanup` + `opfs_cleanup::run_at_boot`).
    #[cfg(target_arch = "wasm32")]
    fn remove_sdk(&mut self, idx: usize) {
        debug_assert!(
            idx != 0,
            "remove_sdk must never drop the boot/primary SDK (slot 0)"
        );
        if idx == 0 || idx >= self.sdks.len() {
            return;
        }
        self.sdks.remove(idx);
        self.rebuild_routes();
    }

    // ---- Primary identity / id queries ------------------------------

    pub fn primary_peer_id(&self) -> &str {
        &self.primary_peer_id
    }

    /// The **system peer** — the single peer that owns all global, app-wide
    /// *control-plane* state: the session/startup config + deployment posture,
    /// AND the diagnostics/roster surface (event log, connection log, listener
    /// state, peer registry, and the System-scoped windows that read them).
    /// This ownership was ratified (F-SYS-1: the diagnostics/roster
    /// surface is **system-owned**, not user-owned). Today this is the
    /// boot/primary peer, but "system peer" is a distinct, fundamental concept
    /// (the future deployment-profile split exposes a *user* peer while the
    /// system peer hosts policy — `project_deployment_profiles_mode_model`).
    /// Every call site that semantically means "the system peer" routes through
    /// here, NOT `primary_peer_id`, so that split becomes a pure change at this
    /// one seam rather than a sweep (handoff §4.5, D5; the F-SYS-1 sweep
    /// completed the adoption across the app-tier writers + reader windows).
    pub fn system_peer_id(&self) -> &str {
        &self.primary_peer_id
    }

    /// Alias for `primary_peer_id` — kept for the migration where call
    /// sites previously did `sdk.default_peer_id()`.
    pub fn default_peer_id(&self) -> &str {
        &self.primary_peer_id
    }

    /// Every peer-id known to any hosted SDK (local + backend),
    /// deduplicated. Order is sdks-first, within-sdk order.
    pub fn peer_ids(&self) -> Vec<String> {
        let mut seen: HashMap<String, ()> = HashMap::new();
        let mut out = Vec::new();
        for sdk in &self.sdks {
            for pid in sdk.peer_ids() {
                if seen.insert(pid.clone(), ()).is_none() {
                    out.push(pid);
                }
            }
        }
        out
    }

    pub fn peer_metadata(&self, peer_id: &str) -> Option<entity_sdk::PeerMetadata> {
        // Miss → None (an unrouted peer has no metadata), never the
        // primary's metadata.
        self.sdk_for(peer_id).ok()?.peer_metadata(peer_id)
    }

    pub fn has_peer_context(&self, peer_id: &str) -> bool {
        self.sdk_for(peer_id)
            .map(|s| s.has_peer_context(peer_id))
            .unwrap_or(false)
    }

    /// True when `peer_id` is hosted in a dedicated SDK separate from
    /// the boot/primary SDK — i.e. a Backend (Memory/OPFS) worker
    /// peer. Frontend and system peers share the primary peer's SDK;
    /// each backend peer gets its own attached worker SDK
    /// (`attach_worker_sdk`, slot >= 1).
    ///
    /// This is the correct frontend-vs-backend discriminator: a
    /// backend worker peer DOES have a `PeerContext` (in its own SDK),
    /// so `has_peer_context` alone misclassifies it as a frontend
    /// peer. Direct-arm (Tauri) backend peers are registered into the
    /// primary SDK as metadata-only, so they route to the primary idx
    /// and fall through to the existing no-context → Remote path.
    pub fn is_backend_hosted(&self, peer_id: &str) -> bool {
        match (
            self.host_sdk_index(peer_id),
            self.host_sdk_index(&self.primary_peer_id),
        ) {
            (Some(idx), Some(primary_idx)) => idx != primary_idx,
            _ => false,
        }
    }

    /// Index of the SDK hosting `peer_id`. Mirrors `sdk_for`'s
    /// cache-then-authoritative-scan: the `peer_routes` cache can lag
    /// (a worker-attached peer whose route insert raced the mirror),
    /// so a miss falls back to scanning each SDK's live peer set
    /// rather than reporting "not backend" off a stale cache.
    fn host_sdk_index(&self, peer_id: &str) -> Option<usize> {
        if let Some(idx) = self.peer_routes.get(peer_id).copied() {
            return Some(idx);
        }
        self.sdks
            .iter()
            .position(|s| s.peer_ids().iter().any(|p| p == peer_id))
    }

    // ---- Read surface (L0 cache hits, sync) -------------------------
    // Route miss → the operation's empty value (None / [] / 0). NEVER
    // the primary SDK's data — silently reading another peer's tree is
    // the bug class §4.4 removes.

    pub fn get_entity(&self, peer_id: &str, path: &str) -> Option<Entity> {
        self.sdk_for(peer_id).ok()?.get_entity(peer_id, path)
    }

    pub fn tree_listing(&self, peer_id: &str, prefix: &str) -> Vec<LocationEntry> {
        self.sdk_for(peer_id)
            .map(|s| s.tree_listing(peer_id, prefix))
            .unwrap_or_default()
    }

    pub fn entity_count(&self, peer_id: &str) -> usize {
        self.sdk_for(peer_id)
            .map(|s| s.entity_count(peer_id))
            .unwrap_or(0)
    }

    pub fn path_count(&self, peer_id: &str) -> usize {
        self.sdk_for(peer_id)
            .map(|s| s.path_count(peer_id))
            .unwrap_or(0)
    }

    // ---- Write surface ----------------------------------------------

    pub fn dispatch_write(&self, peer_id: &str, path: impl Into<String>, entity: Entity) {
        match self.sdk_for(peer_id) {
            Ok(sdk) => sdk.dispatch_write(peer_id, path, entity),
            // Drop + loud, NEVER a silent write to the primary's tree.
            Err(e) => tracing::error!(error = %e, "dispatch_write dropped — unrouted peer"),
        }
    }

    /// Arm-aware **seed write** — the one blessed home for the
    /// "write synchronously on Direct, route on Worker" pattern that the
    /// site-mode / content-site / origin seeds all need. On the Direct
    /// arm it writes via L0 `store().put` so the value is readable in the
    /// **same render pass** (sync `#[test]`s + same-frame readback depend
    /// on this); on the Worker arm (or any unrouted peer) it falls back
    /// to async `dispatch_write` and lets the cache mirror catch up. The
    /// arm is decided from the **target** peer's owning SDK, never the
    /// primary.
    ///
    /// Callers MUST use this instead of open-coding the dance with
    /// `direct_peer_context` — that reaches through the Direct-only L0
    /// escape hatch (tripping its break-glass warning + the
    /// `tests/escape_hatch_budget.rs` budget) when this router method is
    /// the correct seam. The internal arm probe here is the non-warning
    /// `Sdk`-level accessor precisely because routing-both-arms is its
    /// job, not a leak.
    pub fn seed_write(&self, peer_id: &str, path: impl Into<String>, entity: Entity) {
        let path = path.into();
        match self
            .sdk_for(peer_id)
            .ok()
            .and_then(|sdk| sdk.direct_peer_context(peer_id).ok())
        {
            Some(ctx) => {
                ctx.store().put(&path, entity).ok();
            }
            None => self.dispatch_write(peer_id, path, entity),
        }
    }

    pub fn dispatch_remove(&self, peer_id: &str, path: impl Into<String>) {
        match self.sdk_for(peer_id) {
            Ok(sdk) => sdk.dispatch_remove(peer_id, path),
            Err(e) => tracing::error!(error = %e, "dispatch_remove dropped — unrouted peer"),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn put_and_wait(
        &self,
        peer_id: &str,
        path: impl Into<String>,
        entity: Entity,
        timeout_ms: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>> {
        match self.sdk_for(peer_id) {
            Ok(sdk) => sdk.put_and_wait(peer_id, path, entity, timeout_ms),
            Err(e) => Box::pin(async move { Err(e.to_string()) }),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn put_and_wait(
        &self,
        peer_id: &str,
        path: impl Into<String>,
        entity: Entity,
        timeout_ms: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>> {
        match self.sdk_for(peer_id) {
            Ok(sdk) => sdk.put_and_wait(peer_id, path, entity, timeout_ms),
            Err(e) => Box::pin(async move { Err(e.to_string()) }),
        }
    }

    /// **Durable-authoritative seed** — the blessed primitive of the owned
    /// boot-load step (boot-config-surfaces reframe
    /// §2.4). Writes `default` at `path` **only if it is absent in the
    /// durable tree**, and *awaits* the result. `Ok(true)` = seeded,
    /// `Ok(false)` = a value was already present (left untouched), `Err`
    /// on an unrouted peer or transport failure.
    ///
    /// This is the clobber-safe replacement for the "read via `get_entity`
    /// (cache mirror) then `seed_write`" pattern: on a **warm Worker boot**
    /// the cache mirror is cold, so the old pattern read `None` for a
    /// persisted value and re-seeded the default over it. `put_if_absent`
    /// consults the *durable* store — sync L0 on the Direct arm, an L1
    /// `proxy.get` round-trip on the Worker arm — so absence is
    /// authoritative and persisted state is never clobbered.
    ///
    /// Arm + ordering live in [`Sdk::put_if_absent`] /
    /// [`WorkerPeerStore::put_if_absent`]; the arm is decided from the
    /// **target** peer's owning SDK, never the primary.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn put_if_absent(
        &self,
        peer_id: &str,
        path: impl Into<String>,
        default: Entity,
        timeout_ms: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send>> {
        match self.sdk_for(peer_id) {
            Ok(sdk) => sdk.put_if_absent(peer_id, path, default, timeout_ms),
            Err(e) => Box::pin(async move { Err(e.to_string()) }),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn put_if_absent(
        &self,
        peer_id: &str,
        path: impl Into<String>,
        default: Entity,
        timeout_ms: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>>>> {
        match self.sdk_for(peer_id) {
            Ok(sdk) => sdk.put_if_absent(peer_id, path, default, timeout_ms),
            Err(e) => Box::pin(async move { Err(e.to_string()) }),
        }
    }

    // ---- Subscriptions ----------------------------------------------

    pub fn watch_prefix(
        &self,
        watch: &mut WindowWatch,
        peer_id: &str,
        prefix: impl Into<String>,
    ) {
        match self.sdk_for(peer_id) {
            Ok(sdk) => sdk.watch_prefix(watch, peer_id, prefix),
            // No-op + loud: subscribing to the primary's tree for an
            // unrouted peer would silently feed a window the wrong
            // data (the subscribe-scoping bug class).
            Err(e) => tracing::error!(error = %e, "watch_prefix skipped — unrouted peer"),
        }
    }

    /// Per-event subscription with seed. See [`Sdk::observe_with_events`]
    /// for the semantics.
    pub fn observe_with_events<F>(
        &self,
        watch: &mut WindowWatch,
        peer_id: &str,
        prefix: impl Into<String>,
        on_event: F,
    ) where
        F: Fn(ChangeOp) + Send + Sync + 'static,
    {
        match self.sdk_for(peer_id) {
            Ok(sdk) => sdk.observe_with_events(watch, peer_id, prefix, on_event),
            Err(e) => {
                tracing::error!(error = %e, "observe_with_events skipped — unrouted peer")
            }
        }
    }

    // ---- L1 dispatch surface ----------------------------------------

    #[cfg(not(target_arch = "wasm32"))]
    pub fn execute(
        &self,
        peer_id: &str,
        handler_uri: String,
        operation: String,
        params: Entity,
        opts: entity_handler::ExecuteOptions,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<entity_handler::HandlerResult, String>> + Send>>
    {
        match self.sdk_for(peer_id) {
            Ok(sdk) => sdk.execute(peer_id, handler_uri, operation, params, opts),
            Err(e) => Box::pin(async move { Err(e.to_string()) }),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn execute(
        &self,
        peer_id: &str,
        handler_uri: String,
        operation: String,
        params: Entity,
        opts: entity_handler::ExecuteOptions,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<entity_handler::HandlerResult, String>>>>
    {
        match self.sdk_for(peer_id) {
            Ok(sdk) => sdk.execute(peer_id, handler_uri, operation, params, opts),
            Err(e) => Box::pin(async move { Err(e.to_string()) }),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn query(
        &self,
        peer_id: &str,
        expression: Entity,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<entity_sdk::QueryResults, String>> + Send>>
    {
        match self.sdk_for(peer_id) {
            Ok(sdk) => sdk.query(peer_id, expression),
            Err(e) => Box::pin(async move { Err(e.to_string()) }),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn query(
        &self,
        peer_id: &str,
        expression: Entity,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<entity_sdk::QueryResults, String>>>>
    {
        match self.sdk_for(peer_id) {
            Ok(sdk) => sdk.query(peer_id, expression),
            Err(e) => Box::pin(async move { Err(e.to_string()) }),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn count(
        &self,
        peer_id: &str,
        expression: Entity,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<u64, String>> + Send>> {
        match self.sdk_for(peer_id) {
            Ok(sdk) => sdk.count(peer_id, expression),
            Err(e) => Box::pin(async move { Err(e.to_string()) }),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn count(
        &self,
        peer_id: &str,
        expression: Entity,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<u64, String>>>> {
        match self.sdk_for(peer_id) {
            Ok(sdk) => sdk.count(peer_id, expression),
            Err(e) => Box::pin(async move { Err(e.to_string()) }),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn discover_handlers_async(
        &self,
        peer_id: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<entity_sdk::HandlerInfo>, String>> + Send>>
    {
        match self.sdk_for(peer_id) {
            Ok(sdk) => sdk.discover_handlers_async(peer_id),
            Err(e) => Box::pin(async move { Err(e.to_string()) }),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn discover_handlers_async(
        &self,
        peer_id: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<entity_sdk::HandlerInfo>, String>>>>
    {
        match self.sdk_for(peer_id) {
            Ok(sdk) => sdk.discover_handlers_async(peer_id),
            Err(e) => Box::pin(async move { Err(e.to_string()) }),
        }
    }

    /// On-demand async tree get, routed to the peer's owning SDK. Works
    /// on both arms (Direct = sync store read wrapped ready; Worker = `Get`
    /// round-trip). Used by `compute show`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn get_entity_async(
        &self,
        peer_id: &str,
        path: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<Entity>, String>> + Send>>
    {
        match self.sdk_for(peer_id) {
            Ok(sdk) => sdk.get_entity_async(peer_id, path),
            Err(e) => Box::pin(async move { Err(e.to_string()) }),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn get_entity_async(
        &self,
        peer_id: &str,
        path: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<Entity>, String>>>> {
        match self.sdk_for(peer_id) {
            Ok(sdk) => sdk.get_entity_async(peer_id, path),
            Err(e) => Box::pin(async move { Err(e.to_string()) }),
        }
    }

    /// Authoritative async prefix-scan, routed to the peer's owning SDK.
    /// Works on both arms (Direct = sync store read wrapped ready; Worker =
    /// `List` round-trip). Use this — not the sync `tree_listing` — to read
    /// the roster at boot / in the reconcile gate, since the Worker sync
    /// mirror only sees subscribed prefixes (returns silently empty for the
    /// roster prefix no window watches).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn tree_listing_async(
        &self,
        peer_id: &str,
        prefix: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<LocationEntry>, String>> + Send>>
    {
        match self.sdk_for(peer_id) {
            Ok(sdk) => sdk.tree_listing_async(peer_id, prefix),
            Err(e) => Box::pin(async move { Err(e.to_string()) }),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn tree_listing_async(
        &self,
        peer_id: &str,
        prefix: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<LocationEntry>, String>>>> {
        match self.sdk_for(peer_id) {
            Ok(sdk) => sdk.tree_listing_async(peer_id, prefix),
            Err(e) => Box::pin(async move { Err(e.to_string()) }),
        }
    }

    // ---- Direct-arm escape hatches (route to peer's SDK) ----------
    // These return `None` (or an explicit `Option`) when the hosting
    // SDK is Worker — they expose Direct-only main-thread state that
    // has no Worker analogue. Callers that need cross-arm execution
    // go through the L1 router methods (`execute`, `query`, …).
    // §4.4 makes unrouted peers a typed error (never a silent primary
    // misroute). §L1 renamed these `direct_*` so every
    // reach-through for Direct-only main-thread state screams at the
    // call site, deleted the `_or_default` primary-fallback footgun,
    // and added a break-glass alarm to `direct_peer_context`.

    /// Direct-arm `PeerContext` lookup — **a Direct-only L0 escape
    /// hatch**, not a general accessor. Returns `Err(WorkerArm)` if the
    /// peer is Worker-hosted (the browser default!) or `Err(UnknownPeer)`
    /// if unrouted. On the `WorkerArm` branch it logs a break-glass
    /// warning with the call site, because a read/write through this
    /// hatch on a Worker peer is **silently dropped** — that is the
    /// `list_child_pages` empty-sidebar bug class. Cross-arm code MUST
    /// use the router (`get_entity` / `tree_listing` / `dispatch_write`
    /// / `put_and_wait`). Every legitimate caller is on the
    /// `tests/escape_hatch_budget.rs` allowlist.
    #[track_caller]
    pub fn direct_peer_context(
        &self,
        peer_id: &str,
    ) -> Result<&entity_sdk::PeerContext, DirectArmError> {
        let sdk = self
            .sdk_for(peer_id)
            .map_err(|_| DirectArmError::UnknownPeer)?;
        let result = sdk.direct_peer_context(peer_id);
        if matches!(result, Err(DirectArmError::WorkerArm)) {
            let loc = std::panic::Location::caller();
            tracing::warn!(
                peer_id = %peer_id,
                caller = %loc,
                "BREAK-GLASS: direct_peer_context() reached through on a Worker-arm \
                 peer — this L0 read/write is silently dropped on the browser \
                 default. Route via Peers::{{get_entity,tree_listing,dispatch_write,\
                 put_and_wait}} instead."
            );
        }
        result
    }

    pub fn direct_peer_shared(
        &self,
        peer_id: &str,
    ) -> Option<std::sync::Arc<entity_peer::PeerShared>> {
        self.sdk_for(peer_id).ok()?.direct_peer_shared(peer_id)
    }

    /// Test-only Direct L0 context for seeding a peer's tree directly.
    /// Tests always run on the Direct arm — this panics loudly if not,
    /// so it can never become a prod footgun (the reason
    /// `peer_context_or_default` was deleted). Not on the escape-hatch
    /// allowlist because it is `#[cfg(test)]` and unreachable in a
    /// shipped binary.
    #[cfg(test)]
    pub fn test_seed_ctx(&self, peer_id: &str) -> &entity_sdk::PeerContext {
        self.direct_peer_context(peer_id)
            .expect("test_seed_ctx: tests must run on the Direct arm with a routed peer")
    }

    #[cfg(feature = "native-ws")]
    pub fn peer(&self, peer_id: &str) -> Option<&entity_peer::Peer> {
        self.sdk_for(peer_id).ok()?.peer(peer_id)
    }

    #[cfg(test)]
    pub fn put_entity(&self, peer_id: &str, path: &str, entity: Entity) -> Option<entity_hash::Hash> {
        self.sdk_for(peer_id).ok()?.put_entity(peer_id, path, entity)
    }

    // ---- Primary-SDK ops (no peer_id) -------------------------------
    // Renamed `*_primary` (§4.2) so the primary binding is explicit at
    // the call site. `primary_as_direct().is_none()` self-documents as
    // "is the PRIMARY a worker" — distinct from a per-peer arm query,
    // the exact confusion that caused the delete bug.

    /// Returns `Some(&PeerManager)` when the **primary** SDK is Direct,
    /// `None` when it's Worker. NOT a per-peer arm query — use the
    /// per-peer methods for that.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub fn primary_as_direct(&self) -> Option<&entity_sdk::PeerManager> {
        self.primary_sdk().as_direct()
    }

    /// L0 SDK access on the **primary**. Panics if the primary's host
    /// is not Direct.
    pub fn sdk_primary(&self) -> &entity_sdk::EntitySDK {
        self.primary_sdk().sdk()
    }

    /// The durable-commit handle for the system peer's IndexedDB store, when
    /// one exists. `Some` only on the **Direct/IDB arm** (the main-thread
    /// system peer built with `.idb()`); `None` on the Worker arm (OPFS is
    /// flush-on-write — a write is durable the instant `put()` returns, so no
    /// checkpoint is needed) and on the ephemeral in-memory fallback (no IDB
    /// store). Identity/destructive ops (create/delete peer) `await
    /// .checkpoint()` on this before acking, so a roster write survives an
    /// immediate reload instead of riding the write-behind debounce — closing
    /// the BUG-A loss window on the write-behind arm. Returns an OWNED handle
    /// (`IdbCheckpoint` is `Clone`) so callers hold it across an `await`
    /// without borrowing `self`. Routes via `primary_as_direct` (NOT
    /// `direct_peer_context`) to avoid firing the Worker-arm break-glass warn.
    ///
    /// `wasm32`-only, matching `new_direct_idb`: the `entity-sdk`
    /// `wasm-idb-persist` feature is unconditionally enabled in Cargo.toml, so
    /// `entity_store::idb` is always present on wasm builds (there is no
    /// crate-level feature to gate on — gating on one would silently disable
    /// this).
    #[cfg(target_arch = "wasm32")]
    pub fn idb_checkpoint(&self) -> Option<entity_store::idb::IdbCheckpoint> {
        let pm = self.primary_as_direct()?;
        pm.peer_context(self.system_peer_id())?
            .idb_checkpoint()
            .cloned()
    }

    /// Mutable L0 SDK access on the **primary**. Panics on non-Direct.
    pub fn sdk_mut_primary(&mut self) -> &mut entity_sdk::EntitySDK {
        self.primary_sdk_mut().sdk_mut()
    }

    /// Bootstrap-time persisted-peer load on the **primary** SDK.
    /// Refreshes peer_routes so newly loaded peers point at it.
    /// Panics on Worker primary.
    pub fn load_persisted_primary(&mut self, persisted: Vec<entity_sdk::PersistedPeer>) {
        self.primary_sdk_mut().load_persisted(persisted);
        self.refresh_routes_for_sdk(0);
    }

    /// Create a new local peer on the primary SDK — **uniform across
    /// arms (§4.1b)**. Returns a detached future resolving to
    /// `(peer_id, keypair_seed, metadata)`. The caller persists the
    /// seed app-side. Direct path also seeds `PeerMetadata` on the
    /// local SDK so the resulting peer surfaces with the same shape
    /// as a worker-created one. Direct's future is already-ready;
    /// Worker awaits the proxy round-trip. Routes are refreshed
    /// eagerly for the Direct arm (the new peer-id is known
    /// synchronously); the Worker arm's mirror is maintained by the
    /// worker store.
    #[cfg(target_arch = "wasm32")]
    pub fn create_new_peer(
        &mut self,
        label: Option<String>,
    ) -> CreatePeerFuture<'static> {
        let direct_done: Option<(String, [u8; 32], entity_sdk::PeerMetadata)> =
            if let Sdk::Direct(pm) = self.primary_sdk_mut() {
                let (pid, seed) = pm.create_new_peer(label.clone());
                let metadata = entity_sdk::PeerMetadata {
                    label: label.clone(),
                    persisted: true,
                    ..entity_sdk::PeerMetadata::default()
                };
                pm.sdk_mut().set_metadata(&pid, metadata.clone());
                Some((pid, seed, metadata))
            } else {
                None
            };
        if let Some((pid, seed, metadata)) = direct_done {
            self.refresh_routes_for_sdk(0);
            // Direct arm: spin up the per-peer event-bridge here so the
            // caller doesn't need to know the arm. Worker arm doesn't
            // need this — the worker host owns the bridge inside the
            // worker context.
            if let Ok(ctx) = self.direct_peer_context(&pid) {
                wasm_bindgen_futures::spawn_local(ctx.event_bridge());
            }
            return Box::pin(std::future::ready(Ok((pid, seed, metadata))));
        }
        // Worker primary (the only other variant).
        if let Sdk::Worker(w) = self.primary_sdk() {
            return Box::pin(w.create_peer(label));
        }
        unreachable!("primary_sdk arm covered above")
    }

    /// Native variant — only the Direct arm exists off-wasm.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn create_new_peer(
        &mut self,
        label: Option<String>,
    ) -> CreatePeerFuture<'static> {
        let Sdk::Direct(pm) = self.primary_sdk_mut();
        let (pid, seed) = pm.create_new_peer(label.clone());
        let metadata = entity_sdk::PeerMetadata {
            label: label.clone(),
            persisted: true,
            ..entity_sdk::PeerMetadata::default()
        };
        pm.sdk_mut().set_metadata(&pid, metadata.clone());
        self.refresh_routes_for_sdk(0);
        Box::pin(std::future::ready(Ok((pid, seed, metadata))))
    }

    /// Update the metadata label for a peer. Arm-uniform: Direct
    /// updates the in-process SDK via `set_metadata`; Worker spawns
    /// the proxy round-trip and lets the mirror catch up on the next
    /// `peer_metadata` read.
    ///
    /// `peer_id` is resolved through the multi-SDK router (`sdk_for`),
    /// so labeling a backend peer routes to its worker SDK, not the
    /// primary. Returns `Err` if the peer isn't routed.
    pub fn set_peer_label(
        &mut self,
        peer_id: &str,
        label: Option<String>,
    ) -> Result<(), String> {
        // Read current metadata so we can preserve `persisted` /
        // `listen_addresses` while overwriting `label`. The full
        // metadata struct is what `set_metadata` accepts — there's no
        // field-level setter today.
        let current = self
            .peer_metadata(peer_id)
            .ok_or_else(|| format!("set_peer_label: unknown peer {}", peer_id))?;
        let new_metadata = entity_sdk::PeerMetadata {
            label,
            ..current
        };
        let sdk = self.sdk_for_mut(peer_id).map_err(|e| e.to_string())?;
        match sdk {
            Sdk::Direct(pm) => {
                pm.sdk_mut().set_metadata(peer_id, new_metadata);
                Ok(())
            }
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => {
                // Worker set_metadata returns a future; spawn it
                // fire-and-forget. The mirror update happens inside
                // the future's success branch (see peers_worker.rs).
                let fut = w.set_metadata(peer_id.to_string(), new_metadata);
                wasm_bindgen_futures::spawn_local(async move {
                    if let Err(e) = fut.await {
                        tracing::warn!(error = %e, "set_peer_label: worker set_metadata failed");
                    }
                });
                Ok(())
            }
        }
    }

    /// Delete a peer — **uniform across arms (§4.1a)**. Returns a
    /// detached future; the caller never chooses Direct vs Worker and
    /// there is no `peer_host_is_worker` band-aid. Direct resolves
    /// synchronously (its future is already-ready); Worker awaits the
    /// proxy round-trip. Route pruning (§4.5): Direct prunes on
    /// confirmed success; Worker prunes eagerly — a peer mid-delete
    /// must not stay routable, and a post-delete `UnknownPeer` is the
    /// correct outcome. Unrouted peer → `Err`, never a silent
    /// slot-0 delete (the original delete bug class).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn delete_peer(
        &mut self,
        peer_id: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send>> {
        let outcome = match self.sdk_for_mut(peer_id) {
            Ok(Sdk::Direct(pm)) => Ok(pm.delete_peer(peer_id)),
            Err(e) => Err(e.to_string()),
        };
        match outcome {
            Ok(deleted) => {
                if deleted {
                    self.peer_routes.remove(peer_id);
                }
                Box::pin(async move { Ok(deleted) })
            }
            Err(m) => Box::pin(async move { Err(m) }),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn delete_peer(
        &mut self,
        peer_id: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>>>> {
        // A backend (Memory/OPFS) peer is the SOLE PRIMARY of its own
        // dedicated Worker SDK (slot ≥ 1). Routing it to
        // `WorkerPeerStore::delete_peer` asks that worker to delete its
        // own primary, which it refuses — the err is swallowed to
        // `Ok(false)`, the registry never prunes, and the row sticks
        // forever (the "can't delete backend peers / 24
        // stuck" bug). The correct
        // teardown is to drop the whole dedicated SDK, not delete-a-peer
        // within it. Frontend peers share the primary SDK
        // (`is_backend_hosted == false`) and fall through to the normal
        // worker-delete path below, which deletes a non-primary peer
        // cleanly. The primary/boot peer is also `false` here (its SDK
        // index == the primary's), so it correctly stays undeletable.
        if self.is_backend_hosted(peer_id) {
            if let Some(idx) = self.host_sdk_index(peer_id) {
                self.remove_sdk(idx);
                tracing::info!(
                    peer_id = %peer_id,
                    sdk_idx = idx,
                    remaining_sdks = self.sdks.len(),
                    "delete: tore down backend peer's dedicated Worker SDK"
                );
            }
            return Box::pin(async move { Ok(true) });
        }
        enum Step {
            Direct(bool),
            Worker(std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>>>>),
            Unrouted(String),
        }
        let step = match self.sdk_for_mut(peer_id) {
            Err(e) => Step::Unrouted(e.to_string()),
            Ok(Sdk::Direct(pm)) => Step::Direct(pm.delete_peer(peer_id)),
            Ok(Sdk::Worker(w)) => Step::Worker(Box::pin(w.delete_peer(peer_id.to_string()))),
        };
        match step {
            Step::Unrouted(m) => Box::pin(async move { Err(m) }),
            Step::Direct(deleted) => {
                if deleted {
                    self.peer_routes.remove(peer_id);
                }
                Box::pin(async move { Ok(deleted) })
            }
            Step::Worker(fut) => {
                self.peer_routes.remove(peer_id);
                fut
            }
        }
    }

    /// Connect from `peer_id` to a remote peer at `address` —
    /// **uniform across arms (§4.1b)**. Returns a detached future
    /// resolving to the remote peer's id on success. Direct: clones
    /// shared state, runs transport-connect + handshake inline, then
    /// installs the remote into the shared pool. Worker: routes
    /// through `WorkerPeerStore::connect_peer`. The caller does not
    /// choose the arm. Unrouted `peer_id` → `Err`.
    #[cfg(target_arch = "wasm32")]
    pub fn connect_peer(&self, peer_id: &str, address: String) -> ConnectPeerFuture<'static> {
        let sdk = match self.sdk_for(peer_id) {
            Ok(s) => s,
            Err(e) => {
                let m = e.to_string();
                return Box::pin(async move { Err(m) });
            }
        };
        match sdk {
            Sdk::Direct(pm) => direct_connect_future(pm.peer_shared(peer_id), peer_id, address),
            Sdk::Worker(w) => Box::pin(w.connect_peer(peer_id.to_string(), address)),
        }
    }

    /// Native variant — Direct arm only.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn connect_peer(&self, peer_id: &str, address: String) -> ConnectPeerFuture<'static> {
        let sdk = match self.sdk_for(peer_id) {
            Ok(s) => s,
            Err(e) => {
                let m = e.to_string();
                return Box::pin(async move { Err(m) });
            }
        };
        let Sdk::Direct(pm) = sdk;
        direct_connect_future(pm.peer_shared(peer_id), peer_id, address)
    }

    /// Register a Tauri-backend peer's metadata on the **primary**
    /// Direct SDK. Panics on Worker primary (use the worker's
    /// `Request::RegisterBackendPeer` instead).
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub fn register_backend_peer_primary(
        &mut self,
        peer_id: String,
        label: Option<String>,
        listen_addresses: Vec<String>,
    ) -> bool {
        let r = self.primary_sdk_mut().register_backend_peer(peer_id, label, listen_addresses);
        if r {
            self.refresh_routes_for_sdk(0);
        }
        r
    }

    /// Cloneable write handle bound to the primary (system) peer.
    /// Same shape as today — Stage 2B may add a peer_id-parameterized
    /// variant for app-tier writers that target non-primary peers.
    pub fn writer_handle(&self) -> Option<crate::writer_handle::WriterHandle> {
        self.primary_sdk().writer_handle()
    }

    /// A cloneable writer bound to `peer_id`'s OWNING SDK — the per-peer
    /// counterpart to [`writer_handle`](Self::writer_handle). Routes through
    /// `sdk_for(peer_id)` so a write to a `/{peer_id}/...` path lands in the
    /// store the matching reader (`get_entity(peer_id, …)`) reads from, on
    /// BOTH arms. `None` on an unrouted peer (never silently the primary's
    /// store — that silent fall-through is the divergence this exists to kill).
    /// Use this for any write whose path is keyed to a non-system peer; reserve
    /// [`writer_handle`](Self::writer_handle) for genuinely system-owned state
    /// (event log, connections, peer registry).
    pub fn writer_handle_for(&self, peer_id: &str) -> Option<crate::writer_handle::WriterHandle> {
        self.sdk_for(peer_id).ok()?.writer_handle_for(peer_id)
    }

    /// Peer-scoped worker proxy — the proxy owning `peer_id`'s SDK, or
    /// `None` if that peer is Direct-backed or unknown. Use this for
    /// per-peer wire ops (e.g. connecting from a non-primary Peer window).
    /// There is deliberately **no** primary-defaulting `worker_proxy_handle`
    /// variant — a primary-only accessor would silently default cross-peer
    /// surfaces to the primary (AP2). Per-peer is the only shape; app-tier
    /// writers use [`writer_handle`](Self::writer_handle). See
    /// `Action::ConnectPeer`.
    #[cfg(target_arch = "wasm32")]
    pub fn worker_proxy_handle_for(
        &self,
        peer_id: &str,
    ) -> Option<std::rc::Rc<entity_wasm_worker_proxy::WorkerProxy<entity_wasm_worker_proxy::WebTransport>>> {
        self.sdk_for(peer_id).ok()?.worker_proxy_handle()
    }

    /// Register a per-peer inspect-sink callback. Both arms produce
    /// the same `entity_sdk::InspectFact` shape — Direct arm marshals
    /// in-process via the SDK demuxer; Worker arm receives the wire
    /// shape from the proxy and we convert at this boundary.
    ///
    /// Returns a handle whose drop detaches the sink synchronously
    /// (Direct) or fires a `SetInspectEnabled(false)` Request to the
    /// worker on last-drop (Worker).
    ///
    /// Per the upstream inspect-worker-arm design §7.
    pub fn install_inspect_sink<F>(
        &self,
        peer_id: &str,
        cb: F,
    ) -> Result<crate::inspect_router::PeersInspectSinkHandle, crate::inspect_router::InstallError>
    where
        F: Fn(&entity_sdk::InspectFact) + Send + Sync + 'static,
    {
        let sdk = self
            .sdk_for(peer_id)
            .map_err(|_| crate::inspect_router::InstallError::UnknownPeer)?;
        match sdk {
            Sdk::Direct(pm) => {
                let ctx = pm.peer_context(peer_id).ok_or(
                    crate::inspect_router::InstallError::UnknownPeer,
                )?;
                let handle = ctx
                    .install_inspect_sink(cb)
                    .map_err(crate::inspect_router::InstallError::Sdk)?;
                Ok(crate::inspect_router::PeersInspectSinkHandle::Direct(handle))
            }
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => {
                let proxy = w.proxy_handle();
                // Convert wire-shape → SDK-shape at the boundary so the
                // user's callback gets the unified `entity_sdk::InspectFact`
                // regardless of arm.
                let wrapped = move |wire: &entity_wasm_worker_protocol::InspectFact| {
                    let sdk_fact = crate::inspect_router::wire_to_sdk_fact(wire);
                    cb(&sdk_fact);
                };
                let handle = proxy.install_inspect_sink(peer_id.to_string(), wrapped);
                Ok(crate::inspect_router::PeersInspectSinkHandle::Worker(handle))
            }
        }
    }

    // ---- Multi-SDK attachment (Stage 2B) ----------------------------

    /// Attach an already-spawned `WorkerPeerStore` as a new SDK in the
    /// pool. Returns the index it landed at. Routes for the store's
    /// known peer_ids are inserted into `peer_routes` so subsequent
    /// per-peer ops land on the new SDK.
    ///
    /// Use when lazily growing the multi-SDK set in response to a
    /// user creating a `BackendMemory` or `BackendOpfs` peer — the
    /// action handler `wasm_bindgen_futures::spawn_local`s the
    /// `WorkerProxy::spawn`, then queues a pending attachment which
    /// the next frame drains and integrates via this method.
    #[cfg(target_arch = "wasm32")]
    pub fn attach_worker_sdk(&mut self, store: WorkerPeerStore) -> usize {
        let idx = self.sdks.len();
        let ids = store.peer_ids();
        self.sdks.push(Sdk::Worker(store));
        for pid in ids {
            self.peer_routes.insert(pid, idx);
        }
        idx
    }

    /// Number of hosted SDKs (1 in Stage 2A; grows in Stage 2B as
    /// non-primary host configs come online).
    pub fn sdk_count(&self) -> usize {
        self.sdks.len()
    }
}

/// Direct-arm builder for the connect future. Shared by the wasm and
/// native arms of `Peers::connect_peer` to keep the four-step body
/// (`connector.connect` → `perform_connect` → `remote.insert`) in one
/// place. The future is detached (owns its inputs) so callers
/// `spawn_local`/`tokio::spawn` it without lifetime tangle.
fn direct_connect_future(
    shared: Option<std::sync::Arc<entity_peer::PeerShared>>,
    peer_id: &str,
    address: String,
) -> ConnectPeerFuture<'static> {
    let Some(shared) = shared else {
        let m = format!("connect_peer: shared state for {peer_id} not found");
        return Box::pin(async move { Err(m) });
    };
    Box::pin(async move {
        let conn = shared
            .connector
            .connect(&address)
            .await
            .map_err(|e| format!("Connect to {address} failed: {e}"))?;
        let remote = entity_peer::remote::perform_connect(conn, &shared.keypair, shared.config.home_hash_format)
            .await
            .map_err(|e| format!("Handshake failed: {e}"))?;
        let remote_peer_id = remote.remote_peer_id.clone();
        shared.remote.insert(&remote_peer_id, remote);
        Ok(remote_peer_id)
    })
}

// =====================================================================
// In-process memory transport — consumer-side integration.
//
// Drives upstream `MemoryConnector` / `MemoryListener` /
// `MemoryTransportRegistry` (entity-core-rust) through the
// `Peers` multi-SDK router. Two `Peers` instances built against the
// same registry, each binds a `MemoryListener` for its primary peer,
// runs `entity_peer::server::run` on a tokio task, and
// `Peers::connect_peer` from A to `memory://<B-pid>` completes the
// entity-protocol handshake end-to-end — no networking, no ports.
//
// Native-only — `MemoryConnector` is `#[cfg(not(target_arch = "wasm32"))]`.
// =====================================================================
#[cfg(all(test, not(target_arch = "wasm32")))]
mod memory_transport_tests {
    use super::*;
    use entity_peer::transport::{MemoryConnector, MemoryListener, MemoryTransportRegistry};
    use std::time::Duration;

    fn spawn_peer_on_registry(
        registry: std::sync::Arc<MemoryTransportRegistry>,
    ) -> (Peers, String, tokio::task::JoinHandle<()>) {
        let peers =
            Peers::new_direct_with_connector(std::sync::Arc::new(MemoryConnector::new(registry.clone())));
        let pid = peers.primary_peer_id().to_string();

        let shared = peers.direct_peer_shared(&pid).expect("primary peer_shared");
        let pm = peers.primary_as_direct().expect("primary is Direct");
        let peer = pm.sdk().peer(&pid).expect("primary peer").peer();
        peer.start_engines(&shared);

        let listener = MemoryListener::bind(pid.clone(), registry).expect("bind listener");
        let shared_for_server = shared.clone();
        let handle = tokio::spawn(async move {
            let _ = entity_peer::server::run(listener, shared_for_server).await;
        });

        (peers, pid, handle)
    }

    #[tokio::test]
    async fn peers_connect_over_memory_transport() {
        let registry = MemoryTransportRegistry::new();

        let (peers_a, pid_a, handle_a) = spawn_peer_on_registry(registry.clone());
        let (_peers_b, pid_b, handle_b) = spawn_peer_on_registry(registry.clone());
        assert_ne!(pid_a, pid_b);

        tokio::task::yield_now().await;

        let connect_fut = peers_a.connect_peer(&pid_a, format!("memory://{pid_b}"));
        let remote_pid = tokio::time::timeout(Duration::from_secs(2), connect_fut)
            .await
            .expect("connect_peer timed out")
            .expect("connect_peer must succeed");
        assert_eq!(remote_pid, pid_b);

        let shared_a = peers_a.direct_peer_shared(&pid_a).unwrap();
        assert!(shared_a.remote.get(&pid_b).is_some());

        handle_a.abort();
        handle_b.abort();
    }

    #[tokio::test]
    async fn peers_connect_unknown_endpoint_errors_cleanly() {
        let registry = MemoryTransportRegistry::new();
        let (peers, pid, _handle) = spawn_peer_on_registry(registry);

        let err = peers
            .connect_peer(&pid, "memory://nobody-home".to_string())
            .await
            .expect_err("connect to absent endpoint must error");
        assert!(
            err.contains("no listener") || err.contains("nobody-home"),
            "unexpected error message: {err}"
        );
    }
}

// =====================================================================
// put_if_absent — durable-authoritative seed (Direct arm).
//
// The Worker arm (L1 `proxy.get` + `put_and_wait`) is exercised by
// `tests/e2e_worker.rs`; native tests can only reach the Direct arm,
// where the in-process store is authoritative. The contract under test:
// seed exactly once, never clobber a present value, error (not panic) on
// an unrouted peer. See the boot-config-surfaces reframe §2.4.
// =====================================================================
#[cfg(all(test, not(target_arch = "wasm32")))]
mod put_if_absent_tests {
    use super::*;

    fn test_entity(tag: &str) -> Entity {
        let data = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
            "tag" => entity_ecf::text(tag)
        });
        Entity::new("app/state/test", data).unwrap()
    }

    #[tokio::test]
    async fn seeds_once_and_never_clobbers() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let path = format!("/{pid}/app/entity-browser/settings/test");

        // Absent → seeds, returns true, value is readable.
        let seeded = peers
            .put_if_absent(&pid, path.clone(), test_entity("first"), 1000)
            .await
            .expect("put_if_absent must not error on a routed Direct peer");
        assert!(seeded, "absent path → seeded (true)");
        let first = peers.get_entity(&pid, &path).expect("value present after seed");
        assert_eq!(first.data, test_entity("first").data);

        // Present → no write, returns false, original value untouched.
        let seeded_again = peers
            .put_if_absent(&pid, path.clone(), test_entity("second"), 1000)
            .await
            .expect("put_if_absent must not error");
        assert!(!seeded_again, "present path → not seeded (false)");
        let still = peers.get_entity(&pid, &path).expect("value still present");
        assert_eq!(
            still.data,
            test_entity("first").data,
            "a present value must never be clobbered by put_if_absent"
        );
    }

    #[tokio::test]
    async fn unrouted_peer_errors_not_panics() {
        let peers = Peers::new_direct();
        let err = peers
            .put_if_absent(
                "peer-that-does-not-exist",
                "/peer-that-does-not-exist/app/x".to_string(),
                test_entity("z"),
                1000,
            )
            .await
            .expect_err("an unrouted peer must error, never silently misroute");
        assert!(!err.is_empty(), "error must carry a message");
    }
}

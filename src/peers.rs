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

/// Future yielding the `maintain-peer` handler result. Used by
/// `Peers::maintain_peer`; the caller keeps its own retry judgement, so the
/// raw result travels rather than a bool.
#[cfg(not(target_arch = "wasm32"))]
pub type MaintainPeerFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<entity_handler::HandlerResult, String>> + Send + 'a>,
>;

#[cfg(target_arch = "wasm32")]
pub type MaintainPeerFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<entity_handler::HandlerResult, String>> + 'a>,
>;

/// The `maintain-peer` request body (EXTENSION-NETWORK §2.2): the peer to keep
/// connected and the address to reach it at. `reconnect` / `resubscribe`
/// default true, so the request carries only those two fields.
///
/// Lives here rather than in `app.rs` because `Peers::maintain_peer` is now the
/// single place that issues the op — see its doc for why that consolidation is
/// load-bearing.
fn maintain_request_entity(peer_id: &str, address: &str) -> Entity {
    let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![
        (entity_ecf::text("peer_id"), entity_ecf::text(peer_id)),
        (entity_ecf::text("address"), entity_ecf::text(address)),
    ]));
    Entity::new(entity_network::TYPE_MAINTAIN_REQUEST, data)
        .expect("maintain-request entity construction is infallible")
}

/// Future for `Peers::disconnect_peer` / `reconnect_peer` — resolves to `()`
/// on success (or the reconnected remote's id, for `reconnect_peer`, reusing
/// `ConnectPeerFuture`).
#[cfg(not(target_arch = "wasm32"))]
pub type DisconnectPeerFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>,
>;

#[cfg(target_arch = "wasm32")]
pub type DisconnectPeerFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<(), String>> + 'a>,
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

    /// Start this peer's kernel extension engines — the subscription **delivery**
    /// loop and the EXTENSION-NETWORK `PeerLink` bind.
    ///
    /// Both the engines and the connection pool are **per-peer**
    /// (`PeerShared::remote`), so this is not a one-off boot call for the system
    /// peer: any peer that must answer `maintain-peer` on its own
    /// `/{peer}/system/network` — or deliver its tree changes to a remote
    /// subscriber — needs its own call, or the handler 500s "network handler not
    /// bound". Idempotent: `Peer::start_engines` guards per-peer on an atomic.
    ///
    /// Direct arm only. The Worker arm's peers live in the worker and must start
    /// their engines there (a separate wiring point, unbuilt).
    pub fn start_engines(&self, peer_id: &str) -> EnginesStart {
        match self {
            Sdk::Direct(pm) => match pm.sdk().peer(peer_id) {
                Some(ctx) => {
                    ctx.peer().start_engines(&ctx.peer_shared());
                    EnginesStart::Started
                }
                None => EnginesStart::Unknown,
            },
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(_) => EnginesStart::NotApplicable,
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
    /// Per-peer dispatch handle — the awaited, read-capable counterpart to
    /// [`writer_handle_for`](Self::writer_handle_for). Direct hands out the
    /// peer's own `Arc<PeerContext>` (whose L1 methods already return owning
    /// futures); Worker hands out the proxy plus the peer id, the same pair
    /// `WorkerPeerStore` dispatches through.
    fn dispatch_handle(&self, peer_id: &str) -> Option<crate::dispatch_handle::DispatchHandle> {
        match self {
            Sdk::Direct(pm) => pm
                .sdk()
                .peer_arc(peer_id)
                .map(crate::dispatch_handle::DispatchHandle::Direct),
            #[cfg(target_arch = "wasm32")]
            Sdk::Worker(w) => Some(crate::dispatch_handle::DispatchHandle::Worker {
                proxy: w.proxy_handle(),
                peer_id: peer_id.to_string(),
            }),
        }
    }

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

/// Outcome of [`Peers::start_engines`]. Three states, not a bool, because the
/// caller sweeps the roster every frame and must tell "nothing to do, ever"
/// apart from "not yet" — collapsing them either re-scans Worker peers forever
/// or permanently skips a peer that was still being built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnginesStart {
    /// Started, or already running (the kernel's own atomic guard made it a
    /// no-op). Settled — don't call again.
    Started,
    /// The peer is known but lives on a Worker SDK; its engines belong in the
    /// worker, which is a separate wiring point. Settled on this thread.
    NotApplicable,
    /// The peer isn't routable — either genuinely unknown, or registered a
    /// moment from now. **Not** settled: retry.
    Unknown,
}

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
    /// Local peers carrying a §6.5 WebRTC establisher.
    ///
    /// Populated from the **install**, not from what was requested — the Worker
    /// arm reads `WireCaps.webrtc_peers` (v12: derived from the install sites so
    /// it cannot agree with the request by construction), the Direct arm records
    /// the peer it actually handed a seam to.
    ///
    /// Why the app needs this at all: the establisher is **primary-only**, but
    /// several surfaces act as the *bound* peer — `meet` announces the bound
    /// peer's id, `ChatDelivery` binds `self.peer_id`. On a single-peer boot they
    /// coincide and the distinction is invisible; with a second local peer, a
    /// meet run from it hands strangers an id that has no way to be connected
    /// back to, and nothing anywhere says so. [AP22]
    webrtc_peers: std::collections::HashSet<String>,
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
            // No seam was passed, so no peer here has one. The one constructor
            // that installs (`new_direct_idb_with_establish`) records it.
            webrtc_peers: std::collections::HashSet::new(),
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
    /// arm — the building block the persistent system peer reuses, and the same
    /// shape generalized to secondary user peers by the `frontend-idb` mode
    /// (`build_idb_ctx` + `insert_built_idb_peer`; see
    /// `docs/architecture/reviews/DESIGN-PERSISTENT-THIS-TAB-PEER.md`).
    // Kept as the symmetric no-establisher constructor (mirrors `new_direct`);
    // the boot path now routes through `new_direct_idb_with_establish` so a
    // primary that provisions Direct-arm WebRTC installs the seam. Retained as a
    // clean public API and referenced by the docs below.
    #[cfg(target_arch = "wasm32")]
    #[allow(dead_code)]
    pub async fn new_direct_idb(
        keypair: entity_crypto::Keypair,
        db_name: &str,
    ) -> Result<Self, entity_sdk::SdkError> {
        Self::new_direct_idb_with_establish(keypair, db_name, None).await
    }

    /// Like [`new_direct_idb`](Self::new_direct_idb) but installs an
    /// EXTENSION-NETWORK §10.3 live-establishment seam on the primary peer —
    /// the **Direct-arm WebRTC** establisher
    /// (`entity_wasm_worker_proxy::MainThreadWebRtcEstablisher`). The seam is a
    /// constructor argument because it MUST be captured before the peer's
    /// `PeerShared` clones do (there is no `&mut Peer` on this arm). `None` is
    /// byte-identical to [`new_direct_idb`](Self::new_direct_idb).
    #[cfg(target_arch = "wasm32")]
    pub async fn new_direct_idb_with_establish(
        keypair: entity_crypto::Keypair,
        db_name: &str,
        live_establish: Option<
            std::sync::Arc<dyn entity_peer::live_establish::LiveEstablish>,
        >,
    ) -> Result<Self, entity_sdk::SdkError> {
        let installed = live_establish.is_some();
        let pm = entity_sdk::PeerManager::with_keypair_idb_and_establish(
            keypair,
            db_name,
            live_establish,
        )
        .await?;
        let mut peers = Self::new_direct_with_sdk(Sdk::Direct(pm));
        if installed {
            let primary = peers.primary_peer_id.clone();
            peers.webrtc_peers.insert(primary);
        }
        Ok(peers)
    }

    /// Worker-mode constructor. Wraps an already-spawned WorkerPeerStore
    /// as the single SDK. The store's primary peer becomes Peers' primary.
    #[cfg(target_arch = "wasm32")]
    pub fn new_worker(store: WorkerPeerStore) -> Self {
        let primary_peer_id = store.primary_peer_id().to_string();
        // v12's install report, read once at construction: the worker tells us
        // which peers actually got an establisher, derived from the install
        // sites rather than recomputed from the request.
        let webrtc_peers = store
            .proxy_handle()
            .capabilities()
            .map(|c| c.webrtc_peers.into_iter().collect())
            .unwrap_or_default();
        let mut peers = Self {
            sdks: vec![Sdk::Worker(store)],
            peer_routes: HashMap::new(),
            primary_peer_id,
            webrtc_peers,
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

    /// Does `peer_id` carry a §6.5 WebRTC establisher?
    ///
    /// **What this answers is "can a stranger connect *back* to this peer".** A
    /// browser peer has no dialable address — its route is WebRTC established at
    /// dispatch time — so without an establisher it can be *introduced* (a meet
    /// works; it is an ordinary WebSocket call to the node) and then never
    /// reached. That asymmetry is the trap: the discovery half succeeds
    /// completely and the connect half cannot even be attempted.
    ///
    /// The establisher is **primary-only** by deliberate policy (an additional
    /// peer must ask explicitly — the v6 lesson), while `meet` announces the
    /// *bound* peer. Any surface that hands this peer's id to a counterpart, or
    /// depends on a counterpart reaching it, should check this and say so rather
    /// than let the failure land on the stranger's side, invisibly, later.
    ///
    /// Native builds have no WebRTC at all, so this is always `false` there —
    /// which is correct, not a gap: a native peer is reached by its address.
    pub fn peer_has_webrtc(&self, peer_id: &str) -> bool {
        self.webrtc_peers.contains(peer_id)
    }

    /// Which arm hosts `peer_id` — `"direct"`, `"worker"`, or `"unknown"` for a
    /// peer this router has never heard of.
    ///
    /// Diagnostics only, and deliberately a display string rather than an enum:
    /// nothing branches on it (per-peer routing goes through `sdk_for`, which is
    /// the point of that method), and an enum would invite exactly the
    /// arm-deciding-at-a-call-site pattern the router exists to prevent. What it
    /// is for is a preflight report saying which substrate the reader is
    /// actually on — the difference decides whether Worker/OPFS applies, and it
    /// is invisible from the page otherwise.
    pub fn arm_of(&self, peer_id: &str) -> &'static str {
        match self.sdk_for(peer_id) {
            Ok(Sdk::Direct(_)) => "direct",
            #[cfg(target_arch = "wasm32")]
            Ok(Sdk::Worker(_)) => "worker",
            Err(_) => "unknown",
        }
    }

    /// Pretend `peer_id` got an establisher — tests only.
    ///
    /// Native has no WebRTC, so without this a test can only ever observe the
    /// "no establisher" branch, and a guard that fires unconditionally would
    /// look identical to a correct one. This is what lets the proof assert
    /// *both* directions.
    #[cfg(test)]
    pub(crate) fn mark_webrtc_peer_for_test(&mut self, peer_id: &str) {
        self.webrtc_peers.insert(peer_id.to_string());
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

    /// Clobber-safe "seed the default if nothing is durably present" — the
    /// app-tier convenience over [`Self::put_if_absent`] that hides the
    /// arm/cfg split every window model's `ensure_state_in_tree` needs.
    ///
    /// Native (Direct-only): the in-process store is authoritative and the
    /// sync `initialize` → `read_state` sequence (and the tests) expect the
    /// seed dispatched synchronously, so a `get_entity` miss dispatches the
    /// default in-line.
    ///
    /// Wasm: BOTH arms route through the durable `put_if_absent` future. The
    /// Worker cache mirror can be **cold at window create**, so a `get_entity`
    /// miss is NOT absence — the old sync get-then-`dispatch_write` re-seeded
    /// the default over persisted state (the Settings clobber caught by e2e
    /// Phase 26.8; `SettingsModel::ensure_state` is the twin of this). The
    /// seed is fire-and-forget; a following `read_state` legitimately reads
    /// the default until the durable value round-trips through the
    /// subscription. `label` names the surface in the D13 log line — a
    /// `seeded=true` on a profile that should already have state is the
    /// clobber signature.
    pub fn seed_state_if_absent(
        &self,
        peer_id: &str,
        path: impl Into<String>,
        default: Entity,
        label: &'static str,
    ) {
        let path = path.into();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = label;
            if self.get_entity(peer_id, &path).is_none() {
                self.dispatch_write(peer_id, path, default);
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let fut = self.put_if_absent(peer_id, path, default, 5_000);
            wasm_bindgen_futures::spawn_local(async move {
                match fut.await {
                    Ok(seeded) => tracing::info!(seeded, surface = label, "seed_state_if_absent"),
                    Err(e) => {
                        tracing::warn!(error = %e, surface = label, "seed_state_if_absent failed")
                    }
                }
            });
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

    /// Start `peer_id`'s kernel extension engines on whichever SDK routes it —
    /// see [`Sdk::start_engines`] for why this is per-peer and not a boot
    /// one-off. An unrouted peer is [`EnginesStart::Unknown`] (it may simply not
    /// be registered *yet*), which is why the caller must distinguish that from
    /// [`EnginesStart::NotApplicable`] rather than retrying both forever.
    pub fn start_engines(&self, peer_id: &str) -> EnginesStart {
        match self.sdk_for(peer_id) {
            Ok(sdk) => sdk.start_engines(peer_id),
            Err(_) => EnginesStart::Unknown,
        }
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

    /// Build a durable **`frontend-idb`** `PeerContext` off-registry — the
    /// async half of persistent-this-tab creation, split out so it can run in a
    /// `spawn_local` **without borrowing `Peers`** (the runtime create path
    /// dispatches from the sync frame loop and cannot hold `&mut self` across
    /// `build_async().await`). The built ctx is then handed to the sync
    /// [`insert_built_idb_peer`](Self::insert_built_idb_peer).
    ///
    /// The db name is `entity-peer-{peer_id}` — the **same** scheme the primary
    /// uses (`persistence::system_seed_id`) — so a later boot replay reopens the
    /// SAME database. This derivation is the identity danger site (MAP §8 #1);
    /// it lives HERE, in one place, shared by create and replay.
    #[cfg(target_arch = "wasm32")]
    pub async fn build_idb_ctx(
        keypair: entity_crypto::Keypair,
    ) -> Result<entity_sdk::PeerContext, String> {
        let peer_id = keypair.peer_id().to_string();
        let db_name = format!("entity-peer-{peer_id}");
        let config = entity_peer::PeerConfig {
            debug_open_grants: true,
            ..entity_peer::PeerConfig::default()
        };
        let ctx = entity_sdk::PeerContextBuilder::new()
            .keypair(keypair)
            .config(config)
            .connector(std::sync::Arc::new(
                entity_peer::transport::BrowserWebSocketConnector,
            ))
            .idb(&db_name)
            .build_async()
            .await
            .map_err(|e| format!("durable this-tab peer build failed: {e}"))?;
        // Identity self-check (the danger site): the built ctx's id MUST match
        // the id we derived the db name from, or this peer would route under one
        // id while its store lives in another id's database.
        debug_assert_eq!(
            ctx.peer_id().to_string(),
            peer_id,
            "built ctx id diverged from the id used to name its IDB database"
        );
        Ok(ctx)
    }

    /// Register an already-built durable `PeerContext` (from
    /// [`build_idb_ctx`](Self::build_idb_ctx)) into the **primary Direct SDK**
    /// and wire it up — the sync half, safe to call from the frame loop. Seeds
    /// metadata (`persisted: true`), refreshes routes, spawns the per-peer event
    /// bridge. Returns the registered peer-id.
    ///
    /// **Option A placement:** the peer lives in slot-0's Direct SDK alongside
    /// the system peer (the tested many-peers-in-one-Direct-SDK pattern); its
    /// store is its own isolated IDB database. Direct posture only.
    ///
    /// Checkpoint-on-create is the caller's job (`create` path) via
    /// [`idb_checkpoint_for`](Self::idb_checkpoint_for) — this sync method
    /// cannot await; replay skips it (read-side rehydrate, no fresh write).
    #[cfg(target_arch = "wasm32")]
    pub fn insert_built_idb_peer(
        &mut self,
        ctx: entity_sdk::PeerContext,
        label: Option<String>,
    ) -> Result<String, String> {
        if !matches!(self.primary_sdk(), Sdk::Direct(_)) {
            return Err(
                "persistent this-tab peers require the Direct arm (the Worker \
                 arm uses OPFS-backed backend peers instead)"
                    .into(),
            );
        }
        let metadata = entity_sdk::PeerMetadata {
            label,
            persisted: true,
            ..entity_sdk::PeerMetadata::default()
        };
        let Sdk::Direct(pm) = self.primary_sdk_mut() else {
            unreachable!("Direct arm checked above")
        };
        // `insert_peer` is the SDK's documented escape hatch for builder
        // customization beyond `create_peer`'s fixed signature — no SDK change.
        let inserted_id = pm
            .sdk_mut()
            .insert_peer(ctx)
            .map_err(|e| format!("durable this-tab peer insert failed: {e}"))?;
        pm.sdk_mut().set_metadata(&inserted_id, metadata);

        // Route + per-peer event bridge, same as create_new_peer's Direct arm.
        self.refresh_routes_for_sdk(0);
        if let Ok(ctx) = self.direct_peer_context(&inserted_id) {
            wasm_bindgen_futures::spawn_local(ctx.event_bridge());
        }
        Ok(inserted_id)
    }

    /// Reconstruct a persisted `frontend-idb` peer at boot from its **saved**
    /// keypair, reopening its existing `entity-peer-{id}` database. The durable
    /// analog of the in-memory `load_persisted_primary` replay. No checkpoint:
    /// this is a read-side rehydrate (open + journal replay), not a fresh write.
    /// Direct posture only — a `frontend-idb` entry on a Worker-primary boot is a
    /// named deferred hole (the caller warns). Runs in `new_wasm`, which is
    /// async and owns `peer_manager`, so build + insert can be sequenced here.
    #[cfg(target_arch = "wasm32")]
    pub async fn replay_persisted_idb_peer(
        &mut self,
        keypair: entity_crypto::Keypair,
        label: Option<String>,
    ) -> Result<String, String> {
        let ctx = Self::build_idb_ctx(keypair).await?;
        self.insert_built_idb_peer(ctx, label)
    }

    /// The durable-commit checkpoint handle for a **specific** peer's IndexedDB
    /// store (not just the primary's — cf. [`idb_checkpoint`](Self::idb_checkpoint)).
    /// `Some` only for a Direct/IDB-backed peer. Owned (`IdbCheckpoint: Clone`)
    /// so the caller can `await` it across a `spawn_local` without borrowing
    /// `self` — used to flush a fresh `frontend-idb` create durably.
    #[cfg(target_arch = "wasm32")]
    pub fn idb_checkpoint_for(
        &self,
        peer_id: &str,
    ) -> Option<entity_store::idb::IdbCheckpoint> {
        self.primary_as_direct()?
            .peer_context(peer_id)?
            .idb_checkpoint()
            .cloned()
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
        let writer = self.writer_handle_for(peer_id);
        let inner = match sdk {
            Sdk::Direct(pm) => {
                direct_connect_future(pm.peer_shared(peer_id), peer_id, address.clone())
            }
            Sdk::Worker(w) => Box::pin(w.connect_peer(peer_id.to_string(), address.clone())),
        };
        publishing_transport_profile(inner, writer, peer_id.to_string(), address)
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
        let writer = self.writer_handle_for(peer_id);
        let inner = direct_connect_future(pm.peer_shared(peer_id), peer_id, address.clone());
        publishing_transport_profile(inner, writer, peer_id.to_string(), address)
    }

    /// EXECUTE `maintain-peer` on `local_pid`'s own `/{local}/system/network`,
    /// handing the network extension a peer and the address to reach it — and,
    /// on the 200 that means "established, reconnect graph installed", publish
    /// the transport profile for that address.
    ///
    /// **Why this exists as a seam.** `connect_peer` is not how peers actually
    /// connect in the shipped app: the backend auto-connect drain and the
    /// window-bound maintain sweep both go through `maintain-peer`, where the
    /// extension dials *inside the kernel* and never touches `connect_peer`. So
    /// publishing only at `connect_peer` covered the manual Connect button and
    /// missed every automatic connection — which is most of them. Routing both
    /// drains through one helper is deliberate: per-call-site publishing is how
    /// half the roster silently ends up without a profile, the same disease as
    /// per-creation-site `start_engines` calls.
    ///
    /// The 200 is what makes this honest. `maintain-peer` returning 200 means
    /// the address *worked*; anything else (a 502 boot-readiness race, an
    /// unreachable peer) publishes nothing, so a dead address never becomes a
    /// route the ladder will spend a timeout on.
    ///
    /// **It publishes TWO facts on that 200, and the second one was missing for
    /// months.** The route is what the dispatch ladder reads; the connections
    /// registry row is what every *human-facing target picker* reads
    /// (`connections::read_connections` — Chat's "start a chat with", File
    /// Transfer's device list, Peer Connections' known devices). The registry
    /// write used to live only at `connect_peer` and in `reach_keeper`, i.e.
    /// exactly the manual paths — so inside Tori the app auto-connected to its
    /// own backend, the kernel said `connected`, File Transfer worked against
    /// it if you could name it, and **every picker showed nothing**, telling
    /// the user to "go to Peer Connections and connect" to a peer they were
    /// already connected to. `the_system_backend_is_shown_like_any_other_device`
    /// did not catch it because it seeds the row by hand and so only ever
    /// asserted the *downstream* half.
    ///
    /// That is this doc comment's own paragraph happening a second time to a
    /// different fact. When a seam exists because "the manual path is not how
    /// peers actually connect", **every** fact derived from connecting belongs
    /// on it — not just the one that motivated it.
    ///
    /// Returns the raw `HandlerResult` so callers keep their own retry/backoff
    /// judgement — this helper adds the publish, it does not interpret failure.
    pub fn maintain_peer(
        &self,
        local_pid: &str,
        remote_pid: &str,
        address: &str,
    ) -> MaintainPeerFuture<'static> {
        let writer = self.writer_handle_for(local_pid);
        // System-peer-scoped by construction (`ConnectionsWriter::new`), because
        // that is the one namespace `read_connections` reads — a row written
        // under the dialing local peer would be invisible to every picker.
        let connections = crate::connections::ConnectionsWriter::new(self);
        let fut = self.execute(
            local_pid,
            format!("/{local_pid}/system/network"),
            "maintain-peer".to_string(),
            maintain_request_entity(remote_pid, address),
            entity_handler::ExecuteOptions::default(),
        );
        let local = local_pid.to_string();
        let remote = remote_pid.to_string();
        let addr = address.to_string();
        Box::pin(async move {
            let res = fut.await?;
            if res.status == 200 {
                match writer {
                    Some(w) => crate::transport_profiles::publish_dialed(
                        &w,
                        &local,
                        &remote,
                        &addr,
                        crate::transport_profiles::now_epoch_ms(),
                    ),
                    None => tracing::debug!(
                        local = %local,
                        remote = %remote,
                        "maintain-peer established, but no writer handle for the local peer — \
                         transport profile not published"
                    ),
                }
                // "We have connected to this peer at least once" — the fact
                // every target picker reads. Idempotent on the path, so the
                // repeated maintains of a long-lived link just refresh
                // `last_seen`.
                connections.add(&remote);
            }
            Ok(res)
        })
    }

    /// Drop every published route to `remote_pid`, across **all** local peers.
    ///
    /// The counterpart of the publish in [`maintain_peer`](Self::maintain_peer)
    /// / [`connect_peer`](Self::connect_peer). It sweeps the whole local roster
    /// rather than one peer because a profile is written by whichever local
    /// peer dialed, and more than one may have: the system peer's backend
    /// drain and a window-bound peer's maintain sweep publish under their own
    /// roots. Forgetting from only the primary would leave a route behind on
    /// exactly the multi-peer setups where it is hardest to notice.
    ///
    /// Returns the number of local peers swept, for the caller's log line.
    pub fn forget_routes_to(&self, remote_pid: &str) -> usize {
        let locals = self.peer_ids();
        for local in &locals {
            if let Some(w) = self.writer_handle_for(local) {
                crate::transport_profiles::forget(&w, local, remote_pid);
            }
        }
        locals.len()
    }

    /// Evict the pooled connection `peer_id → remote_peer_id` — **uniform
    /// across arms**. The next `connect_peer` re-handshakes fresh, which is
    /// how `peer_id` adopts a capability grant authored on the remote *after*
    /// it connected (the granter re-mints at authenticate; a pooled reuse
    /// keeps the stale cap — see `DESIGN-ENFORCEMENT-CUTOVER.md`). Direct:
    /// `RemoteState::remove` (sync, wrapped). Worker: routes through the
    /// proxy. Idempotent (absent connection = success). Unrouted → `Err`.
    #[cfg(target_arch = "wasm32")]
    pub fn disconnect_peer(&self, peer_id: &str, remote_peer_id: &str) -> DisconnectPeerFuture<'static> {
        let sdk = match self.sdk_for(peer_id) {
            Ok(s) => s,
            Err(e) => {
                let m = e.to_string();
                return Box::pin(async move { Err(m) });
            }
        };
        match sdk {
            Sdk::Direct(pm) => {
                let shared = pm.peer_shared(peer_id);
                let remote = remote_peer_id.to_string();
                Box::pin(async move {
                    match shared {
                        Some(s) => { s.remote.remove(&remote); Ok(()) }
                        None => Err("disconnect_peer: no shared state".to_string()),
                    }
                })
            }
            Sdk::Worker(w) => Box::pin(w.disconnect_peer(peer_id.to_string(), remote_peer_id.to_string())),
        }
    }

    /// Native variant — Direct arm only.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn disconnect_peer(&self, peer_id: &str, remote_peer_id: &str) -> DisconnectPeerFuture<'static> {
        let sdk = match self.sdk_for(peer_id) {
            Ok(s) => s,
            Err(e) => {
                let m = e.to_string();
                return Box::pin(async move { Err(m) });
            }
        };
        let Sdk::Direct(pm) = sdk;
        let shared = pm.peer_shared(peer_id);
        let remote = remote_peer_id.to_string();
        Box::pin(async move {
            match shared {
                Some(s) => { s.remote.remove(&remote); Ok(()) }
                None => Err("disconnect_peer: no shared state".to_string()),
            }
        })
    }

    /// Drop the pooled connection `peer_id → remote_peer_id` and re-dial it at
    /// `address` — the reconnect that lets a just-authorized peer adopt its new
    /// grant without user re-pairing. `disconnect` then `connect`; a plain
    /// re-connect alone is insufficient because the pool `insert` is
    /// insert-if-absent, so the stale connection must be evicted first.
    pub fn reconnect_peer(
        &self,
        peer_id: &str,
        remote_peer_id: &str,
        address: String,
    ) -> ConnectPeerFuture<'static> {
        let disconnect = self.disconnect_peer(peer_id, remote_peer_id);
        // `connect_peer` borrows `self`; build its future eagerly (it's already
        // detached to 'static) so the returned future owns everything.
        let connect = self.connect_peer(peer_id, address);
        Box::pin(async move {
            // A disconnect failure is non-fatal for reconnect intent — if there
            // was nothing to evict, the connect still runs and re-handshakes.
            let _ = disconnect.await;
            connect.await
        })
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

    /// A cloneable **dispatch** handle bound to `peer_id` — the awaited
    /// counterpart to [`writer_handle_for`](Self::writer_handle_for), for a
    /// flow that must keep dispatching *after* its first `.await`
    /// (`crate::file_offer`'s list-then-read and blob-then-chunks walks).
    /// `None` on an unrouted peer, never silently the primary's.
    ///
    /// See [`crate::dispatch_handle::DispatchHandle`] for why an owned handle
    /// is needed at all — in short, every L1 future here already owns itself,
    /// but making the *second* call needs a `&Peers` the spawned task no
    /// longer has.
    pub fn dispatch_handle(
        &self,
        peer_id: &str,
    ) -> Option<crate::dispatch_handle::DispatchHandle> {
        self.sdk_for(peer_id).ok()?.dispatch_handle(peer_id)
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
        // Dial through the kernel's `connect_and_pool` — the SAME primitive
        // `Peer::connect_to` runs — NOT a hand-rolled `perform_connect_with_dispatch`
        // + `remote.insert`. That hand-roll (which this used to be) skipped the four
        // post-handshake writes `connect_and_pool` does: the R6 held-session cap, the
        // §3.13 `active` connection entity, the Amendment 12 §A3
        // `system/peer/status=connected` write (the whole input to the `peer_liveness`
        // read-model), and §5 keepalive. So the app's Direct arm produced NO kernel
        // liveness surface for connections it established — the read-model was inert
        // (`FINDING-2026-08-10-app-connect-bypasses-kernel-liveness`). `connect_and_pool`
        // takes `&Arc<PeerShared>`, so there's no `&Peer`-across-await borrow to dodge;
        // it keeps the reentry-dispatch (`Some(shared)`, for the pushes a listener-less
        // browser peer can only receive over a connection IT dialed — chat delivery,
        // subscription `receive`) and rebinds the endpoint (fresh dial wins — the
        // insert-if-absent B3/B4/B5 footgun `reconnect_peer` worked around is gone).
        let endpoint = entity_peer::remote::connect_and_pool(&shared, &address)
            .await
            .map_err(|e| format!("Connect to {address} failed: {e}"))?;
        Ok(endpoint.remote_peer_id().to_string())
    })
}

/// Wrap a connect future so a **successful** dial publishes the durable
/// transport profile the kernel's dispatch ladder reads (rung 2,
/// `resolve_transport_address`) — see `crate::transport_profiles`.
///
/// This wraps `connect_peer` rather than living inside `direct_connect_future`
/// for two reasons. It is the **arm-uniform** seam: the Worker arm connects
/// inside the worker and never touches `direct_connect_future`, so publishing
/// there would have been a Direct-only fix that compiled clean — the
/// silently-stubbed-worker-arm footgun `WriterHandle` exists to prevent. And it
/// is the **address-dial** seam: `connect_peer` is reached only when a caller
/// had an address to dial, which is what keeps traversal (WebRTC) connections
/// from publishing a profile they must never publish.
///
/// A failed connect publishes nothing — an address that didn't work is not a
/// route, and recording it would hand the ladder a dial that costs a timeout on
/// every later dispatch.
fn publishing_transport_profile(
    inner: ConnectPeerFuture<'static>,
    writer: Option<crate::writer_handle::WriterHandle>,
    local_peer_id: String,
    address: String,
) -> ConnectPeerFuture<'static> {
    Box::pin(async move {
        let remote_peer_id = inner.await?;
        match writer {
            Some(w) => crate::transport_profiles::publish_dialed(
                &w,
                &local_peer_id,
                &remote_peer_id,
                &address,
                crate::transport_profiles::now_epoch_ms(),
            ),
            // No handle means no route to this peer's tree at all — the connect
            // itself came from somewhere else. Log rather than swallow: a
            // missing profile is a rung-2 miss much later, far from the cause.
            None => tracing::debug!(
                local = %local_peer_id,
                remote = %remote_peer_id,
                "connected, but no writer handle for the local peer — transport profile not published"
            ),
        }
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

    /// **THE PROFILE PROOF** — a dialed address becomes a route the *kernel*
    /// owns, not one the app has to hand back.
    ///
    /// Rung 2 of the dispatch ladder (`resolve_transport_address`) reads
    /// `/{local}/system/peer/transport/{remote_hex}/{profile-id}`. Nothing in
    /// production ever wrote those entities, so the app carried the address book
    /// instead and a dispatch that outlived its pooled connection died with
    /// `no transport profile for peer` (`REVIEW-CONNECTIVITY-LAYER-COHERENCE`
    /// §4). This asserts the two halves of the fix:
    ///
    /// 1. the profile lands, decodes through the **upstream** decoder, and
    ///    carries the address that actually worked;
    /// 2. — the load-bearing half — **losing the pooled connection is now
    ///    survivable**. Evicting A→B is what a reload costs us: the in-memory
    ///    pool is gone while the durable tree remains. A cross-peer fetch after
    ///    the eviction must still reach B, and the dial counter must climb,
    ///    proving the kernel re-dialed from the tree rather than the probe
    ///    quietly reusing something still pooled.
    ///
    /// **Mutation check:** drop the `publish_dialed` call in
    /// `publishing_transport_profile` and step (1) fails on the missing entity;
    /// keep the write but corrupt the path (any prefix the resolver doesn't
    /// list) and (1) still passes while (2) fails on the probe — which is
    /// precisely the failure mode a path-only test would miss.
    ///
    /// Not covered: the wasm/Worker arm (the publish routes through the same
    /// `WriterHandle`, but no harness drives it), and profile staleness — an
    /// address that stops working is never pruned.
    #[tokio::test]
    async fn a_dialed_address_survives_losing_the_pooled_connection() {
        use entity_peer::transport_profile::TcpProfileData;

        let registry = MemoryTransportRegistry::new();
        let (peers_a, pid_a, handle_a) = spawn_peer_on_registry(registry.clone());
        let (_peers_b, pid_b, handle_b) = spawn_peer_on_registry(registry.clone());
        tokio::task::yield_now().await;

        let addr_b = format!("memory://{pid_b}");
        let connect_fut = peers_a.connect_peer(&pid_a, addr_b.clone());
        let remote_pid = tokio::time::timeout(Duration::from_secs(2), connect_fut)
            .await
            .expect("connect_peer timed out")
            .expect("connect_peer must succeed");
        assert_eq!(remote_pid, pid_b);

        // (1) The profile is where the resolver lists, in the shape it decodes.
        let hex = crate::transport_profiles::remote_hex(&pid_b)
            .expect("an identity-form PID derives its hex locally");
        let path = crate::transport_profiles::profile_path(
            &pid_a,
            &hex,
            crate::transport_profiles::PROFILE_ID_PRIMARY,
        );
        let entity = peers_a.get_entity(&pid_a, &path).unwrap_or_else(|| {
            panic!(
                "no transport profile at {path} — a successful dial must leave the \
                 kernel a route, or the app stays the address book"
            )
        });
        let profile =
            TcpProfileData::from_entity(&entity).expect("upstream must decode what we published");
        assert_eq!(profile.peer_id, pid_b);
        assert_eq!(
            profile.endpoint_url, addr_b,
            "the profile must carry the address that actually worked"
        );

        // (2) Lose the pool — what a reload costs — and dispatch anyway.
        let shared_a = peers_a.direct_peer_shared(&pid_a).expect("A shared");
        assert!(shared_a.remote.get(&pid_b).is_some(), "connected first");
        shared_a.remote.remove(&pid_b);
        assert!(
            shared_a.remote.get(&pid_b).is_none(),
            "the eviction must actually empty the pool — otherwise the probe \
             below proves nothing about rung 2"
        );

        let dials_before = registry.dial_count();
        assert!(
            cross_peer_probe(&peers_a, &pid_a, &pid_b).await,
            "a dispatch that outlives its pooled connection must resolve the \
             published profile and re-dial — this is the `no transport profile \
             for peer` symptom, in miniature"
        );
        assert!(
            registry.dial_count() > dials_before,
            "the probe must have caused a real dial (rung 2 → connector); an \
             unchanged count would mean it answered from something still pooled"
        );

        handle_a.abort();
        handle_b.abort();
    }

    /// **The auto-connect half of the profile proof.**
    ///
    /// `connect_peer` is the manual Connect button. It is NOT how peers connect
    /// in the shipped app: the backend drain and the window-bound maintain
    /// sweep both go through `maintain-peer`, where the extension dials inside
    /// the kernel. So the sibling proof
    /// (`a_dialed_address_survives_losing_the_pooled_connection`) covered a path
    /// a desktop boot never takes, and every automatic connection published
    /// nothing. This asserts the seam that closed that: a `maintain-peer` 200
    /// leaves the same route behind.
    ///
    /// The **200 gate** is the other half, and it is the one worth protecting:
    /// a `maintain-peer` against an address nothing answers must publish
    /// nothing, or a dead address becomes a route the ladder spends a timeout
    /// on at every later dispatch. Asserted here against an unregistered
    /// endpoint on the same registry.
    ///
    /// **Mutation check:** publish unconditionally instead of on 200 and the
    /// second half fails; route `sync_maintained_peers` back through a bare
    /// `execute` and the first half fails.
    #[tokio::test]
    async fn maintaining_a_peer_publishes_the_route_but_only_when_it_worked() {
        use entity_peer::transport_profile::TcpProfileData;

        let registry = MemoryTransportRegistry::new();
        let peers_a =
            Peers::new_direct_with_connector(std::sync::Arc::new(MemoryConnector::new(registry.clone())));
        let pid_a = peers_a.primary_peer_id().to_string();
        let shared_a = peers_a.direct_peer_shared(&pid_a).expect("A shared");
        assert_eq!(
            peers_a.start_engines(&pid_a),
            EnginesStart::Started,
            "the network handler's PeerLink binds here — without it maintain-peer 500s"
        );
        let _srv_a = serve_peer(&pid_a, shared_a.clone(), registry.clone());

        let peers_b =
            Peers::new_direct_with_connector(std::sync::Arc::new(MemoryConnector::new(registry.clone())));
        let pid_b = peers_b.primary_peer_id().to_string();
        let shared_b = peers_b.direct_peer_shared(&pid_b).expect("B shared");
        assert_eq!(peers_b.start_engines(&pid_b), EnginesStart::Started);
        let _srv_b = serve_peer(&pid_b, shared_b.clone(), registry.clone());
        tokio::task::yield_now().await;

        let profile_path = |remote: &str| {
            crate::transport_profiles::profile_path(
                &pid_a,
                &crate::transport_profiles::remote_hex(remote).expect("identity-form hex"),
                crate::transport_profiles::PROFILE_ID_PRIMARY,
            )
        };

        // (1) A reachable peer: maintain-peer 200 ⇒ the route is published.
        let addr_b = format!("memory://{pid_b}");
        let res = peers_a
            .maintain_peer(&pid_a, &pid_b, &addr_b)
            .await
            .expect("maintain-peer dispatch");
        assert_eq!(res.status, 200, "maintain-peer must establish");
        let entity = peers_a
            .get_entity(&pid_a, &profile_path(&pid_b))
            .expect("a maintained peer must leave a route — this is how the app actually connects");
        let profile = TcpProfileData::from_entity(&entity).expect("upstream must decode it");
        assert_eq!(profile.endpoint_url, addr_b);

        // (2) An address nothing answers publishes nothing. `ghost` is a valid
        // identity-form PeerID that was never bound on this registry, so the
        // dial cannot succeed and `maintain-peer` cannot return 200.
        let ghost = entity_crypto::Keypair::generate().peer_id().to_string();
        let ghost_addr = format!("memory://{ghost}");
        let res = peers_a.maintain_peer(&pid_a, &ghost, &ghost_addr).await;
        let status = res.as_ref().map(|r| r.status).unwrap_or(0);
        assert_ne!(
            status, 200,
            "the premise of this half is that an unreachable peer does not \
             return 200 — if it does, the gate below proves nothing"
        );
        assert!(
            peers_a.get_entity(&pid_a, &profile_path(&ghost)).is_none(),
            "an address that never connected must NOT become a route — the \
             ladder would spend a dial timeout on it at every later dispatch"
        );

        // (3) The OTHER fact this seam owes, and the one that was missing.
        //
        // The route is what the dispatch ladder reads. The connections registry
        // is what every human-facing picker reads, and it used to be written
        // only on the manual paths — so inside Tori the app auto-connected to
        // its own backend over `maintain-peer`, the kernel said `connected`,
        // and Chat + File Transfer both showed an empty device list and told
        // the user to go connect to a peer they were already connected to.
        //
        // Asserted here rather than in a new test precisely because it is the
        // same seam with the same 200 gate: a fact published beside another
        // fact should be gated by the same evidence, and keeping them in one
        // test makes a future edit that splits them visible.
        let remembered = crate::connections::read_connections(&peers_a);
        assert!(
            remembered.iter().any(|p| p.remote_pid == pid_b),
            "a maintained peer must be REMEMBERED, not just routed — this is \
             the row Chat's 'start a chat with' and File Transfer's device list \
             read, and an auto-connected peer that is missing from it is \
             invisible while being connected. Got: {:?}",
            remembered.iter().map(|p| &p.remote_pid).collect::<Vec<_>>()
        );
        assert!(
            !remembered.iter().any(|p| p.remote_pid == ghost),
            "the 200 gate binds this fact too: a peer we never reached must not \
             appear in a picker as somewhere you can send a file"
        );
    }

    /// **Forget must forget the route, not just the row.**
    ///
    /// Publishing profiles created this hazard: before it, forgetting a peer
    /// dropped a registry row and a dial marker and nothing durable survived.
    /// Now a forgotten peer would keep a durable address inside the kernel's
    /// own tree, and the ladder would go on dialing someone the user
    /// dismissed — a surface saying "gone" over a durable store saying "here",
    /// which is the `connection_health` disease wearing new clothes.
    ///
    /// Asserted at the level that matters. Not "the entity is absent" — that
    /// is a path assertion, and the sibling proof already showed a path
    /// assertion can pass while the behaviour is broken. Instead: after
    /// forgetting, **losing the pool must once again cost reachability**. The
    /// same probe that succeeds while the route exists must fail once it is
    /// gone, which is the only statement a user would recognise.
    ///
    /// **Mutation check:** drop the `forget_routes_to` call and the final probe
    /// succeeds — the route outlived the forget.
    #[tokio::test]
    async fn forgetting_a_peer_drops_the_route_the_ladder_would_have_dialed() {
        let registry = MemoryTransportRegistry::new();
        let (peers_a, pid_a, handle_a) = spawn_peer_on_registry(registry.clone());
        let (_peers_b, pid_b, handle_b) = spawn_peer_on_registry(registry.clone());
        tokio::task::yield_now().await;

        let addr_b = format!("memory://{pid_b}");
        peers_a
            .connect_peer(&pid_a, addr_b.clone())
            .await
            .expect("connect_peer must succeed");

        let shared_a = peers_a.direct_peer_shared(&pid_a).expect("A shared");

        // Baseline: with the route published, losing the pool is survivable.
        // Without this the final assertion could pass for the wrong reason —
        // a probe that never worked proves nothing about forgetting.
        shared_a.remote.remove(&pid_b);
        assert!(
            cross_peer_probe(&peers_a, &pid_a, &pid_b).await,
            "the published route must carry a dispatch across a lost pool — \
             otherwise this test's premise is false"
        );

        // Forget, then lose the pool again. Now there is nothing to resolve.
        let swept = peers_a.forget_routes_to(&pid_b);
        assert!(swept >= 1, "the sweep must cover at least the local peer");
        shared_a.remote.remove(&pid_b);
        assert!(
            !cross_peer_probe(&peers_a, &pid_a, &pid_b).await,
            "a forgotten peer must not still be reachable from a durable route \
             — Forget dropped the row the user sees while the kernel kept the \
             address it dials"
        );

        handle_a.abort();
        handle_b.abort();
    }

    /// **The real-transport liveness proof** — the Piece B gate the design named
    /// (`DESIGN-CONNECTIVITY-UX §6`, Piece A: "the first e2e that doesn't exist
    /// today: connect → kill peer → assert the row flips to `disconnected`
    /// reactively"). Piece A's unit tests proved the read-model decodes a
    /// *hand-seeded* status entity and that a subscription wakes; this closes the
    /// gap they left — the full loop over a **real connection** through the app's
    /// `Peers` router:
    ///
    ///   1. A real handshake makes the kernel write `system/peer/status/{B}` =
    ///      `connected` on A's tree at establish (Amendment 12 §A3, dialer side,
    ///      `remote::adopt_transport_connection`) — unconditional, no
    ///      `maintain-peer`. The app read-model reads that real write back.
    ///   2. B dies; A's next dispatch over the now-dead pooled connection fails at
    ///      the send seam, and the kernel demotes B off `connected`
    ///      (transport-error → `suspect`, Amendment 12 §5). The read-model
    ///      surfaces the drop — the reactive liveness the app never observed
    ///      before (the old connection-health mirror only ever *guessed* from
    ///      connect attempts, so it lied "Connected" through exactly this drop —
    ///      `BUGLOG-2026-07-14`).
    #[tokio::test]
    async fn read_model_sees_a_real_connection_and_its_drop() {
        use crate::peer_liveness::{liveness_of, read_peer_liveness, LiveStatus};

        let registry = MemoryTransportRegistry::new();
        let (peers_a, pid_a, handle_a) = spawn_peer_on_registry(registry.clone());
        // Hold B's `Peers` alive but abortable: killing only the server task (not
        // the whole peer) is the honest "backend went dark mid-session" shape.
        let (_peers_b, pid_b, handle_b) = spawn_peer_on_registry(registry.clone());
        tokio::task::yield_now().await;

        // 1. Real handshake through the app's own connect — the shipped path a
        //    user drives via ConnectPeer. This is what must produce the kernel
        //    liveness surface; today it does NOT (the bypass this test guards).
        let connect_fut = peers_a.connect_peer(&pid_a, format!("memory://{pid_b}"));
        let remote_pid = tokio::time::timeout(Duration::from_secs(2), connect_fut)
            .await
            .expect("connect_peer timed out")
            .expect("connect_peer must succeed");
        assert_eq!(remote_pid, pid_b);

        // 2. The read-model reads the kernel's REAL `connected` write — a real
        //    connection, not a seeded entity (the coverage that didn't exist).
        let mut saw_connected = false;
        for _ in 0..50 {
            if liveness_of(&peers_a, &pid_b) == LiveStatus::Connected {
                saw_connected = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            saw_connected,
            "read-model must see the kernel's `connected` write after a real handshake"
        );
        let rows = read_peer_liveness(&peers_a, &pid_a);
        assert!(
            rows.iter().any(|r| r.remote_pid == pid_b && r.status == LiveStatus::Connected),
            "the connected row keys by B's Base58 peer_id (the entity-body join key), \
             read straight off the kernel surface: {rows:?}"
        );

        // 3. B goes dark — abort its server task. The memory-transport channel to
        //    A's pooled connection closes.
        handle_b.abort();

        // 4. Dispatch A→B over the dead pooled connection. Raw `Peers::execute`
        //    (NOT `ops::execute`, which would evict+reconnect and mask the drop) so
        //    the transport failure is the one signal. The send-seam failure demotes
        //    B in the kernel.
        let empty =
            entity_entity::Entity::new("system/empty", entity_ecf::to_ecf(&entity_ecf::Value::Null))
                .unwrap();
        let dispatch = peers_a.execute(
            &pid_a,
            format!("entity://{pid_b}/system/tree"),
            "get".into(),
            empty,
            entity_handler::ExecuteOptions::default(),
        );
        // The dispatch is expected to fail (dead conn) — we assert on the liveness
        // side effect, not the dispatch result.
        let _ = tokio::time::timeout(Duration::from_secs(2), dispatch).await;

        // 5. The read-model reactively flips OFF `connected` — the disconnect the
        //    app never wrote itself.
        let mut flipped_off_connected = false;
        for _ in 0..100 {
            if liveness_of(&peers_a, &pid_b) != LiveStatus::Connected {
                flipped_off_connected = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            flipped_off_connected,
            "a real transport drop must demote B off Connected in the read-model \
             (final: {:?})",
            liveness_of(&peers_a, &pid_b)
        );

        handle_a.abort();
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

    /// The connect-refresh footgun behind `BUGLOG-2026-07-14` B3/B4/B5 is now
    /// fixed **at the connect primitive**. `connect_peer` used to end in
    /// `remote.insert` (insert-if-absent), so a repeat Connect to an
    /// already-pooled peer kept the existing (possibly dead) connection while
    /// the UI reported success — Tori's durable-identity backend, after a
    /// restart, left a stale conn under that id that Connect could never
    /// replace. Routing `direct_connect_future` through the kernel's
    /// `connect_and_pool` (which `rebind_endpoint`s — "explicit dial ⇒ this
    /// fresh connection becomes the binding unconditionally") means a repeat
    /// Connect now establishes a genuinely NEW connection. `reconnect_peer`
    /// (evict + re-dial) still works and is still the explicit refresh verb.
    #[tokio::test]
    async fn connect_peer_refreshes_the_pooled_connection() {
        let registry = MemoryTransportRegistry::new();
        let (peers_a, pid_a, _ha) = spawn_peer_on_registry(registry.clone());
        let (_peers_b, pid_b, _hb) = spawn_peer_on_registry(registry.clone());
        tokio::task::yield_now().await;
        let addr = format!("memory://{pid_b}");

        // First Connect — establishes conn1, pooled under pid_b.
        peers_a.connect_peer(&pid_a, addr.clone()).await.expect("connect 1");
        let shared_a = peers_a.direct_peer_shared(&pid_a).unwrap();
        let e1 = shared_a.remote.get(&pid_b).expect("pooled after connect 1");

        // Second Connect to the SAME peer — THE FIX: connect_and_pool rebinds, so
        // the pooled connection is a genuinely NEW object (a stale conn1 is
        // replaced, not kept — B3/B4/B5 closed at the primitive).
        peers_a.connect_peer(&pid_a, addr.clone()).await.expect("connect 2");
        let e2 = shared_a.remote.get(&pid_b).expect("pooled after connect 2");
        assert!(
            !std::sync::Arc::ptr_eq(&e1, &e2),
            "connect_peer now refreshes the pooled connection (rebind, not \
             insert-if-absent) — B3/B4/B5 root cause fixed"
        );

        // reconnect_peer — still evict + re-dial → also a genuinely NEW conn.
        peers_a
            .reconnect_peer(&pid_a, &pid_b, addr.clone())
            .await
            .expect("reconnect");
        let e3 = shared_a.remote.get(&pid_b).expect("pooled after reconnect");
        assert!(
            !std::sync::Arc::ptr_eq(&e2, &e3),
            "reconnect_peer MUST replace the pooled connection"
        );
    }

    /// `start_engines` is a **per-peer** router op, not a boot one-off for the
    /// system peer. The engines and the connection pool both hang off the
    /// peer's own `PeerShared`, so every local peer that must answer
    /// `maintain-peer` on `/{peer}/system/network` — or deliver its tree changes
    /// to a remote subscriber — needs its own call. This pins the three outcomes
    /// the frame sweep branches on; collapsing them back to a bool is what makes
    /// the sweep either re-scan forever or skip a still-building peer.
    #[tokio::test]
    async fn start_engines_is_per_peer_and_settles_exactly_once() {
        let registry = MemoryTransportRegistry::new();
        // Two independent local peers, each its own Direct router.
        let (peers_a, pid_a, _ha) = spawn_peer_on_registry(registry.clone());
        let (peers_b, pid_b, _hb) = spawn_peer_on_registry(registry.clone());

        // Each router starts ITS OWN peer's engines — A's start says nothing
        // about B (the per-peer point; a shared/global start would let this
        // pass while `maintain-peer` on B still 500s "not bound").
        assert_eq!(peers_a.start_engines(&pid_a), EnginesStart::Started);
        assert_eq!(peers_b.start_engines(&pid_b), EnginesStart::Started);

        // Idempotent: the kernel guards on an atomic, so a repeat call is a
        // no-op that still reports Started — the sweep may legitimately race a
        // re-registration and must not panic or double-start the engine (the
        // wasm subscription engine panics on a genuine double `start()`).
        assert_eq!(peers_a.start_engines(&pid_a), EnginesStart::Started);

        // A peer this router does not host is Unknown — NOT settled, because it
        // may simply not be registered yet (a durable idb peer lands frames
        // after boot). The sweep retries these.
        assert_eq!(peers_a.start_engines(&pid_b), EnginesStart::Unknown);
        assert_eq!(
            peers_a.start_engines("2Kdefinitelynotapeer"),
            EnginesStart::Unknown
        );
    }

    /// Spawn a server for `pid` on `registry`, returning the task handle.
    /// Aborting the handle drops the `MemoryListener`, which **unregisters the
    /// endpoint** — so the peer stops answering AND new dials to it fail with a
    /// clean `ConnectError`. That is the whole "the remote disappeared"
    /// simulation, and re-calling this brings it back.
    fn serve_peer(
        pid: &str,
        shared: std::sync::Arc<entity_peer::PeerShared>,
        registry: std::sync::Arc<MemoryTransportRegistry>,
    ) -> tokio::task::JoinHandle<()> {
        let listener = MemoryListener::bind(pid.to_string(), registry).expect("bind listener");
        tokio::spawn(async move {
            let _ = entity_peer::server::run(listener, shared).await;
        })
    }

    /// Build a `maintain-peer` request with a **fast** backoff so the retry loop
    /// is observable inside a test's patience. The shipped app omits `backoff`
    /// and takes the §2.2 defaults (1s, exponential); only the pacing differs.
    fn fast_maintain_request(peer_id: &str, address: &str) -> Entity {
        let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![
            (entity_ecf::text("peer_id"), entity_ecf::text(peer_id)),
            (entity_ecf::text("address"), entity_ecf::text(address)),
            (
                entity_ecf::text("backoff"),
                entity_ecf::Value::Map(vec![
                    (entity_ecf::text("min_ms"), entity_ecf::integer(100)),
                    (entity_ecf::text("max_ms"), entity_ecf::integer(200)),
                    (
                        entity_ecf::text("strategy"),
                        entity_ecf::text("constant"),
                    ),
                ]),
            ),
        ]));
        Entity::new(entity_network::TYPE_MAINTAIN_REQUEST, data)
            .expect("maintain-request entity construction is infallible")
    }

    /// The cross-peer read a bound conversation lives on — `ChatDelivery` runs
    /// this shape against each remote every poll. `true` iff it reached the
    /// remote, so it doubles as "is the conversation still working?".
    async fn cross_peer_probe(peers: &Peers, local: &str, remote: &str) -> bool {
        let params = Entity::new("system/empty", entity_ecf::to_ecf(&entity_ecf::Value::Null))
            .expect("empty params");
        matches!(
            peers
                .execute(
                    local,
                    format!("entity://{remote}/system/tree"),
                    "get".to_string(),
                    params,
                    entity_handler::ExecuteOptions::default(),
                )
                .await,
            Ok(r) if r.status < 400
        )
    }

    /// Poll `f` until it holds or `budget` elapses. Returns whether it held.
    async fn eventually(budget: std::time::Duration, mut f: impl FnMut() -> bool) -> bool {
        let deadline = std::time::Instant::now() + budget;
        loop {
            if f() {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// **THE RECONNECT PROOF** — the validation gap the Piece C review named.
    ///
    /// Both landed steps claim that a `maintain-peer`'d relationship *survives a
    /// drop without the app re-dialing*. Nothing proved it: the e2e is Worker-arm
    /// (where the app deliberately skips maintain), and the Tauri session has a
    /// single local peer. This is two real peers over a real (in-process)
    /// transport, and it asserts the claim end to end:
    ///
    ///   1. A maintains B → B is `connected` in **the app's own read-model**
    ///      (`peer_liveness`, the surface every window renders).
    ///   2. B disappears — its listener is unregistered, so it neither answers
    ///      nor accepts new dials.
    ///   3. A's read-model stops saying `connected` (the stale-"Connected" lie
    ///      this whole arc exists to kill).
    ///   4. **The retry loop runs with zero app involvement** — dials keep
    ///      arriving at the registry although nothing in the app re-dials. This
    ///      is the actual hand-off: the extension owns reconnect now.
    ///   5. B comes back → A is `connected` again, again with no app action.
    ///
    /// What this does NOT cover: the wasm frame sweep that picks the pairs
    /// (`sync_maintained_peers`, unit-tested separately) and a real WebSocket.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_maintained_peer_reconnects_itself_after_the_remote_disappears() {
        use crate::peer_liveness::{liveness_of, LiveStatus};

        let registry = MemoryTransportRegistry::new();

        // A — the maintaining side.
        let peers_a =
            Peers::new_direct_with_connector(std::sync::Arc::new(MemoryConnector::new(registry.clone())));
        let pid_a = peers_a.primary_peer_id().to_string();
        let shared_a = peers_a.direct_peer_shared(&pid_a).expect("A shared");
        assert_eq!(
            peers_a.start_engines(&pid_a),
            EnginesStart::Started,
            "the network handler's PeerLink binds here — without it maintain-peer 500s"
        );
        let _srv_a = serve_peer(&pid_a, shared_a.clone(), registry.clone());

        // B — the peer that will vanish and return.
        let peers_b =
            Peers::new_direct_with_connector(std::sync::Arc::new(MemoryConnector::new(registry.clone())));
        let pid_b = peers_b.primary_peer_id().to_string();
        let shared_b = peers_b.direct_peer_shared(&pid_b).expect("B shared");
        assert_eq!(peers_b.start_engines(&pid_b), EnginesStart::Started);
        let mut srv_b = serve_peer(&pid_b, shared_b.clone(), registry.clone());
        tokio::task::yield_now().await;

        let addr_b = format!("memory://{pid_b}");

        // (1) Maintain B from A — exactly what `sync_maintained_peers` EXECUTEs
        // for a bound conversation, on A's OWN system/network (per-peer pool).
        let res = peers_a
            .execute(
                &pid_a,
                format!("/{pid_a}/system/network"),
                "maintain-peer".to_string(),
                fast_maintain_request(&pid_b, &addr_b),
                entity_handler::ExecuteOptions::default(),
            )
            .await
            .expect("maintain-peer dispatch");
        assert_eq!(
            res.status, 200,
            "maintain-peer must establish + install the reconnect graph"
        );
        assert!(
            eventually(Duration::from_secs(3), || liveness_of(&peers_a, &pid_b)
                == LiveStatus::Connected)
                .await,
            "the app read-model must show B connected after maintain-peer"
        );

        // (2) B disappears. Aborting the server drops its listener, which
        // unregisters the endpoint: B now neither answers nor accepts dials.
        srv_b.abort();
        let endpoints_before = registry.len();
        assert!(
            eventually(Duration::from_secs(2), || registry.len() < endpoints_before).await,
            "B's endpoint must leave the registry once its server is aborted \
             (it was {endpoints_before}) — otherwise the 'remote is gone' \
             premise of this test is false and the rest proves nothing"
        );

        // Provoke the drop detection: one dispatch over the dead connection.
        // The transport error demotes B (§3.13) — the same path a real chat
        // fetch takes when the other side goes away. Without this we would be
        // waiting ~60s on keepalive.
        //
        // This is also the *conversation's* symptom: a cross-peer fetch to B is
        // exactly what `ChatDelivery` does every poll, so its failure here is
        // "the conversation stopped delivering" in miniature.
        let during_outage = cross_peer_probe(&peers_a, &pid_a, &pid_b).await;
        assert!(
            !during_outage,
            "a fetch to a vanished peer cannot succeed — the outage is not real"
        );

        // (3) The read-model must stop claiming `connected` — the stale
        // "Connected" lie is exactly what this arc deleted the mirror to fix.
        assert!(
            eventually(Duration::from_secs(5), || liveness_of(&peers_a, &pid_b)
                != LiveStatus::Connected)
                .await,
            "a dead peer must not still read as Connected"
        );

        // (4) THE HAND-OFF: dials keep arriving although nothing in the app
        // re-dials. `dial_count` is the retry loop's observable from outside the
        // peer — the app-tier loop that used to do this is deleted.
        let dials_before = registry.dial_count();
        tokio::time::sleep(Duration::from_millis(800)).await;
        let dials_after = registry.dial_count();
        assert!(
            dials_after > dials_before,
            "the network extension must keep retrying on its own \
             (dials {dials_before} → {dials_after}); nothing in the app re-dials"
        );

        // (5) B comes back — and A reconnects with no app action whatsoever.
        srv_b = serve_peer(&pid_b, shared_b.clone(), registry.clone());
        assert!(
            eventually(Duration::from_secs(10), || liveness_of(&peers_a, &pid_b)
                == LiveStatus::Connected)
                .await,
            "once B is reachable again the maintained relationship must heal itself"
        );

        // (6) …and the conversation's own traffic flows again. A healed status
        // entity would be worth little if the fetches a chat actually makes
        // still failed, so assert the thing the user cares about, not just the
        // chip: the same cross-peer probe that failed mid-outage now succeeds.
        assert!(
            cross_peer_probe(&peers_a, &pid_a, &pid_b).await,
            "after the heal, the cross-peer fetch a bound conversation depends on \
             must work again"
        );

        srv_b.abort();
    }

    /// Build a `release-peer` request. `reason` is load-bearing, not a label
    /// (EXTENSION-NETWORK §4.2): `"shutdown"` evicts the connection and writes
    /// the terminal status; anything else drops the maintain machinery and
    /// leaves the connection up. The app closes a window with `"idle"`, so
    /// that is what this test drives.
    fn release_request(peer_id: &str, reason: &str) -> Entity {
        let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![
            (entity_ecf::text("peer_id"), entity_ecf::text(peer_id)),
            (entity_ecf::text("reason"), entity_ecf::text(reason)),
        ]));
        Entity::new(entity_network::TYPE_RELEASE_REQUEST, data)
            .expect("release-request entity construction is infallible")
    }

    /// **THE RELEASE PROOF** — the counterpart to the reconnect proof above,
    /// and the gap the Piece C handoff called "the weakest point in the arc":
    /// `release_candidates` (which pairs to drop) was unit-tested, but nothing
    /// had ever observed an actual `release-peer` round-trip. Both halves of
    /// the decision that carries the teardown rested on having *read* the
    /// handler. This asserts them.
    ///
    ///   1. A maintains B → connected, reconnect graph installed.
    ///   2. A releases B with reason **`idle`** — what closing a chat window
    ///      sends. The connection **stays up**: the cross-peer fetch a
    ///      conversation depends on still works, and the read-model still says
    ///      `connected` (truthfully — nothing was evicted). This is why the app
    ///      uses `idle` and not `shutdown`: closing a chat means "stop
    ///      auto-reconnecting", not "hang up on a peer the user may still be
    ///      using from Peer Connections or File Transfer".
    ///   3. B then disappears — and **no dials follow**. The retry loop the
    ///      reconnect proof watched climb is genuinely gone, not merely
    ///      answered with a 200.
    ///
    /// Step 3 is what makes this mutation-checkable: delete the release and
    /// this becomes exactly the reconnect proof's sequence, where dials *do*
    /// climb — so the assertion fails. Keep it that way.
    ///
    /// What this does NOT cover: the wasm frame sweep that decides to release
    /// (`release_unbound_peers`, unit-tested via `release_candidates`) and a
    /// real WebSocket.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn releasing_a_conversation_stops_the_retries_but_leaves_the_connection_up() {
        use crate::peer_liveness::{liveness_of, LiveStatus};

        let registry = MemoryTransportRegistry::new();

        // A — the maintaining, then releasing, side.
        let peers_a = Peers::new_direct_with_connector(std::sync::Arc::new(MemoryConnector::new(
            registry.clone(),
        )));
        let pid_a = peers_a.primary_peer_id().to_string();
        let shared_a = peers_a.direct_peer_shared(&pid_a).expect("A shared");
        assert_eq!(
            peers_a.start_engines(&pid_a),
            EnginesStart::Started,
            "the network handler's PeerLink binds here — without it both \
             maintain-peer and release-peer 500 with 'not bound'"
        );
        let _srv_a = serve_peer(&pid_a, shared_a.clone(), registry.clone());

        // B — the conversation partner, released and then vanished.
        let peers_b = Peers::new_direct_with_connector(std::sync::Arc::new(MemoryConnector::new(
            registry.clone(),
        )));
        let pid_b = peers_b.primary_peer_id().to_string();
        let shared_b = peers_b.direct_peer_shared(&pid_b).expect("B shared");
        assert_eq!(peers_b.start_engines(&pid_b), EnginesStart::Started);
        // Not reassigned (unlike the reconnect proof, B never comes back here —
        // the point is that nothing re-dials it).
        let srv_b = serve_peer(&pid_b, shared_b.clone(), registry.clone());
        tokio::task::yield_now().await;

        let addr_b = format!("memory://{pid_b}");

        // (1) Bind the conversation: exactly what `sync_maintained_peers`
        // EXECUTEs for a window that binds B.
        let res = peers_a
            .execute(
                &pid_a,
                format!("/{pid_a}/system/network"),
                "maintain-peer".to_string(),
                fast_maintain_request(&pid_b, &addr_b),
                entity_handler::ExecuteOptions::default(),
            )
            .await
            .expect("maintain-peer dispatch");
        assert_eq!(res.status, 200, "maintain-peer must install the reconnect graph");
        assert!(
            eventually(Duration::from_secs(3), || liveness_of(&peers_a, &pid_b)
                == LiveStatus::Connected)
                .await,
            "the app read-model must show B connected after maintain-peer"
        );

        // (2) The window closes. `release_unbound_peers` EXECUTEs this on A's
        // OWN system/network — reason `idle`, never `shutdown`.
        let res = peers_a
            .execute(
                &pid_a,
                format!("/{pid_a}/system/network"),
                "release-peer".to_string(),
                release_request(&pid_b, "idle"),
                entity_handler::ExecuteOptions::default(),
            )
            .await
            .expect("release-peer dispatch");
        assert_eq!(
            res.status, 200,
            "release-peer must succeed — on anything else the app deliberately \
             leaves the relationship maintained rather than half-torn-down"
        );

        // The connection SURVIVES. If `idle` were ever treated like `shutdown`
        // upstream, closing one chat window would hang up a peer the user is
        // still using elsewhere — and this is the assertion that would catch
        // it. Asserted on the traffic, not just the chip: a status entity that
        // still says `connected` would be worth nothing if the fetches a
        // conversation makes had started failing.
        assert!(
            cross_peer_probe(&peers_a, &pid_a, &pid_b).await,
            "release-peer(idle) must LEAVE the connection up — the cross-peer \
             fetch must still reach B after the release"
        );
        assert_eq!(
            crate::peer_liveness::liveness_of(&peers_a, &pid_b),
            crate::peer_liveness::LiveStatus::Connected,
            "release-peer(idle) writes no terminal status and evicts nothing, \
             so the read-model must still (truthfully) say connected"
        );

        // (3) Now B disappears — the same way the reconnect proof kills it.
        srv_b.abort();
        let endpoints_before = registry.len();
        assert!(
            eventually(Duration::from_secs(2), || registry.len() < endpoints_before).await,
            "B's endpoint must leave the registry once its server is aborted \
             (it was {endpoints_before}) — otherwise the 'remote is gone' \
             premise of this test is false and the rest proves nothing"
        );

        // Provoke drop detection with one dispatch over the dead connection —
        // the same probe the reconnect proof uses. This is load-bearing for the
        // mutation check: without it nothing would dial even in the
        // still-maintained case, and the assertion below could not tell the two
        // apart.
        assert!(
            !cross_peer_probe(&peers_a, &pid_a, &pid_b).await,
            "a fetch to a vanished peer cannot succeed — the outage is not real"
        );

        // THE RELEASE: no retry loop follows. In the reconnect proof this same
        // window showed dials climbing; here it must be flat, because
        // `release-peer` dropped the session that owned them.
        tokio::time::sleep(Duration::from_millis(300)).await; // let anything in flight settle
        let dials_before = registry.dial_count();
        tokio::time::sleep(Duration::from_millis(800)).await;
        let dials_after = registry.dial_count();
        assert_eq!(
            dials_after, dials_before,
            "a RELEASED relationship must not be retried — dials went \
             {dials_before} → {dials_after}. The extension is still \
             reconnecting a conversation no window binds any more."
        );
    }

    /// **A peer we actually reached becomes a peer the UI can see.**
    ///
    /// The registry (`connections.rs`) means "we have connected at least once",
    /// and until the reach keeper landed, only the manual **Connect** button
    /// ever wrote it. So a peer met by NAME and reached over WebRTC existed
    /// nowhere any window looks — the File Transfer target list reads exactly
    /// this registry, and it was empty for the one peer the entire rendezvous
    /// path produces. The bytes could cross while the UI insisted there was
    /// nobody to send them to.
    ///
    /// Asserted against a REAL kernel status rather than a stubbed one: A dials
    /// B over the memory transport, so `liveness_of` reads `Connected` because
    /// the kernel wrote it, which is the same signal the keeper reads in the
    /// browser.
    ///
    /// **Mutation check:** drop the `remember` branch in `ReachKeeper::due` and
    /// the row never appears (verified). Note what this does NOT cover: that
    /// the row is written only ONCE per connect — that guard is a `bool` on the
    /// intent, and a test that pumps twice would pass either way, since the
    /// write is idempotent on the path.
    #[tokio::test]
    async fn a_peer_the_keeper_reached_lands_in_the_ever_connected_registry() {
        let registry = MemoryTransportRegistry::new();
        let (peers_a, pid_a, handle_a) = spawn_peer_on_registry(registry.clone());
        let (_peers_b, pid_b, handle_b) = spawn_peer_on_registry(registry.clone());
        tokio::task::yield_now().await;

        let keeper = crate::reach_keeper::ReachKeeper::new();
        keeper.want(&pid_a, &pid_b);
        assert!(
            !crate::connections::read_connections(&peers_a)
                .iter()
                .any(|c| c.remote_pid == pid_b),
            "the premise: an unreached peer is in no registry"
        );

        let connect = peers_a.connect_peer(&pid_a, format!("memory://{pid_b}"));
        tokio::time::timeout(Duration::from_secs(2), connect)
            .await
            .expect("connect timed out")
            .expect("A connects to B");
        assert_eq!(
            crate::peer_liveness::liveness_of(&peers_a, &pid_b),
            crate::peer_liveness::LiveStatus::Connected,
            "the kernel must have written the status this turns on"
        );

        keeper.pump(&peers_a);
        assert!(
            eventually(Duration::from_secs(2), || crate::connections::read_connections(&peers_a)
                .iter()
                .any(|c| c.remote_pid == pid_b))
            .await,
            "a peer we reached must become a peer the UI can offer as a target"
        );

        handle_a.abort();
        handle_b.abort();
    }

    /// **THE TRANSFER PROOF** — one peer offers a file, another pulls it back
    /// byte for byte over a real connection, with **no `local/files` anywhere**.
    ///
    /// This is the claim `file_offer` exists to make: the serving side of a
    /// transfer is not native-only. Until now every transfer op targeted
    /// `entity://{peer}/local/files`, whose handler is
    /// `#![cfg(not(target_arch = "wasm32"))]` and mounted by the Tauri backend —
    /// so two browsers had nobody to receive. The counterpart here is
    /// `system/content` + an offer manifest in our own app namespace, both of
    /// which a browser peer has.
    ///
    /// What it asserts, in the order that matters:
    ///
    /// 1. **A can serve.** `offer_file` ingests and publishes with only
    ///    dispatched (L1) calls — no `PeerContext`, no `ContentStore` off the
    ///    peer — which is what makes the same path work on the Worker arm.
    /// 2. **B can discover.** `list_offers` reads A's offers prefix across the
    ///    connection and decodes a name and a size for a hash.
    /// 3. **B can pull, across the frame budget.** The payload is deliberately
    ///    **multi-chunk**, so `pull_offer` must walk the closure — blob first,
    ///    then its chunks — rather than getting lucky with a single self-
    ///    contained response. That walk is the piece `ops::download` names as
    ///    its follow-up and the reason a "small file only" transfer is not the
    ///    goal.
    /// 4. **The bytes are the bytes.** Compared in full, not by length: content
    ///    addressing means a wrong chunk cannot reassemble, and asserting the
    ///    length alone would not notice if it could.
    ///
    /// **Mutation check:** cap `pull_offer` at the first `fetch_into` (skip the
    /// chunk loop) and step 4 fails with the blob present and no chunks; point
    /// `list_offers` at our own prefix instead of the remote's and step 2 goes
    /// empty. Both were run.
    ///
    /// **Not covered here:** wasm (both arms are Direct in this harness), a
    /// WebRTC data channel (the transport is deliberately immaterial — nothing
    /// in `file_offer` names one, and the two-browser gate is what proves that
    /// claim), grants (`debug_open_grants` posture, so this says nothing about
    /// what a *stranger* may pull), and the UI.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_file_offered_by_one_peer_is_pulled_byte_for_byte_by_another() {
        use crate::file_offer;

        let registry = MemoryTransportRegistry::new();
        let (peers_a, pid_a, handle_a) = spawn_peer_on_registry(registry.clone());
        let (peers_b, pid_b, handle_b) = spawn_peer_on_registry(registry.clone());
        tokio::task::yield_now().await;

        // B dials A — the puller holds the connection, mirroring a receiver who
        // was handed an id by a `meet`.
        let connect = peers_b.connect_peer(&pid_b, format!("memory://{pid_a}"));
        tokio::time::timeout(Duration::from_secs(2), connect)
            .await
            .expect("connect timed out")
            .expect("B connects to A");

        // Two full chunks and a tail: enough that the closure walk is exercised
        // and a single-response shortcut cannot pass.
        let raw: Vec<u8> = (0..(file_offer::CHUNK_SIZE * 2 + 129))
            .map(|i| (i.wrapping_mul(31) % 251) as u8)
            .collect();

        // The handles are what a spawned UI action would hold: owned, so the
        // flows keep dispatching after their first await.
        let dispatch_a = peers_a.dispatch_handle(&pid_a).expect("A dispatch handle");
        let dispatch_b = peers_b.dispatch_handle(&pid_b).expect("B dispatch handle");

        let offer = file_offer::offer_file(&dispatch_a, "report.bin", &raw)
            .await
            .expect("A must be able to offer a file with no local/files handler");

        let listed = file_offer::list_offers(&dispatch_b, &pid_a)
            .await
            .expect("B lists A's offers over the connection");
        assert_eq!(listed, vec![offer.clone()], "exactly what A offered");
        assert_eq!(listed[0].name, "report.bin", "a hash gets a filename");
        assert_eq!(listed[0].size, raw.len() as u64);
        assert_eq!(listed[0].from, pid_a, "the manifest remembers its source");

        // Progress is reported per batch, and the first report lands as soon as
        // the blob names its chunks — that is what lets a caller state the size
        // of the job before waiting for it. Recorded rather than asserted
        // step-by-step (a batch size is upstream's to change); what must hold is
        // that it starts at "none of them" and finishes at "all of them".
        let seen = std::sync::Mutex::new(Vec::new());
        let pulled = file_offer::pull_offer_with(&dispatch_b, &pid_a, &listed[0].blob, |h, t| {
            seen.lock().unwrap().push((h, t))
        })
        .await
        .expect("B pulls the closure");
        assert_eq!(pulled, raw, "the file must arrive byte for byte");
        let seen = seen.into_inner().unwrap();
        assert_eq!(seen.first(), Some(&(0, 3)), "the job's size is known up front");
        assert_eq!(
            seen.last(),
            Some(&(3, 3)),
            "progress must finish at every chunk held, got {seen:?}"
        );

        // A blob nobody offered must fail loudly rather than returning short
        // bytes — the receiver's only defence against a stale listing.
        let (ghost, _) = file_offer::chunk_bytes(b"never offered").unwrap();
        assert!(
            file_offer::pull_offer(&dispatch_b, &pid_a, &ghost.content_hash)
                .await
                .is_err(),
            "pulling content the peer does not hold must error, not truncate"
        );

        // 5. **A can see what it is serving.** The window renders this list, and
        //    it is the only place a person learns what strangers may read from
        //    them — a local read of our own tree, not the remote `list_offers`
        //    shape pointed at ourselves.
        let own = file_offer::read_own_offers(&peers_a, &pid_a);
        assert_eq!(own, vec![offer.clone()], "A lists exactly what it offered");
        assert!(
            file_offer::read_own_offers(&peers_b, &pid_b).is_empty(),
            "B offered nothing and must say so"
        );

        // 6. **A stated ceiling is refused at the door**, before anything is
        //    allocated or dispatched — the alternative is a tab that dies
        //    partway through an ingest nobody can see.
        let too_big = vec![7u8; file_offer::MAX_OFFER_BYTES as usize + 1];
        let refusal = file_offer::offer_file(&dispatch_a, "huge.bin", &too_big)
            .await
            .expect_err("a file over the ceiling must be refused, not attempted");
        assert!(refusal.contains("huge.bin"), "the refusal names the file: {refusal}");
        assert_eq!(
            file_offer::read_own_offers(&peers_a, &pid_a).len(),
            1,
            "a refused offer must leave nothing behind"
        );

        // 7. **Withdrawal takes the NAME down, not the bytes** — and the button
        //    says exactly that. B can no longer discover the file; B (or anyone)
        //    who already holds the content id still can. Asserting both halves
        //    is what keeps "stop offering" from drifting into "delete".
        let writer_a = peers_a.writer_handle_for(&pid_a).expect("A writer");
        file_offer::withdraw_offer(&writer_a, &pid_a, &offer.id());
        assert!(
            file_offer::read_own_offers(&peers_a, &pid_a).is_empty(),
            "A's own list drops the withdrawn offer"
        );
        assert!(
            file_offer::list_offers(&dispatch_b, &pid_a)
                .await
                .expect("B can still ask")
                .is_empty(),
            "B must no longer be able to discover it"
        );
        assert_eq!(
            file_offer::pull_offer(&dispatch_b, &pid_a, &offer.blob)
                .await
                .expect("content stays addressable by hash"),
            raw,
            "withdrawing a listing does not unpublish the content — if this ever \
             starts failing, the surface may finally say 'delete'"
        );

        handle_a.abort();
        handle_b.abort();
    }

    /// **THE ACCOUNTING PROOF — what an offer costs, permanently.** This test
    /// exists to *measure* a gap, not to assert a behaviour we like: it is the
    /// D-accounting discipline applied to the transfer arc before the arc is
    /// called stable.
    ///
    /// Offering ingests a blob and its chunks into `system/content`, and the
    /// handler writes a §6.4.2 presence binding **per ingested entity** (the
    /// root *and* each `included`). Withdrawing an offer removes the manifest —
    /// the name — and nothing else. So each offer permanently costs its bytes in
    /// the content store plus one tree entity per chunk, and re-offering an
    /// edited file hashes differently and costs a fresh set.
    ///
    /// **Nothing can reclaim it today, and that is a fact about the surface
    /// rather than a thing we forgot to call:** `system/content` exposes exactly
    /// `get` and `ingest` (`extensions/content/src/handler.rs`) — there is no
    /// forget/unbind op — and `handle_get` serves straight from the content
    /// store by hash without consulting the binding, so removing the binding
    /// would not even stop us serving. `WriterHandle::content_remove` is
    /// Direct-arm only and refuses while a live path binds the blob, which the
    /// presence binding is.
    ///
    /// If this test starts failing because the numbers went *down*, something
    /// grew a reclaim path and the honest surface can finally say "delete".
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_offer_costs_content_and_presence_bindings_that_nothing_reclaims() {
        use crate::file_offer;

        let registry = MemoryTransportRegistry::new();
        let (peers, pid, handle) = spawn_peer_on_registry(registry.clone());
        tokio::task::yield_now().await;
        let dispatch = peers.dispatch_handle(&pid).expect("dispatch handle");

        // Three full chunks and a tail — four chunks, so "one per chunk" is a
        // count and not a coincidence of a single-chunk file.
        let raw: Vec<u8> = (0..(file_offer::CHUNK_SIZE * 3 + 11))
            .map(|i| (i.wrapping_mul(17) % 251) as u8)
            .collect();
        let offer = file_offer::offer_file(&dispatch, "big.bin", &raw)
            .await
            .expect("offer");

        let bindings_prefix = format!("{}/", file_offer::namespace_resource(&pid));
        let bound = |peers: &Peers| peers.tree_listing(&pid, &bindings_prefix).len();
        let after_offer = bound(&peers);
        assert_eq!(
            after_offer, 5,
            "a 4-chunk offer binds the blob plus every chunk (§6.4.2, written per \
             ingested entity) — got {after_offer}"
        );

        let writer = peers.writer_handle_for(&pid).expect("writer");
        file_offer::withdraw_offer(&writer, &pid, &offer.id());
        assert!(
            file_offer::read_own_offers(&peers, &pid).is_empty(),
            "the manifest is gone"
        );
        assert_eq!(
            bound(&peers),
            after_offer,
            "withdrawal reclaims NOTHING — the bindings and the bytes outlive the \
             name. This is the measurement, not a regression: `system/content` has \
             no forget op, so a peer cannot unpublish what it ingested."
        );

        handle.abort();
    }

    /// THE DELIVERY PROOF: a signed chat message authored by peer A crosses a
    /// real (memory-transport) connection and appears in peer B's §1.4 union
    /// view — end to end, using the shipped app surface (`Peers`, `ChatModel`,
    /// `subscribe_at`, cross-peer `system/tree:get`, `dispatch_write`).
    ///
    /// This is the mechanism the app's ChatDelivery will drive, proven here at
    /// the primitive level so it isn't speculation:
    ///   1. B connects to A (pooled connection).
    ///   2. B **subscribes** to A's messages prefix on A's engine
    ///      (`subscribe_at`) — notifications (path+hash) stream back over the
    ///      connection. A remote subscription does NOT replicate content.
    ///   3. A authors a message into A's OWN namespace (`ChatModel::send`).
    ///   4. On each notification B **fetches** the signed entity from A
    ///      (`execute entity://{A}/system/tree get`) and **caches** it into B's
    ///      own store at the same `/{A}/…` path (`dispatch_write`) — the §1.4
    ///      "cache the others' messages" step.
    ///   5. B's `ChatModel` union render now shows A's message as **not-mine**.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn chat_message_crosses_from_a_to_b_over_the_wire() {
        use crate::views::chat::model::{conversation_messages_prefix, ChatModel};
        use entity_capability::ResourceTarget;
        use entity_handler::ExecuteOptions;

        let registry = MemoryTransportRegistry::new();
        let (peers_a, pid_a, handle_a) = spawn_peer_on_registry(registry.clone());
        let (peers_b, pid_b, handle_b) = spawn_peer_on_registry(registry.clone());
        assert_ne!(pid_a, pid_b);
        tokio::task::yield_now().await;

        // (1) Establish the connection BOTH ways (as two mutually-connected chat
        // peers have) via the REAL app path `Peers::connect_peer` — B→A carries
        // B's subscribe + tree:get; A→B carries A's subscription-notification
        // push. connect_peer is now reentry-wired, so each dialed connection
        // serves the inbound `receive` the other side pushes back.
        peers_b
            .connect_peer(&pid_b, format!("memory://{pid_a}"))
            .await
            .expect("B connects to A");
        peers_a
            .connect_peer(&pid_a, format!("memory://{pid_b}"))
            .await
            .expect("A connects to B");

        let conv = "room-1".to_string();
        let prefix = conversation_messages_prefix(&pid_a, &conv);

        // Baseline: B's union of this room is empty before anything is delivered.
        let model_b =
            ChatModel::with_conversation(pid_b.clone(), conv.clone(), vec![pid_a.clone(), pid_b.clone()]);
        assert_eq!(
            model_b.load_messages(&peers_b).len(),
            0,
            "precondition: B has none of A's messages yet"
        );

        // (2) B subscribes to A's messages prefix on A's engine. The callback
        // just forwards each changed path; the fetch+cache runs on the test task
        // (mirrors the app's ChatDelivery driver loop).
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let sub = {
            let ctx_b = peers_b
                .direct_peer_context(&pid_b)
                .expect("B has a Direct PeerContext");
            ctx_b.subscribe_at(pid_a.clone(), format!("{prefix}*"), move |ev| {
                let _ = tx.send(ev.path);
            })
        }
        .await
        .expect("B subscribes to A's messages prefix");

        // (3) A authors a message into its own namespace.
        let model_a =
            ChatModel::with_conversation(pid_a.clone(), conv.clone(), vec![pid_a.clone(), pid_b.clone()]);
        assert!(model_a.send(&peers_a, "hello from A"));

        // Wait for the remote subscription to notify B of A's write.
        let mut paths: Vec<String> = Vec::new();
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            while let Ok(p) = rx.try_recv() {
                paths.push(p);
            }
            if !paths.is_empty() {
                break;
            }
        }
        assert!(
            !paths.is_empty(),
            "B must receive a subscription notification for A's write (delivery signal)"
        );

        // (4) For each notified path: fetch the signed entity from A over the
        // connection, then cache it into B's own store at the same path.
        for path in &paths {
            let opts = ExecuteOptions {
                resource: Some(ResourceTarget {
                    targets: vec![path.clone()],
                    exclude: vec![],
                }),
                ..Default::default()
            };
            let params =
                Entity::new("system/empty", entity_ecf::to_ecf(&entity_ecf::Value::Null)).unwrap();
            let hr = peers_b
                .execute(
                    &pid_b,
                    format!("entity://{pid_a}/system/tree"),
                    "get".to_string(),
                    params,
                    opts,
                )
                .await
                .expect("cross-peer tree:get dispatches");
            assert_eq!(hr.status, 200, "A serves the message entity (status 200)");
            // Cache A's signed entity under /{A}/… in B's own store (§1.4).
            peers_b.dispatch_write(&pid_b, path.clone(), hr.result);
        }
        // Let the cache write land.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // (5) B's union view now contains A's message, flagged not-mine.
        let out = model_b.render_output(&peers_b, &crate::dial_markers::DialMarkers::new());
        assert_eq!(out.messages.len(), 1, "exactly A's one message crossed");
        assert_eq!(out.messages[0].body, "hello from A");
        assert!(
            !out.messages[0].mine,
            "A's message is authored by A, so it renders as not-mine in B's view"
        );
        assert_eq!(out.messages[0].author, pid_a, "authored by A");

        drop(sub);
        handle_a.abort();
        handle_b.abort();
    }

    /// The same crossing, but driven by the app's `ChatDelivery` service (not
    /// hand-rolled subscribe/fetch/cache) — proves the shipped delivery
    /// component works: `subscribe` once, then `pump` each "frame" until A's
    /// message lands in B's union view.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn chat_delivery_service_delivers_a_to_b() {
        use crate::views::chat::delivery::ChatDelivery;
        use crate::views::chat::model::ChatModel;

        let registry = MemoryTransportRegistry::new();
        let (peers_a, pid_a, handle_a) = spawn_peer_on_registry(registry.clone());
        let (peers_b, pid_b, handle_b) = spawn_peer_on_registry(registry.clone());
        tokio::task::yield_now().await;

        // Both directions connected (subscribe + notify push).
        peers_b
            .connect_peer(&pid_b, format!("memory://{pid_a}"))
            .await
            .expect("B connects to A");
        peers_a
            .connect_peer(&pid_a, format!("memory://{pid_b}"))
            .await
            .expect("A connects to B");

        let conv = "room-2".to_string();
        let participants = vec![pid_a.clone(), pid_b.clone()];

        // B's delivery service subscribes to every remote participant (A).
        let mut delivery = ChatDelivery::new(pid_b.clone(), conv.clone(), participants.clone());
        delivery.subscribe(&peers_b).await;

        // A authors a message.
        let model_a =
            ChatModel::with_conversation(pid_a.clone(), conv.clone(), participants.clone());
        assert!(model_a.send(&peers_a, "delivered by the service"));

        // Drive delivery "frames": pump until B's union has the message.
        let model_b =
            ChatModel::with_conversation(pid_b.clone(), conv.clone(), participants.clone());
        let mut crossed = false;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            delivery.pump(&peers_b);
            if !model_b.load_messages(&peers_b).is_empty() {
                crossed = true;
                break;
            }
        }
        assert!(crossed, "ChatDelivery must deliver A's message into B's store");

        let out = model_b.render_output(&peers_b, &crate::dial_markers::DialMarkers::new());
        assert_eq!(out.messages.len(), 1);
        assert_eq!(out.messages[0].body, "delivered by the service");
        assert!(!out.messages[0].mine, "authored by A → not-mine in B's view");

        handle_a.abort();
        handle_b.abort();
    }

    /// The POLL path in isolation — the mechanism that carries delivery on the
    /// Worker arm (`?worker=1`) and thus over the worker-only WebRTC channel,
    /// where the reactive `subscribe_at` is unavailable. NO subscribe is called:
    /// `pump`'s throttled `tree:get`-listing is the only discovery. It routes
    /// over the connection pool via `execute`, so proving it over the memory
    /// transport is evidence it works over any transport.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn chat_delivery_poll_delivers_without_subscribe() {
        use crate::views::chat::delivery::ChatDelivery;
        use crate::views::chat::model::ChatModel;

        let registry = MemoryTransportRegistry::new();
        let (peers_a, pid_a, handle_a) = spawn_peer_on_registry(registry.clone());
        let (peers_b, pid_b, handle_b) = spawn_peer_on_registry(registry.clone());
        tokio::task::yield_now().await;
        peers_b
            .connect_peer(&pid_b, format!("memory://{pid_a}"))
            .await
            .expect("B connects to A");

        let conv = "room-poll".to_string();
        let participants = vec![pid_a.clone(), pid_b.clone()];

        let model_a =
            ChatModel::with_conversation(pid_a.clone(), conv.clone(), participants.clone());
        assert!(model_a.send(&peers_a, "polled across the wire"));

        // NO subscribe — poll only.
        let mut delivery = ChatDelivery::new(pid_b.clone(), conv.clone(), participants.clone());
        let model_b =
            ChatModel::with_conversation(pid_b.clone(), conv.clone(), participants.clone());
        let mut crossed = false;
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            delivery.pump(&peers_b);
            if !model_b.load_messages(&peers_b).is_empty() {
                crossed = true;
                break;
            }
        }
        assert!(crossed, "poll-only delivery must land A's message in B's store");
        let out = model_b.render_output(&peers_b, &crate::dial_markers::DialMarkers::new());
        assert_eq!(out.messages[0].body, "polled across the wire");
        assert!(!out.messages[0].mine);

        handle_a.abort();
        handle_b.abort();
    }

    /// THE FULL FLOW through the actual Chat WINDOW: two `ChatWindow`s on two
    /// connected peers, each bound to the well-known 1:1 conversation. A sends
    /// via the window's `ChatSend` action; B's window `tick`s its delivery each
    /// "frame"; A's message appears in B's window render — and B replies and it
    /// appears in A's. This is the end-to-end app flow (window → delivery →
    /// model → render), minus only the DOM/browser layer.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_chat_windows_exchange_messages_full_flow() {
        use crate::action::Action;
        use crate::views::chat::ChatWindow;
        use crate::window::WindowView;

        let registry = MemoryTransportRegistry::new();
        let (peers_a, pid_a, handle_a) = spawn_peer_on_registry(registry.clone());
        let (peers_b, pid_b, handle_b) = spawn_peer_on_registry(registry.clone());
        tokio::task::yield_now().await;

        // Both peers connect (each subscribes to the other; each pushes to the
        // other) — the real `connect_peer` path.
        peers_a
            .connect_peer(&pid_a, format!("memory://{pid_b}"))
            .await
            .expect("A connects to B");
        peers_b
            .connect_peer(&pid_b, format!("memory://{pid_a}"))
            .await
            .expect("B connects to A");

        // A Chat window on each peer, each bound to the 1:1 with the other. Both
        // derive the SAME well-known conversation id, so they share a room with
        // no genesis exchange.
        let mut win_a = ChatWindow::new(1, pid_a.clone());
        let mut win_b = ChatWindow::new(2, pid_b.clone());
        win_a.bind_and_subscribe(&peers_a, &pid_b).await;
        win_b.bind_and_subscribe(&peers_b, &pid_a).await;

        // A → B: A sends through the window's `ChatSend` action; drive both
        // windows' delivery each "frame" until A's message shows in B's window.
        win_a.handle_action(
            &Action::ChatSend {
                window_id: 1,
                body: "hi B, it's A".into(),
            },
            &peers_a,
        );
        let mut saw_on_b = false;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            win_a.tick(&peers_a);
            win_b.tick(&peers_b);
            if win_b
                .render_output(&peers_b, &crate::dial_markers::DialMarkers::new())
                .messages
                .iter()
                .any(|m| m.body == "hi B, it's A")
            {
                saw_on_b = true;
                break;
            }
        }
        assert!(saw_on_b, "A's message must reach B's window");
        let out_b = win_b.render_output(&peers_b, &crate::dial_markers::DialMarkers::new());
        let a_msg = out_b
            .messages
            .iter()
            .find(|m| m.body == "hi B, it's A")
            .expect("A's message in B's window");
        assert!(!a_msg.mine, "A's message is not-mine in B's window");
        assert_eq!(a_msg.author, pid_a);

        // B → A: B replies; it appears in A's window. Proves the flow is
        // symmetric (each peer both sends and receives).
        win_b.handle_action(
            &Action::ChatSend {
                window_id: 2,
                body: "got it, A — B here".into(),
            },
            &peers_b,
        );
        let mut saw_on_a = false;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            win_a.tick(&peers_a);
            win_b.tick(&peers_b);
            if win_a
                .render_output(&peers_a, &crate::dial_markers::DialMarkers::new())
                .messages
                .iter()
                .any(|m| m.body == "got it, A — B here")
            {
                saw_on_a = true;
                break;
            }
        }
        assert!(saw_on_a, "B's reply must reach A's window");
        let out_a = win_a.render_output(&peers_a, &crate::dial_markers::DialMarkers::new());
        assert!(
            out_a.messages.iter().any(|m| m.body == "hi B, it's A" && m.mine),
            "A's own message stays mine in A's window"
        );
        let b_reply = out_a
            .messages
            .iter()
            .find(|m| m.body == "got it, A — B here")
            .expect("B's reply in A's window");
        assert!(!b_reply.mine, "B's reply is not-mine in A's window");

        handle_a.abort();
        handle_b.abort();
    }

    /// File Transfer can tell "offline" from "not tried yet".
    ///
    /// The defect: `classify_target_access` deliberately skips transport errors
    /// (never manufacture a denial from silence — a correct rule, and itself a
    /// past bug fix), so an unreachable target fell through to
    /// `TargetAccess::Unknown`, whose whole meaning is *"nothing tried yet — do
    /// not alarm"*. A device that could not be reached at all was therefore
    /// pixel-identical to one you had simply never used.
    ///
    /// Both peers below are **remembered** — the registry means "connected at
    /// least once", never "up now" — so the target list cannot tell them apart
    /// either. The reachability axis is the only thing that can, which is the
    /// whole point of adding it rather than folding it into `access`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn file_transfer_distinguishes_an_unreachable_target_from_an_untried_one() {
        use crate::views::file_transfer::model::FileTransferModel;
        use crate::views::file_transfer::output::TargetAccess;

        let registry = MemoryTransportRegistry::new();
        let (peers_a, pid_a, handle_a) = spawn_peer_on_registry(registry.clone());
        let (_peers_b, pid_b, handle_b) = spawn_peer_on_registry(registry.clone());
        tokio::task::yield_now().await;

        peers_a
            .connect_peer(&pid_a, format!("memory://{pid_b}"))
            .await
            .expect("A connects to B");

        // Remember BOTH: B, which is genuinely up, and a peer that has never
        // existed. Identical registry rows — that is the premise.
        let ghost = "2KghostPeerRememberedButNeverReachabezzzzzzzzz";
        let writer = crate::connections::ConnectionsWriter::new(&peers_a);
        writer.add(&pid_b);
        writer.add(ghost);
        for _ in 0..40 {
            if crate::connections::read_connections(&peers_a).len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let dials = crate::dial_markers::DialMarkers::new();
        let model = FileTransferModel::new(1, pid_a.clone());

        // Select the live peer: reachable, and the kernel says so.
        model.select_target(&pid_b);
        let mut live_ok = false;
        for _ in 0..40 {
            let out = model.render_output(&peers_a, &dials);
            if out.target_reach == crate::peer_liveness::ConnDisplay::Connected {
                live_ok = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let out = model.render_output(&peers_a, &dials);
        assert!(
            live_ok,
            "a genuinely connected target must read Connected, got {:?}",
            out.target_reach
        );

        // Select the ghost: same kind of registry row, but nothing can reach it.
        model.select_target(ghost);
        let out = model.render_output(&peers_a, &dials);
        assert!(out.has_target, "the ghost is still a remembered, selectable target");
        assert_ne!(
            out.target_reach,
            crate::peer_liveness::ConnDisplay::Connected,
            "nothing has ever reached this peer — it must not read Connected"
        );
        // The axis that was already there stays put and stays honest: no
        // operation was refused, so authorization is genuinely unknown. If the
        // fix had been "infer a denial from the transport error" this would be
        // Denied, and the app would be accusing a peer of refusing us when it
        // simply is not there.
        assert_eq!(
            out.access,
            TargetAccess::Unknown,
            "reachability must not contaminate the authorization axis"
        );

        handle_a.abort();
        handle_b.abort();
    }

    /// The Chat header tells the truth about reachability — the regression gate
    /// for "a failed delivery was a `tracing::warn!` and nothing else".
    ///
    /// Three branches, because the surface has two independent decisions and a
    /// one-sided test would pass over either being stuck on:
    ///
    /// 1. **Bound + genuinely connected** — one row, for the *other* peer only,
    ///    reading `Connected` from the kernel read-model. Never a row for
    ///    ourselves: "am I reachable from here" is not a thing to paint.
    /// 2. **Bound + unreachable + no establisher** — the reason line fires. This
    ///    is the state the whole feature exists for.
    /// 3. **Bound + unreachable + we DO have an establisher** — no reason line,
    ///    because it would be a wrong diagnosis. Native has no WebRTC at all, so
    ///    without this branch a guard stuck permanently on would still pass.
    ///
    /// Branch 1 doubles as the relevance check on the reason line: `peers` has
    /// no establisher there either (native never does), so a `no_establisher`
    /// that ignored "is anything actually reachable" would fire next to a
    /// working conversation — the standing-warning shape users learn to ignore.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_chat_header_says_whether_the_other_participant_is_reachable() {
        use crate::views::chat::ChatWindow;

        let registry = MemoryTransportRegistry::new();
        let (peers_a, pid_a, handle_a) = spawn_peer_on_registry(registry.clone());
        let (peers_b, pid_b, handle_b) = spawn_peer_on_registry(registry.clone());
        tokio::task::yield_now().await;

        peers_a
            .connect_peer(&pid_a, format!("memory://{pid_b}"))
            .await
            .expect("A connects to B");
        peers_b
            .connect_peer(&pid_b, format!("memory://{pid_a}"))
            .await
            .expect("B connects to A");

        let dials = crate::dial_markers::DialMarkers::new();

        // An UNBOUND window is about nobody — no rows, and no reason line, or
        // the default self-conversation would paint a status for a relationship
        // the user never created.
        let unbound = ChatWindow::new(1, pid_a.clone());
        let out = unbound.render_output(&peers_a, &dials);
        assert!(!out.bound);
        assert!(out.reachability.is_empty(), "unbound chat has nobody to reach");
        assert!(!out.no_establisher, "no reason line without a conversation");

        // (1) Bound to a peer we really are connected to. The kernel writes
        // `system/peer/status` on handshake, so this is the read-model's own
        // answer, not something the window inferred from its own dial.
        let mut win_a = ChatWindow::new(1, pid_a.clone());
        win_a.bind_and_subscribe(&peers_a, &pid_b).await;
        let mut connected = false;
        for _ in 0..40 {
            let out = win_a.render_output(&peers_a, &dials);
            if out.reachability.iter().any(|r| {
                r.status == crate::peer_liveness::ConnDisplay::Connected
            }) {
                connected = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let out = win_a.render_output(&peers_a, &dials);
        assert!(
            connected,
            "a live 1:1 must read Connected from the kernel read-model, got {:?}",
            out.reachability.iter().map(|r| r.status).collect::<Vec<_>>()
        );
        assert_eq!(out.reachability.len(), 1, "exactly the other participant");
        assert_eq!(out.reachability[0].peer_id, pid_b);
        assert!(
            !out.reachability.iter().any(|r| r.peer_id == pid_a),
            "never a row for ourselves"
        );
        assert!(
            !out.no_establisher,
            "no reason line next to a working conversation — this peer has no \
             establisher either (native never does), so firing here would make \
             the warning permanent noise"
        );

        // (2) Bound to a peer nothing can reach, with no establisher: the reason
        // line is the only thing that explains the silence.
        let unreachable = "2KnobodyHomeAtThisPeerdForTheReachabiityGatezz";
        let mut win_dark = ChatWindow::new(2, pid_a.clone());
        win_dark.bind_and_subscribe(&peers_a, unreachable).await;
        let out = win_dark.render_output(&peers_a, &dials);
        assert_eq!(out.reachability.len(), 1);
        assert_ne!(
            out.reachability[0].status,
            crate::peer_liveness::ConnDisplay::Connected,
            "nothing was ever connected to this id"
        );
        assert!(
            out.no_establisher,
            "unreachable + no establisher is exactly the state that needs saying"
        );

        // (3) Same unreachable conversation, but this peer DOES install an
        // establisher — so a counterpart could reach back and the missing
        // establisher is not the diagnosis. Kill this branch and a guard wired
        // permanently on still passes (2), which is how a useless warning ships.
        let mut peers_webrtc = Peers::new_direct();
        let webrtc_pid = peers_webrtc.primary_peer_id().to_string();
        peers_webrtc.mark_webrtc_peer_for_test(&webrtc_pid);
        let mut win_ok = ChatWindow::new(3, webrtc_pid.clone());
        win_ok.bind_and_subscribe(&peers_webrtc, unreachable).await;
        let out = win_ok.render_output(&peers_webrtc, &dials);
        assert_eq!(out.reachability.len(), 1);
        assert_ne!(
            out.reachability[0].status,
            crate::peer_liveness::ConnDisplay::Connected,
            "still nothing connected — only the establisher differs"
        );
        assert!(
            !out.no_establisher,
            "we CAN be reached back, so the missing establisher is not the reason"
        );

        handle_a.abort();
        handle_b.abort();
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

// =====================================================================
// AUDIT REPRO (A1, `AUDIT-CONNECT-PEER-FILETRANSFER-2026-07-14.md`) —
// symptom 2: "Connected, but Browse shared files → handler error."
//
// Drives the REAL app router (`Peers::connect_peer` / `execute` /
// `reconnect_peer`) over a real WebSocket against a backend B with a real
// `local/files` share, and PRINTS the actual browse error at each stage.
// It answers the audit's single most important untraced value (§1 CANNOT-
// say): is the browse failure a capability 403 (`Ok(status=403)`) or a
// transport error over a dead pooled connection (`Err`)?
//
// Lives in-crate (not tests/) because the app crate is bin-only, so the
// `Peers` router can't be imported from an external integration test —
// same reason the memory-transport router test is here.
// =====================================================================
#[cfg(all(test, not(target_arch = "wasm32")))]
mod connect_browse_stale_repro {
    use super::*;
    use entity_capability::ResourceTarget;
    use entity_crypto::Keypair;
    use entity_handler::ExecuteOptions;
    use entity_peer::local_files::RootConfigData;
    use entity_peer::transport::{Connector, WebSocketConnector, WebSocketListener};
    use entity_peer::{PeerBuilder, PeerConfig, PeerShared};
    use std::sync::Arc;
    use std::time::Duration;

    const SEED_NAME: &str = "welcome.txt";
    const SEED_BODY: &[u8] = b"hello from the backend share";
    /// B's stable seed — a fresh listener with the SAME seed re-mints the SAME
    /// peer-id (models a Tori backend restart: durable identity, new socket).
    const B_SEED: [u8; 32] = [9u8; 32];

    fn empty_params() -> Entity {
        Entity::new("system/empty", entity_ecf::to_ecf(&entity_ecf::Value::Null)).unwrap()
    }

    /// Stand up backend B on a real WS listener with a writable
    /// `local/files/shared` root seeded with `welcome.txt`.
    /// `debug_open_grants = true` — the shipped desktop backend's posture
    /// (open by default unless `ENTITY_BROWSER_ENFORCE`), so a browse failure
    /// here is NOT a capability gate. Returns `(shared_b, pid_b, ws_addr, task)`.
    async fn start_backend(
        tmp: &tempfile::TempDir,
    ) -> (Arc<PeerShared>, String, String, tokio::task::JoinHandle<()>) {
        let kp_b = Keypair::from_seed(B_SEED);
        let pid_b = kp_b.peer_id().to_string();
        let peer_b = PeerBuilder::new()
            .keypair(kp_b)
            .config(PeerConfig { debug_open_grants: true, ..PeerConfig::default() })
            .build()
            .expect("peer B builds");
        let shared_b = peer_b.shared();
        peer_b.start_engines(&shared_b);
        peer_b
            .local_files_handler()
            .add_root(
                "shared",
                RootConfigData {
                    prefix: "local/files/shared/".to_string(),
                    filesystem_root: tmp.path().to_string_lossy().into_owned(),
                    read_only: false,
                    ..Default::default()
                },
            )
            .expect("add root");

        let shared_b_run = shared_b.clone();
        let listener = WebSocketListener::bind("127.0.0.1:0").await.expect("ws bind");
        let addr = format!("ws://{}", listener.socket_addr());
        let task = tokio::spawn(async move {
            let _ = entity_peer::server::run(listener, shared_b_run).await;
        });
        (shared_b, pid_b, addr, task)
    }

    /// The EXACT browse the File Transfer window drives (`file_transfer/mod.rs`
    /// `load_dir` → `ops::execute` → `Peers::execute`): a `local/files:list` on
    /// B's share root, `entity://{pid_b}/…`, from the LOCAL frontend through its
    /// pooled connection. Returns the router's raw `Result` (Err=transport,
    /// Ok(status)=capability), wrapped in a timeout so a dead conn can't hang.
    #[allow(clippy::type_complexity)]
    async fn browse(
        frontend: &Peers,
        frontend_pid: &str,
        pid_b: &str,
    ) -> Result<Result<entity_handler::HandlerResult, String>, tokio::time::error::Elapsed> {
        let opts = ExecuteOptions {
            resource: Some(ResourceTarget {
                targets: vec![format!("/{}/local/files/shared/", pid_b)],
                exclude: vec![],
            }),
            ..Default::default()
        };
        let fut = frontend.execute(
            frontend_pid,
            format!("entity://{}/local/files", pid_b),
            "list".to_string(),
            empty_params(),
            opts,
        );
        tokio::time::timeout(Duration::from_secs(5), fut).await
    }

    /// Print the browse outcome as one ground-truth line; return whether it
    /// showed the seed (a genuine success).
    fn describe(
        label: &str,
        outcome: &Result<
            Result<entity_handler::HandlerResult, String>,
            tokio::time::error::Elapsed,
        >,
    ) -> bool {
        match outcome {
            Err(_) => {
                eprintln!("[{label}] browse => TIMEOUT (no response within 5s)");
                false
            }
            Ok(Err(e)) => {
                eprintln!("[{label}] browse => Err (transport/dispatch): {e:?}");
                false
            }
            Ok(Ok(r)) => {
                let body = String::from_utf8_lossy(&r.result.data);
                let shows_seed = body.contains(SEED_NAME);
                eprintln!(
                    "[{label}] browse => Ok(status={}), shows {SEED_NAME}={shows_seed}",
                    r.status
                );
                r.status == 200 && shows_seed
            }
        }
    }

    /// Symptom 2, end to end, through the real router — actual error printed at
    /// each stage. Run with `cargo test connected_then_browse -- --nocapture`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn connected_then_browse_breaks_when_pool_goes_stale() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(SEED_NAME), SEED_BODY).unwrap();

        // Frontend A: the real app router with a WS connector (exactly the
        // browser/Tori frontend's outbound transport).
        let frontend =
            Peers::new_direct_with_connector(Arc::new(WebSocketConnector) as Arc<dyn Connector>);
        let a_pid = frontend.primary_peer_id().to_string();

        // --- Backend B up, A connects. ---
        let (_shared_b1, pid_b, addr1, task1) = start_backend(&tmp).await;
        let connected = frontend.connect_peer(&a_pid, addr1.clone()).await;
        eprintln!("[connect#1] => {connected:?}");
        assert_eq!(connected.as_deref(), Ok(pid_b.as_str()), "first connect pools B");

        // --- Baseline: browse works (symptom 2: "earlier showed welcome.txt"). ---
        let base = browse(&frontend, &a_pid, &pid_b).await;
        assert!(describe("baseline", &base), "baseline browse must show the seed");

        // --- The break: B's connection goes stale (Tori backend restart / dropped WS). ---
        task1.abort();
        let _ = task1.await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        // THE UNTRACED VALUE: what does browse over the now-dead pooled conn do?
        let after_drop = browse(&frontend, &a_pid, &pid_b).await;
        let after_drop_ok = describe("after-drop", &after_drop);
        assert!(!after_drop_ok, "browse over the dead pooled connection must fail (symptom 2)");

        // --- B back, same identity, new socket (durable Tori backend restart). ---
        let (_shared_b2, pid_b2, addr2, _task2) = start_backend(&tmp).await;
        assert_eq!(pid_b2, pid_b, "restarted backend keeps its durable identity");

        // --- Plain re-Connect (the Connect button): does it recover? ---
        // Hypothesis: NO — `remote.insert` is insert-if-absent, dead conn survives.
        let reconnected = frontend.connect_peer(&a_pid, addr2.clone()).await;
        eprintln!("[connect#2] => {reconnected:?}");
        let after_plain = browse(&frontend, &a_pid, &pid_b).await;
        let plain_connect_recovers = describe("after-plain-connect", &after_plain);

        // --- reconnect_peer (evict + re-dial): does IT recover? ---
        let re = frontend.reconnect_peer(&a_pid, &pid_b, addr2.clone()).await;
        eprintln!("[reconnect_peer] => {re:?}");
        let after_recon = browse(&frontend, &a_pid, &pid_b).await;
        let reconnect_peer_recovers = describe("after-reconnect_peer", &after_recon);

        eprintln!(
            "\n==== SYMPTOM-2 GROUND TRUTH ====\n\
             plain connect_peer recovers browse: {plain_connect_recovers}\n\
             reconnect_peer recovers browse:      {reconnect_peer_recovers}\n\
             ================================\n"
        );

        // The permanent gate: reconnect_peer is the recovery path the app's
        // Connect (and boot auto-connect) MUST route through.
        assert!(
            reconnect_peer_recovers,
            "reconnect_peer (evict + re-dial) MUST recover browse after the pool goes stale"
        );
    }

    /// The same browse driven through the `ops::execute` chokepoint — the
    /// EXACT path the File Transfer window's `load_dir` takes.
    async fn ops_browse(
        frontend: &Peers,
        frontend_pid: &str,
        pid_b: &str,
    ) -> Result<crate::ops::ExecuteResponse, String> {
        let fut = crate::ops::execute(
            frontend,
            crate::ops::ExecuteRequest {
                peer_id: frontend_pid.to_string(),
                handler_uri: format!("entity://{}/local/files", pid_b),
                operation: "list".to_string(),
                params: None,
                resource: Some(format!("/{}/local/files/shared/", pid_b)),
            },
        );
        tokio::time::timeout(Duration::from_secs(5), fut)
            .await
            .map_err(|_| "timeout: no response within 5s".to_string())?
    }

    /// B1, the fix: `ops::execute` (the one dispatch chokepoint — File
    /// Transfer browse, uploads, every remote execute) must SELF-heal a stale
    /// pooled connection: evict + reconnect through the peer's remembered
    /// listen address + retry once, keyed on the transport `Err` class that a
    /// dead pool actually produces (AP14) — no manual reconnect.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ops_execute_self_heals_browse_over_stale_pool() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(SEED_NAME), SEED_BODY).unwrap();

        let mut frontend =
            Peers::new_direct_with_connector(Arc::new(WebSocketConnector) as Arc<dyn Connector>);
        let a_pid = frontend.primary_peer_id().to_string();

        // --- Backend B up, A connects; baseline browse through the chokepoint. ---
        let (_shared_b1, pid_b, addr1, task1) = start_backend(&tmp).await;
        let connected = frontend.connect_peer(&a_pid, addr1.clone()).await;
        assert_eq!(connected.as_deref(), Ok(pid_b.as_str()), "first connect pools B");
        let baseline = ops_browse(&frontend, &a_pid, &pid_b).await.expect("baseline browse");
        assert!(
            String::from_utf8_lossy(&baseline.result.result.data).contains(SEED_NAME),
            "baseline browse must show the seed"
        );

        // --- B's connection goes stale; B restarts (durable identity, new socket). ---
        task1.abort();
        let _ = task1.await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let (_shared_b2, pid_b2, addr2, _task2) = start_backend(&tmp).await;
        assert_eq!(pid_b2, pid_b, "restarted backend keeps its durable identity");

        // Remember B's current listen address, as the app does on connect /
        // backend registration. (A real Tori backend restarts on its configured
        // address; the test re-points the metadata because an OS-assigned
        // ephemeral port can't be re-bound deterministically.)
        assert!(frontend.register_backend_peer_primary(
            pid_b.clone(),
            None,
            vec![addr2.clone()]
        ));

        // Below the chokepoint the pool is genuinely stale: a raw
        // `Peers::execute` browse must still fail — no magic at this layer.
        let raw = browse(&frontend, &a_pid, &pid_b).await;
        assert!(!describe("raw-after-restart", &raw), "raw browse over the dead pool must fail");

        // --- THE GATE: the chokepoint heals it, no manual reconnect. ---
        let healed = ops_browse(&frontend, &a_pid, &pid_b)
            .await
            .expect("ops::execute must self-heal the stale pool (evict + reconnect + retry)");
        assert!(
            String::from_utf8_lossy(&healed.result.result.data).contains(SEED_NAME),
            "healed browse must show the seed"
        );
    }
}

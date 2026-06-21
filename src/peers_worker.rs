//! `WorkerPeerStore` — the Worker arm of the `Peers` facade.
//!
//! Wraps `entity_wasm_worker_proxy::WorkerProxy<WebTransport>` plus the
//! cached subscription state needed to make synchronous-read methods on
//! `Peers` work against the proxy's per-prefix mirror.
//!
//! Only compiles when the `worker` cargo feature is on AND the target is
//! wasm32 (the proxy crate itself is `#![cfg(target_arch = "wasm32")]`).
//!
//! Lifecycle:
//! 1. `main.rs` bootstrap branch spawns the worker via
//!    `WorkerProxy::spawn("/entity-worker.js", init_params).await`.
//! 2. Wraps the resulting proxy in
//!    `WorkerPeerStore::new(proxy, primary_peer_id)`.
//! 3. Stored as `Peers::Worker(...)` and threaded through the app the
//!    same way `Peers::Direct(PeerManager)` is.
//!
//! Each window that wants to render against a prefix calls
//! `peers.watch_prefix(&mut window_watch, pid, prefix)` at spawn time;
//! that translates to `proxy.observe(prefix)` and starts mirroring.
//! Then subsequent `peers.get_entity(pid, path)` calls within the
//! window's render hit the mirror synchronously.

#![cfg(target_arch = "wasm32")]

use std::rc::Rc;

use entity_entity::Entity;
use entity_hash::Hash;
use entity_sdk::PeerMetadata;
use entity_store::LocationEntry;
use entity_wasm_worker_protocol::{WireEntity, WirePeerMetadata};
use entity_wasm_worker_proxy::{WebTransport, WorkerProxy};

use crate::window_watch::WindowWatch;

/// Per-peer info mirrored on the main thread for synchronous palette /
/// peer-selector reads. Populated from `InitParams` at boot; extended
/// when `Request::RegisterBackendPeer` fires (Phase 3.x — not in v1).
#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub peer_id: String,
    pub metadata: PeerMetadata,
    /// True if this peer was loaded via `InitParams` (primary or
    /// additional). Backend peers registered after init are local=false.
    pub local: bool,
}

pub struct WorkerPeerStore {
    /// Shared with bridging tasks spawned by `watch_prefix` and
    /// `dispatch_write`. The proxy itself is not Send/Sync (it holds
    /// `Rc`s); we stay on the main thread.
    proxy: Rc<WorkerProxy<WebTransport>>,
    primary_peer_id: String,
    /// Main-thread mirror of peer-list info that the app reads
    /// synchronously each frame (palette, peer selector, etc.).
    /// Worker side is authoritative; we replicate at boot so we don't
    /// have to round-trip on every render. `Rc<RefCell<...>>` so async
    /// create/delete futures can clone the handle and update the
    /// mirror on completion (Parity-B) without borrowing
    /// `&self` for their full lifetime.
    peers: Rc<std::cell::RefCell<Vec<PeerInfo>>>,
    /// Prefixes this store has been asked to mirror (via `watch_prefix` /
    /// `observe_with_events`). Recorded synchronously at call time —
    /// before the async `observe` resolves — so it reflects subscription
    /// *intent*. Read only by the `audit-worker-reads` lamp to make the
    /// "Worker read of an unsubscribed prefix returns silently empty" leak
    /// loud — the cache mirror only holds subscribed prefixes, so an
    /// uncovered read is the `list_child_pages`/site-mode-toggle bug class
    /// (`feedback_worker_cache_get_needs_subscription`, bitten twice).
    #[cfg_attr(not(feature = "audit-worker-reads"), allow(dead_code))]
    subscribed_prefixes: Rc<std::cell::RefCell<Vec<String>>>,
    /// De-dupe so an uncovered read in a render loop warns once per path,
    /// not every frame (audit lamp only).
    #[cfg_attr(not(feature = "audit-worker-reads"), allow(dead_code))]
    unsubscribed_warned: Rc<std::cell::RefCell<std::collections::HashSet<String>>>,
}

impl WorkerPeerStore {
    pub fn new(
        proxy: WorkerProxy<WebTransport>,
        primary_peer_id: String,
        peers: Vec<PeerInfo>,
    ) -> Self {
        Self {
            proxy: Rc::new(proxy),
            primary_peer_id,
            peers: Rc::new(std::cell::RefCell::new(peers)),
            subscribed_prefixes: Rc::new(std::cell::RefCell::new(Vec::new())),
            unsubscribed_warned: Rc::new(std::cell::RefCell::new(std::collections::HashSet::new())),
        }
    }

    /// Record subscription intent for `prefix` (idempotent). Called
    /// synchronously by `watch_prefix` / `observe_with_events`. Always
    /// recorded (cheap) so the `audit-worker-reads` lamp has ground truth
    /// whenever it is switched on.
    fn record_subscription(&self, prefix: &str) {
        let mut subs = self.subscribed_prefixes.borrow_mut();
        if !subs.iter().any(|p| p == prefix) {
            subs.push(prefix.to_string());
        }
    }

    /// True if any recorded subscription prefix covers `path`. A read of
    /// an uncovered path hits an empty slot in the cache mirror.
    #[cfg(feature = "audit-worker-reads")]
    fn covered_by_subscription(&self, path: &str) -> bool {
        self.subscribed_prefixes
            .borrow()
            .iter()
            .any(|p| path.starts_with(p.as_str()))
    }

    /// Break-glass audit lamp for the unsubscribed-Worker-read leak.
    /// Compiled out unless the `audit-worker-reads` feature is on (the
    /// covering scan is per-render, and some unsubscribed reads are
    /// benign — default-is-correct or write-seeds-cache — so it is an
    /// opt-in hunting tool, not an always-on guard). Warns once per
    /// uncovered path.
    #[cfg(feature = "audit-worker-reads")]
    fn warn_if_unsubscribed(&self, what: &str, path_or_prefix: &str) {
        if self.covered_by_subscription(path_or_prefix) {
            return;
        }
        if self
            .unsubscribed_warned
            .borrow_mut()
            .insert(path_or_prefix.to_string())
        {
            tracing::warn!(
                target = %path_or_prefix,
                op = %what,
                "BREAK-GLASS: Worker-arm {what} on a prefix with no active \
                 subscription — the cache mirror holds only subscribed prefixes, \
                 so this read is silently empty. Register a WindowWatch \
                 (peers.watch_prefix) for this path before reading it.",
            );
        }
    }
    #[cfg(not(feature = "audit-worker-reads"))]
    #[inline]
    fn warn_if_unsubscribed(&self, _what: &str, _path_or_prefix: &str) {}

    pub fn primary_peer_id(&self) -> &str {
        &self.primary_peer_id
    }

    pub fn peer_ids(&self) -> Vec<String> {
        self.peers.borrow().iter().map(|p| p.peer_id.clone()).collect()
    }

    pub fn peer_metadata(&self, peer_id: &str) -> Option<PeerMetadata> {
        self.peers
            .borrow()
            .iter()
            .find(|p| p.peer_id == peer_id)
            .map(|p| p.metadata.clone())
    }

    pub fn has_local_peer(&self, peer_id: &str) -> bool {
        self.peers
            .borrow()
            .iter()
            .any(|p| p.peer_id == peer_id && p.local)
    }

    /// Synchronous read from the cache mirror. Returns `None` if the
    /// path isn't mirrored (no `observe` covers it) or if the cached
    /// `WireEntity` fails to convert back to `Entity` (which would
    /// indicate a protocol-version skew the version handshake should
    /// have caught at boot).
    pub fn cache_get(&self, path: &str) -> Option<Entity> {
        self.warn_if_unsubscribed("cache_get", path);
        let wire = self.proxy.cache_get(path)?;
        match Entity::try_from(wire) {
            Ok(e) => Some(e),
            Err(err) => {
                tracing::warn!(path = %path, error = %err, "cache_get: WireEntity→Entity conversion failed");
                None
            }
        }
    }

    /// Synchronous prefix-scan over the cache mirror. Conversion errors
    /// drop the offending entry with a warning (same rationale as
    /// `cache_get`).
    pub fn cache_list(&self, prefix: &str) -> Vec<LocationEntry> {
        self.warn_if_unsubscribed("cache_list", prefix);
        self.proxy
            .cache_list(prefix)
            .into_iter()
            .filter_map(|(path, wire_entity)| {
                let hash = match Hash::try_from(&wire_entity.content_hash) {
                    Ok(h) => h,
                    Err(err) => {
                        tracing::warn!(path = %path, error = %err, "cache_list: WireHash→Hash conversion failed");
                        return None;
                    }
                };
                Some(LocationEntry { path, hash })
            })
            .collect()
    }

    /// Cache-derived count of entities under the peer's qualified
    /// prefix. Accurate only insofar as the consumer has subscribed to
    /// the entire peer prefix; partial subscriptions return partial
    /// counts. Phase 3.3 can add a proper L1 `entity_count` round-trip
    /// if a UI needs the authoritative number.
    pub fn entity_count_estimate(&self, peer_id: &str) -> usize {
        let prefix = format!("/{}/", peer_id);
        self.proxy.cache_list(&prefix).len()
    }

    /// Same shape as `entity_count_estimate`; identical for cache-backed
    /// reads since the cache stores one entry per path.
    pub fn path_count_estimate(&self, peer_id: &str) -> usize {
        self.entity_count_estimate(peer_id)
    }

    /// Fire-and-forget remove. Mirrors `dispatch_write` shape: spawns
    /// `proxy.remove(...)` and logs the outcome.
    pub fn dispatch_remove(&self, peer_id: String, path: String) {
        let proxy = self.proxy.clone();
        wasm_bindgen_futures::spawn_local(async move {
            match proxy.remove(peer_id.clone(), path.clone()).await {
                Ok(true) => tracing::trace!(peer_id = %peer_id, path = %path, "dispatch_remove: removed"),
                Ok(false) => tracing::trace!(peer_id = %peer_id, path = %path, "dispatch_remove: not present"),
                Err(err) => tracing::warn!(peer_id = %peer_id, path = %path, error = ?err, "dispatch_remove: failed"),
            }
        });
    }

    /// Return a clone of the inner `Rc<WorkerProxy>` for components
    /// that need to dispatch writes / RPCs directly (e.g.
    /// `event_log_writer`'s fire-and-forget appends). Cheap — just an
    /// `Rc::clone`.
    pub fn proxy_handle(&self) -> Rc<WorkerProxy<WebTransport>> {
        self.proxy.clone()
    }

    /// Dispatch L1 `execute` through the worker. Wraps
    /// `proxy.execute(...)` and converts the wire result to the SDK
    /// `HandlerResult` shape so callers can share code with the
    /// Direct arm. Returns a `'static` future so callers can move it
    /// into `spawn_local`.
    pub fn execute(
        &self,
        peer_id: String,
        handler_uri: String,
        operation: String,
        params: Entity,
        opts: entity_handler::ExecuteOptions,
    ) -> impl std::future::Future<Output = Result<entity_handler::HandlerResult, String>> + 'static
    {
        let proxy = self.proxy.clone();
        async move {
            let wire_params = WireEntity::try_from(params)
                .map_err(|e| format!("Entity→WireEntity conversion: {e}"))?;
            let wire_opts = entity_wasm_worker_protocol::WireExecuteOptions::from(&opts);
            let wire_result = proxy
                .execute(peer_id, handler_uri, operation, wire_params, wire_opts)
                .await
                .map_err(|e| format!("proxy.execute: {e:?}"))?;
            entity_handler::HandlerResult::try_from(wire_result)
                .map_err(|e| format!("WireHandlerResult→HandlerResult: {e}"))
        }
    }

    /// Dispatch L1 `query` through the worker. Full fidelity as of
    /// PROTOCOL_VERSION=4: `WireQueryResults` carries `total`,
    /// `cursor`, and per-match `entity_type` — same shape as
    /// `entity_sdk::QueryResults`. (Was lossy in v3.)
    pub fn query(
        &self,
        peer_id: String,
        expression: Entity,
    ) -> impl std::future::Future<Output = Result<entity_sdk::QueryResults, String>> + 'static
    {
        let proxy = self.proxy.clone();
        async move {
            let wire_expr = WireEntity::try_from(expression)
                .map_err(|e| format!("Entity→WireEntity conversion: {e}"))?;
            let wire_results = proxy
                .query(peer_id, wire_expr)
                .await
                .map_err(|e| format!("proxy.query: {e:?}"))?;
            wire_query_results_into_sdk(wire_results)
        }
    }

    /// Dispatch L1 `count` through the worker. Full fidelity — wire
    /// returns `u64`, same shape as SDK.
    pub fn count(
        &self,
        peer_id: String,
        expression: Entity,
    ) -> impl std::future::Future<Output = Result<u64, String>> + 'static {
        let proxy = self.proxy.clone();
        async move {
            let wire_expr = WireEntity::try_from(expression)
                .map_err(|e| format!("Entity→WireEntity conversion: {e}"))?;
            proxy
                .count(peer_id, wire_expr)
                .await
                .map_err(|e| format!("proxy.count: {e:?}"))
        }
    }

    /// On-demand L1 `get` through the worker. Unlike [`Self::cache_get`]
    /// (which reads only the watch-primed mirror), this issues a `Get`
    /// round-trip so callers can read a path they never subscribed to —
    /// needed for `compute show`, which reads one subgraph metadata entity
    /// the shell never watched. Returns a `'static` future for
    /// `spawn_local`.
    pub fn get_entity_async(
        &self,
        peer_id: String,
        path: String,
    ) -> impl std::future::Future<Output = Result<Option<Entity>, String>> + 'static {
        let proxy = self.proxy.clone();
        async move {
            let wire = proxy
                .get(peer_id, path)
                .await
                .map_err(|e| format!("proxy.get: {e:?}"))?;
            match wire {
                Some(w) => Entity::try_from(w)
                    .map(Some)
                    .map_err(|e| format!("WireEntity→Entity: {e}")),
                None => Ok(None),
            }
        }
    }

    /// On-demand L1 `list` through the worker — the authoritative prefix
    /// enumeration that does NOT depend on a watch-primed mirror (unlike
    /// [`Self::cache_list`], which returns silently-empty for an
    /// unsubscribed prefix). Issues a `List` round-trip so callers can
    /// enumerate a prefix they never subscribed to (boot-time roster read /
    /// reconcile). Returns a `'static` future for `spawn_local`.
    pub fn list_async(
        &self,
        peer_id: String,
        prefix: String,
    ) -> impl std::future::Future<Output = Result<Vec<LocationEntry>, String>> + 'static {
        let proxy = self.proxy.clone();
        async move {
            let wire = proxy
                .list(peer_id, prefix)
                .await
                .map_err(|e| format!("proxy.list: {e:?}"))?;
            Ok(wire
                .into_iter()
                .filter_map(|w| match Hash::try_from(&w.content_hash) {
                    Ok(hash) => Some(LocationEntry { path: w.path, hash }),
                    Err(err) => {
                        tracing::warn!(path = %w.path, error = %err, "list_async: WireHash→Hash conversion failed");
                        None
                    }
                })
                .collect())
        }
    }

    /// Dispatch L1 `discover_handlers` through the worker. Full
    /// fidelity — `WireHandlerInfo` carries `pattern`/`name`/`operations`,
    /// same shape as SDK `HandlerInfo`.
    pub fn discover_handlers(
        &self,
        peer_id: String,
    ) -> impl std::future::Future<Output = Result<Vec<entity_sdk::HandlerInfo>, String>> + 'static
    {
        let proxy = self.proxy.clone();
        async move {
            let wire = proxy
                .discover_handlers(peer_id)
                .await
                .map_err(|e| format!("proxy.discover_handlers: {e:?}"))?;
            Ok(wire
                .into_iter()
                .map(|w| entity_sdk::HandlerInfo {
                    pattern: w.pattern,
                    name: w.name,
                    operations: w.operations,
                })
                .collect())
        }
    }

    /// Create a new peer inside the worker. Awaits the proxy round
    /// trip, then registers the new peer in the main-thread mirror
    /// so palette / peer-selector reads pick it up immediately.
    /// Returns `(peer_id, keypair_seed, metadata)` so the caller can
    /// persist the seed (localStorage) — the host does NOT retain it.
    pub fn create_peer(
        &self,
        label: Option<String>,
    ) -> impl std::future::Future<Output = Result<(String, [u8; 32], PeerMetadata), String>> + 'static
    {
        let proxy = self.proxy.clone();
        let peers_mirror = self.peers.clone();
        async move {
            let ok = proxy
                .create_peer(label.clone())
                .await
                .map_err(|e| format!("proxy.create_peer: {e:?}"))?;
            let seed: [u8; 32] = ok.keypair_seed.as_slice().try_into().map_err(|_| {
                format!(
                    "proxy.create_peer: keypair_seed length {} != 32",
                    ok.keypair_seed.len()
                )
            })?;
            let metadata = PeerMetadata {
                label: ok.metadata.label.clone(),
                persisted: ok.metadata.persisted,
                listen_addresses: ok.metadata.listen_addresses.clone(),
            };
            peers_mirror.borrow_mut().push(PeerInfo {
                peer_id: ok.peer_id.clone(),
                metadata: metadata.clone(),
                local: true,
            });
            Ok((ok.peer_id, seed, metadata))
        }
    }

    /// Delete a peer inside the worker. On success, drops the peer
    /// from the main-thread mirror so palette / peer-selector reads
    /// stop returning it. Returns `true` on success; `false` if the
    /// worker refused (e.g. trying to delete the primary peer).
    pub fn delete_peer(
        &self,
        peer_id: String,
    ) -> impl std::future::Future<Output = Result<bool, String>> + 'static {
        let proxy = self.proxy.clone();
        let peers_mirror = self.peers.clone();
        async move {
            match proxy.delete_peer(peer_id.clone()).await {
                Ok(()) => {
                    peers_mirror.borrow_mut().retain(|p| p.peer_id != peer_id);
                    Ok(true)
                }
                Err(e) => {
                    tracing::warn!(peer_id = %peer_id, error = ?e, "proxy.delete_peer failed");
                    Ok(false)
                }
            }
        }
    }

    /// Update peer metadata (label, persisted flag, listen addresses)
    /// inside the worker. Parity-C, PROTOCOL_VERSION=5. On success,
    /// updates the main-thread mirror so palette / peer-selector reads
    /// pick up the new label immediately.
    ///
    /// The create-peer flow already folds metadata into
    /// `CreatePeerOk` (worker calls `sdk.set_metadata` internally),
    /// so this method exists for future rename / label-edit UI
    /// surfaces, not for any current consumer call site.
    #[allow(dead_code)]
    pub fn set_metadata(
        &self,
        peer_id: String,
        metadata: PeerMetadata,
    ) -> impl std::future::Future<Output = Result<(), String>> + 'static {
        let proxy = self.proxy.clone();
        let peers_mirror = self.peers.clone();
        async move {
            let wire = WirePeerMetadata {
                label: metadata.label.clone(),
                persisted: metadata.persisted,
                listen_addresses: metadata.listen_addresses.clone(),
            };
            proxy
                .set_metadata(peer_id.clone(), wire)
                .await
                .map_err(|e| format!("proxy.set_metadata: {e:?}"))?;
            // Mirror update: replace metadata for this peer if present.
            // No-op if peer isn't local-known (worker is authoritative
            // anyway — the call would have errored out there).
            if let Some(info) = peers_mirror
                .borrow_mut()
                .iter_mut()
                .find(|p| p.peer_id == peer_id)
            {
                info.metadata = metadata;
            }
            Ok(())
        }
    }

    /// Connect to a remote peer via the worker's connector. Parity-D-narrow,
    /// PROTOCOL_VERSION=5. Returns the remote peer's id on success — the
    /// caller can construct `entity://{remote_pid}/...` URIs for
    /// subsequent execute round-trips against it.
    ///
    /// `peer_id` is the local peer initiating the connection (typically
    /// the primary). Connection state is pooled inside the worker;
    /// we don't mirror remote-peer info on the main thread (remote
    /// peers aren't in the local-peer list).
    pub fn connect_peer(
        &self,
        peer_id: String,
        address: String,
    ) -> impl std::future::Future<Output = Result<String, String>> + 'static {
        let proxy = self.proxy.clone();
        async move {
            let ok = proxy
                .connect_peer(peer_id, address)
                .await
                .map_err(|e| format!("proxy.connect_peer: {e:?}"))?;
            Ok(ok.remote_peer_id)
        }
    }

    /// Awaitable write. Returns a future that resolves only after the
    /// proxy's per-prefix mirror reflects the new entity (or after the
    /// timeout elapses). Use this when an action handler must transition
    /// view state to display the just-written entity — without
    /// `put_and_wait`, the in-memory state mutation races the
    /// subscription-driven cache update and the reader briefly shows
    /// "no longer available".
    ///
    /// `dispatch_write` remains the right choice for fire-and-forget
    /// state writes that don't gate any subsequent UI transition.
    pub fn put_and_wait(
        &self,
        peer_id: String,
        path: String,
        entity: Entity,
        timeout_ms: u32,
    ) -> impl std::future::Future<Output = Result<(), String>> + 'static {
        let proxy = self.proxy.clone();
        async move {
            let wire = WireEntity::try_from(entity)
                .map_err(|e| format!("Entity→WireEntity conversion: {e}"))?;
            proxy
                .put_and_wait_for_cache(peer_id, path, wire, timeout_ms)
                .await
                .map_err(|e| format!("proxy.put_and_wait_for_cache: {e:?}"))?;
            Ok(())
        }
    }

    /// Durable-authoritative seed: write `default` only if the path is
    /// **absent in the durable tree**, awaiting the result. This is the
    /// Worker-arm half of [`crate::peers::Peers::put_if_absent`] and the
    /// load-bearing primitive of the owned boot-load step
    /// (boot-config-surfaces reframe §2.4).
    ///
    /// **Why `proxy.get` and not [`Self::cache_get`]:** the cache mirror
    /// is fed only for *subscribed* prefixes and is cold at boot, so a
    /// mirror read would report a persisted value as absent → clobber it.
    /// `proxy.get` is an L1 round-trip into the worker's **durable** tree
    /// (OPFS journal already replayed before Ready), so absence is
    /// authoritative ([[feedback_worker_cache_get_needs_subscription]]).
    ///
    /// **Ordering (load-bearing):** the `get` runs *first*. In the
    /// single-threaded wasm executor its `.await` is the yield that lets
    /// any queued `watch_prefix` → `observe` tasks register their
    /// subscription in the proxy registry. So by the time the write path
    /// calls `put_and_wait_for_cache`, the relevant prefix is `covered`
    /// and the cache reflection is honored rather than silently skipped
    /// (`wasm-worker-proxy` `put_and_wait_for_cache`). This is what makes
    /// the seed readable on the first frame without leaning on incidental
    /// boot-write timing — the Phase-21 race.
    ///
    /// Returns `Ok(true)` if it seeded, `Ok(false)` if a value was already
    /// present (no write), `Err` on transport failure.
    pub fn put_if_absent(
        &self,
        peer_id: String,
        path: String,
        default: Entity,
        timeout_ms: u32,
    ) -> impl std::future::Future<Output = Result<bool, String>> + 'static {
        let proxy = self.proxy.clone();
        async move {
            // Durable, authoritative absence check (NOT the cache mirror).
            let existing = proxy
                .get(peer_id.clone(), path.clone())
                .await
                .map_err(|e| format!("proxy.get (put_if_absent): {e:?}"))?;
            if existing.is_some() {
                return Ok(false);
            }
            let wire = WireEntity::try_from(default)
                .map_err(|e| format!("Entity→WireEntity conversion: {e}"))?;
            proxy
                .put_and_wait_for_cache(peer_id, path, wire, timeout_ms)
                .await
                .map_err(|e| format!("proxy.put_and_wait_for_cache (put_if_absent): {e:?}"))?;
            Ok(true)
        }
    }

    /// Fire-and-forget write. Spawns a wasm-bindgen-futures task that
    /// calls `proxy.put(...).await`; logs on success/failure. Returns
    /// immediately. Per Rev 6 cache invariant #4, the cache reflects
    /// the write only after the worker round-trips a Change event back.
    pub fn dispatch_write(&self, peer_id: String, path: String, entity: Entity) {
        let proxy = self.proxy.clone();
        let wire = match WireEntity::try_from(entity) {
            Ok(w) => w,
            Err(err) => {
                tracing::warn!(peer_id = %peer_id, path = %path, error = %err, "dispatch_write: Entity→WireEntity conversion failed");
                return;
            }
        };
        wasm_bindgen_futures::spawn_local(async move {
            match proxy.put(peer_id.clone(), path.clone(), wire).await {
                Ok(_hash) => {
                    tracing::trace!(peer_id = %peer_id, path = %path, "dispatch_write: put ok");
                }
                Err(err) => {
                    tracing::warn!(peer_id = %peer_id, path = %path, error = ?err, "dispatch_write: put failed");
                }
            }
        });
    }

    /// Establish a subscription on `prefix` and bridge its
    /// `NotifyChannel` into the window's dirty flag.
    ///
    /// `proxy.observe(prefix)` returns `(SubHandle, NotifyChannel)`.
    /// The `SubHandle` keeps the subscription alive — dropping it
    /// cancels the worker-side subscription and closes the channel.
    /// We stash it on `WindowWatch.worker_subs` so its lifetime matches
    /// the window.
    ///
    /// The `NotifyChannel` is polled by a spawn_local task that flips
    /// the dirty flag on each notification. When the SubHandle drops
    /// (window close), `NotifyChannel.next().await` returns `None` and
    /// the task exits.
    pub fn watch_prefix(&self, watch: &mut WindowWatch, peer_id: String, prefix: String) {
        self.record_subscription(&prefix);
        let proxy = self.proxy.clone();
        let dirty = watch.flag();
        let watch_handle = watch.worker_subs_slot();

        wasm_bindgen_futures::spawn_local(async move {
            // peer_id targets which peer's L1 dispatch the host registers
            // the subscription callback against (PROTOCOL_VERSION=6,
            // §3.8 in the living doc). Without this, the host hardcoded
            // primary's dispatch and non-primary peer subscriptions
            // silently dropped Change events.
            let (sub_handle, mut notify) = match proxy.observe(peer_id.clone(), prefix.clone()).await {
                Ok(pair) => pair,
                Err(err) => {
                    tracing::warn!(peer_id = %peer_id, prefix = %prefix, error = ?err, "observe: subscribe failed");
                    return;
                }
            };

            // Stash the handle on the WindowWatch so it lives as long as
            // the window does. If the WindowWatch was dropped between
            // the await above and now, push() is a no-op (slot dropped).
            watch_handle.attach(sub_handle);

            // Bridge notifications to the dirty flag until the
            // subscription closes.
            while notify.next().await.is_some() {
                dirty.mark();
            }
        });
    }

    /// Worker-arm form of [`crate::peers::Peers::observe_with_events`].
    /// Drives the proxy's `EventChannel` and normalizes per-event
    /// payloads into [`crate::peers::ChangeOp`] for the consumer
    /// callback. Each delivered event also marks the window's dirty
    /// flag so the DOM rebuild path picks it up.
    pub fn observe_with_events(
        &self,
        watch: &mut WindowWatch,
        peer_id: String,
        prefix: String,
        on_event: std::sync::Arc<dyn Fn(crate::peers::ChangeOp) + Send + Sync + 'static>,
    ) {
        self.record_subscription(&prefix);
        let proxy = self.proxy.clone();
        let dirty = watch.flag();
        let watch_handle = watch.worker_subs_slot();

        wasm_bindgen_futures::spawn_local(async move {
            let (sub_handle, _notify, mut events) =
                match proxy.observe_with_events(peer_id.clone(), prefix.clone()).await {
                    Ok(triple) => triple,
                    Err(err) => {
                        tracing::warn!(
                            peer_id = %peer_id,
                            prefix = %prefix,
                            error = ?err,
                            "observe_with_events: subscribe failed"
                        );
                        return;
                    }
                };

            watch_handle.attach(sub_handle);

            while let Some(event) = events.next().await {
                use entity_wasm_worker_proxy::ChangeEvent;
                let op = match event {
                    ChangeEvent::Created { path, .. }
                    | ChangeEvent::Updated { path, .. } => {
                        crate::peers::ChangeOp::Put { path }
                    }
                    ChangeEvent::Removed { path, .. } => {
                        crate::peers::ChangeOp::Remove { path }
                    }
                    ChangeEvent::Lagged { count } => {
                        tracing::warn!(
                            peer_id = %peer_id,
                            prefix = %prefix,
                            count = count,
                            "observe_with_events: lagged — consumer resyncing"
                        );
                        crate::peers::ChangeOp::Resync
                    }
                };
                on_event(op);
                dirty.mark();
            }
        });
    }
}

/// Convert a wire query result into the SDK shape. **Lossy** until the
/// Carries full fidelity since PROTOCOL_VERSION=4: `total`, `cursor`,
/// and per-match `entity_type` all come over the wire. For matches
/// that include the entity inline, we prefer the entity's own
/// `entity_type` (it's the same value, but treats the inlined entity
/// as canonical when present).
fn wire_query_results_into_sdk(
    wire: entity_wasm_worker_protocol::WireQueryResults,
) -> Result<entity_sdk::QueryResults, String> {
    let mut matches = Vec::with_capacity(wire.matches.len());
    for m in wire.matches {
        let hash = Hash::try_from(&m.content_hash)
            .map_err(|e| format!("WireHash→Hash: {e}"))?;
        let (entity, entity_type) = match m.entity {
            Some(we) => {
                let ent = Entity::try_from(we)
                    .map_err(|e| format!("WireEntity→Entity: {e}"))?;
                let ty = ent.entity_type.clone();
                (Some(ent), ty)
            }
            None => (None, m.entity_type),
        };
        matches.push(entity_sdk::QueryMatch {
            path: m.path,
            content_hash: hash,
            entity_type,
            entity,
        });
    }
    Ok(entity_sdk::QueryResults {
        matches,
        has_more: wire.has_more,
        total: wire.total,
        cursor: wire.cursor,
    })
}

//! Cloneable, arm-agnostic write handle for app-tier modules that
//! publish to the system peer's tree.
//!
//! Use this whenever a writer needs to be moved into a spawned future
//! (event log appends, peer-registry bumps, connection records, etc.).
//! The handle owns the underlying transport — `Arc<PeerShared>` for
//! Direct mode or `Rc<WorkerProxy>` for Worker mode — so the call
//! site doesn't need to branch on `Peers` arm or hold a `&Peers`
//! across `.await`.
//!
//! Construct via [`Peers::writer_handle`](crate::peers::Peers::writer_handle).
//! Both `put` and `remove` are fire-and-forget; failures log via
//! `tracing::warn!` but don't surface to the caller. Consumers that
//! need read-after-write should use `Peers::put_and_wait` instead.
//!
//! ## Why this exists
//!
//! Before this module, each app-tier writer
//! (`event_log_writer`, the since-removed peer-registry signal,
//! `connections`) carried its own dual-arm boilerplate: an
//! `Option<Arc<PeerShared>>` field for Direct, an
//! `Option<Rc<WorkerProxy>>` field for Worker, plus matching cfg
//! gates and a per-call branch. The Worker arm was easy to
//! forget, and a stub-only ("trace stub") Worker arm compiled
//! cleanly while silently no-op'ing — which bit us three times in
//! one session (event_log_writer, the peer-registry signal,
//! connections) before the abstraction landed.
//! See WORKER-MODE-LIVING-DOC §3.7 for the history.

use std::sync::Arc;

use entity_entity::Entity;
use entity_peer::PeerShared;

#[cfg(target_arch = "wasm32")]
use std::rc::Rc;
#[cfg(target_arch = "wasm32")]
use entity_wasm_worker_proxy::{WebTransport, WorkerProxy};

#[derive(Clone)]
pub enum WriterHandle {
    /// Direct mode: writes go straight through `PeerShared::tree::put` /
    /// `::remove`. Synchronous, in-process. Errors are logged.
    Direct(Arc<PeerShared>),
    /// Worker mode: writes spawn a `WorkerProxy::put` (or `::remove`)
    /// fire-and-forget task. The peer_id targets which peer's tree the
    /// write lands in; for app-tier writers this is always the system
    /// peer (captured at handle-construction time).
    #[cfg(target_arch = "wasm32")]
    Worker {
        proxy: Rc<WorkerProxy<WebTransport>>,
        peer_id: String,
    },
}

impl WriterHandle {
    /// Fire-and-forget write at `path`. Direct: sync L0 `tree.put`
    /// (errors logged). Worker: spawns `proxy.put` on the local
    /// runtime; the consumer-side `tree` reflects the value once the
    /// worker round-trips a Change event back through any covering
    /// subscription (~10–20 ms).
    pub fn put(&self, path: String, entity: Entity) {
        match self {
            WriterHandle::Direct(shared) => {
                if let Err(e) = shared.tree.put(&path, entity) {
                    tracing::warn!(error = %e, path = %path, "writer: direct put failed");
                }
            }
            #[cfg(target_arch = "wasm32")]
            WriterHandle::Worker { proxy, peer_id } => {
                let wire = match entity_wasm_worker_protocol::WireEntity::try_from(entity) {
                    Ok(w) => w,
                    Err(err) => {
                        tracing::warn!(error = %err, path = %path, "writer: WireEntity conversion failed");
                        return;
                    }
                };
                let proxy_clone = proxy.clone();
                let pid = peer_id.clone();
                let path_for_log = path.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    if let Err(err) = proxy_clone.put(pid, path, wire).await {
                        tracing::warn!(error = ?err, path = %path_for_log, "writer: worker put failed");
                    }
                });
            }
        }
    }

    /// Binding-safe content-store reclaim: drop the blob `hash` **iff** no
    /// live path still binds it (`entity_sdk::content_remove_if_unbound` —
    /// GUIDE-GC pitfall #1). Fire-and-forget; used by app-level retention
    /// policies (bounded save-state) to reclaim the superseded blobs the
    /// append-only content store would otherwise keep forever.
    ///
    /// Direct (IDB) arm only — that's where games run, and where the
    /// content store reclaims a real per-record delete. The Worker/OPFS
    /// arm is a no-op here (its remove would be a soft delete pending log
    /// compaction, and the proxy has no content-remove verb yet); growth
    /// there is bounded later by kernel GC, not this handle.
    pub fn content_remove(&self, hash: entity_hash::Hash) {
        match self {
            WriterHandle::Direct(shared) => {
                let outcome = entity_sdk::content_remove_if_unbound(
                    shared.content_store.as_ref(),
                    shared.location_index.as_ref(),
                    &hash,
                );
                tracing::trace!(?outcome, hash = %hash.to_hex(), "writer: content_remove");
            }
            #[cfg(target_arch = "wasm32")]
            WriterHandle::Worker { .. } => {
                tracing::debug!(
                    hash = %hash.to_hex(),
                    "writer: content_remove skipped on Worker arm (no proxy verb; \
                     OPFS reclaim is kernel-GC's job)"
                );
            }
        }
    }

    /// Fire-and-forget remove at `path`. Same shape as `put`.
    pub fn remove(&self, path: String) {
        match self {
            WriterHandle::Direct(shared) => {
                shared.tree.remove(&path);
            }
            #[cfg(target_arch = "wasm32")]
            WriterHandle::Worker { proxy, peer_id } => {
                let proxy_clone = proxy.clone();
                let pid = peer_id.clone();
                let path_for_log = path.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    if let Err(err) = proxy_clone.remove(pid, path).await {
                        tracing::warn!(error = ?err, path = %path_for_log, "writer: worker remove failed");
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peers::Peers;
    use entity_ecf::{cbor_map, to_ecf};

    fn make_entity() -> Entity {
        let data = to_ecf(&cbor_map! { "marker" => entity_ecf::text("test") });
        Entity::new("app/test/marker", data).expect("entity construction")
    }

    /// Constructing a handle from a Direct `Peers` returns the
    /// Direct variant wired to the primary peer's PeerShared.
    #[test]
    fn direct_writer_handle_is_constructed() {
        let peers = Peers::new_direct();
        let handle = peers.writer_handle().expect("Direct peers yield a handle");
        match handle {
            WriterHandle::Direct(_) => {}
            #[cfg(target_arch = "wasm32")]
            WriterHandle::Worker { .. } => panic!("expected Direct variant"),
        }
    }

    /// `put` through the Direct variant lands in the underlying tree
    /// — verifies the abstraction doesn't drop writes.
    #[test]
    fn direct_put_writes_to_tree() {
        let peers = Peers::new_direct();
        let handle = peers.writer_handle().expect("handle");
        let pid = peers.primary_peer_id().to_string();
        let path = format!("/{}/app/test/marker", pid);
        handle.put(path.clone(), make_entity());

        let read_back = peers.get_entity(&pid, &path).expect("entity present");
        assert_eq!(read_back.entity_type, "app/test/marker");
    }

    /// `remove` through the Direct variant drops the entry.
    #[test]
    fn direct_remove_drops_entry() {
        let peers = Peers::new_direct();
        let handle = peers.writer_handle().expect("handle");
        let pid = peers.primary_peer_id().to_string();
        let path = format!("/{}/app/test/marker", pid);
        handle.put(path.clone(), make_entity());
        assert!(peers.get_entity(&pid, &path).is_some(), "put landed");

        handle.remove(path.clone());
        assert!(peers.get_entity(&pid, &path).is_none(), "remove dropped it");
    }

    /// `writer_handle_for(peer)` round-trips a write keyed to that peer.
    /// The point of the per-peer variant is that on the Worker arm it targets
    /// the peer's OWN store (so a `/{peer}/...` read sees it) instead of the
    /// primary's — native shares one store, so this asserts the API exists and
    /// lands the write where the matching reader looks. Pairs with the
    /// Worker-arm e2e that exercises the cross-store divergence directly.
    #[test]
    fn per_peer_writer_handle_writes_under_its_peer() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let handle = peers
            .writer_handle_for(&pid)
            .expect("per-peer handle for a hosted peer");
        let path = format!("/{}/app/test/per_peer", pid);
        handle.put(path.clone(), make_entity());
        assert!(
            peers.get_entity(&pid, &path).is_some(),
            "per-peer writer lands where get_entity(peer, …) reads"
        );
        // An unrouted peer yields no handle — never a silent fall-through to
        // the primary's store (the divergence this method exists to prevent).
        assert!(peers.writer_handle_for("ghost-peer").is_none());
    }

    /// The handle is `Clone` — required so writers can move clones
    /// into spawned futures. This is a compile-time guarantee but
    /// asserting it in a test makes the intent explicit and locks the
    /// Clone bound in.
    #[test]
    fn handle_is_clone_and_clones_share_target() {
        let peers = Peers::new_direct();
        let handle = peers.writer_handle().expect("handle");
        let clone = handle.clone();

        let pid = peers.primary_peer_id().to_string();
        let path = format!("/{}/app/test/from_clone", pid);
        clone.put(path.clone(), make_entity());

        // Original handle (not the clone) reads via the same Peers —
        // they're two views of the same underlying transport.
        assert!(peers.get_entity(&pid, &path).is_some());
    }
}


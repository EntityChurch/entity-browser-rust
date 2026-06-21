//! Local-mirror cache for chain trace data — continuation entities +
//! chain-error markers — feeding the Chain Trace inspect window.
//!
//! Two subscription prefixes feed a single cache: the continuation
//! family at `/{peer_id}/system/continuation/**` and the chain-error
//! marker family at `/{peer_id}/system/runtime/chain-errors/**` (per
//! EXTENSION-CONTINUATION §3.10 and §6.5, declared local-namespace
//! per AUDIT-PRIVACY-AND-CROSS-PEER §2.5). Both are buildable today —
//! they're path-bound substrate state, no L1 hook required (per
//! GUIDE-INSPECTABILITY v1.2 §2.4 projection table: chain trace
//! composes from entity reader + path enumerator, both of which need
//! no event hooks).
//!
//! At render time the model asks the cache for entries matching a
//! specific `chain_id`; the cache walks its known paths and returns
//! the filtered slice. Path-segment-based parsing — the chain_id
//! lives at segment index 3 for continuations and segment index 5 for
//! chain-error markers (lost or rejected).
//!
//! Pattern mirrors `event_log_cache.rs` — subscription callback
//! receives `ChangeOp`s, applies to inner state, render reads from
//! memory with lazy refill for decoded bodies.

#![allow(dead_code)]

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use entity_entity::Entity;

use crate::peers::{ChangeOp, Peers};

/// Local-mirror cache. Cheap to clone (Arc).
#[derive(Clone, Default, Debug)]
pub struct ChainTraceCache {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default, Debug)]
pub struct Inner {
    /// All continuation paths observed.
    /// Shape: `/{peer_id}/system/continuation/{chain_id}`
    continuation_paths: BTreeSet<String>,
    /// All chain-error marker paths observed.
    /// Shape: `/{peer_id}/system/runtime/chain-errors/{lost|rejected}/{chain_id}/{step_index}/{reason}/{marker_hash}`
    marker_paths: BTreeSet<String>,
    /// Lazy-decoded entity bodies keyed by path.
    decoded: HashMap<String, Entity>,
    /// Worker overflow recovery flag.
    needs_resync: bool,
}

impl ChainTraceCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn inner_arc(&self) -> Arc<Mutex<Inner>> {
        self.inner.clone()
    }

    /// Refill any missing decoded bodies for the chain_id in question
    /// (so render never has to fetch). Bounded by the count of paths
    /// matching the chain_id.
    fn refill_for(&self, peers: &Peers, peer_id: &str, chain_id: &str) {
        let to_fill: Vec<String> = {
            let inner = self.inner.lock().unwrap();
            let mut paths = Vec::new();
            for p in &inner.continuation_paths {
                if !inner.decoded.contains_key(p)
                    && continuation_chain_id(p).map(|c| c == chain_id).unwrap_or(false)
                {
                    paths.push(p.clone());
                }
            }
            for p in &inner.marker_paths {
                if !inner.decoded.contains_key(p)
                    && marker_chain_id(p).map(|c| c == chain_id).unwrap_or(false)
                {
                    paths.push(p.clone());
                }
            }
            paths
        };
        if to_fill.is_empty() {
            return;
        }
        for path in to_fill {
            if let Some(entity) = peers.get_entity(peer_id, &path) {
                self.inner.lock().unwrap().decoded.insert(path, entity);
            }
        }
    }

    /// Read the trace for one `chain_id`. Returns (continuation paths,
    /// marker paths) — both filtered, in lexicographic order.
    /// Marker paths sort by `{lost|rejected}/{chain_id}/{step}/...`
    /// so lexical order ≈ step ordering when step is zero-padded.
    pub fn paths_for_chain(&self, peer_id: &str, chain_id: &str) -> ChainTracePaths {
        let inner = self.inner.lock().unwrap();
        let continuations: Vec<String> = inner
            .continuation_paths
            .iter()
            .filter(|p| {
                continuation_chain_id(p)
                    .map(|c| c == chain_id)
                    .unwrap_or(false)
                    && path_peer_id(p) == Some(peer_id)
            })
            .cloned()
            .collect();
        let markers: Vec<String> = inner
            .marker_paths
            .iter()
            .filter(|p| {
                marker_chain_id(p)
                    .map(|c| c == chain_id)
                    .unwrap_or(false)
                    && path_peer_id(p) == Some(peer_id)
            })
            .cloned()
            .collect();
        ChainTracePaths { continuations, markers }
    }

    /// Decoded entity body for a given path, if cached.
    pub fn decoded(&self, path: &str) -> Option<Entity> {
        self.inner.lock().unwrap().decoded.get(path).cloned()
    }

    /// Refill lazily then read paths + decoded bodies for one chain.
    pub fn snapshot_for(&self, peers: &Peers, peer_id: &str, chain_id: &str) -> ChainTracePaths {
        self.refill_for(peers, peer_id, chain_id);
        self.paths_for_chain(peer_id, chain_id)
    }

    /// True if any path is known for this chain_id (used by the
    /// renderer to distinguish "no chain by that id" from "empty
    /// trace").
    pub fn has_chain(&self, peer_id: &str, chain_id: &str) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.continuation_paths.iter().any(|p| {
            continuation_chain_id(p).map(|c| c == chain_id).unwrap_or(false)
                && path_peer_id(p) == Some(peer_id)
        }) || inner.marker_paths.iter().any(|p| {
            marker_chain_id(p).map(|c| c == chain_id).unwrap_or(false)
                && path_peer_id(p) == Some(peer_id)
        })
    }
}

/// Per-chain path slice returned by [`ChainTraceCache::snapshot_for`].
#[derive(Debug, Clone, Default)]
pub struct ChainTracePaths {
    pub continuations: Vec<String>,
    pub markers: Vec<String>,
}

/// Subscription callback for the continuation-prefix observe. Apply
/// one [`ChangeOp`] to the cache, classifying the path as a
/// continuation entry.
pub fn apply_continuation_change(inner: &Mutex<Inner>, op: ChangeOp) {
    let mut guard = inner.lock().unwrap();
    match op {
        ChangeOp::Put { path } => {
            guard.continuation_paths.insert(path.clone());
            guard.decoded.remove(&path);
        }
        ChangeOp::Remove { path } => {
            guard.continuation_paths.remove(&path);
            guard.decoded.remove(&path);
        }
        ChangeOp::Resync => {
            guard.needs_resync = true;
        }
    }
}

/// Subscription callback for the chain-errors-prefix observe.
pub fn apply_marker_change(inner: &Mutex<Inner>, op: ChangeOp) {
    let mut guard = inner.lock().unwrap();
    match op {
        ChangeOp::Put { path } => {
            guard.marker_paths.insert(path.clone());
            guard.decoded.remove(&path);
        }
        ChangeOp::Remove { path } => {
            guard.marker_paths.remove(&path);
            guard.decoded.remove(&path);
        }
        ChangeOp::Resync => {
            guard.needs_resync = true;
        }
    }
}

/// Extract chain_id from a continuation path
/// (`/{peer_id}/system/continuation/{chain_id}`).
fn continuation_chain_id(path: &str) -> Option<&str> {
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    if segments.len() >= 4
        && segments[1] == "system"
        && segments[2] == "continuation"
    {
        Some(segments[3])
    } else {
        None
    }
}

/// Extract chain_id from a chain-error marker path
/// (`/{peer_id}/system/runtime/chain-errors/{lost|rejected}/{chain_id}/...`).
fn marker_chain_id(path: &str) -> Option<&str> {
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    if segments.len() >= 6
        && segments[1] == "system"
        && segments[2] == "runtime"
        && segments[3] == "chain-errors"
        && (segments[4] == "lost" || segments[4] == "rejected")
    {
        Some(segments[5])
    } else {
        None
    }
}

/// Extract peer_id from any tree path.
fn path_peer_id(path: &str) -> Option<&str> {
    path.trim_start_matches('/').split('/').next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuation_path_parses() {
        assert_eq!(
            continuation_chain_id("/PEER1/system/continuation/CHAIN_A"),
            Some("CHAIN_A"),
        );
        assert_eq!(continuation_chain_id("/PEER1/system/other"), None);
    }

    #[test]
    fn marker_path_parses_both_kinds() {
        assert_eq!(
            marker_chain_id(
                "/PEER1/system/runtime/chain-errors/lost/CHAIN_A/0/timeout/0xabc",
            ),
            Some("CHAIN_A"),
        );
        assert_eq!(
            marker_chain_id(
                "/PEER1/system/runtime/chain-errors/rejected/CHAIN_B/2/cap_denied/0xdef",
            ),
            Some("CHAIN_B"),
        );
        assert_eq!(
            marker_chain_id("/PEER1/system/runtime/chain-errors/lost"),
            None,
        );
    }

    #[test]
    fn marker_path_rejects_unknown_marker_kind() {
        // Forward-compatible: only "lost" or "rejected" today. Other
        // segment values are not chain-error markers.
        assert_eq!(
            marker_chain_id(
                "/PEER1/system/runtime/chain-errors/other-kind/CHAIN_A/0/r/0xabc",
            ),
            None,
        );
    }

    #[test]
    fn apply_put_inserts_path() {
        let inner = Arc::new(Mutex::new(Inner::default()));
        apply_continuation_change(
            &inner,
            ChangeOp::Put { path: "/P/system/continuation/C1".into() },
        );
        let guard = inner.lock().unwrap();
        assert!(guard.continuation_paths.contains("/P/system/continuation/C1"));
    }

    #[test]
    fn apply_remove_drops_path_and_decoded() {
        let cache = ChainTraceCache::new();
        let inner = cache.inner_arc();
        apply_marker_change(
            &inner,
            ChangeOp::Put {
                path: "/P/system/runtime/chain-errors/lost/C1/0/r/0xabc".into(),
            },
        );
        apply_marker_change(
            &inner,
            ChangeOp::Remove {
                path: "/P/system/runtime/chain-errors/lost/C1/0/r/0xabc".into(),
            },
        );
        let guard = inner.lock().unwrap();
        assert!(guard.marker_paths.is_empty());
    }

    #[test]
    fn paths_for_chain_filters_by_chain_id_and_peer() {
        let cache = ChainTraceCache::new();
        let inner = cache.inner_arc();
        apply_continuation_change(
            &inner,
            ChangeOp::Put { path: "/PEER1/system/continuation/CHAIN_A".into() },
        );
        apply_continuation_change(
            &inner,
            ChangeOp::Put { path: "/PEER1/system/continuation/CHAIN_B".into() },
        );
        apply_continuation_change(
            &inner,
            ChangeOp::Put { path: "/PEER2/system/continuation/CHAIN_A".into() },
        );
        apply_marker_change(
            &inner,
            ChangeOp::Put {
                path: "/PEER1/system/runtime/chain-errors/lost/CHAIN_A/0/timeout/0xabc".into(),
            },
        );

        let paths = cache.paths_for_chain("PEER1", "CHAIN_A");
        assert_eq!(paths.continuations, vec!["/PEER1/system/continuation/CHAIN_A"]);
        assert_eq!(
            paths.markers,
            vec!["/PEER1/system/runtime/chain-errors/lost/CHAIN_A/0/timeout/0xabc"],
        );
    }

    // -- End-to-end pipeline: B1 marker bound by substrate → cache observes via subscription -------

    #[tokio::test]
    async fn substrate_written_marker_flows_into_cache_via_subscription() {
        use crate::peers::Peers;
        use crate::window_watch::WindowWatch;
        use entity_ecf::{cbor_map, text, to_ecf};
        use entity_entity::Entity;

        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();

        // Install the same subscription the window factory does.
        let cache = ChainTraceCache::new();
        let inner = cache.inner_arc();
        let mut watch = WindowWatch::new();
        let chain_errors_prefix = format!("/{pid}/system/runtime/chain-errors/");
        peers.observe_with_events(
            &mut watch,
            &pid,
            chain_errors_prefix,
            move |op| apply_marker_change(&inner, op),
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // Place a marker entity at the canonical §3.10 path.
        let chain_id = "CHAIN_VALIDATION_42";
        let marker_path = format!(
            "/{pid}/system/runtime/chain-errors/lost/{chain_id}/sub-1/max_events_reached/0xabcdef",
        );
        let marker = Entity::new(
            "system/runtime/chain-error-lost",
            to_ecf(&cbor_map! {
                "chain_id" => text(chain_id),
                "reason" => text("max_events_reached"),
                "code" => text("429"),
            }),
        )
        .unwrap();
        peers.dispatch_write(&pid, marker_path.clone(), marker);

        // The subscription event must reach the cache.
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;

        assert!(
            cache.has_chain(&pid, chain_id),
            "cache must observe substrate-written markers via the \
             prefix subscription — this is the B1+pipeline bridge \
             for Chain Trace window",
        );
        let paths = cache.paths_for_chain(&pid, chain_id);
        assert!(paths.markers.contains(&marker_path));

        // snapshot_for refills decoded bodies so the renderer can
        // surface them (subject to RenderPolicy).
        let snap = cache.snapshot_for(&peers, &pid, chain_id);
        assert_eq!(snap.markers.len(), 1);
        let decoded = cache.decoded(&marker_path).unwrap();
        assert_eq!(decoded.entity_type, "system/runtime/chain-error-lost");

        drop(watch);
    }

    #[tokio::test]
    async fn cache_is_peer_scoped_no_defaults_to_primary() {
        // Defends against the `feedback_audit_peer_scoping` class of
        // bugs (defaults-to-primary). A marker bound on one peer must
        // not appear in queries against a different peer_id.
        use crate::peers::Peers;
        use crate::window_watch::WindowWatch;
        use entity_ecf::{text, to_ecf};
        use entity_entity::Entity;

        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let other_pid = "OTHER_PEER_ID";

        let cache = ChainTraceCache::new();
        let inner = cache.inner_arc();
        let mut watch = WindowWatch::new();
        let prefix = format!("/{pid}/system/runtime/chain-errors/");
        peers.observe_with_events(
            &mut watch,
            &pid,
            prefix,
            move |op| apply_marker_change(&inner, op),
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let chain_id = "CHAIN_X";
        let marker_path = format!(
            "/{pid}/system/runtime/chain-errors/lost/{chain_id}/sub-1/timeout/0x111",
        );
        let entity = Entity::new(
            "system/runtime/chain-error-lost",
            to_ecf(&text("body")),
        )
        .unwrap();
        peers.dispatch_write(&pid, marker_path, entity);
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;

        assert!(cache.has_chain(&pid, chain_id), "marker visible on bound peer");
        assert!(
            !cache.has_chain(other_pid, chain_id),
            "marker MUST NOT leak across peer boundary",
        );

        drop(watch);
    }

    #[test]
    fn has_chain_distinguishes_empty_from_unknown() {
        let cache = ChainTraceCache::new();
        let inner = cache.inner_arc();
        apply_marker_change(
            &inner,
            ChangeOp::Put {
                path: "/PEER1/system/runtime/chain-errors/lost/CHAIN_A/0/timeout/0xabc".into(),
            },
        );
        assert!(cache.has_chain("PEER1", "CHAIN_A"));
        assert!(!cache.has_chain("PEER1", "UNKNOWN_CHAIN"));
        assert!(!cache.has_chain("OTHER_PEER", "CHAIN_A"));
    }
}

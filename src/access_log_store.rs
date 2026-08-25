//! App-tier access-log store — a process-global ring of dispatched accesses,
//! recorded at the **`ops::execute` chokepoint** (the one place every
//! app-issued L1 execute funnels through, local OR remote).
//!
//! Why this exists / why not the per-peer inspect sink: the inspect sink fires
//! `InspectFact::Dispatch` on the peer that *executes the handler body*. A file
//! transfer `entity://{B}/local/files list` executes on **B**, so its Dispatch
//! fact fires in B's process — the local caller is structurally blind to it, and
//! browsing your own tree (a cache/subscription read) never dispatches at all.
//! Result: a near-empty log that misses exactly the accesses an operator cares
//! about. This store taps one layer higher — the *request*, before it routes —
//! so it records the `(actor, target, operation, resource, outcome)` tuple for
//! every execute this app issues, remote transfers included. See
//! `RESEARCH-CAPABILITY-MANAGEMENT-UX §4 Step 1` and the 2026-07-13 handoff.
//!
//! **Actor is load-bearing:** each row records *who* made the request
//! (`ExecuteRequest.peer_id`), not only the target — an access log without the
//! requesting peer can't answer "who is doing this."
//!
//! Ephemeral per session (a ring, not tree-backed). WASM is single-threaded but
//! the native tests aren't, so the ring is `Arc<Mutex>`. It's a process-global
//! (`OnceLock`) because `ops::execute` is a free function with no app handle to
//! thread through; the window holds a clone of the same ring and subscribes a
//! dirty flag so a new record re-renders it (the store isn't tree-backed, so a
//! `WindowWatch` prefix subscription would never fire).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

use crate::window_watch::DirtyFlag;

/// Max completed accesses retained; older rows drop from the front. Sized for a
/// visual scan of recent activity (matches the Inspect-family ring caps).
pub const RING_CAP: usize = 200;

/// The result of one dispatched operation, classified from its status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessOutcome {
    /// A 2xx — the operation was permitted and succeeded.
    Allowed,
    /// A 401/403 — a capability refusal (the enforcement signal).
    Denied,
    /// Any other non-2xx, or a transport/dispatch failure — a real error, not
    /// an authorization decision.
    Error,
}

/// Which way the access crossed the boundary — so the one unified log can show
/// both "I called out" and "someone called me".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessDirection {
    /// This app dispatched a handler on a *remote* peer (e.g. a file transfer).
    /// `actor` is a local peer; `target_peer` is the remote.
    Outbound,
    /// A local handler dispatch on one of this app's own peers (queries, counts,
    /// local executes). `actor` is the local peer.
    Local,
    /// A *remote* peer reached into this device's backend share. `actor` is the
    /// remote caller; the target is the backend peer. Sourced from the native
    /// backend's `backend_access_log_tail` IPC.
    Inbound,
}

/// One access-log entry: a completed operation, its actor, target, and outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessEntry {
    /// Which way the access crossed the boundary.
    pub direction: AccessDirection,
    /// The peer that *made* the request — the "who". A local peer for
    /// Outbound/Local; the remote caller for Inbound.
    pub actor: String,
    /// Target peer id, from an `entity://{peer}/...` handler URI; `None` for a
    /// local/self dispatch (a bare handler path like `system/tree`).
    pub target_peer: Option<String>,
    /// The handler invoked — the "where" (e.g. `local/files`, `system/tree`).
    pub handler: String,
    /// The operation verb — the "how" (e.g. `list`, `read`, `write`).
    pub operation: String,
    /// The resource path the operation targeted, if any (from
    /// `ExecuteOptions.resource`).
    pub resource: Option<String>,
    /// The classified result.
    pub outcome: AccessOutcome,
    /// Human-readable detail for the tooltip — the status code or error text.
    pub detail: String,
}

#[derive(Default)]
struct Inner {
    ring: VecDeque<AccessEntry>,
    /// Windows subscribed for re-render on a new record. Marked (not read)
    /// here; the DOM frame loop reads + clears each flag.
    subscribers: Vec<DirtyFlag>,
}

/// Process-global access-log ring. Cheap to clone (shared `Arc<Mutex>`).
#[derive(Clone)]
pub struct AccessStore {
    inner: Arc<Mutex<Inner>>,
}

impl AccessStore {
    fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(Inner::default())) }
    }

    /// Append a completed access and wake every subscribed window.
    pub fn record(&self, entry: AccessEntry) {
        let mut g = self.inner.lock().unwrap();
        g.ring.push_back(entry);
        while g.ring.len() > RING_CAP {
            g.ring.pop_front();
        }
        for f in &g.subscribers {
            f.mark();
        }
    }

    /// Register a window's dirty flag so future records re-render it.
    pub fn subscribe(&self, flag: DirtyFlag) {
        self.inner.lock().unwrap().subscribers.push(flag);
    }

    /// Snapshot of all retained accesses, newest first.
    pub fn snapshot_newest_first(&self) -> Vec<AccessEntry> {
        let g = self.inner.lock().unwrap();
        g.ring.iter().rev().cloned().collect()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().ring.len()
    }
}

/// The one process-global store. `ops::execute` records into it; the Access Log
/// window reads a clone.
pub fn global() -> &'static AccessStore {
    static STORE: OnceLock<AccessStore> = OnceLock::new();
    STORE.get_or_init(AccessStore::new)
}

/// Split a handler URI into `(target_peer, handler_path)`. `entity://{peer}/{rest}`
/// yields the remote peer and the handler; a bare local path yields `(None, path)`.
pub fn parse_target(uri: &str) -> (Option<String>, String) {
    if let Some(rest) = uri.strip_prefix("entity://") {
        let mut parts = rest.splitn(2, '/');
        let peer = parts.next().unwrap_or("").to_string();
        let handler = parts.next().unwrap_or("").to_string();
        (Some(peer).filter(|p| !p.is_empty()), handler)
    } else {
        (None, uri.to_string())
    }
}

/// Classify a dispatch status code into an access outcome. 401/403 are the
/// capability refusal — the enforcement signal we most want visible.
pub fn classify(status: u32) -> AccessOutcome {
    match status {
        200..=299 => AccessOutcome::Allowed,
        401 | 403 => AccessOutcome::Denied,
        _ => AccessOutcome::Error,
    }
}

/// Fold an inspect-sink `Dispatch` fact into an access row for a **local**
/// dispatch on `actor` (the peer the sink is bound to). Returns `None` for
/// entry-phase facts (`status == 0`, not yet resolved) and non-`Dispatch`
/// variants — the caller records only what this returns.
///
/// This is the local half of the log (queries, counts, local executes); the
/// outbound-remote half comes from `ops::execute`. See the module doc.
pub fn dispatch_to_entry(
    actor: &str,
    fact: &entity_sdk::InspectFact,
) -> Option<AccessEntry> {
    let entity_sdk::InspectFact::Dispatch { handler_uri, operation, status, .. } = fact else {
        return None;
    };
    if *status == 0 {
        return None; // entry phase — wait for the resolved exit fact
    }
    let (target_peer, handler) = parse_target(handler_uri);
    Some(AccessEntry {
        direction: AccessDirection::Local,
        actor: actor.to_string(),
        target_peer,
        handler,
        operation: operation.clone(),
        resource: None, // the Dispatch fact doesn't carry the resource path
        outcome: classify(*status),
        detail: format!("status {status}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(actor: &str, uri: &str, op: &str, outcome: AccessOutcome) -> AccessEntry {
        let (target_peer, handler) = parse_target(uri);
        AccessEntry {
            direction: AccessDirection::Outbound,
            actor: actor.into(),
            target_peer,
            handler,
            operation: op.into(),
            resource: None,
            outcome,
            detail: String::new(),
        }
    }

    #[test]
    fn parses_remote_and_local_targets() {
        assert_eq!(
            parse_target("entity://PEER_B/local/files"),
            (Some("PEER_B".to_string()), "local/files".to_string())
        );
        assert_eq!(parse_target("system/tree"), (None, "system/tree".to_string()));
        assert_eq!(parse_target("entity://PEER_B"), (Some("PEER_B".to_string()), String::new()));
    }

    #[test]
    fn classifies_outcomes() {
        assert_eq!(classify(200), AccessOutcome::Allowed);
        assert_eq!(classify(204), AccessOutcome::Allowed);
        assert_eq!(classify(403), AccessOutcome::Denied);
        assert_eq!(classify(401), AccessOutcome::Denied);
        assert_eq!(classify(500), AccessOutcome::Error);
    }

    #[test]
    fn record_captures_actor_and_target() {
        let store = AccessStore::new();
        store.record(entry("PEER_S", "entity://PEER_B/local/files", "read", AccessOutcome::Denied));
        let rows = store.snapshot_newest_first();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].actor, "PEER_S");
        assert_eq!(rows[0].target_peer.as_deref(), Some("PEER_B"));
        assert_eq!(rows[0].handler, "local/files");
        assert_eq!(rows[0].operation, "read");
        assert_eq!(rows[0].outcome, AccessOutcome::Denied);
    }

    #[test]
    fn ring_caps_and_newest_first() {
        let store = AccessStore::new();
        for i in 0..(RING_CAP + 10) {
            store.record(entry("A", "system/tree", &format!("op{i}"), AccessOutcome::Allowed));
        }
        assert_eq!(store.len(), RING_CAP);
        let rows = store.snapshot_newest_first();
        assert_eq!(rows[0].operation, format!("op{}", RING_CAP + 9), "newest first");
    }

    fn dispatch(uri: &str, op: &str, status: u32) -> entity_sdk::InspectFact {
        entity_sdk::InspectFact::Dispatch {
            request_id: "r".into(),
            handler_uri: uri.into(),
            operation: op.into(),
            status,
            elapsed_micros: None,
            chain_id: None,
        }
    }

    #[test]
    fn dispatch_fact_folds_to_entry_exit_phase_only() {
        // Entry phase (status 0) is skipped.
        assert!(dispatch_to_entry("S", &dispatch("system/tree", "get", 0)).is_none());
        // Exit phase becomes a row, actor = the bound peer.
        let e = dispatch_to_entry("PEER_S", &dispatch("system/handler", "query", 200)).unwrap();
        assert_eq!(e.direction, AccessDirection::Local);
        assert_eq!(e.actor, "PEER_S");
        assert_eq!(e.target_peer, None, "a bare handler is a local dispatch");
        assert_eq!(e.handler, "system/handler");
        assert_eq!(e.operation, "query");
        assert_eq!(e.outcome, AccessOutcome::Allowed);
    }

    #[test]
    fn non_dispatch_facts_are_ignored() {
        let wire = entity_sdk::InspectFact::Wire {
            direction: entity_sdk::InspectWireFrameDirection::Outbound,
            peer_remote: Some("B".to_string()),
            frame_kind: "ExecuteRequest".into(),
            bytes: 100,
            request_id: Some("r".to_string()),
        };
        assert!(dispatch_to_entry("S", &wire).is_none());
    }

    #[test]
    fn subscribers_are_marked_on_record() {
        use crate::window_watch::WindowWatch;
        let store = AccessStore::new();
        let watch = WindowWatch::new();
        watch.take_dirty(); // clear the initial set flag
        store.subscribe(watch.flag());
        assert!(!watch.take_dirty(), "no record yet → not dirty");
        store.record(entry("A", "system/tree", "op", AccessOutcome::Allowed));
        assert!(watch.take_dirty(), "record should mark the subscribed window dirty");
    }
}

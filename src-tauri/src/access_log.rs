//! Inbound access log for the backend peer(s) — "who reached into my share, and
//! did I allow it." The security counterpart to `backend_log` (which streams B's
//! `tracing` output): this streams the *access events* on B's own handlers.
//!
//! **Why two hooks correlated by `request_id`:** a `DispatchEvent` carries the
//! *what* (`target_uri`, `operation`) and the *outcome* (exit `status` →
//! allow/deny) but NOT the caller identity — the inspect fact-tuple
//! (GUIDE-INSPECTABILITY v1.2 §2.1 #3) is metadata-only. The *who* lives on the
//! `WireEvent` (`peer_address` = the authenticated remote PeerID, §2.1 #5). Both
//! facts share the envelope's `request_id`, so we stitch them: a `Recv` wire
//! frame parks `(request_id → caller)`, and the matching dispatch `Exit` emits
//! one access row `{caller, target_uri, operation, status}`. No kernel change —
//! both hooks are the public `PeerBuilder::with_{wire,dispatch}_hook` surface.
//!
//! A dispatch with no parked caller is a *local / internal* op on B (not an
//! inbound wire request), so it's skipped — this ring is inbound-only by
//! construction, which is exactly the "who accessed me" question.
//!
//! Transport mirrors `backend_log`: an in-memory ring, polled over the
//! `backend_access_log_tail` IPC by the WebView, which folds the rows into the
//! app-tier access log (`crate::access_log_store`, direction = Inbound).

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use entity_peer::{DispatchEvent, DispatchPhase, WireDirection, WireEvent};
use serde::Serialize;

/// Max buffered access rows (ring — oldest drop first). Sized for a useful scan
/// of recent inbound activity without unbounded growth on a busy share.
const RING_CAP: usize = 2000;

/// Cap on parked in-flight callers. A request that never dispatches (dropped
/// connection, non-dispatch frame) would otherwise leak; if the map blows past
/// this, clear it — worst case a few rows lose their caller, never a memory leak.
const PENDING_CAP: usize = 4096;

/// One inbound access: who called, what they hit, and the outcome. `seq` is the
/// monotonic cursor the client advances past (never repeats for the process
/// lifetime), so a poll is a simple "everything after `after`".
#[derive(Clone, Serialize)]
pub struct AccessRecord {
    pub seq: u64,
    /// Authenticated caller PeerID (base58) — the "who".
    pub caller: String,
    /// Resolved handler target URI on this peer — the "what/where".
    pub target_uri: String,
    pub operation: String,
    /// Handler exit status (V7 §8.3). 2xx allowed; 401/403 denied; else error.
    pub status: u32,
    pub timestamp_ms: u64,
}

/// Response for the `backend_access_log_tail` IPC. `cursor` is the exclusive
/// high-water mark to pass as `after` next poll — stable even when `records` is
/// empty, so a caller that misses a poll still advances.
#[derive(Serialize)]
pub struct AccessTail {
    pub records: Vec<AccessRecord>,
    pub cursor: u64,
}

struct State {
    /// Parked callers keyed by `request_id`, set on the inbound wire frame,
    /// consumed on the matching dispatch exit.
    pending: HashMap<String, String>,
    next_seq: u64,
    ring: VecDeque<AccessRecord>,
}

impl State {
    fn new() -> Self {
        Self {
            pending: HashMap::new(),
            next_seq: 0,
            ring: VecDeque::new(),
        }
    }
}

/// Process-global correlation state. Lazy (not const-init) because `HashMap::new`
/// isn't const — unlike `backend_log`'s VecDeque-only ring.
fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::new()))
}

/// Wire-hook side: park the authenticated caller for an inbound frame so the
/// matching dispatch can attribute it. Only `Recv` frames with a known peer
/// (post-handshake) are relevant; sends and handshake frames are ignored.
pub fn on_wire(ev: &WireEvent) {
    if ev.direction != WireDirection::Recv || ev.peer_address.is_empty() {
        return;
    }
    if let Ok(mut st) = state().lock() {
        if st.pending.len() >= PENDING_CAP {
            st.pending.clear(); // defensive: never leak in-flight entries
        }
        st.pending
            .insert(ev.request_id.clone(), ev.peer_address.clone());
    }
}

/// Dispatch-hook side: on the exit event, stitch the parked caller (if any) with
/// the target + outcome and append one access row. A dispatch with no parked
/// caller is a local/internal op — skipped (this ring is inbound-only).
pub fn on_dispatch(ev: &DispatchEvent) {
    let DispatchPhase::Exit { status, .. } = ev.phase else {
        return; // entry phase carries no outcome
    };
    if let Ok(mut st) = state().lock() {
        let Some(caller) = st.pending.remove(&ev.request_id) else {
            return; // not an inbound wire request
        };
        let seq = st.next_seq;
        st.next_seq += 1;
        // Surface each captured inbound access in the backend log too (parallel to
        // `backend_log`) — an at-a-glance audit line, and it makes the stream
        // observable without the WebView.
        log::info!(
            "inbound access: caller={} {} {} -> status {}",
            &caller[..12.min(caller.len())],
            ev.operation,
            ev.target_uri,
            status
        );
        st.ring.push_back(AccessRecord {
            seq,
            caller,
            target_uri: ev.target_uri.clone(),
            operation: ev.operation.clone(),
            status,
            timestamp_ms: ev.timestamp_ms,
        });
        while st.ring.len() > RING_CAP {
            st.ring.pop_front();
        }
    }
}

/// Return every buffered access with `seq >= after`, plus the new cursor. A
/// fresh client passes `after = 0`; it then passes back the returned `cursor`.
pub fn tail(after: u64) -> AccessTail {
    let st = state().lock().unwrap_or_else(|e| e.into_inner());
    let records: Vec<AccessRecord> = st
        .ring
        .iter()
        .filter(|r| r.seq >= after)
        .cloned()
        .collect();
    AccessTail {
        records,
        cursor: st.next_seq,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset() {
        let mut st = state().lock().unwrap();
        *st = State::new();
    }

    fn wire(request_id: &str, peer: &str, dir: WireDirection) -> WireEvent {
        WireEvent {
            direction: dir,
            request_id: request_id.into(),
            frame_bytes: vec![],
            peer_address: peer.into(),
            timestamp_ms: 0,
        }
    }

    fn dispatch_exit(request_id: &str, uri: &str, op: &str, status: u32) -> DispatchEvent {
        DispatchEvent {
            target_uri: uri.into(),
            operation: op.into(),
            params_hash: entity_hash::Hash::zero(),
            request_id: request_id.into(),
            timestamp_ms: 0,
            phase: DispatchPhase::Exit {
                status,
                response_hash: entity_hash::Hash::zero(),
            },
        }
    }

    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn wire_then_dispatch_stitches_caller_target_outcome() {
        let _g = SERIAL.lock().unwrap();
        reset();
        on_wire(&wire("r1", "PEER_CALLER", WireDirection::Recv));
        on_dispatch(&dispatch_exit("r1", "local/files/shared", "read", 403));
        let tail = tail(0);
        assert_eq!(tail.records.len(), 1);
        assert_eq!(tail.records[0].caller, "PEER_CALLER");
        assert_eq!(tail.records[0].target_uri, "local/files/shared");
        assert_eq!(tail.records[0].operation, "read");
        assert_eq!(tail.records[0].status, 403);
        assert_eq!(tail.cursor, 1);
    }

    #[test]
    fn dispatch_without_wire_is_skipped_local_op() {
        let _g = SERIAL.lock().unwrap();
        reset();
        // No parked caller → a local/internal dispatch, not inbound.
        on_dispatch(&dispatch_exit("r-local", "system/tree", "get", 200));
        assert_eq!(tail(0).records.len(), 0);
    }

    #[test]
    fn entry_phase_and_sends_are_ignored() {
        let _g = SERIAL.lock().unwrap();
        reset();
        // A Send frame parks nothing.
        on_wire(&wire("r2", "PEER", WireDirection::Send));
        // Entry-phase dispatch is ignored.
        let entry = DispatchEvent {
            target_uri: "local/files".into(),
            operation: "list".into(),
            params_hash: entity_hash::Hash::zero(),
            request_id: "r2".into(),
            timestamp_ms: 0,
            phase: DispatchPhase::Entry,
        };
        on_dispatch(&entry);
        assert_eq!(tail(0).records.len(), 0);
    }

    #[test]
    fn tail_after_cursor_returns_only_new() {
        let _g = SERIAL.lock().unwrap();
        reset();
        on_wire(&wire("a", "P", WireDirection::Recv));
        on_dispatch(&dispatch_exit("a", "local/files", "list", 200));
        let first = tail(0);
        assert_eq!(first.records.len(), 1);
        on_wire(&wire("b", "P", WireDirection::Recv));
        on_dispatch(&dispatch_exit("b", "local/files", "read", 200));
        let next = tail(first.cursor);
        assert_eq!(next.records.len(), 1);
        assert_eq!(next.records[0].operation, "read");
    }

    #[test]
    fn ring_caps_at_capacity() {
        let _g = SERIAL.lock().unwrap();
        reset();
        for i in 0..(RING_CAP + 25) {
            let id = format!("r{i}");
            on_wire(&wire(&id, "P", WireDirection::Recv));
            on_dispatch(&dispatch_exit(&id, "local/files", &format!("op{i}"), 200));
        }
        let tail = tail(0);
        assert_eq!(tail.records.len(), RING_CAP);
        assert_eq!(tail.records[0].operation, "op25", "oldest dropped");
    }
}

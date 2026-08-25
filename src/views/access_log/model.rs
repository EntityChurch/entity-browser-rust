//! Access Log — a bounded ring of completed dispatch operations, fed by the
//! same `InspectFact::Dispatch` stream as Path Tap (via
//! `Peers::install_inspect_sink`), but reframed as a **user-facing access log**:
//! target · operation · outcome (`RESEARCH-CAPABILITY-MANAGEMENT-UX §4 Step 1`).
//!
//! Why its own ring (not `PathTapRing`): the raw dispatch stream fires twice per
//! op — an entry fact (`status == 0`) then an exit fact with the result. Path Tap
//! keeps both (a raw dev trace); the access log keeps **only the exit phase**, so
//! each completed access is exactly one row. Same substrate, different retention.
//!
//! Ephemeral per session (a ring, not tree-backed) — the durable, subscribable
//! audit surface + the aggregated minimal-permission map are the next steps; this
//! is the live view.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use entity_sdk::InspectFact;

use super::output::{AccessEntry, AccessLogOutput, AccessOutcome};
use crate::window::WindowId;

/// Max completed accesses retained; older rows drop from the front. Sized for a
/// visual scan of recent activity (matches the Inspect-family ring caps).
pub const RING_CAP: usize = 200;

#[derive(Clone, Default)]
pub struct AccessRing {
    inner: Arc<Mutex<VecDeque<AccessEntry>>>,
}

impl AccessRing {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold a fact into the ring. Only **exit-phase** `Dispatch` facts become
    /// rows (one per completed access); entry-phase (`status == 0`) and
    /// non-Dispatch variants are ignored.
    pub fn push(&self, fact: &InspectFact) {
        let InspectFact::Dispatch { handler_uri, operation, status, .. } = fact else {
            return;
        };
        if *status == 0 {
            return; // entry phase — wait for the resolved exit fact
        }
        let (peer, handler) = parse_target(handler_uri);
        let entry = AccessEntry {
            peer,
            handler,
            operation: operation.clone(),
            outcome: classify(*status),
            status: *status,
        };
        let mut g = self.inner.lock().unwrap();
        g.push_back(entry);
        while g.len() > RING_CAP {
            g.pop_front();
        }
    }

    pub fn snapshot_newest_first(&self) -> Vec<AccessEntry> {
        let g = self.inner.lock().unwrap();
        g.iter().rev().cloned().collect()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }
}

/// Split a handler URI into (target peer, handler path). `entity://{peer}/{rest}`
/// yields the peer and the handler; a bare local path yields `(None, path)`.
pub(crate) fn parse_target(uri: &str) -> (Option<String>, String) {
    if let Some(rest) = uri.strip_prefix("entity://") {
        let mut parts = rest.splitn(2, '/');
        let peer = parts.next().unwrap_or("").to_string();
        let handler = parts.next().unwrap_or("").to_string();
        (Some(peer).filter(|p| !p.is_empty()), handler)
    } else {
        (None, uri.to_string())
    }
}

/// Classify a dispatch status into an access outcome. 401/403 are the capability
/// refusal — the enforcement signal we most want visible.
pub(crate) fn classify(status: u32) -> AccessOutcome {
    match status {
        0 => AccessOutcome::Pending,
        200..=299 => AccessOutcome::Allowed,
        401 | 403 => AccessOutcome::Denied,
        _ => AccessOutcome::Error,
    }
}

pub struct AccessLogModel {
    ring: AccessRing,
    routing_active: bool,
}

impl AccessLogModel {
    // `window_id`/`peer_id` accepted for factory-signature parity, not stored —
    // a passive sink-fed window; the renderer reads no identity.
    pub fn new(_window_id: WindowId, _peer_id: String) -> Self {
        Self {
            ring: AccessRing::new(),
            routing_active: false,
        }
    }

    pub fn ring(&self) -> AccessRing {
        self.ring.clone()
    }

    pub fn mark_routing_active(&mut self) {
        self.routing_active = true;
    }

    pub fn render_output(&self) -> AccessLogOutput {
        AccessLogOutput {
            routing_active: self.routing_active,
            entries: self.ring.snapshot_newest_first(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dispatch(uri: &str, op: &str, status: u32) -> InspectFact {
        InspectFact::Dispatch {
            request_id: "r".into(),
            handler_uri: uri.into(),
            operation: op.into(),
            status,
            elapsed_micros: None,
            chain_id: None,
        }
    }

    #[test]
    fn entry_phase_is_dropped_only_exit_is_logged() {
        let ring = AccessRing::new();
        ring.push(&dispatch("entity://B/local/files", "list", 0)); // entry
        assert_eq!(ring.len(), 0, "entry phase is not an access row");
        ring.push(&dispatch("entity://B/local/files", "list", 200)); // exit
        assert_eq!(ring.len(), 1);
    }

    #[test]
    fn parses_remote_and_local_targets() {
        assert_eq!(
            parse_target("entity://PEER_B/local/files"),
            (Some("PEER_B".to_string()), "local/files".to_string())
        );
        assert_eq!(parse_target("system/tree"), (None, "system/tree".to_string()));
        // Peer with no trailing handler.
        assert_eq!(parse_target("entity://PEER_B"), (Some("PEER_B".to_string()), String::new()));
    }

    #[test]
    fn classifies_outcomes() {
        assert_eq!(classify(200), AccessOutcome::Allowed);
        assert_eq!(classify(204), AccessOutcome::Allowed);
        assert_eq!(classify(403), AccessOutcome::Denied);
        assert_eq!(classify(401), AccessOutcome::Denied);
        assert_eq!(classify(500), AccessOutcome::Error);
        assert_eq!(classify(0), AccessOutcome::Pending);
    }

    #[test]
    fn entry_captures_target_op_and_outcome() {
        let ring = AccessRing::new();
        ring.push(&dispatch("entity://PEER_B/local/files", "read", 403));
        let rows = ring.snapshot_newest_first();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].peer.as_deref(), Some("PEER_B"));
        assert_eq!(rows[0].handler, "local/files");
        assert_eq!(rows[0].operation, "read");
        assert_eq!(rows[0].outcome, AccessOutcome::Denied);
    }

    #[test]
    fn ring_caps_and_newest_first() {
        let ring = AccessRing::new();
        for i in 0..(RING_CAP + 10) {
            ring.push(&dispatch("system/tree", &format!("op{i}"), 200));
        }
        assert_eq!(ring.len(), RING_CAP);
        let rows = ring.snapshot_newest_first();
        assert_eq!(rows[0].operation, format!("op{}", RING_CAP + 9), "newest first");
    }
}

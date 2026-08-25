//! Access Log model — a thin reader over the app-tier
//! [`crate::access_log_store`] global ring.
//!
//! The store is fed at the `ops::execute` chokepoint (every app-issued execute,
//! local OR remote — see the store's module doc for why not the per-peer inspect
//! sink). The window subscribes its dirty flag to the store so a new access
//! re-renders it; this model just snapshots the ring on render.
//!
//! Ephemeral per session — the durable, subscribable audit surface + the
//! aggregated minimal-permission map are the next steps; this is the live view.

use std::collections::HashMap;

use crate::access_log_store::{self, AccessStore};
use crate::peers::Peers;
use crate::window::WindowId;

use super::output::{subject_key, AccessLogOutput, DirectionFilter, PeerOption};

/// Stable subject key for inbound rows when the System backend peer id isn't
/// resolvable yet (it registers asynchronously at boot). Base58 peer ids never
/// start with `@`, so this can't collide with a real actor.
pub const SYSTEM_BACKEND_KEY: &str = "@system-backend";

pub struct AccessLogModel {
    store: AccessStore,
}

impl AccessLogModel {
    // `window_id`/`peer_id` accepted for factory-signature parity, not stored —
    // the access store is app-global, not per-peer (a file transfer's actor and
    // target are both recorded, whichever peer this window is bound to).
    pub fn new(_window_id: WindowId, _peer_id: String) -> Self {
        Self {
            store: access_log_store::global().clone(),
        }
    }

    pub fn store(&self) -> &AccessStore {
        &self.store
    }

    /// Snapshot the ring (newest first), attribute each row to its **subject
    /// peer** (whose access log it is), build the peer dropdown, and narrow to the
    /// active direction + peer filters. Filtering here (not in the DOM) keeps the
    /// rebuilt row count proportional to what's shown — the point of the dropdowns
    /// when one peer/direction floods.
    ///
    /// Peers is needed to resolve the System backend peer (so inbound rows read as
    /// "Native backend", not a raw caller) and to label each subject.
    pub fn render_output(
        &self,
        peers: &Peers,
        direction: DirectionFilter,
        peer_filter: &str,
    ) -> AccessLogOutput {
        let all = self.store.snapshot_newest_first();

        // The System backend peer (System + Native), if provisioned — inbound
        // access is attributed to it. Same structural test the System Overview
        // uses (no magic label).
        let modes = crate::persistence::peer_modes();
        let backend_pid = crate::peer_registry::read_registry(peers)
            .into_iter()
            .map(|r| r.peer_id)
            .find(|pid| {
                let d = crate::peer_display::PeerDescriptor::describe(peers, pid, &modes);
                d.role == crate::peer_display::PeerRole::System
                    && d.runtime == crate::peer_display::PeerRuntime::Native
            });
        let backend_key = backend_pid
            .clone()
            .unwrap_or_else(|| SYSTEM_BACKEND_KEY.to_string());
        let system_pid = peers.system_peer_id().to_string();

        // Label every distinct subject present, preserving first-seen order but
        // floating the two named system peers to the top of the dropdown.
        let mut labels: HashMap<String, String> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for e in &all {
            let key = subject_key(e, &backend_key);
            if !labels.contains_key(&key) {
                let label = label_for(peers, &key, &system_pid, backend_pid.as_deref(), &modes);
                labels.insert(key.clone(), label);
                order.push(key);
            }
        }
        order.sort_by_key(|k| subject_rank(k, &system_pid, &backend_key));
        let peer_options: Vec<PeerOption> = order
            .iter()
            .map(|k| PeerOption { key: k.clone(), label: labels[k].clone() })
            .collect();

        let entries = all
            .into_iter()
            .filter(|e| direction.matches(e.direction))
            .filter(|e| peer_filter.is_empty() || subject_key(e, &backend_key) == peer_filter)
            .collect();

        AccessLogOutput {
            entries,
            direction,
            peer_filter: peer_filter.to_string(),
            peer_options,
            backend_key,
            labels,
        }
    }
}

/// A friendly label for a subject peer — the two named system peers get plain,
/// operator-legible names; anything else falls back to its descriptor + short id.
fn label_for(
    peers: &Peers,
    key: &str,
    system_pid: &str,
    backend_pid: Option<&str>,
    modes: &HashMap<String, crate::peer_mode::PeerMode>,
) -> String {
    // Canonical UI names (see docs/architecture/specs/TERMINOLOGY-AND-WINDOWS.md):
    // the frontend peer is the "System peer"; the native backend is the
    // "System backend". One name each, used on every surface.
    if key == SYSTEM_BACKEND_KEY || Some(key) == backend_pid {
        return "System backend".to_string();
    }
    if key == system_pid {
        return "System peer".to_string();
    }
    let d = crate::peer_display::PeerDescriptor::describe(peers, key, modes);
    format!("{} · {}", d.role_name(), crate::views::short_pid(key))
}

/// Sort key: system peer first, native backend second, everyone else after (by
/// key) — a stable, predictable dropdown ordering.
fn subject_rank(key: &str, system_pid: &str, backend_key: &str) -> (u8, String) {
    if key == system_pid {
        (0, String::new())
    } else if key == backend_key {
        (1, String::new())
    } else {
        (2, key.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access_log_store::{AccessDirection, AccessEntry, AccessOutcome};

    fn entry(actor: &str, op: &str) -> AccessEntry {
        AccessEntry {
            direction: AccessDirection::Local,
            actor: actor.into(),
            target_peer: None,
            handler: "system/tree".into(),
            operation: op.into(),
            resource: None,
            outcome: AccessOutcome::Allowed,
            detail: "status 200".into(),
        }
    }

    fn inbound(actor: &str, op: &str) -> AccessEntry {
        AccessEntry {
            direction: AccessDirection::Inbound,
            actor: actor.into(),
            target_peer: None,
            handler: "local/files".into(),
            operation: op.into(),
            resource: None,
            outcome: AccessOutcome::Allowed,
            detail: "status 200".into(),
        }
    }

    #[test]
    fn render_output_reflects_the_store() {
        // The model reads the app-global store, shared across parallel tests, so
        // assert on presence of a uniquely-tagged row rather than exact counts.
        let peers = Peers::new_direct();
        let model = AccessLogModel::new(1, "peer".into());
        let uniq = "op-model-render-test";
        model.store().record(entry("PEER_S", uniq));
        let rows = model.render_output(&peers, DirectionFilter::All, "").entries;
        assert!(
            rows.iter().any(|e| e.operation == uniq && e.actor == "PEER_S"),
            "recorded row should be visible in render_output"
        );
    }

    #[test]
    fn direction_filter_narrows_to_one_direction() {
        let peers = Peers::new_direct();
        let model = AccessLogModel::new(1, "peer".into());
        // A uniquely-tagged Local row (the app-global store is shared across
        // parallel tests, so assert on the tag, not counts).
        let uniq = "op-filter-local-test";
        model.store().record(entry("PEER_S", uniq)); // entry() is Local
        // Inbound must not appear when filtering to Local.
        let local = model.render_output(&peers, DirectionFilter::Local, "").entries;
        assert!(local.iter().any(|e| e.operation == uniq), "Local row shows under Local");
        let inbound = model.render_output(&peers, DirectionFilter::Inbound, "").entries;
        assert!(
            !inbound.iter().any(|e| e.operation == uniq),
            "a Local row must not appear under the Inbound filter"
        );
    }

    #[test]
    fn peer_filter_narrows_to_one_subject_and_inbound_buckets_to_backend() {
        let peers = Peers::new_direct();
        let model = AccessLogModel::new(1, "peer".into());
        let out_op = "op-peer-out-test";
        let in_op = "op-peer-in-test";
        model.store().record(entry("PEER_ACTOR_X", out_op)); // Local, subject = actor
        model.store().record(inbound("REMOTE_CALLER", in_op)); // Inbound, subject = backend

        // Inbound rows are attributed to the native-backend key, not the caller.
        let out = model.render_output(&peers, DirectionFilter::All, &model_backend_key());
        assert!(out.entries.iter().any(|e| e.operation == in_op), "inbound shows under the backend subject");
        assert!(
            !out.entries.iter().any(|e| e.operation == out_op),
            "a local actor's row must not appear under the backend subject"
        );

        // Filtering to the local actor shows its row, not the inbound one.
        let byactor = model.render_output(&peers, DirectionFilter::All, "PEER_ACTOR_X");
        assert!(byactor.entries.iter().any(|e| e.operation == out_op));
        assert!(!byactor.entries.iter().any(|e| e.operation == in_op));
    }

    // No native backend is registered in this unit test, so inbound buckets under
    // the sentinel key.
    fn model_backend_key() -> String {
        SYSTEM_BACKEND_KEY.to_string()
    }
}

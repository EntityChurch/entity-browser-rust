//! Peer liveness — the subscribed connection-liveness facet of the unified
//! Peer read-model (Piece A of `DESIGN-CONNECTIVITY-UX-2026-08-09`; P2/§F2 of
//! `DESIGN-BROWSER-CONNECTIVITY-WEBRTC-AND-LIVENESS`).
//!
//! Projects the **kernel-owned** reactive liveness surface —
//! `system/peer/status/{remote_hex}` (EXTENSION-NETWORK Amendment 12 §A3): the
//! 3-state `connected` / `suspect` / `disconnected` + a transition `reason` —
//! into an app read-model. This is the replacement for the event-sourced
//! [`crate::connection_health`] mirror: instead of the app *writing* the
//! liveness it guesses from connect attempts, it **subscribes** the liveness the
//! kernel already observes — including the reactive keepalive-miss disconnect
//! the app never saw. No mirror, no write path (the substrate rule: subscribe,
//! don't poll; no fourth parallel store — AP12 / D1 / D8).
//!
//! **Join key.** The status entity *body* carries the remote's Base58 `peer_id`
//! ([`PeerStatusData::peer_id`], distinct from the hex path segment), so this
//! read-model keys by the same Base58 `remote_pid` as the [`crate::connections`]
//! registry and the `connection_health` mirror — a direct join for shadow-parity
//! now and the unified `Peer` object later.
//!
//! **Worker-arm rule.** A caller MUST `watch_prefix(peer_status_prefix(vantage))`
//! for each vantage it reads, or the Worker-arm cache mirror is unseeded and the
//! read is empty (same cache-seeding rule as every other subscribed surface).

use entity_entity::Entity;
use entity_peer::peer_status::{
    PeerStatusData, PEER_STATUS_CONNECTED, PEER_STATUS_DISCONNECTED, PEER_STATUS_SUSPECT,
    TYPE_PEER_STATUS,
};

use crate::peers::Peers;

/// The 3-state kernel liveness, projected (plus `Unknown` for "no status entity
/// yet"). The app's old `Connecting` (dial-in-progress) is **not** a kernel
/// state — it is a local, in-flight fact the connect action owns, layered on top
/// at render time, never mirrored here. Calm display vocabulary
/// (`DESIGN-CONNECTIVITY-UX §4c`) lives in [`PeerLiveness::label`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LiveStatus {
    /// No `system/peer/status` entity for this remote yet.
    #[default]
    Unknown,
    Connected,
    /// Transient failure — renders "Reconnecting…", never red (§4c).
    Suspect,
    Disconnected,
}

impl LiveStatus {
    fn from_kernel(status: &str) -> Self {
        match status {
            PEER_STATUS_CONNECTED => LiveStatus::Connected,
            PEER_STATUS_SUSPECT => LiveStatus::Suspect,
            PEER_STATUS_DISCONNECTED => LiveStatus::Disconnected,
            // MUST-ignore-unknown: a forward-compat status value we don't know
            // reads as Unknown rather than crashing the projection.
            _ => LiveStatus::Unknown,
        }
    }

    /// Whether the connection is live enough to dispatch over right now.
    pub fn is_connected(self) -> bool {
        matches!(self, LiveStatus::Connected)
    }

    /// Rank for merging the same remote seen from multiple local vantages —
    /// the most-alive view wins.
    fn rank(self) -> u8 {
        match self {
            LiveStatus::Connected => 3,
            LiveStatus::Suspect => 2,
            LiveStatus::Disconnected => 1,
            LiveStatus::Unknown => 0,
        }
    }
}

/// One remote peer's liveness, projected from its kernel status entity.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PeerLiveness {
    /// Remote Base58 `peer_id` (from the entity body) — the join key.
    pub remote_pid: String,
    pub status: LiveStatus,
    /// Kernel transition reason (`keepalive-miss`, `transport-error`,
    /// `retry-exhausted`, …). Present on a demotion, absent on `connected`.
    pub reason: Option<String>,
    /// ms since epoch, last message received — a snapshot taken at a transition
    /// write, never a cadence refresh.
    pub last_seen: Option<u64>,
    /// ms since epoch the current failure episode began; absent ⇒ not failing.
    pub failing_since: Option<u64>,
}

impl PeerLiveness {
    /// Decode a `system/peer/status` entity into a read-model row. `None` on a
    /// type / CBOR mismatch — a malformed entity is skipped, never a panic.
    pub fn from_status_entity(entity: &Entity) -> Option<Self> {
        let d = PeerStatusData::from_entity(entity).ok()?;
        Some(Self {
            remote_pid: d.peer_id,
            status: LiveStatus::from_kernel(&d.status),
            reason: d.reason,
            last_seen: d.last_seen,
            failing_since: d.failing_since,
        })
    }

    /// Calm, non-alarming display label (`DESIGN-CONNECTIVITY-UX §4c`). The
    /// failure reason is surfaced only on a real disconnect; colour / severity
    /// is the renderer's job (Piece C) — this is the vocabulary.
    pub fn label(&self) -> String {
        match self.status {
            LiveStatus::Connected => "Connected".to_string(),
            LiveStatus::Suspect => "Reconnecting…".to_string(),
            LiveStatus::Disconnected => match &self.reason {
                Some(r) => format!("Offline · {r}"),
                None => "Offline".to_string(),
            },
            LiveStatus::Unknown => "—".to_string(),
        }
    }
}

/// The fully-qualified tree prefix where `vantage_pid`'s view of remote-peer
/// liveness lives: `/{vantage}/system/peer/status/`. Watch this (per vantage)
/// to seed the Worker-arm cache and repaint on a kernel transition.
pub fn peer_status_prefix(vantage_pid: &str) -> String {
    format!("/{vantage_pid}/{TYPE_PEER_STATUS}/")
}

/// Read `vantage_pid`'s view of every remote peer's liveness from the kernel
/// status surface. Rows sorted by `remote_pid`. Empty when nothing is connected
/// — or, in the Worker arm, when the prefix wasn't watched.
pub fn read_peer_liveness(peers: &Peers, vantage_pid: &str) -> Vec<PeerLiveness> {
    let prefix = peer_status_prefix(vantage_pid);
    let mut entries = peers.tree_listing(vantage_pid, &prefix);
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
        .into_iter()
        .filter_map(|entry| {
            // One level under the prefix — `{prefix}{remote_hex}`. Skip empties
            // / any deeper path (defensive; the surface never nests).
            let seg = entry.path.strip_prefix(&prefix)?;
            if seg.is_empty() || seg.contains('/') {
                return None;
            }
            let e = peers.get_entity(vantage_pid, &entry.path)?;
            PeerLiveness::from_status_entity(&e)
        })
        .collect()
}

/// Merge every LOCAL peer's view of remote liveness into one read-model, keyed
/// by `remote_pid` — the app's unified "is this peer live" answer regardless of
/// which local peer holds the connection. When a remote appears under more than
/// one vantage, the most-alive view wins (Connected > Suspect > Disconnected),
/// tie-broken by the freshest `last_seen`. Rows sorted by `remote_pid`.
///
/// Callers must have watched each vantage's [`peer_status_prefix`] (Worker-arm
/// cache seeding).
pub fn read_peer_liveness_all(peers: &Peers) -> Vec<PeerLiveness> {
    use std::collections::BTreeMap;
    let mut best: BTreeMap<String, PeerLiveness> = BTreeMap::new();
    for vantage in peers.peer_ids() {
        for row in read_peer_liveness(peers, &vantage) {
            match best.get(&row.remote_pid) {
                Some(existing) if !supersedes(&row, existing) => {}
                _ => {
                    best.insert(row.remote_pid.clone(), row);
                }
            }
        }
    }
    best.into_values().collect()
}

/// Does `candidate` represent a more-authoritative liveness than `current`?
fn supersedes(candidate: &PeerLiveness, current: &PeerLiveness) -> bool {
    match candidate.status.rank().cmp(&current.status.rank()) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => candidate.last_seen >= current.last_seen,
    }
}

/// Look up one remote's merged liveness, `Unknown` if it has no status entity.
pub fn liveness_of(peers: &Peers, remote_pid: &str) -> LiveStatus {
    read_peer_liveness_all(peers)
        .into_iter()
        .find(|r| r.remote_pid == remote_pid)
        .map(|r| r.status)
        .unwrap_or_default()
}

/// **Piece A shadow-parity probe** (`DESIGN-CONNECTIVITY-UX` handoff, Piece A
/// gate). Logs the kernel liveness surface alongside the old
/// [`crate::connection_health`] mirror so we can eyeball convergence in the live
/// build / e2e before migrating consumers off the mirror (Piece B). Read-only,
/// dev-observability; carries no behaviour. WASM-only (the shadow is validated
/// against the live substrate, not native unit tests, which already prove the
/// read path).
#[cfg(target_arch = "wasm32")]
pub fn log_shadow_parity(peers: &Peers) {
    let kernel = read_peer_liveness_all(peers);
    if kernel.is_empty() {
        return;
    }
    for row in &kernel {
        let mirror = crate::connection_health::read(peers, &row.remote_pid);
        // Coarse convergence: do both agree the peer is up (or not)?
        let agree = row.status.is_connected()
            == matches!(mirror, crate::connection_health::Liveness::Connected);
        tracing::debug!(
            remote = %row.remote_pid,
            kernel = ?row.status,
            reason = ?row.reason,
            mirror = ?mirror,
            agree,
            "P2.0 shadow-parity: kernel system/peer/status vs connection_health mirror"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use entity_peer::peer_status::{
        PEER_STATUS_REASON_KEEPALIVE_MISS, PEER_STATUS_REASON_TRANSPORT_ERROR,
    };

    fn status_entity(peer_id: &str, status: &str, reason: Option<&str>) -> Entity {
        let mut d = PeerStatusData::bare(peer_id, status);
        d.reason = reason.map(|s| s.to_string());
        d.last_seen = Some(1_000);
        d.to_entity()
    }

    #[test]
    fn decodes_connected() {
        let e = status_entity("REMOTE_B", PEER_STATUS_CONNECTED, None);
        let row = PeerLiveness::from_status_entity(&e).expect("decodes");
        assert_eq!(row.remote_pid, "REMOTE_B");
        assert_eq!(row.status, LiveStatus::Connected);
        assert_eq!(row.reason, None);
        assert_eq!(row.label(), "Connected");
    }

    #[test]
    fn decodes_disconnected_with_keepalive_miss() {
        // The exact reactive mid-session-death signal the app never had before.
        let e = status_entity(
            "REMOTE_B",
            PEER_STATUS_DISCONNECTED,
            Some(PEER_STATUS_REASON_KEEPALIVE_MISS),
        );
        let row = PeerLiveness::from_status_entity(&e).expect("decodes");
        assert_eq!(row.status, LiveStatus::Disconnected);
        assert_eq!(row.reason.as_deref(), Some("keepalive-miss"));
        assert_eq!(row.label(), "Offline · keepalive-miss");
    }

    #[test]
    fn decodes_suspect() {
        let e = status_entity(
            "REMOTE_B",
            PEER_STATUS_SUSPECT,
            Some(PEER_STATUS_REASON_TRANSPORT_ERROR),
        );
        let row = PeerLiveness::from_status_entity(&e).unwrap();
        assert_eq!(row.status, LiveStatus::Suspect);
        assert_eq!(row.label(), "Reconnecting…");
    }

    #[test]
    fn unknown_status_value_projects_unknown() {
        let e = status_entity("REMOTE_B", "some-future-state", None);
        let row = PeerLiveness::from_status_entity(&e).unwrap();
        assert_eq!(row.status, LiveStatus::Unknown);
    }

    #[test]
    fn wrong_type_entity_is_skipped() {
        // Valid CBOR body (empty map, 0xa0), wrong entity_type — the type check
        // in `from_entity` rejects it before the body is even parsed.
        let e = Entity::new("app/entity-browser/connection", vec![0xa0]).unwrap();
        assert!(PeerLiveness::from_status_entity(&e).is_none());
    }

    #[test]
    fn prefix_is_fully_qualified() {
        assert_eq!(peer_status_prefix("PID"), "/PID/system/peer/status/");
    }

    #[test]
    fn reads_liveness_from_seeded_tree() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let ctx = peers.test_seed_ctx(&pid);
        let prefix = peer_status_prefix(&pid);

        // Seed two kernel status entities directly (L0), as the kernel would.
        ctx.store()
            .put(
                &format!("{prefix}aa01"),
                status_entity("REMOTE_A", PEER_STATUS_CONNECTED, None),
            )
            .unwrap();
        ctx.store()
            .put(
                &format!("{prefix}bb02"),
                status_entity(
                    "REMOTE_B",
                    PEER_STATUS_DISCONNECTED,
                    Some(PEER_STATUS_REASON_KEEPALIVE_MISS),
                ),
            )
            .unwrap();

        let rows = read_peer_liveness(&peers, &pid);
        assert_eq!(rows.len(), 2, "both status entities read back");
        // Sorted by path (aa01 < bb02).
        assert_eq!(rows[0].remote_pid, "REMOTE_A");
        assert_eq!(rows[0].status, LiveStatus::Connected);
        assert_eq!(rows[1].remote_pid, "REMOTE_B");
        assert_eq!(rows[1].status, LiveStatus::Disconnected);
        assert_eq!(rows[1].reason.as_deref(), Some("keepalive-miss"));

        assert_eq!(liveness_of(&peers, "REMOTE_A"), LiveStatus::Connected);
        assert_eq!(liveness_of(&peers, "REMOTE_B"), LiveStatus::Disconnected);
        assert_eq!(liveness_of(&peers, "NEVER_SEEN"), LiveStatus::Unknown);
    }

    #[test]
    fn merge_prefers_most_alive_across_vantages() {
        // Same remote seen Disconnected from one vantage, Connected from another
        // → the merged answer is Connected (most-alive wins).
        let disc = PeerLiveness {
            remote_pid: "R".into(),
            status: LiveStatus::Disconnected,
            reason: Some("keepalive-miss".into()),
            last_seen: Some(10),
            failing_since: Some(10),
        };
        let conn = PeerLiveness {
            remote_pid: "R".into(),
            status: LiveStatus::Connected,
            reason: None,
            last_seen: Some(20),
            failing_since: None,
        };
        assert!(supersedes(&conn, &disc));
        assert!(!supersedes(&disc, &conn));
    }
}

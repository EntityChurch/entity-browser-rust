//! Authoritative peer roster — the durable, **public** spawn list.
//!
//! Replaces the localStorage `entity_peers` lines as the source of truth
//! for *which peers exist* (set A in the lifecycle model). One PUBLIC
//! entity per peer under the system peer's tree at
//! `system/roster/{derived_id}`, carrying `{peer_id, mode, label}` — keyed
//! on the **seed-derived id** (the BUG-A invariant: identity is always
//! re-derived, never the stored field).
//!
//! **No private key material lives here** (design §13 inv. 8,
//! user-mandated). The tree is authoritative for the peer *set*; the
//! localStorage **vault** holds the secrets, keyed by id. Boot reads the
//! roster, then fetches each peer's private key from the vault to spawn it.
//! A vault↔roster mismatch is a detectable, cleanable edge case — not the
//! structural authority-split BUG-A was.
//!
//! Relationship to [`crate::peer_registry`]: that module owns the *derived*
//! public **display** set (`system/peers/`, reconciled against the live
//! host set for rendering). This roster is the *authoritative* spawn list
//! (`system/roster/`). Kept on distinct prefixes so the source of truth and
//! the display projection stay separable through the migration; they
//! converge once boot reads from here.
//!
//! **Status: foundation (Brick 3).** Defined + unit-tested; NOT yet wired
//! into boot or the identity-op write paths. Brick 4 dual-writes this
//! alongside `entity_peers`, verifies they agree, then flips boot onto it
//! with checkpoint-flush on create/delete/mode-change.

use entity_ecf::{cbor_map, to_ecf};
use entity_entity::Entity;

use crate::app_paths;
use crate::peer_mode::PeerMode;
use crate::peers::Peers;
use crate::writer_handle::WriterHandle;

/// Entity type name for authoritative roster records.
pub const ROSTER_ENTRY_TYPE: &str = "app/entity-browser/peer-roster-entry";

/// Roster-record schema version, written as a `schema` field for symmetry with
/// the `entity_peers` vault marker (`vault_codec::VAULT_VERSION`). The CBOR map
/// is already field-extensible (unknown keys ignored, missing keys defaulted),
/// so this is an explicit forward-compat marker rather than a hard requirement.
/// MIGRATION INVARIANT (MAP §8): bump only alongside a `from_entity` branch that
/// migrates the prior layout.
pub const ROSTER_SCHEMA_VERSION: u32 = 1;

/// One peer's authoritative, PUBLIC roster facts — sufficient to spawn the
/// peer once its private key is fetched from the vault by id. Carries no
/// secret material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterEntry {
    /// Seed-derived id — the authoritative identity. Never the stored
    /// `peer_id` field of the legacy record (that drift was BUG-A).
    pub peer_id: String,
    /// Backend role: `frontend` | `backend-memory` | `backend-opfs`.
    pub mode: PeerMode,
    /// User label; `None` when unset (consumers fall back to a short id).
    pub label: Option<String>,
}

impl RosterEntry {
    pub fn to_entity(&self) -> Entity {
        let data = to_ecf(&cbor_map! {
            "schema" => entity_ecf::integer(ROSTER_SCHEMA_VERSION as i64),
            "peer_id" => entity_ecf::text(&self.peer_id),
            "mode" => entity_ecf::text(self.mode.persist_key()),
            "label" => entity_ecf::text(self.label.as_deref().unwrap_or(""))
        });
        Entity::new(ROSTER_ENTRY_TYPE, data).expect("roster entity construction is infallible")
    }

    pub fn from_entity(entity: &Entity) -> Option<Self> {
        let value: ciborium::Value = ciborium::from_reader(entity.data.as_slice()).ok()?;
        let map = value.as_map()?;

        let mut peer_id = String::new();
        let mut mode = PeerMode::Frontend;
        let mut label: Option<String> = None;

        for (k, v) in map {
            match k.as_text() {
                Some("peer_id") => {
                    if let Some(s) = v.as_text() {
                        peer_id = s.to_string();
                    }
                }
                Some("mode") => {
                    if let Some(s) = v.as_text() {
                        // Unknown/missing mode → Frontend: the never-drop-a-peer
                        // default, matching `entity_peers` mode parsing so a
                        // future mode string can't silently lose a peer.
                        mode = PeerMode::from_persist_key(s).unwrap_or(PeerMode::Frontend);
                    }
                }
                Some("label") => {
                    if let Some(s) = v.as_text() {
                        if !s.is_empty() {
                            label = Some(s.to_string());
                        }
                    }
                }
                // `schema` is the forward-compat version marker; only v1 exists,
                // so nothing branches on it yet (a real layout change adds the
                // migration here). Tolerated like any unknown key.
                Some("schema") => {}
                _ => {}
            }
        }

        if peer_id.is_empty() {
            return None;
        }
        Some(Self {
            peer_id,
            mode,
            label,
        })
    }
}

/// Upsert one roster entry into the system peer's tree. Public-only; the
/// caller supplies the handle (works on both arms). Content-addressed, so
/// re-putting an unchanged entry is idempotent at the store level.
#[allow(dead_code)] // Brick 4: wired into create/mode-change identity ops
pub fn put_entry(handle: &WriterHandle, system_peer_id: &str, entry: &RosterEntry) {
    let path = app_paths::roster_entry_path(app_paths::APP_ID, system_peer_id, &entry.peer_id);
    handle.put(path, entry.to_entity());
}

/// Remove one roster entry (on delete). Keyed on the seed-derived id.
#[allow(dead_code)] // Brick 4: wired into the delete path (checkpoint-flushed)
pub fn remove_entry(handle: &WriterHandle, system_peer_id: &str, peer_id: &str) {
    let path = app_paths::roster_entry_path(app_paths::APP_ID, system_peer_id, peer_id);
    handle.remove(path);
}

/// Read the authoritative roster from the system peer's tree, sorted by
/// peer id.
///
/// Unlike the display registry's `read_registry`, this is **NOT** reconciled
/// against the live host set — it IS the authoritative source of which peers
/// *should* be hosted (boot reads it to decide spawns). Reconciliation runs
/// the other way: the live set is built from this roster + the vault.
#[allow(dead_code)] // Brick 4: boot reads spawns from here
pub fn read_roster(peers: &Peers) -> Vec<RosterEntry> {
    let sys = peers.system_peer_id();
    let prefix = app_paths::roster_prefix(app_paths::APP_ID, sys);
    let mut entries = peers.tree_listing(sys, &prefix);
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
        .into_iter()
        .filter_map(|e| {
            peers
                .get_entity(sys, &e.path)
                .and_then(|ent| RosterEntry::from_entity(&ent))
        })
        .collect()
}

/// Authoritative async read of the roster — the cross-arm version of
/// [`read_roster`]. **Use this at boot / in the reconcile gate**, never the
/// sync `read_roster`, on any path that may run under the Worker arm: the
/// Worker sync mirror only sees subscribed prefixes, so a sync read of the
/// (unwatched) roster prefix returns silently empty
/// (`feedback_worker_cache_get_needs_subscription`). This routes through the
/// L1 `List` round-trip (`Peers::tree_listing_async`) + per-entry
/// `get_entity_async`, which are subscription-independent.
///
/// Sorted by peer id. Decode failures drop the offending entry with a warn.
#[allow(dead_code)] // Brick 3: reconcile gate + backfill; Brick 4: boot-flip spawn source
pub async fn read_roster_async(peers: &Peers) -> Vec<RosterEntry> {
    let sys = peers.system_peer_id().to_string();
    let prefix = app_paths::roster_prefix(app_paths::APP_ID, &sys);
    let mut entries = match peers.tree_listing_async(&sys, &prefix).await {
        Ok(e) => e,
        Err(err) => {
            tracing::warn!(error = %err, "read_roster_async: list failed; treating roster as empty");
            return Vec::new();
        }
    };
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let mut out = Vec::with_capacity(entries.len());
    for e in entries {
        match peers.get_entity_async(&sys, &e.path).await {
            Ok(Some(ent)) => {
                if let Some(re) = RosterEntry::from_entity(&ent) {
                    out.push(re);
                }
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(path = %e.path, error = %err, "read_roster_async: get failed");
            }
        }
    }
    out
}

/// The set-A-derived `(seed-derived id, mode)` pairs — the comparison side
/// for [`reconcile_report`]. ALWAYS re-derives the id from the keypair, never
/// the stored `peer_id` field (the BUG-A drift surface).
#[allow(dead_code)] // Brick 3: reconcile gate
pub fn spawn_list_derived() -> Vec<(String, PeerMode)> {
    crate::persistence::load_all_peer_entries()
        .into_iter()
        .map(|e| (e.persisted.keypair.peer_id().to_string(), e.mode))
        .collect()
}

/// Diff between the authoritative roster and the durable spawn-list (set A),
/// keyed on the seed-derived id. The Brick-3 gate: prove the roster
/// shadow-matches A before any boot logic depends on it. Also the boot-time
/// D13 honesty surface — a non-clean report is logged loud, not swallowed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RosterReconcile {
    /// Derived ids in set A but missing from the roster.
    pub missing_from_roster: Vec<String>,
    /// Derived ids in the roster but not in set A (stale).
    pub extra_in_roster: Vec<String>,
    /// Ids in both whose mode disagrees (`"id: A=… roster=…"`).
    pub mode_mismatch: Vec<String>,
}

impl RosterReconcile {
    pub fn is_clean(&self) -> bool {
        self.missing_from_roster.is_empty()
            && self.extra_in_roster.is_empty()
            && self.mode_mismatch.is_empty()
    }
}

/// Compute the reconcile diff. `spawn_list` is the set-A-derived `(id, mode)`
/// pairs (see [`spawn_list_derived`]); `roster` is from [`read_roster`] /
/// [`read_roster_async`]. Compares the spawn-critical facts (id + mode);
/// labels are display-only and intentionally not gated on.
#[allow(dead_code)] // Brick 3: reconcile gate
pub fn reconcile_report(
    roster: &[RosterEntry],
    spawn_list: &[(String, PeerMode)],
) -> RosterReconcile {
    use std::collections::HashMap;
    let roster_map: HashMap<&str, PeerMode> =
        roster.iter().map(|e| (e.peer_id.as_str(), e.mode)).collect();
    let a_map: HashMap<&str, PeerMode> =
        spawn_list.iter().map(|(id, m)| (id.as_str(), *m)).collect();

    let mut report = RosterReconcile::default();
    for (id, a_mode) in &a_map {
        match roster_map.get(id) {
            None => report.missing_from_roster.push((*id).to_string()),
            Some(r_mode) if r_mode != a_mode => report.mode_mismatch.push(format!(
                "{id}: A={} roster={}",
                a_mode.persist_key(),
                r_mode.persist_key()
            )),
            Some(_) => {}
        }
    }
    for id in roster_map.keys() {
        if !a_map.contains_key(id) {
            report.extra_in_roster.push((*id).to_string());
        }
    }
    report.missing_from_roster.sort();
    report.extra_in_roster.sort();
    report.mode_mismatch.sort();
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_roundtrips_with_label() {
        let entry = RosterEntry {
            peer_id: "PEERAAAA".into(),
            mode: PeerMode::BackendOpfs,
            label: Some("Alice".into()),
        };
        let back = RosterEntry::from_entity(&entry.to_entity()).expect("decodes");
        assert_eq!(entry, back);
    }

    #[test]
    fn entry_roundtrips_without_label() {
        let entry = RosterEntry {
            peer_id: "PEERBBBB".into(),
            mode: PeerMode::Frontend,
            label: None,
        };
        let back = RosterEntry::from_entity(&entry.to_entity()).expect("decodes");
        assert_eq!(entry, back, "empty-string label must round-trip back to None");
    }

    #[test]
    fn unknown_mode_decodes_as_frontend_not_dropped() {
        // Forge an entity with an unrecognized mode string; decoding must
        // keep the peer (Frontend default), never drop it.
        let data = to_ecf(&cbor_map! {
            "peer_id" => entity_ecf::text("PEERCCCC"),
            "mode" => entity_ecf::text("backend-future-substrate"),
            "label" => entity_ecf::text("")
        });
        let ent = Entity::new(ROSTER_ENTRY_TYPE, data).unwrap();
        let back = RosterEntry::from_entity(&ent).expect("decodes");
        assert_eq!(back.peer_id, "PEERCCCC");
        assert_eq!(back.mode, PeerMode::Frontend, "unknown mode falls back, peer kept");
    }

    #[test]
    fn entry_with_empty_peer_id_is_rejected() {
        let data = to_ecf(&cbor_map! {
            "peer_id" => entity_ecf::text(""),
            "mode" => entity_ecf::text("frontend"),
            "label" => entity_ecf::text("x")
        });
        let ent = Entity::new(ROSTER_ENTRY_TYPE, data).unwrap();
        assert!(
            RosterEntry::from_entity(&ent).is_none(),
            "a record with no peer id is not a valid roster entry"
        );
    }

    #[test]
    fn put_then_read_via_tree() {
        let pm = Peers::new_direct();
        let sys = pm.system_peer_id().to_string();
        let handle = pm.writer_handle().expect("direct arm has a writer handle");

        put_entry(
            &handle,
            &sys,
            &RosterEntry {
                peer_id: "PEER0001".into(),
                mode: PeerMode::BackendMemory,
                label: Some("scratch".into()),
            },
        );
        put_entry(
            &handle,
            &sys,
            &RosterEntry {
                peer_id: "PEER0002".into(),
                mode: PeerMode::Frontend,
                label: None,
            },
        );

        let roster = read_roster(&pm);
        assert_eq!(roster.len(), 2, "both entries present");
        // Sorted by path == by peer id.
        assert_eq!(roster[0].peer_id, "PEER0001");
        assert_eq!(roster[0].mode, PeerMode::BackendMemory);
        assert_eq!(roster[0].label.as_deref(), Some("scratch"));
        assert_eq!(roster[1].peer_id, "PEER0002");
        assert_eq!(roster[1].label, None);
    }

    #[test]
    fn reconcile_clean_when_roster_matches_spawn_list() {
        let pm = Peers::new_direct();
        let sys = pm.system_peer_id().to_string();
        let handle = pm.writer_handle().unwrap();
        put_entry(&handle, &sys, &RosterEntry { peer_id: "P1".into(), mode: PeerMode::BackendOpfs, label: None });
        put_entry(&handle, &sys, &RosterEntry { peer_id: "P2".into(), mode: PeerMode::Frontend, label: Some("x".into()) });

        let roster = read_roster(&pm);
        let spawn_list = vec![
            ("P1".to_string(), PeerMode::BackendOpfs),
            ("P2".to_string(), PeerMode::Frontend),
        ];
        let report = reconcile_report(&roster, &spawn_list);
        assert!(report.is_clean(), "matching sets must reconcile clean: {report:?}");
    }

    #[test]
    fn reconcile_flags_missing_extra_and_mode_mismatch() {
        let roster = vec![
            RosterEntry { peer_id: "P1".into(), mode: PeerMode::Frontend, label: None }, // mode differs vs A
            RosterEntry { peer_id: "P3".into(), mode: PeerMode::Frontend, label: None }, // extra in roster
        ];
        let spawn_list = vec![
            ("P1".to_string(), PeerMode::BackendOpfs), // mode differs
            ("P2".to_string(), PeerMode::Frontend),    // missing from roster
        ];
        let report = reconcile_report(&roster, &spawn_list);
        assert_eq!(report.missing_from_roster, vec!["P2"]);
        assert_eq!(report.extra_in_roster, vec!["P3"]);
        assert_eq!(report.mode_mismatch.len(), 1, "P1 mode mismatch: {report:?}");
        assert!(!report.is_clean());
    }

    #[test]
    fn remove_drops_the_entry() {
        let pm = Peers::new_direct();
        let sys = pm.system_peer_id().to_string();
        let handle = pm.writer_handle().unwrap();

        let entry = RosterEntry {
            peer_id: "PEER0003".into(),
            mode: PeerMode::BackendOpfs,
            label: None,
        };
        put_entry(&handle, &sys, &entry);
        assert_eq!(read_roster(&pm).len(), 1);

        remove_entry(&handle, &sys, "PEER0003");
        assert!(
            read_roster(&pm).is_empty(),
            "removed roster entry must not be read back"
        );
    }
}

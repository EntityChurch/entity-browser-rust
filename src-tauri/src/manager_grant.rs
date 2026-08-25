//! Seed the **manager capability** for the system peer (S) on this backend
//! peer (B), authored as a policy entity on B's own tree
//! (`DESIGN-AUTHORIZE-GATE-INCREMENT-3 §3 Step 1`,
//! `DESIGN-SYSTEM-BACKEND-PEER §8.2`).
//!
//! Why B self-authors: B owns its tree, so it can write
//! `/{B}/system/capability/policy/{S}` locally with no grant. This is the
//! **only** pre-authored grant in the locked-down posture — it must exist
//! before `debug_open_grants` is retired (step 7) or S loses its read of B's
//! `system/*` observability paths and its ability to author per-peer grants,
//! i.e. locks itself out. Re-seeded on every backend start (idempotent on the
//! path), so a lost/corrupt grant self-heals — a robustness the connecting
//! side couldn't provide once `debug_open_grants` is off.
//!
//! The grant is a `system/capability/policy-entry` entity; the kernel unions it
//! onto S at connect (`build_authenticate_response_envelope` →
//! `lookup_capability_policy_grants`, `core/peer/src/connection.rs`). The grantee
//! path segment is S's Base58 PeerID — the pre-connect affordance the kernel
//! resolves and self-heals to the identity-hash hex form at handshake
//! (`connection.rs:3099`).
//!
//! NOTE (verified under enforcement at step 7): the exact operation/resource
//! tuples below are the intended manager scope. While `debug_open_grants` is
//! still on, S receives a wildcard grant regardless, so this seed is inert until
//! the flip — its correctness is proven when debug is retired and S connects.

use entity_capability::{encode_grant_entry, GrantEntry, IdScope, PathScope};
use entity_ecf::{text, to_ecf, Value};
use entity_entity::Entity;
use entity_peer::PeerShared;
use entity_types::TYPE_CAP_POLICY_ENTRY;

/// Build the manager-capability policy entity granting `manager_pid` (S) the
/// scope it needs over `backend_pid` (B): read + subscribe on B's observability
/// paths, and author per-peer grants via the capability handler.
///
/// Grants (four dimensions must all match per grant, `core/capability` §5.4):
/// - **read** — handler `system/tree`, op `get`, over B's session + capability
///   subtrees (the `prefix/*` glob is subtree-recursive, so `capability/*`
///   covers both `policy/*` and `pending/*`).
/// - **subscribe** — handler `system/subscription`, ops `subscribe`/`unsubscribe`
///   over the same subtrees (for the live surface; today's UI reads on refresh,
///   but the manager role owns this scope).
/// - **author** — handler `system/capability`, op `configure`, over
///   `system/capability/policy/*` (the validated grant-authoring API).
fn build_manager_grant_entity(backend_pid: &str, manager_pid: &str) -> Result<Entity, String> {
    let scoped = |p: &str| format!("/{}/{}", backend_pid, p);
    let observe = vec![
        scoped("system/peer/session/*"),
        scoped("system/capability/*"),
    ];

    let grants = vec![
        GrantEntry {
            handlers: PathScope::new(vec!["system/tree".into()]),
            resources: PathScope::new(observe.clone()),
            operations: IdScope::new(vec!["get".into()]),
            peers: None,
            constraints: None,
            allowances: None,
        },
        GrantEntry {
            handlers: PathScope::new(vec!["system/subscription".into()]),
            resources: PathScope::new(observe),
            operations: IdScope::new(vec!["subscribe".into(), "unsubscribe".into()]),
            peers: None,
            constraints: None,
            allowances: None,
        },
        GrantEntry {
            handlers: PathScope::new(vec!["system/capability".into()]),
            resources: PathScope::new(vec![scoped("system/capability/policy/*")]),
            operations: IdScope::new(vec!["configure".into()]),
            peers: None,
            constraints: None,
            allowances: None,
        },
    ];

    let arr: Vec<Value> = grants.iter().map(encode_grant_entry).collect();
    // Body shape the kernel decodes (`decode_policy_grants_at` reads `grants`;
    // the `configure` writer requires `peer_pattern` — include both).
    let data = to_ecf(&Value::Map(vec![
        (text("grants"), Value::Array(arr)),
        (text("peer_pattern"), text(manager_pid)),
    ]));
    Entity::new(TYPE_CAP_POLICY_ENTRY, data)
        .map_err(|e| format!("manager-grant entity construction failed: {e}"))
}

/// Author the manager grant onto B's tree at
/// `/{backend_pid}/system/capability/policy/{manager_pid}`. Fire-and-forget:
/// a failure is logged, not fatal (the backend still runs; observability just
/// won't work once debug grants are retired). No-op when `manager_pid` is empty
/// (no designated manager — e.g. a headless spawn with no frontend).
pub fn seed_manager_grant(shared: &PeerShared, backend_pid: &str, manager_pid: &str) {
    if manager_pid.is_empty() {
        log::warn!("no manager peer designated for backend {}; skipping manager-grant seed", short(backend_pid));
        return;
    }
    let entity = match build_manager_grant_entity(backend_pid, manager_pid) {
        Ok(e) => e,
        Err(e) => {
            log::warn!("{e}");
            return;
        }
    };
    let path = format!("/{}/system/capability/policy/{}", backend_pid, manager_pid);
    match shared.tree.put(&path, entity) {
        Ok(_hash) => log::info!(
            "seeded manager grant for {} on backend {}",
            short(manager_pid),
            short(backend_pid)
        ),
        Err(e) => log::warn!("failed to seed manager grant at {path}: {e}"),
    }
}

fn short(pid: &str) -> &str {
    &pid[..12.min(pid.len())]
}

#[cfg(test)]
mod tests {
    use super::*;

    // Decode the built entity back the way the kernel does
    // (`decode_policy_grants_at`): read the `grants` array, decode each element
    // via the capability crate's own decoder. This asserts our authored shape
    // is exactly what the connect-time union will accept.
    fn decode_grants(entity: &Entity) -> Vec<GrantEntry> {
        let val: ciborium::Value =
            ciborium::de::from_reader(entity.data.as_slice()).expect("entity body is CBOR");
        let map = val.as_map().expect("body is a map");
        let grants = map
            .iter()
            .find(|(k, _)| k.as_text() == Some("grants"))
            .map(|(_, v)| v)
            .expect("grants key present");
        grants
            .as_array()
            .expect("grants is an array")
            .iter()
            .map(|e| entity_capability::decode_grant_entry(e).expect("grant decodes"))
            .collect()
    }

    #[test]
    fn entity_has_policy_type() {
        let e = build_manager_grant_entity("BPID", "SPID").unwrap();
        assert_eq!(e.entity_type, TYPE_CAP_POLICY_ENTRY);
    }

    #[test]
    fn peer_pattern_is_the_manager_id() {
        let e = build_manager_grant_entity("BPID", "SPID").unwrap();
        let val: ciborium::Value = ciborium::de::from_reader(e.data.as_slice()).unwrap();
        let map = val.as_map().unwrap();
        let pat = map
            .iter()
            .find(|(k, _)| k.as_text() == Some("peer_pattern"))
            .and_then(|(_, v)| v.as_text())
            .unwrap();
        assert_eq!(pat, "SPID", "grantee key must be the manager (S) peer id");
    }

    #[test]
    fn grants_round_trip_through_kernel_decoder() {
        let e = build_manager_grant_entity("BPID", "SPID").unwrap();
        let grants = decode_grants(&e);
        assert_eq!(grants.len(), 3, "read, subscribe, author");
    }

    #[test]
    fn read_grant_covers_session_and_capability_subtrees() {
        let e = build_manager_grant_entity("BPID", "SPID").unwrap();
        let grants = decode_grants(&e);
        let read = grants
            .iter()
            .find(|g| g.handlers.include.contains(&"system/tree".to_string()))
            .expect("a system/tree read grant");
        assert!(read.operations.include.contains(&"get".to_string()));
        assert!(read
            .resources
            .include
            .contains(&"/BPID/system/peer/session/*".to_string()));
        assert!(read
            .resources
            .include
            .contains(&"/BPID/system/capability/*".to_string()));
    }

    #[test]
    fn author_grant_targets_policy_prefix_via_configure() {
        let e = build_manager_grant_entity("BPID", "SPID").unwrap();
        let grants = decode_grants(&e);
        let author = grants
            .iter()
            .find(|g| g.handlers.include.contains(&"system/capability".to_string()))
            .expect("a system/capability author grant");
        assert!(author.operations.include.contains(&"configure".to_string()));
        assert!(author
            .resources
            .include
            .contains(&"/BPID/system/capability/policy/*".to_string()));
    }
}

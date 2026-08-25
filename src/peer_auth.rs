//! Derive per-peer authorization state for the backend-peer observability
//! surface (`DESIGN-AUTHORIZE-GATE-INCREMENT-3 §2, §2.0, §3 Step 3`).
//!
//! **Pending is derived, not stored** — the protocol has no request→approve→grant
//! flow (§2.0), so a peer "wants access" state is composed from two spec-defined
//! inputs rather than authored as a new entity:
//! - the kernel's **session** records (`/{B}/system/peer/session/{A}`), written
//!   on every inbound connect — "who is connected";
//! - the **authorization set** — "who holds a file-transfer grant".
//!
//! Nothing here authors an entity or touches the wire. It is pure set logic over
//! peer-id keys, unit-testable on its own; the wiring that feeds it the two
//! inputs (a remote read of B's tree) lives in the window layer.
//!
//! ## Keying contract (load-bearing — read before wiring)
//!
//! The kernel keys the session entity by the connecting peer's **identity-hash
//! hex** (`PeerSession::relative_path(remote_identity_hash)`,
//! `core/peer/src/remote.rs`), and the capability policy table self-heals its
//! grantee key to that same hex at the handshake. So `connected` and
//! `authorized` here — both sourced from B's own tree — share ONE key space
//! (identity-hash hex). Do **not** mix in the app's base58 `remote_pid` or the
//! `connections`/`authz` mirror keys (a different space): a cross-space lookup
//! silently never matches. The caller normalizes; this module treats keys as
//! opaque strings and only compares like against like.
//!
//! Lands ahead of its caller: the Peer Connections observability surface (step 4)
//! wires the remote read that feeds `connected`/`authorized`. Exercised by unit
//! tests meanwhile.
#![allow(dead_code)]

use std::collections::BTreeSet;

/// Tree prefix (relative to the backend peer) under which the kernel records
/// one entity per live inbound session, keyed by the connected peer's
/// identity-hash hex.
pub const SESSION_PREFIX: &str = "system/peer/session/";

/// Normalize an app-layer **Base58** peer id to the kernel's canonical
/// **identity-hash hex** — the one key space of `session/*`, `policy/*`, and
/// the `authz` mirror the authorize action writes (module keying contract).
///
/// The Base58 form is an identity-multihash (`bs58(key_type ‖ hash_type ‖
/// digest)`); for canonical Ed25519 ids (`HASH_TYPE_IDENTITY`) the digest **is**
/// the raw public key, so we recover it and re-derive `peer_identity_hash` — the
/// same content-hash the kernel keys sessions by. This is the ONLY computable
/// direction: hex is a one-way hash of the peer entity, so hex→Base58 is
/// impossible. Callers that hold a Base58 id (e.g. the connections registry)
/// normalize to hex to join against the hex-keyed `authz` mirror.
///
/// Returns `None` when the id is malformed, or a legacy SHA-256-form id (whose
/// digest hides the public key — not recoverable); callers fall back to the raw
/// key (self-consistent legacy/test entries written under the same string).
pub fn identity_hash_hex(peer_id_base58: &str) -> Option<String> {
    let decoded = entity_crypto::PeerId::from(peer_id_base58).decode().ok()?;
    if decoded.hash_type != entity_crypto::HASH_TYPE_IDENTITY {
        return None;
    }
    let public_key: [u8; 32] = decoded.digest.try_into().ok()?;
    Some(entity_crypto::peer_identity_hash(&public_key).ok()?.to_hex())
}

/// Authorization state of a peer connected to the backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthState {
    /// Holds a file-transfer grant on the backend.
    Authorized,
    /// Connected but not yet authorized — the actionable "wants in" state.
    Pending,
}

/// A connected peer and whether it is authorized. `peer_id` is in the key space
/// of the inputs to [`classify`] (identity-hash hex when sourced from B's tree).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerAuthRow {
    pub peer_id: String,
    pub state: AuthState,
}

/// Extract connected peer ids from a backend's `session/*` path listing.
/// `paths` are full tree paths `/{backend_pid}/system/peer/session/{key}`.
/// Returns deduped, sorted keys; skips empty and nested paths defensively
/// (the registry is one level).
pub fn connected_peer_ids(paths: &[String], backend_pid: &str) -> Vec<String> {
    let prefix = format!("/{}/{}", backend_pid, SESSION_PREFIX);
    let mut ids: Vec<String> = paths
        .iter()
        .filter_map(|p| {
            let rest = p.strip_prefix(&prefix)?;
            if rest.is_empty() || rest.contains('/') {
                return None;
            }
            Some(rest.to_string())
        })
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// Classify connected peers into pending vs authorized.
///
/// - `connected` — connected peer keys (from [`connected_peer_ids`]).
/// - `authorized` — keys holding a file-transfer grant, **same key space** as
///   `connected` (see the module keying contract).
/// - `exclude` — keys never shown as authorizable: the manager (S) itself, in
///   every form it might appear (base58 and hex), plus the backend's own id.
///
/// Pending = `connected − authorized − exclude`. Output is in `connected`'s
/// order (sort upstream via [`connected_peer_ids`]).
pub fn classify(
    connected: &[String],
    authorized: &BTreeSet<String>,
    exclude: &BTreeSet<String>,
) -> Vec<PeerAuthRow> {
    connected
        .iter()
        .filter(|pid| !exclude.contains(pid.as_str()))
        .map(|pid| {
            let state = if authorized.contains(pid.as_str()) {
                AuthState::Authorized
            } else {
                AuthState::Pending
            };
            PeerAuthRow {
                peer_id: pid.clone(),
                state,
            }
        })
        .collect()
}

/// The actionable subset: connected peers awaiting authorization.
pub fn pending_peer_ids(
    connected: &[String],
    authorized: &BTreeSet<String>,
    exclude: &BTreeSet<String>,
) -> Vec<String> {
    classify(connected, authorized, exclude)
        .into_iter()
        .filter(|r| r.state == AuthState::Pending)
        .map(|r| r.peer_id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn base58_identity_id_maps_to_kernel_hex() {
        // A canonical identity-multihash Base58 id embeds the raw public key;
        // normalizing it must reproduce the kernel's session/policy hex key
        // (`peer_identity_hash(pubkey).to_hex()`) exactly — the join contract.
        let public_key = [7u8; 32];
        let base58 = entity_crypto::PeerId::from_public_key(&public_key)
            .as_str()
            .to_string();
        let expected = entity_crypto::peer_identity_hash(&public_key)
            .unwrap()
            .to_hex();
        assert_eq!(identity_hash_hex(&base58).as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn non_base58_id_yields_none_for_raw_fallback() {
        // Malformed / non-multihash strings (e.g. legacy or test fixtures)
        // don't convert — the caller falls back to the raw key.
        assert_eq!(identity_hash_hex("REMOTE_A"), None);
        assert_eq!(identity_hash_hex(""), None);
    }

    #[test]
    fn parses_session_keys_from_paths() {
        let paths = vec![
            "/BPID/system/peer/session/AAAA".to_string(),
            "/BPID/system/peer/session/BBBB".to_string(),
        ];
        assert_eq!(connected_peer_ids(&paths, "BPID"), vec!["AAAA", "BBBB"]);
    }

    #[test]
    fn session_parse_skips_foreign_prefixes_and_nesting() {
        let paths = vec![
            "/BPID/system/peer/session/AAAA".to_string(),
            "/BPID/system/peer/session/".to_string(),        // empty key
            "/BPID/system/peer/session/AAAA/extra".to_string(), // nested
            "/BPID/system/peer/transport/CCCC".to_string(),  // wrong subtree
            "/OTHER/system/peer/session/DDDD".to_string(),   // wrong peer
        ];
        assert_eq!(connected_peer_ids(&paths, "BPID"), vec!["AAAA"]);
    }

    #[test]
    fn session_parse_dedups_and_sorts() {
        let paths = vec![
            "/BPID/system/peer/session/CCCC".to_string(),
            "/BPID/system/peer/session/AAAA".to_string(),
            "/BPID/system/peer/session/AAAA".to_string(),
        ];
        assert_eq!(connected_peer_ids(&paths, "BPID"), vec!["AAAA", "CCCC"]);
    }

    #[test]
    fn unauthorized_connected_peer_is_pending() {
        let rows = classify(&["AAAA".into()], &set(&[]), &set(&[]));
        assert_eq!(rows, vec![PeerAuthRow { peer_id: "AAAA".into(), state: AuthState::Pending }]);
    }

    #[test]
    fn authorized_peer_is_not_pending() {
        let rows = classify(&["AAAA".into()], &set(&["AAAA"]), &set(&[]));
        assert_eq!(rows[0].state, AuthState::Authorized);
    }

    #[test]
    fn manager_and_self_are_excluded() {
        // S (the manager) and B (self) have session-ish entries but must never
        // appear as authorizable peers.
        let connected = vec!["AAAA".to_string(), "SPID".to_string(), "BPID".to_string()];
        let rows = classify(&connected, &set(&[]), &set(&["SPID", "BPID"]));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].peer_id, "AAAA");
    }

    #[test]
    fn pending_helper_returns_only_actionable() {
        let connected = vec!["AAAA".to_string(), "BBBB".to_string(), "SPID".to_string()];
        let pending = pending_peer_ids(&connected, &set(&["BBBB"]), &set(&["SPID"]));
        assert_eq!(pending, vec!["AAAA"], "BBBB authorized, SPID excluded");
    }
}

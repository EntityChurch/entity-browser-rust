//! The app's **petname / authorization registry** for remote peers — what the
//! app knows that the kernel does not model. **Not an address book.**
//!
//! One entity per known peer at
//! `/{system_peer}/app/entity-browser/connections/{remote_pid}`. The path's
//! last segment is the remote peer id; presence means "we have connected to
//! this peer at least once." The body carries `last_seen`; `authorized` joins
//! from the sibling `authz` entity; `label` lands with the known-name beat.
//!
//! ## The address is NOT here (`MODEL-REMOTE-PEER-FACTS` §1)
//!
//! It used to be, and that was the layering inversion the connectivity review
//! named: the app carried the address book because the kernel's rung-2 route
//! (`system/peer/transport/{hex}/primary`) was never published. Routes are
//! published now, so the address has exactly one durable home — the kernel's —
//! and [`RememberedPeer::addr`] is **resolved from it on read**, not stored.
//!
//! The `addr` field in an entity body is therefore **legacy**: read as a
//! fallback so a user upgrading with no route yet doesn't lose their Address
//! column (D16 cold return), never written, removable once a release has
//! passed. Do not reintroduce a write — see AP17.
//!
//! Writes go through [`ConnectionsWriter`] (clonable, suitable for
//! spawned tasks). Reads via [`read_connected`] from any consumer that
//! has a `&Peers`.
//!
//! Both arms supported via [`WriterHandle`]; no per-arm boilerplate.
//!
//! **D9 lifecycle:** entries are added on successful connect; eviction is wired
//! through [`ConnectionsWriter::remove`] but **no consumer calls it
//! today** because we have no disconnect-detection signal. In the
//! Worker arm (OPFS-persistent tree), this means the connections
//! prefix accumulates one stale entity per peer ever connected,
//! across boots, until disconnect detection lands upstream and we
//! wire eviction. Cost is low (one empty entity per remote ever
//! seen), but it is unbounded over time. Accept-and-document until
//! upstream provides the signal.

use entity_ecf::{cbor_map, integer, text, to_ecf};
use entity_entity::Entity;
use crate::peers::Peers;
use crate::writer_handle::WriterHandle;

use crate::app_paths;

/// Entity type name for connection-presence entries.
pub const CONNECTION_TYPE: &str = "app/entity-browser/connection";

/// Entity type name for the per-peer authorization mirror
/// (`DESIGN-AUTHORIZE-GATE-INCREMENT-3 §4`).
pub const AUTHZ_TYPE: &str = "app/entity-browser/authz";

/// A remembered remote peer — the enriched connection record
/// (`DESIGN-CROSS-DEVICE-FILE-TRANSFER §13.2`). Presence in the registry
/// means "we have connected at least once"; the body says how to reach it
/// again and when we last did.
///
/// `authorized` joins in a **sibling** `authz` entity
/// (`DESIGN-AUTHORIZE-GATE-INCREMENT-3 §4`): the connect path overwrites the
/// whole connection entity fire-and-forget, so authorization is stored apart
/// where a reconnect can't clobber it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RememberedPeer {
    /// Remote peer id (the registry entry's last path segment).
    pub remote_pid: String,
    /// The address this peer was last reached at — **resolved from the
    /// kernel's route entity, not stored on this row** (`MODEL-REMOTE-PEER-FACTS`
    /// §1). Empty when no route is published: never connected, a PeerID form
    /// whose hex we cannot derive locally, or (Worker arm) the caller forgot to
    /// watch `transport_profiles::routes_prefix`. Empty means "we cannot say
    /// how to reach this peer" — never a remembered guess.
    pub addr: String,
    /// Epoch-ms of the most recent successful connect; 0 if unknown.
    pub last_seen: u64,
    /// Granted profile name (§2.2: `file-transfer` / `file-transfer-rw` /
    /// `trusted`), or `None` when paired but not authorized. A local mirror
    /// of the authoritative policy grant on the exposing peer.
    pub authorized: Option<String>,
}

#[derive(Clone)]
pub struct ConnectionsWriter {
    system_peer_id: String,
    handle: Option<WriterHandle>,
}

impl ConnectionsWriter {
    pub fn new(peers: &Peers) -> Self {
        Self {
            system_peer_id: peers.system_peer_id().to_string(),
            handle: peers.writer_handle(),
        }
    }

    /// Record that we have connected to `remote_pid`. Idempotent on the path —
    /// a repeat connect overwrites the record, refreshing `last_seen`.
    ///
    /// **This no longer stores an address.** The address is a kernel fact with
    /// exactly one durable home — the transport-profile route the dispatch
    /// ladder reads — and an app-tier copy is by construction a mirror of it
    /// (`MODEL-REMOTE-PEER-FACTS` §1, AP17). This row now answers only what the
    /// kernel does not model: *have I ever connected to this peer, and when*.
    /// A caller with an address in hand does not pass it here; publishing the
    /// route is `Peers::connect_peer` / `maintain_peer`'s job.
    pub fn add(&self, remote_pid: &str) {
        let Some(handle) = &self.handle else { return };
        let path = app_paths::connection_entry_path(app_paths::APP_ID, &self.system_peer_id, remote_pid);
        handle.put(path, make_connection_entity(now_epoch_ms()));
    }

    /// Remove a connection record. No-op if not present.
    #[allow(dead_code)] // wired up when the app learns to detect disconnects
    pub fn remove(&self, remote_pid: &str) {
        let Some(handle) = &self.handle else { return };
        let path = app_paths::connection_entry_path(app_paths::APP_ID, &self.system_peer_id, remote_pid);
        handle.remove(path);
    }

    /// Record that `remote_pid` is authorized under the given grant `profile`
    /// (§2.2). Writes the sibling `authz` entity — disjoint from the connection
    /// record, so a later reconnect (`add`) can never clobber it. Idempotent on
    /// the path; a re-authorize overwrites the profile. A local mirror of the
    /// authoritative policy grant on the exposing peer.
    pub fn set_authorized(&self, remote_pid: &str, profile: &str) {
        let Some(handle) = &self.handle else { return };
        let path = app_paths::authz_entry_path(app_paths::APP_ID, &self.system_peer_id, remote_pid);
        handle.put(path, make_authz_entity(profile));
    }

    /// Clear a peer's authorization mirror (revoke). No-op if not present.
    #[allow(dead_code)] // wired up with the revoke UI (later increment)
    pub fn clear_authorized(&self, remote_pid: &str) {
        let Some(handle) = &self.handle else { return };
        let path = app_paths::authz_entry_path(app_paths::APP_ID, &self.system_peer_id, remote_pid);
        handle.remove(path);
    }
}

fn make_connection_entity(last_seen: u64) -> Entity {
    let data = to_ecf(&cbor_map! {
        "last_seen" => integer(last_seen as i64)
    });
    Entity::new(CONNECTION_TYPE, data).expect("connection entity construction is infallible")
}

fn make_authz_entity(profile: &str) -> Entity {
    let data = to_ecf(&cbor_map! {
        "profile" => text(profile)
    });
    Entity::new(AUTHZ_TYPE, data).expect("authz entity construction is infallible")
}

/// Decode an `authz` entity's profile string. Best-effort: a malformed or
/// bodyless entity decodes to `None` (treated as not authorized).
fn decode_authz(entity: &Entity) -> Option<String> {
    let value = ciborium::from_reader::<ciborium::Value, _>(entity.data.as_slice()).ok()?;
    let map = value.as_map()?;
    for (k, v) in map {
        if let (Some("profile"), ciborium::Value::Text(s)) = (k.as_text(), v) {
            if !s.is_empty() {
                return Some(s.clone());
            }
        }
    }
    None
}

/// Decode a connection entity's remembered-peer body. Best-effort: a
/// legacy body-less `{}` entity decodes to empty `addr` / 0 `last_seen`
/// (still a valid "known peer", just without the reconnect hint).
fn decode_connection(entity: &Entity) -> (String, u64) {
    let Ok(value) = ciborium::from_reader::<ciborium::Value, _>(entity.data.as_slice()) else {
        return (String::new(), 0);
    };
    let Some(map) = value.as_map() else {
        return (String::new(), 0);
    };
    let mut addr = String::new();
    let mut last_seen = 0u64;
    for (k, v) in map {
        match (k.as_text(), v) {
            (Some("addr"), ciborium::Value::Text(s)) => addr = s.clone(),
            (Some("last_seen"), ciborium::Value::Integer(i)) => {
                last_seen = u64::try_from(i128::from(*i)).unwrap_or(0);
            }
            _ => {}
        }
    }
    (addr, last_seen)
}

#[cfg(target_arch = "wasm32")]
fn now_epoch_ms() -> u64 {
    js_sys::Date::now() as u64
}

#[cfg(not(target_arch = "wasm32"))]
fn now_epoch_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Read the list of connected remote peer ids from the system peer's tree.
/// Returns ids in lexicographic order.
pub fn read_connected(peers: &Peers) -> Vec<String> {
    let pid = peers.system_peer_id();
    let prefix = app_paths::connections_prefix(app_paths::APP_ID, pid);
    let mut entries = peers.tree_listing(pid, &prefix);
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
        .into_iter()
        .filter_map(|entry| {
            entry
                .path
                .strip_prefix(&prefix)
                .map(|remote| remote.to_string())
        })
        .collect()
}

/// Read the full remembered-peer records — the enriched form of
/// [`read_connected`] used by the known-backends surface + one-tap reconnect
/// (`DESIGN-CROSS-DEVICE-FILE-TRANSFER §13.4` increment 2). Returns records
/// sorted by remote peer id, one per registry entry.
///
/// **Worker-arm note:** this reads each entry's body via [`Peers::get_entity`],
/// which on the Worker arm hits the cache mirror populated only for
/// *subscribed* prefixes — any surface that calls this must watch **both**
/// [`app_paths::connections_prefix`] and [`app_paths::authz_prefix`]
/// (`[[feedback_worker_cache_get_needs_subscription]]`).
/// Read the `authz` mirror profile for a remote, **reconciling the two key
/// spaces**. The connections registry keys by the app's Base58 `remote_pid`;
/// the authorize action (`app.rs::handle_authorize_peer`) writes the mirror
/// keyed by the target's canonical **identity-hash hex** (that's how B's tree
/// reports the connected peer — `DESIGN-AUTHORIZE-GATE-INCREMENT-3 §2`). So we
/// normalize Base58 → hex ([`peer_auth::identity_hash_hex`]) and look up under
/// hex first; falling back to the raw Base58 key covers legacy / test entries
/// written self-consistently under the same string. Also resolves a row keyed
/// directly by hex (the authorizations table): `identity_hash_hex` no-ops on a
/// non-Base58 string, so the raw-key fallback finds it.
///
/// Callers must watch **both** the connections and authz prefixes (Worker-arm
/// cache seeding) — the System Overview + Peer Connections windows do.
pub fn read_authz(peers: &Peers, sys_pid: &str, remote: &str) -> Option<String> {
    if let Some(hex) = crate::peer_auth::identity_hash_hex(remote) {
        let hex_path = app_paths::authz_entry_path(app_paths::APP_ID, sys_pid, &hex);
        if let Some(e) = peers.get_entity(sys_pid, &hex_path) {
            // Entity present under the canonical key — its decode is definitive
            // (a malformed body ⇒ None; don't fall through to the raw key).
            return decode_authz(&e);
        }
    }
    let raw_path = app_paths::authz_entry_path(app_paths::APP_ID, sys_pid, remote);
    peers.get_entity(sys_pid, &raw_path).and_then(|e| decode_authz(&e))
}

pub fn read_connections(peers: &Peers) -> Vec<RememberedPeer> {
    let pid = peers.system_peer_id();
    let prefix = app_paths::connections_prefix(app_paths::APP_ID, pid);
    let mut entries = peers.tree_listing(pid, &prefix);
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
        .into_iter()
        .filter_map(|entry| {
            let remote = entry.path.strip_prefix(&prefix)?.to_string();
            // The registry is one level — `{prefix}{remote}`. Skip empties
            // and any deeper path (defensive; it never nests today).
            if remote.is_empty() || remote.contains('/') {
                return None;
            }
            let (legacy_addr, last_seen) = peers
                .get_entity(pid, &entry.path)
                .map(|e| decode_connection(&e))
                .unwrap_or_default();
            // The address is a KERNEL fact with one durable home — the route
            // entity the dispatch ladder reads (`MODEL-REMOTE-PEER-FACTS` §1).
            // The legacy body field is a **migration read only**: a returning
            // user has rows carrying `addr` and may have no route yet (routes
            // are written on connect, and they have not connected since
            // upgrading), so reading the route alone would blank the Address
            // column and hide Reconnect on the first boot after upgrade — a
            // D16 cold-return regression. Never written; removable once a
            // release has passed.
            let addr = crate::transport_profiles::address_for(peers, pid, &remote)
                .unwrap_or(legacy_addr);
            // Join the sibling authz mirror (absent ⇒ paired, not authorized).
            let authorized = read_authz(peers, pid, &remote);
            Some(RememberedPeer {
                remote_pid: remote,
                addr,
                last_seen,
                authorized,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_records_one_connection() {
        let pm = Peers::new_direct();
        let writer = ConnectionsWriter::new(&pm);
        writer.add("REMOTE_AAAA");

        assert_eq!(read_connected(&pm), vec!["REMOTE_AAAA".to_string()]);
    }

    #[test]
    fn add_is_idempotent() {
        let pm = Peers::new_direct();
        let writer = ConnectionsWriter::new(&pm);
        writer.add("REMOTE_X");
        writer.add("REMOTE_X");
        writer.add("REMOTE_X");

        assert_eq!(read_connected(&pm).len(), 1);
    }

    #[test]
    fn multiple_connections_listed_sorted() {
        let pm = Peers::new_direct();
        let writer = ConnectionsWriter::new(&pm);
        writer.add("REMOTE_C");
        writer.add("REMOTE_A");
        writer.add("REMOTE_B");

        assert_eq!(
            read_connected(&pm),
            vec![
                "REMOTE_A".to_string(),
                "REMOTE_B".to_string(),
                "REMOTE_C".to_string(),
            ]
        );
    }

    #[test]
    fn remove_drops_entry() {
        let pm = Peers::new_direct();
        let writer = ConnectionsWriter::new(&pm);
        writer.add("REMOTE_KEEP");
        writer.add("REMOTE_DROP");
        writer.remove("REMOTE_DROP");

        assert_eq!(read_connected(&pm), vec!["REMOTE_KEEP".to_string()]);
    }

    #[test]
    fn the_address_comes_from_the_route_not_the_row() {
        let pm = Peers::new_direct();
        let sys = pm.system_peer_id().to_string();
        let writer = ConnectionsWriter::new(&pm);
        let handle = pm.writer_handle().expect("Direct writer handle");

        // Two real identity-form PeerIDs — the route path is keyed by the
        // identity hash, which only derives from a genuine PeerID.
        let a = entity_crypto::Keypair::generate().peer_id().to_string();
        let b = entity_crypto::Keypair::generate().peer_id().to_string();

        writer.add(&a);
        writer.add(&b);
        crate::transport_profiles::publish_dialed(&handle, &sys, &a, "ws://192.168.1.11:4041", None);
        crate::transport_profiles::publish_dialed(&handle, &sys, &b, "ws://192.168.1.10:4041", None);

        let records = read_connections(&pm);
        assert_eq!(records.len(), 2);
        let addr_of = |pid: &str| {
            records
                .iter()
                .find(|r| r.remote_pid == pid)
                .unwrap_or_else(|| panic!("{pid} missing"))
                .addr
                .clone()
        };
        assert_eq!(addr_of(&a), "ws://192.168.1.11:4041");
        assert_eq!(addr_of(&b), "ws://192.168.1.10:4041");
        assert!(records[0].last_seen > 0, "last_seen should be stamped");

        // The row itself must carry NO address — one durable home, and the
        // whole point of the migration is that this body stopped being one
        // (`MODEL-REMOTE-PEER-FACTS` §1). Asserted on the raw entity, because
        // `read_connections` deliberately falls back to the legacy field and
        // would mask a regression here.
        let path = app_paths::connection_entry_path(app_paths::APP_ID, &sys, &a);
        let raw = pm.get_entity(&sys, &path).expect("row present");
        assert_eq!(
            decode_connection(&raw).0,
            "",
            "the app row must not store an address — it is a kernel fact"
        );
    }

    /// **The D16 cold-return case.** A user who upgrades has rows carrying the
    /// legacy `addr` and no route yet (routes are written on connect, and they
    /// have not connected since). Reading the route alone would blank the
    /// Address column and hide Reconnect on that first boot. The legacy field
    /// is a migration READ only — never written — and removable once a release
    /// has passed.
    #[test]
    fn a_legacy_row_keeps_its_address_until_the_first_reconnect() {
        let pm = Peers::new_direct();
        let sys = pm.system_peer_id().to_string();
        let handle = pm.writer_handle().expect("Direct writer handle");
        let remote = entity_crypto::Keypair::generate().peer_id().to_string();

        // Hand-write a pre-migration row: body carries `addr`, no route exists.
        let legacy = to_ecf(&cbor_map! {
            "addr" => text("ws://legacy:4041"),
            "last_seen" => integer(1_700_000_000_000i64)
        });
        handle.put(
            app_paths::connection_entry_path(app_paths::APP_ID, &sys, &remote),
            Entity::new(CONNECTION_TYPE, legacy).expect("legacy entity"),
        );

        let records = read_connections(&pm);
        assert_eq!(records[0].addr, "ws://legacy:4041", "legacy addr must survive the upgrade");

        // Once a route exists it WINS — the kernel fact is authoritative and a
        // stale legacy body must never shadow it.
        crate::transport_profiles::publish_dialed(&handle, &sys, &remote, "ws://current:4041", None);
        let records = read_connections(&pm);
        assert_eq!(
            records[0].addr, "ws://current:4041",
            "the route is authoritative; the legacy field is only a fallback"
        );
    }

    #[test]
    fn reconnect_refreshes_addr() {
        let pm = Peers::new_direct();
        let writer = ConnectionsWriter::new(&pm);
        writer.add("REMOTE_X");
        writer.add("REMOTE_X");

        let records = read_connections(&pm);
        assert_eq!(records.len(), 1, "same peer overwrites, not duplicates");
        assert!(records[0].last_seen > 0, "last_seen refreshed on reconnect");
    }

    #[test]
    fn unauthorized_peer_reads_none() {
        let pm = Peers::new_direct();
        let writer = ConnectionsWriter::new(&pm);
        writer.add("REMOTE_A");

        let records = read_connections(&pm);
        assert_eq!(records[0].authorized, None, "paired but not authorized");
    }

    #[test]
    fn set_authorized_records_profile() {
        let pm = Peers::new_direct();
        let writer = ConnectionsWriter::new(&pm);
        writer.add("REMOTE_A");
        writer.set_authorized("REMOTE_A", "file-transfer");

        let records = read_connections(&pm);
        assert_eq!(records.len(), 1, "authz is a sibling, not a second known peer");
        assert_eq!(records[0].authorized.as_deref(), Some("file-transfer"));
    }

    #[test]
    fn authz_written_under_hex_joins_base58_connection() {
        // Production reality: the connections registry keys by Base58
        // `remote_pid`, but `handle_authorize_peer` writes the authz mirror
        // under the target's canonical identity-hash **hex** (that's how B's
        // tree reports the peer). read_connections must reconcile the two.
        let pm = Peers::new_direct();
        let writer = ConnectionsWriter::new(&pm);
        let public_key = [9u8; 32];
        let base58 = entity_crypto::PeerId::from_public_key(&public_key)
            .as_str()
            .to_string();
        let hex = entity_crypto::peer_identity_hash(&public_key).unwrap().to_hex();

        writer.add(&base58);
        writer.set_authorized(&hex, "file-transfer"); // keyed as the authorize path keys it

        let records = read_connections(&pm);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].remote_pid, base58);
        assert_eq!(
            records[0].authorized.as_deref(),
            Some("file-transfer"),
            "Base58 connection must join the hex-keyed authz mirror",
        );
    }

    /// The load-bearing invariant (§4): a reconnect (`add`) must NOT clobber an
    /// existing authorization. `add` overwrites the connection entity
    /// fire-and-forget with no read; storing authz in a disjoint sibling entity
    /// is what makes this safe.
    #[test]
    fn reconnect_preserves_authorization() {
        let pm = Peers::new_direct();
        let writer = ConnectionsWriter::new(&pm);
        writer.add("REMOTE_A");
        writer.set_authorized("REMOTE_A", "file-transfer-rw");
        // Reconnect from a new address — overwrites the connection record.
        writer.add("REMOTE_A");

        let records = read_connections(&pm);
        assert_eq!(
            records[0].authorized.as_deref(),
            Some("file-transfer-rw"),
            "reconnect must not wipe authorization"
        );
    }

    #[test]
    fn clear_authorized_revokes() {
        let pm = Peers::new_direct();
        let writer = ConnectionsWriter::new(&pm);
        writer.add("REMOTE_A");
        writer.set_authorized("REMOTE_A", "trusted");
        writer.clear_authorized("REMOTE_A");

        let records = read_connections(&pm);
        assert_eq!(records.len(), 1, "still a known peer after revoke");
        assert_eq!(records[0].authorized, None, "authorization cleared");
    }

    #[test]
    fn reauthorize_overwrites_profile() {
        let pm = Peers::new_direct();
        let writer = ConnectionsWriter::new(&pm);
        writer.add("REMOTE_A");
        writer.set_authorized("REMOTE_A", "file-transfer");
        writer.set_authorized("REMOTE_A", "trusted");

        let records = read_connections(&pm);
        assert_eq!(records[0].authorized.as_deref(), Some("trusted"));
    }
}

//! Tree-backed registry of remote peer connections this app has
//! established.
//!
//! One entity per connected peer at
//! `/{system_peer}/app/entity-browser/connections/{remote_pid}`.
//! The path's last segment is the remote peer id; presence means "we have
//! connected to this peer at least once." The body carries the enriched
//! **remembered-peer** record (`DESIGN-CROSS-DEVICE-FILE-TRANSFER §13.2`):
//! `addr` (last address that worked → the one-tap reconnect target) and
//! `last_seen` (epoch-ms of the most recent connect → how the connection
//! lives over time). `authorized` / `label` land with their later beats
//! (the authorize gate / known-name). When the app gains the ability to
//! detect a remote disconnect, the entity is removed.
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

/// A remembered remote peer — the enriched connection record
/// (`DESIGN-CROSS-DEVICE-FILE-TRANSFER §13.2`). Presence in the registry
/// means "we have connected at least once"; the body says how to reach it
/// again and when we last did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RememberedPeer {
    /// Remote peer id (the registry entry's last path segment).
    pub remote_pid: String,
    /// Last address that connected — the one-tap reconnect target. Empty
    /// for legacy entries written before enrichment (body-less `{}`).
    pub addr: String,
    /// Epoch-ms of the most recent successful connect; 0 if unknown.
    pub last_seen: u64,
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

    /// Record a successful connection to `remote_pid` at `addr`. Idempotent
    /// on the path — a repeat connect overwrites the record, refreshing
    /// `last_seen` and the reconnect `addr`. `addr` is the address that just
    /// worked (empty is tolerated for callers without one).
    pub fn add(&self, remote_pid: &str, addr: &str) {
        let Some(handle) = &self.handle else { return };
        let path = app_paths::connection_entry_path(app_paths::APP_ID, &self.system_peer_id, remote_pid);
        handle.put(path, make_connection_entity(addr, now_epoch_ms()));
    }

    /// Remove a connection record. No-op if not present.
    #[allow(dead_code)] // wired up when the app learns to detect disconnects
    pub fn remove(&self, remote_pid: &str) {
        let Some(handle) = &self.handle else { return };
        let path = app_paths::connection_entry_path(app_paths::APP_ID, &self.system_peer_id, remote_pid);
        handle.remove(path);
    }
}

fn make_connection_entity(addr: &str, last_seen: u64) -> Entity {
    let data = to_ecf(&cbor_map! {
        "addr" => text(addr),
        "last_seen" => integer(last_seen as i64)
    });
    Entity::new(CONNECTION_TYPE, data).expect("connection entity construction is infallible")
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
/// *subscribed* prefixes — any surface that calls this must also watch
/// [`app_paths::connections_prefix`]
/// (`[[feedback_worker_cache_get_needs_subscription]]`).
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
            let (addr, last_seen) = peers
                .get_entity(pid, &entry.path)
                .map(|e| decode_connection(&e))
                .unwrap_or_default();
            Some(RememberedPeer {
                remote_pid: remote,
                addr,
                last_seen,
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
        writer.add("REMOTE_AAAA", "ws://10.0.0.5:4041");

        assert_eq!(read_connected(&pm), vec!["REMOTE_AAAA".to_string()]);
    }

    #[test]
    fn add_is_idempotent() {
        let pm = Peers::new_direct();
        let writer = ConnectionsWriter::new(&pm);
        writer.add("REMOTE_X", "ws://a:1");
        writer.add("REMOTE_X", "ws://a:1");
        writer.add("REMOTE_X", "ws://a:1");

        assert_eq!(read_connected(&pm).len(), 1);
    }

    #[test]
    fn multiple_connections_listed_sorted() {
        let pm = Peers::new_direct();
        let writer = ConnectionsWriter::new(&pm);
        writer.add("REMOTE_C", "ws://c:3");
        writer.add("REMOTE_A", "ws://a:1");
        writer.add("REMOTE_B", "ws://b:2");

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
        writer.add("REMOTE_KEEP", "ws://k:1");
        writer.add("REMOTE_DROP", "ws://d:1");
        writer.remove("REMOTE_DROP");

        assert_eq!(read_connected(&pm), vec!["REMOTE_KEEP".to_string()]);
    }

    #[test]
    fn remembers_addr_and_last_seen() {
        let pm = Peers::new_direct();
        let writer = ConnectionsWriter::new(&pm);
        writer.add("REMOTE_B", "ws://192.168.1.10:4041");
        writer.add("REMOTE_A", "ws://192.168.1.11:4041");

        let records = read_connections(&pm);
        assert_eq!(records.len(), 2);
        // Sorted by remote peer id.
        assert_eq!(records[0].remote_pid, "REMOTE_A");
        assert_eq!(records[0].addr, "ws://192.168.1.11:4041");
        assert_eq!(records[1].remote_pid, "REMOTE_B");
        assert_eq!(records[1].addr, "ws://192.168.1.10:4041");
        // last_seen is stamped (non-zero) on write.
        assert!(records[0].last_seen > 0, "last_seen should be stamped");
    }

    #[test]
    fn reconnect_refreshes_addr() {
        let pm = Peers::new_direct();
        let writer = ConnectionsWriter::new(&pm);
        writer.add("REMOTE_X", "ws://old:4041");
        writer.add("REMOTE_X", "ws://new:4041");

        let records = read_connections(&pm);
        assert_eq!(records.len(), 1, "same peer overwrites, not duplicates");
        assert_eq!(records[0].addr, "ws://new:4041", "addr refreshed to latest");
    }
}

//! Tree-backed registry of remote peer connections this app has
//! established.
//!
//! One entity per connected peer at
//! `/{system_peer}/app/entity-browser/connections/{remote_pid}`.
//! The path's last segment is the remote peer id; the entity body is
//! empty (presence = connected). When the app gains the ability to
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

use entity_ecf::{cbor_map, to_ecf};
use entity_entity::Entity;
use crate::peers::Peers;
use crate::writer_handle::WriterHandle;

use crate::app_paths;

/// Entity type name for connection-presence entries.
pub const CONNECTION_TYPE: &str = "app/entity-browser/connection";

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

    /// Record a successful connection to `remote_pid`. Idempotent —
    /// repeated calls just overwrite the same path.
    pub fn add(&self, remote_pid: &str) {
        let Some(handle) = &self.handle else { return };
        let path = app_paths::connection_entry_path(app_paths::APP_ID, &self.system_peer_id, remote_pid);
        handle.put(path, make_connection_entity());
    }

    /// Remove a connection record. No-op if not present.
    #[allow(dead_code)] // wired up when the app learns to detect disconnects
    pub fn remove(&self, remote_pid: &str) {
        let Some(handle) = &self.handle else { return };
        let path = app_paths::connection_entry_path(app_paths::APP_ID, &self.system_peer_id, remote_pid);
        handle.remove(path);
    }
}

fn make_connection_entity() -> Entity {
    let data = to_ecf(&cbor_map! {});
    Entity::new(CONNECTION_TYPE, data).expect("connection entity construction is infallible")
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
}

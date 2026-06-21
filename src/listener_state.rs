//! Tree-backed publication of the WebSocket listener's current state.
//!
//! When the native WS listener binds successfully it writes its address
//! to `/{system_peer}/app/entity-browser/listener/state`. UI consumers
//! (Peer Connections window) read that path to show the listen address
//! / QR-code target.
//!
//! Single-valued: one entity, overwritten on each (re)bind.

use std::sync::Arc;

use entity_ecf::{cbor_map, text, to_ecf};
use entity_entity::Entity;
use entity_peer::PeerShared;
use crate::peers::Peers;

use crate::app_paths;

/// Entity type name for the listener state record.
pub const LISTENER_TYPE: &str = "app/entity-browser/listener-state";

/// Clonable writer for the listener state. Held by `EntityApp` and
/// moved into the listener-bind spawned task.
///
/// Native-only: WASM never binds a listener.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
#[derive(Clone)]
pub struct ListenerStateWriter {
    system_peer_id: String,
    shared: Arc<PeerShared>,
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
impl ListenerStateWriter {
    pub fn new(peers: &Peers) -> Self {
        let pid = peers.system_peer_id().to_string();
        let shared = peers
            .direct_peer_shared(&pid)
            .expect("system peer must exist at startup");
        Self {
            system_peer_id: pid,
            shared,
        }
    }

    /// Publish the listener's bound address.
    pub fn set_address(&self, addr: &str) {
        let path = app_paths::listener_state_path(app_paths::APP_ID, &self.system_peer_id);
        let entity = make_listener_entity(Some(addr));
        if let Err(e) = self.shared.tree.put(&path, entity) {
            tracing::warn!(error = %e, "listener_state: write failed");
        }
    }

    /// Clear the published address (listener stopped or never started).
    #[allow(dead_code)] // wired up when stop/restart is implemented
    pub fn clear(&self) {
        let path = app_paths::listener_state_path(app_paths::APP_ID, &self.system_peer_id);
        self.shared.tree.remove(&path);
    }
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn make_listener_entity(addr: Option<&str>) -> Entity {
    let data = match addr {
        Some(a) => to_ecf(&cbor_map! { "address" => text(a) }),
        None => to_ecf(&cbor_map! {}),
    };
    Entity::new(LISTENER_TYPE, data).expect("listener entity construction is infallible")
}

/// Read the currently published listen address from the system peer's
/// tree. Returns `None` if no listener is bound.
pub fn read_address(peers: &Peers) -> Option<String> {
    let pid = peers.system_peer_id();
    let path = app_paths::listener_state_path(app_paths::APP_ID, pid);
    let entity = peers.get_entity(pid, &path)?;

    let value: ciborium::Value = ciborium::from_reader(entity.data.as_slice()).ok()?;
    let map = value.as_map()?;
    for (k, v) in map {
        if let Some(key) = k.as_text() {
            if key == "address" {
                return v.as_text().map(|s| s.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_returns_none_when_unset() {
        let pm = Peers::new_direct();
        assert!(read_address(&pm).is_none());
    }

    #[test]
    fn set_then_read_round_trips() {
        let pm = Peers::new_direct();
        let writer = ListenerStateWriter::new(&pm);
        writer.set_address("ws://192.168.1.10:4041");
        assert_eq!(
            read_address(&pm),
            Some("ws://192.168.1.10:4041".to_string())
        );
    }

    #[test]
    fn set_overwrites_previous() {
        let pm = Peers::new_direct();
        let writer = ListenerStateWriter::new(&pm);
        writer.set_address("ws://10.0.0.1:4041");
        writer.set_address("ws://10.0.0.2:4041");
        assert_eq!(read_address(&pm), Some("ws://10.0.0.2:4041".to_string()));
    }

    #[test]
    fn clear_removes_entry() {
        let pm = Peers::new_direct();
        let writer = ListenerStateWriter::new(&pm);
        writer.set_address("ws://x:1");
        writer.clear();
        assert!(read_address(&pm).is_none());
    }
}

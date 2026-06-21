//! Key Manager model — pass-through over the tree-backed peer
//! registry.
//!
//! No in-memory state. One row per hosted peer, read from the
//! `system/peers/` registry (`peer_registry::read_registry`) — the
//! same tree data the window subscribes to. `peer_id` is the
//! Ed25519-derived public identity; no private key material is read.

use crate::peer_registry::read_registry;
use crate::peers::Peers;

use super::output::{KeyEntry, KeyManagerOutput};

#[derive(Debug)]
pub struct KeyManagerModel {
    peer_id: String,
}

impl KeyManagerModel {
    pub fn new(peer_id: String) -> Self {
        Self { peer_id }
    }

    #[allow(dead_code)] // accessed for symmetry; not currently used
    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    #[allow(dead_code)] // called from WASM render path
    pub fn render_output(&self, peers: &Peers) -> KeyManagerOutput {
        let keys = read_registry(peers)
            .into_iter()
            .map(|r| KeyEntry {
                label: r
                    .label
                    .unwrap_or_else(|| crate::views::short_pid(&r.peer_id)),
                peer_id: r.peer_id,
                role: r.role,
            })
            .collect();
        KeyManagerOutput { keys }
    }
}

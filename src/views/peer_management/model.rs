//! Peer Management model — pass-through over the tree-backed peer
//! registry.
//!
//! No in-memory state. Rows are read from the `system/peers/` registry
//! (`peer_registry::read_registry`) — the same tree data the window
//! subscribes to — instead of re-scanning `Peers` per render. `Peers`
//! is still consulted for the two non-per-peer footer facts
//! (`sdk_count`, Tauri availability).

use crate::peer_display::PeerDisplay;
use crate::peer_registry::read_registry;
use crate::peers::Peers;

use super::output::{AddressDisplay, BackendButton, PeerManagementOutput, PeerRow};

#[derive(Debug)]
pub struct PeerManagementModel {
    peer_id: String,
}

impl PeerManagementModel {
    pub fn new(peer_id: String) -> Self {
        Self { peer_id }
    }

    #[allow(dead_code)] // accessed for symmetry; not currently used
    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    #[allow(dead_code)] // called from WASM render path
    pub fn render_output(&self, peers: &Peers) -> PeerManagementOutput {
        let recs = read_registry(peers);
        let mut rows = Vec::with_capacity(recs.len());

        // Start/Stop/"stopped" is a *Tauri-native* backend-peer
        // lifecycle concept (native process bound to ws://). A browser
        // backend peer runs in an in-process Web Worker — it has no
        // listen address but is *running*. Showing it "stopped" with a
        // Start button (which calls Tauri IPC → no-op in a browser) is
        // a flat misrepresentation, so gate that affordance on Tauri.
        let tauri = tauri_available();

        for r in &recs {
            let kind = PeerDisplay::from_tag(&r.display);
            let is_backend = r.role.starts_with("backend");
            // Worker-spawned backend peers (Action::CreatePeerWithMode
            // with BackendMemory/BackendOpfs) live in a Web Worker
            // SDK — NOT the Tauri native process. They're always
            // running and have no Tauri-side IPC handle, so the
            // Start/Stop affordance must be hidden for them even in
            // Tauri. Discriminator: `is_backend_hosted` is true iff
            // the peer is in a worker SDK; Tauri-native backend peers
            // are registered into the primary SDK and return false.
            let is_tauri_native_backend =
                is_backend && tauri && !peers.is_backend_hosted(&r.peer_id);

            let address = if !r.listen_addresses.is_empty() {
                AddressDisplay::Addresses(r.listen_addresses.join(", "))
            } else if is_tauri_native_backend {
                // Tauri native backend peer with no listener = stopped.
                AddressDisplay::Stopped
            } else {
                // Browser worker backend (running, no listener) or a
                // frontend peer — no network address, not "stopped".
                AddressDisplay::None
            };

            let backend_button = if is_tauri_native_backend {
                Some(if r.listen_addresses.is_empty() {
                    BackendButton::Start
                } else {
                    BackendButton::Stop
                })
            } else {
                // Browser worker backend peers are always running —
                // there is nothing to start/stop.
                None
            };

            rows.push(PeerRow {
                peer_id: r.peer_id.clone(),
                short_pid: crate::views::short_pid(&r.peer_id),
                kind,
                role_glyph: r.glyph.clone(),
                role_name: r.role.clone(),
                label: r.label.clone(),
                persisted: r.persisted,
                address,
                show_open_tree: r.has_context,
                backend_button,
                show_delete: r.deletable,
            });
        }

        // 1b capability gate: hide the create panel when the deployment
        // disables peer creation. Read from the system peer's session config
        // (the posture spine) — the same durable source the action guard
        // checks, so UI and guard never disagree.
        let show_peer_create = crate::session_config::read(peers, peers.system_peer_id())
            .peer_creation_enabled;

        PeerManagementOutput {
            total_count: rows.len(),
            sdk_count: peers.sdk_count(),
            rows,
            show_peer_create,
            show_backend_create: tauri_available(),
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn tauri_available() -> bool {
    crate::tauri_ipc::is_tauri()
}

#[cfg(not(target_arch = "wasm32"))]
fn tauri_available() -> bool {
    false
}

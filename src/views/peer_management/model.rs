//! Peer Management model — pass-through over the tree-backed peer
//! registry.
//!
//! No in-memory state. Rows are read from the `system/peers/` registry
//! (`peer_registry::read_registry`) — the same tree data the window
//! subscribes to — instead of re-scanning `Peers` per render. `Peers`
//! is still consulted for the two non-per-peer footer facts
//! (`sdk_count`, Tauri availability).

use crate::peer_display::{PeerDescriptor, PeerDisplay, PeerRole, PeerRuntime};
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

        // Authoritative persisted-mode map — the storage facet for user peers.
        let modes = crate::persistence::peer_modes();

        for r in &recs {
            let kind = PeerDisplay::from_tag(&r.display);
            // The truthful role · runtime · storage facets. `describe` owns the
            // native-vs-worker-vs-main-thread call (replacing the old
            // `role.starts_with("backend")` string sniff, which is exactly what
            // let the native system peer read as "backend (memory)").
            let descriptor = PeerDescriptor::describe(peers, &r.peer_id, &modes);

            // Start/Stop/"stopped" is a *native-process* backend-peer lifecycle
            // concept (a peer bound to ws://). A worker peer runs in-page and is
            // always running — no listen address, not "stopped". So the affordance
            // gates on the Native runtime.
            let is_native = descriptor.runtime == PeerRuntime::Native;
            // The canonical system native peer is infrastructure: always-on and
            // un-deletable (the backend refuses both). Its Start/Stop and Delete
            // are unsupported ⇒ hidden — like the in-app system peer
            // (REFERENCE-UI-DESIGN S3 "no dead controls"). Among native peers,
            // only the system one carries `PeerRole::System`.
            let is_system_native = is_native && descriptor.role == PeerRole::System;

            let address = if !r.listen_addresses.is_empty() {
                AddressDisplay::Addresses(r.listen_addresses.join(", "))
            } else if is_native {
                // Native-process peer with no listener = stopped.
                AddressDisplay::Stopped
            } else {
                // Worker peer (running, no listener) or a main-thread peer —
                // no network address, not "stopped".
                AddressDisplay::None
            };

            let backend_button = if is_native && !is_system_native {
                Some(if r.listen_addresses.is_empty() {
                    BackendButton::Start
                } else {
                    BackendButton::Stop
                })
            } else {
                // Worker peers are always running, and the system native peer is
                // always-on — nothing to start/stop.
                None
            };

            rows.push(PeerRow {
                peer_id: r.peer_id.clone(),
                short_pid: crate::views::short_pid(&r.peer_id),
                kind,
                descriptor,
                label: r.label.clone(),
                persisted: r.persisted,
                address,
                show_open_tree: r.has_context,
                backend_button,
                show_delete: r.deletable && !is_system_native,
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

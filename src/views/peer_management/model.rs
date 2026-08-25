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

use super::output::{AddressDisplay, BackendButton, CreateOption, PeerManagementOutput, PeerRow};

#[derive(Debug)]
pub struct PeerManagementModel {
    peer_id: String,
    /// Ephemeral UI state (S8 create affordance): is the "Add a peer" card open?
    /// In-model (not tree-backed) — a pure UI toggle, like Site Creator's
    /// `create_open`. Lives here so it survives snapshot rebuilds; the window
    /// flips it via `toggle_create`/`close_create` on the toggle/Add events.
    create_open: bool,
}

impl PeerManagementModel {
    pub fn new(peer_id: String) -> Self {
        Self { peer_id, create_open: false }
    }

    /// Flip the create-card open/closed (the collapsible header was clicked).
    pub fn toggle_create(&mut self) {
        self.create_open = !self.create_open;
    }

    /// Collapse the create card — called after a successful Add so it tidies
    /// away and you're back to just the peer list.
    pub fn close_create(&mut self) {
        self.create_open = false;
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
            create_options: build_create_options(peers.primary_as_direct().is_some()),
            create_open: self.create_open,
        }
    }
}

/// The system-aware create-peer option list: the wired modes, each gated by what
/// THIS runtime can actually create. Unsupported ones stay in the list (rendered
/// disabled + reason) so the operator sees *why* — no silent gap. The values are
/// the durable `PeerMode::persist_key`s + `"native"` (the e2e drives by these;
/// never localize them). `direct` is whether the primary booted the Direct/IDB
/// posture (a main-thread IndexedDB store is only reachable there — the
/// `frontend-idb` gate).
fn build_create_options(direct: bool) -> Vec<CreateOption> {
    let (worker, opfs) = probe_caps();
    let native = tauri_available();
    vec![
        CreateOption {
            value: "frontend",
            label: "peers.kind_label.frontend",
            available: true,
            reason: None,
        },
        CreateOption {
            // The durable sibling of `frontend`: main thread + a per-peer
            // IndexedDB store (survives reload). Direct posture only — under a
            // Worker primary there is no main-thread SDK to host it.
            value: "frontend-idb",
            label: "peers.kind_label.frontend_idb",
            available: direct,
            reason: (!direct).then_some("peers.kind_unavail.worker_mode"),
        },
        CreateOption {
            value: "backend-memory",
            label: "peers.kind_label.backend_memory",
            available: worker,
            reason: (!worker).then_some("peers.kind_unavail.web_worker"),
        },
        CreateOption {
            value: "backend-opfs",
            label: "peers.kind_label.backend_opfs",
            available: worker && opfs,
            reason: if !worker {
                Some("peers.kind_unavail.web_worker")
            } else if !opfs {
                Some("peers.kind_unavail.opfs")
            } else {
                None
            },
        },
        CreateOption {
            value: "native",
            label: "peers.kind_label.native",
            available: native,
            reason: (!native).then_some("peers.kind_unavail.desktop"),
        },
    ]
}

/// `(worker, opfs)` capability probe. Cheap sync JS reflection on wasm; on native
/// (unit tests) both false, so only the always-available main-thread mode shows.
#[cfg(target_arch = "wasm32")]
fn probe_caps() -> (bool, bool) {
    let c = crate::capabilities::Capabilities::detect();
    (c.worker_constructor, c.opfs)
}
#[cfg(not(target_arch = "wasm32"))]
fn probe_caps() -> (bool, bool) {
    (false, false)
}

#[cfg(target_arch = "wasm32")]
fn tauri_available() -> bool {
    crate::tauri_ipc::is_tauri()
}

#[cfg(not(target_arch = "wasm32"))]
fn tauri_available() -> bool {
    false
}

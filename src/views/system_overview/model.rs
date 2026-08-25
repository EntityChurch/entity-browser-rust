//! System Overview model — a read-only projection over the system peer(s) and
//! the durable posture. No state of its own; every fact is re-derived from
//! `Peers` + the registry + session config, so it tracks the tree reactively
//! (the window watches the registry + config prefixes).

use crate::peer_display::{PeerDescriptor, PeerRole, PeerRuntime};
use crate::peer_registry::read_registry;
use crate::peers::Peers;
use crate::session_config::{self, BootSurface};
use crate::views::short_pid;

use super::output::{SystemOverviewOutput, SystemPeerCard};

#[derive(Debug, Default)]
pub struct SystemOverviewModel;

impl SystemOverviewModel {
    pub fn new() -> Self {
        Self
    }

    #[allow(dead_code)] // called from the WASM render path
    pub fn render_output(&self, peers: &Peers) -> SystemOverviewOutput {
        let sys = peers.system_peer_id().to_string();
        let modes = crate::persistence::peer_modes();

        // The in-app system peer — the boot/primary peer. No listen address.
        let in_app = SystemPeerCard {
            short_id: short_pid(&sys),
            full_id: sys.clone(),
            descriptor: PeerDescriptor::describe(peers, &sys, &modes),
        };

        // Walk the registry once: find the native system peer (if any) and count
        // user peers. The registry is the same tree data the window subscribes to.
        let recs = read_registry(peers);
        let total_peers = recs.len();
        let mut native: Option<SystemPeerCard> = None;
        let mut user_peers = 0usize;
        for r in &recs {
            let descriptor = PeerDescriptor::describe(peers, &r.peer_id, &modes);
            match descriptor.role {
                PeerRole::User => user_peers += 1,
                PeerRole::System if descriptor.runtime == PeerRuntime::Native => {
                    native = Some(SystemPeerCard {
                        short_id: short_pid(&r.peer_id),
                        full_id: r.peer_id.clone(),
                        descriptor,
                    });
                }
                PeerRole::System => {}
            }
        }

        let cfg = session_config::read(peers, &sys);
        let startup = describe_boot_surface(&cfg.boot_surface, &cfg.home_site.id);

        SystemOverviewOutput {
            in_app,
            native,
            total_peers,
            user_peers,
            startup,
            peer_creation: cfg.peer_creation_enabled,
        }
    }
}

/// Human one-liner for the boot surface. Window targets show the friendly
/// display label, not the durable key. (`window_type` is a runtime `String`, so
/// we can't route through `window_display_name`'s `&'static str` signature —
/// the one override is applied inline; keep in sync.)
fn describe_boot_surface(surface: &BootSurface, home_site_id: &str) -> String {
    match surface {
        BootSurface::Chrome => "Window chrome".to_string(),
        BootSurface::Site => {
            if home_site_id.is_empty() {
                "Content site".to_string()
            } else {
                format!("Content site: {home_site_id}")
            }
        }
        BootSurface::Window { window_type, .. } => {
            let label = match window_type.as_str() {
                "System Backend" => "System Overview",
                other => other,
            };
            format!("Window: {label}")
        }
    }
}

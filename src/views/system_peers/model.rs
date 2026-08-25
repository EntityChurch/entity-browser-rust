//! System-peers model — a read-only projection over the system peer(s) and
//! the durable posture. No state of its own; every fact is re-derived from
//! `Peers` + the registry + session config, so it tracks the tree reactively
//! (the window watches the registry + config prefixes).

use crate::peer_display::{PeerDescriptor, PeerRole, PeerRuntime};
use crate::peer_registry::read_registry;
use crate::peers::Peers;
use crate::session_config::{self, BootSurface};
use crate::views::short_pid;

use super::output::{SystemPeersOutput, SystemPeerCard};

#[derive(Debug, Default)]
pub struct SystemPeersModel;

impl SystemPeersModel {
    pub fn new() -> Self {
        Self
    }

    #[allow(dead_code)] // called from the WASM render path
    pub fn render_output(&self, peers: &Peers) -> SystemPeersOutput {
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

        SystemPeersOutput {
            in_app,
            native,
            total_peers,
            user_peers,
            startup,
            peer_creation: cfg.peer_creation_enabled,
        }
    }
}

/// Human one-liner for the boot surface. Window targets show the friendly,
/// **localized** display label, not the durable key — routed through
/// `window_display_name` (now `&str`), with `canonical_window_type` resolving a
/// legacy persisted key first.
fn describe_boot_surface(surface: &BootSurface, home_site_id: &str) -> String {
    match surface {
        BootSurface::Chrome => crate::i18n::t("boot_surface.chrome", &[]),
        BootSurface::Site => {
            if home_site_id.is_empty() {
                crate::i18n::t("boot_surface.site", &[])
            } else {
                crate::i18n::t("boot_surface.site_named", &[("id", home_site_id)])
            }
        }
        BootSurface::Window { window_type, .. } => {
            let label = crate::window::window_display_name(crate::window::canonical_window_type(
                window_type,
            ));
            crate::i18n::t("boot_surface.window", &[("label", &label)])
        }
    }
}

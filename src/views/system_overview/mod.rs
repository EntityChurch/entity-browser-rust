//! System-overview projection — the system peer(s) + the durable posture.
//!
//! This is no longer a window of its own. After the S2 merge (one window, one
//! job) the projection is folded into the **System Overview** window
//! (`crate::views::system_backend`), which renders these system-peer cards +
//! posture line above the native peer's live detail (status, device auth, logs,
//! share). The model here stays the single, read-only source for that top
//! section; the DOM lives in `crate::dom::system_overview::render_system_peers`.

pub mod model;
pub mod output;

#[cfg(test)]
mod tests {
    use super::model::SystemOverviewModel;
    use crate::peers::Peers;

    #[test]
    fn in_app_system_peer_is_always_present_and_native_absent_in_browser() {
        let pm = Peers::new_direct();
        // total_peers/user_peers read the tree registry, which boot's
        // `PeerRegistry::sync` populates — do the same here.
        let mut reg = crate::peer_registry::PeerRegistry::new(&pm);
        reg.sync(&pm);

        let out = SystemOverviewModel::new().render_output(&pm);
        // The boot peer is the in-app system peer.
        assert_eq!(out.in_app.full_id, pm.system_peer_id());
        assert_eq!(
            out.in_app.descriptor.role,
            crate::peer_display::PeerRole::System
        );
        // A plain Direct/browser session: just the system peer, no native
        // process, no user peers.
        assert!(out.native.is_none(), "no native system peer without a backend");
        assert_eq!(out.total_peers, 1, "only the system peer is hosted");
        assert_eq!(out.user_peers, 0);
    }
}

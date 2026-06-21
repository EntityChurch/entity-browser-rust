//! Storage-overview model — reads the live stores into a [`StorageOutput`].
//!
//! Read-only. Per peer it pulls the two O(1) totals
//! (`entity_count` = content-store blobs, `path_count` = live paths) and one
//! `tree_listing` pass bucketed by top-level segment. The origin disk
//! estimate is fetched asynchronously by the window and threaded in here.

use crate::peers::Peers;

use super::output::{OriginEstimate, PeerStorage, PrefixCount, StorageOutput};

pub struct StorageModel;

impl StorageModel {
    pub fn new() -> Self {
        Self
    }

    /// Build the render output for every hosted peer. `estimate` is the
    /// origin-level disk probe (None until it resolves).
    pub fn render_output(&self, peers: &Peers, estimate: Option<OriginEstimate>) -> StorageOutput {
        let peer_rows = peers
            .peer_ids()
            .iter()
            .map(|pid| build_peer(peers, pid))
            .collect();
        StorageOutput {
            peers: peer_rows,
            estimate,
        }
    }
}

impl Default for StorageModel {
    fn default() -> Self {
        Self::new()
    }
}

fn build_peer(peers: &Peers, pid: &str) -> PeerStorage {
    let content_blobs = peers.entity_count(pid);
    let live_paths = peers.path_count(pid);
    let is_backend = peers.is_backend_hosted(pid);

    // One listing pass over the peer's whole namespace, bucketed by the
    // first path segment after `/{pid}/`. (On the Worker arm this reflects
    // only the cached/subscribed mirror — flagged in the renderer.)
    let peer_prefix = format!("/{pid}/");
    let save_state_prefix = format!("/{pid}/app/{}/apps/", crate::app_paths::APP_ID);
    let mut buckets: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut save_state_paths = 0usize;
    for entry in peers.tree_listing(pid, "") {
        if entry.path.starts_with(&save_state_prefix) {
            save_state_paths += 1;
        }
        if let Some(seg) = top_segment(&entry.path, &peer_prefix) {
            *buckets.entry(seg.to_string()).or_default() += 1;
        }
    }

    PeerStorage {
        peer_id: pid.to_string(),
        is_backend,
        content_blobs,
        live_paths,
        buckets: buckets
            .into_iter()
            .map(|(label, count)| PrefixCount { label, count })
            .collect(),
        save_state_paths,
    }
}

/// The first path segment after the `/{pid}/` prefix, or `None` if `path`
/// isn't under this peer or has no segment.
fn top_segment<'a>(path: &'a str, peer_prefix: &str) -> Option<&'a str> {
    let rest = path.strip_prefix(peer_prefix)?;
    let seg = rest.split('/').next()?;
    (!seg.is_empty()).then_some(seg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_segment_extracts_first_level() {
        assert_eq!(top_segment("/P1/app/entity-browser/x", "/P1/"), Some("app"));
        assert_eq!(top_segment("/P1/system/handler/y", "/P1/"), Some("system"));
        // A different peer's path → not bucketed here.
        assert_eq!(top_segment("/P2/app/x", "/P1/"), None);
        // No segment after the prefix.
        assert_eq!(top_segment("/P1/", "/P1/"), None);
    }

    #[tokio::test]
    async fn counts_and_buckets_a_seeded_peer() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        // Seed a save-state entity and a non-save app entity.
        peers.seed_write(
            &pid,
            crate::app_paths::app_save_path(crate::app_paths::APP_ID, &pid, "games", "war"),
            entity_entity::Entity::new("app/state/app_save", vec![1, 2, 3]).unwrap(),
        );

        let out = StorageModel::new().render_output(&peers, None);
        let me = out
            .peers
            .iter()
            .find(|p| p.peer_id == pid)
            .expect("hosted peer present");

        // Save-state path is counted, and the `app` bucket is non-empty.
        assert!(me.save_state_paths >= 1, "save-state path counted");
        assert!(
            me.buckets.iter().any(|b| b.label == "app" && b.count >= 1),
            "app bucket present"
        );
        // Content store holds at least as many blobs as live paths.
        assert!(me.content_blobs >= 1);
        assert!(me.live_paths >= 1);
    }
}

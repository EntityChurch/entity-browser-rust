//! Storage-overview model — reads the live stores into a [`StorageOutput`].
//!
//! Read-only. Per peer it pulls the two O(1) totals
//! (`entity_count` = content-store blobs, `path_count` = live paths) and one
//! `tree_listing` pass bucketed by top-level segment. The origin disk
//! estimate is fetched asynchronously by the window and threaded in here.

use crate::peers::Peers;

use super::output::{BackendStoreView, EstimateUnavailable, OriginEstimate, PeerStorage, PlaceUsage, PrefixCount, StorageOutput};

/// The Storage window rebuilds on **any** write to a hosted peer's tree, so
/// what it computes per rebuild has to stay cheap. An entity's size never
/// changes — its address is its content — so sizes are remembered by hash and
/// an entity is read once, not once per write anywhere in the tree (measured:
/// reading every entity per rebuild slowed the whole app enough to red two
/// fixed-sleep steps of the e2e monolith with Storage open).
pub struct StorageModel {
    sizes: std::cell::RefCell<std::collections::HashMap<entity_hash::Hash, u64>>,
}

impl StorageModel {
    pub fn new() -> Self {
        Self { sizes: Default::default() }
    }

    /// Build the render output for every hosted peer. `estimate` is the
    /// origin-level disk probe and `backend` the native system-backend store
    /// probe (both `None` until their async IPC/JS probes resolve; both are
    /// threaded in by the window rather than read here, since they're off-tree).
    pub fn render_output(
        &self,
        peers: &Peers,
        estimate: Option<Result<OriginEstimate, EstimateUnavailable>>,
        backend: Option<BackendStoreView>,
    ) -> StorageOutput {
        let peer_rows = peers
            .peer_ids()
            .iter()
            .map(|pid| build_peer(peers, pid, &mut self.sizes.borrow_mut()))
            .collect();
        StorageOutput {
            peers: peer_rows,
            estimate,
            backend,
        }
    }
}

impl Default for StorageModel {
    fn default() -> Self {
        Self::new()
    }
}

fn build_peer(peers: &Peers, pid: &str, sizes: &mut std::collections::HashMap<entity_hash::Hash, u64>) -> PeerStorage {
    let content_blobs = peers.entity_count(pid);
    let live_paths = peers.path_count(pid);
    let is_backend = peers.is_backend_hosted(pid);

    // One listing pass over the peer's whole namespace, bucketed by the
    // first path segment after `/{pid}/`. (On the Worker arm this reflects
    // only the cached/subscribed mirror — flagged in the renderer.)
    let peer_prefix = format!("/{pid}/");
    let save_state_prefix = format!("/{pid}/app/{}/apps/", crate::app_paths::APP_ID);
    let mut buckets: std::collections::BTreeMap<String, (usize, u64)> = std::collections::BTreeMap::new();
    let mut save_state_paths = 0usize;
    let mut live_bytes = 0u64;
    for entry in peers.tree_listing(pid, "") {
        if entry.path.starts_with(&save_state_prefix) {
            save_state_paths += 1;
        }
        // The entity's own bytes, read once per content hash (see `StorageModel`).
        let bytes = match sizes.get(&entry.hash) {
            Some(b) => *b,
            None => match peers.get_entity(pid, &entry.path) {
                Some(e) => {
                    let b = e.data.len() as u64;
                    sizes.insert(entry.hash, b);
                    b
                }
                None => 0,
            },
        };
        live_bytes += bytes;
        if let Some(seg) = top_segment(&entry.path, &peer_prefix) {
            let b = buckets.entry(seg.to_string()).or_default();
            b.0 += 1;
            b.1 += bytes;
        }
    }
    let rows = crate::file_kinds::list(peers, pid);
    let files = crate::file_kinds::Place::ALL
        .iter()
        .filter_map(|place| {
            let here: Vec<_> = rows.iter().filter(|r| r.place() == *place).collect();
            (!here.is_empty()).then(|| PlaceUsage {
                place: *place,
                files: here.len(),
                bytes: here.iter().map(|r| r.size).sum(),
            })
        })
        .collect();

    PeerStorage {
        peer_id: pid.to_string(),
        is_backend,
        content_blobs,
        live_paths,
        buckets: buckets
            .into_iter()
            .map(|(label, (count, bytes))| PrefixCount { label, count, bytes })
            .collect(),
        live_bytes,
        files,
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
            crate::apps::format::AppSave::new("{\"pile\":3}").to_entity(),
        );

        let out = StorageModel::new().render_output(&peers, None, None);
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
        // Sizes, not only counts: the save's 3 bytes are in the app bucket and
        // the total, and the save is a file the File Manager lists.
        let app = me.buckets.iter().find(|b| b.label == "app").expect("app bucket");
        assert!(app.bytes >= 3, "the app bucket carries its entities' bytes: {}", app.bytes);
        assert!(me.live_bytes >= app.bytes);
        let saves = me.files.iter().find(|f| f.place == crate::file_kinds::Place::Saves).expect("saves listed");
        assert_eq!(saves.files, 1);
        // Content store holds at least as many blobs as live paths.
        assert!(me.content_blobs >= 1);
        assert!(me.live_paths >= 1);
    }
}

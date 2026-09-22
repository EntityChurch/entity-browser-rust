//! Read an app set **off a peer's tree** (the publish [A] reader).
//!
//! Sibling of `content_site::read`: where the browse path uses the lazy catalog,
//! publishing reads everything — the catalog plus every listed bundle — so it can
//! re-emit the whole subgraph as static `.bin` content data. Reads are L0/sync
//! (no dispatch, no network), exactly like `read_all_sites`.

use crate::apps::format::{AppBundle, AppCatalog};
use crate::apps::ingest::IngestedApps;
use crate::apps::paths;
use crate::peers::Peers;

/// Read one app set's catalog + every bundle it lists off `peer_id`'s tree.
/// Returns `None` when no catalog is present for that set (nothing to publish);
/// entries whose bundle blob is missing are skipped (the catalog is the source
/// of truth for *which* apps exist, the blob for their bytes).
pub fn read_app_set(peers: &Peers, peer_id: &str, set: &str) -> Option<IngestedApps> {
    let cat_ent = peers.get_entity(peer_id, &paths::catalog_path(peer_id, set))?;
    let catalog = AppCatalog::from_entity(&cat_ent);
    let bundles = catalog
        .entries
        .iter()
        .filter_map(|e| {
            let ent = peers.get_entity(peer_id, &paths::bundle_path(peer_id, set, &e.id))?;
            Some((e.id.clone(), AppBundle::from_entity(&ent)))
        })
        .collect();
    let assets = read_asset_bundles(peers, peer_id, set, &catalog);
    Some(IngestedApps { catalog, bundles, assets })
}

/// Every declared asset bundle's index and content closure, read back off the
/// tree for re-projection.
///
/// **A bundle whose closure is incomplete is still returned, with what is
/// held**, and warned about. Refusing it here would silently drop the whole
/// bundle from the publish; projecting it lets `publish --verify` name the
/// missing hashes, which is the tool built to say exactly that.
fn read_asset_bundles(
    peers: &Peers,
    peer_id: &str,
    set: &str,
    catalog: &AppCatalog,
) -> Vec<crate::apps::assets::IngestedBundle> {
    use crate::apps::assets::{AssetIndex, IngestedBundle};
    use crate::content_site::asset_store::chunk_hashes_of;
    let mut out = Vec::new();
    for entry in &catalog.entries {
        for bundle in &entry.assets {
            let path = paths::asset_index_path(peer_id, set, &entry.id, bundle);
            let Some(ent) = peers.get_entity(peer_id, &path) else {
                tracing::warn!(app = %entry.id, bundle = %bundle, "publish read: declared asset bundle has no index in the tree");
                continue;
            };
            let index = match AssetIndex::from_entity(&ent) {
                Ok(i) => i,
                Err(e) => {
                    tracing::warn!(app = %entry.id, bundle = %bundle, error = %e, "publish read: asset index unreadable — not projected");
                    continue;
                }
            };
            let mut content = Vec::new();
            let mut missing = 0usize;
            for blob_hash in index.blobs() {
                let Some(blob) = peers.content_by_hash(peer_id, &blob_hash) else {
                    missing += 1;
                    continue;
                };
                if let Ok(chunks) = chunk_hashes_of(&blob) {
                    for ch in chunks {
                        match peers.content_by_hash(peer_id, &ch) {
                            Some(e) => content.push(e),
                            None => missing += 1,
                        }
                    }
                }
                content.push(blob);
            }
            if missing > 0 {
                tracing::warn!(app = %entry.id, bundle = %bundle, missing, "publish read: asset bundle closure incomplete — projecting what is held");
            }
            out.push(IngestedBundle { app_id: entry.id.clone(), bundle: bundle.clone(), index, content });
        }
    }
    out
}

/// Read **every** app set ([`paths::APP_SETS`]) off `peer_id`'s tree, keyed by
/// set id — what the publish pipeline emits (every set rides every publish, the
/// way sites do). Sets with no catalog are omitted.
pub fn read_all_app_sets(peers: &Peers, peer_id: &str) -> crate::apps::ingest::IngestedSets {
    paths::APP_SETS
        .iter()
        .filter_map(|set| read_app_set(peers, peer_id, set).map(|ing| (set.to_string(), ing)))
        .collect()
}

/// Back-compat convenience: read just the games set.
pub fn read_all_games(peers: &Peers, peer_id: &str) -> Option<IngestedApps> {
    read_app_set(peers, peer_id, paths::GAMES_SET)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_seeded_games_off_the_tree() {
        use crate::apps::format::AppEntry;
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        // Seed a catalog + bundle directly (no demo-apps fixture dependency) so
        // this read coverage runs under a plain `make test`.
        let catalog = AppCatalog {
            entries: vec![AppEntry {
                id: "tester".into(),
                name: "Tester".into(),
                ..Default::default()
            }],
        };
        peers.seed_write(&pid, paths::catalog_path(&pid, paths::GAMES_SET), catalog.to_entity());
        peers.seed_write(
            &pid,
            paths::bundle_path(&pid, paths::GAMES_SET, "tester"),
            AppBundle::new("<!doctype html><title>t</title>").to_entity(),
        );

        let games = read_all_games(&peers, &pid).expect("catalog present");
        assert!(!games.catalog.entries.is_empty(), "catalog has entries");
        assert_eq!(
            games.bundles.len(),
            games.catalog.entries.len(),
            "every cataloged game has its bundle"
        );
        assert!(games.catalog.entries.iter().any(|e| e.id == "tester"));
        // Bundle bytes are real HTML, not empty.
        assert!(games.bundles.iter().all(|(_, b)| !b.html.is_empty()));
    }

    #[test]
    fn no_catalog_yields_none() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        assert!(read_all_games(&peers, &pid).is_none());
    }
}

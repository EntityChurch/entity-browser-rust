//! [A] The subgraph reader — read a real site off the **live tree**.
//!
//! The emitters ([`super::static_export`] `.html`, [`super::publish_fixture`]
//! `.bin`) render from an in-memory site model. This module produces that
//! model from a peer's actual tree, so "publish" exports your *real* state,
//! not a hand-built fixture. It is the single source the emitters share —
//! one read, N projections (the publish-pipeline map §3, workstream [A]).
//!
//! **Recursive, not lazy.** Browsing uses [`super::discovery`]'s one-level
//! `.list` to avoid materializing a huge tree. Publishing is the opposite:
//! we want *every* page, including nested sections (`guide/advanced/
//! internals`), so we scan the whole `pages/` prefix in one pass. The slug
//! is the full path after the prefix — nested paths come through whole.
//!
//! Arm-aware via [`Peers`] accessors (Direct reads the store index; Worker
//! reads the cache mirror, fed only for subscribed prefixes — a publish that
//! wants a not-yet-observed site must subscribe first, same caveat as
//! discovery). Reads are L0/sync; no dispatch, no network. Cross-peer reads
//! (fetch a *remote* peer's site to republish) are the deferred HTTP-poll
//! job — this reads the **bound peer's own** sites.

#![allow(dead_code)] // the export consumer is native-only; the reader is arm-neutral

use std::collections::BTreeSet;

use super::format::{SiteAsset, SiteManifest, SitePage, SITE_ASSET_TYPE, SITE_PAGE_TYPE};
use super::paths;
use crate::peers::Peers;

/// A whole site read off the tree — owned (vs the borrowing
/// [`super::static_export::ExportSite`] the emitter consumes), because the
/// data comes from decoding entities, not from `'static` literals. Pages are
/// `(slug, page)` with the **full** slug (`guide/advanced/internals`),
/// sorted for deterministic, byte-stable export output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedSite {
    pub peer_id: String,
    pub site_id: String,
    pub manifest: SiteManifest,
    pub pages: Vec<(String, SitePage)>,
    /// The site's embedded assets — `(name, asset)` where `name` is the path
    /// under `assets/` (`figures/x.png`). These are the closure an embed `ref`
    /// pulls in; carried so publish/cache emit the bytes alongside the pages.
    pub assets: Vec<(String, SiteAsset)>,
}

/// Read one site (manifest + every page) off `peer_id`'s tree. Returns
/// `None` when there is no manifest at the site's manifest path — i.e. the
/// site doesn't exist (or isn't local/observed yet). A site with a manifest
/// but no pages reads back as `Some` with an empty `pages` (a valid, if
/// empty, site — the caller decides whether that's worth exporting).
pub fn read_site(peers: &Peers, peer_id: &str, site_id: &str) -> Option<OwnedSite> {
    let manifest = peers
        .get_entity(peer_id, &paths::manifest_path(peer_id, site_id))
        .map(|e| SiteManifest::from_entity(&e))?;

    // One full scan of the pages prefix → every page slug (nested included).
    // tree_listing yields concrete entity paths; the slug is the remainder
    // after the prefix. Dedup + sort via a BTreeSet for stable output.
    let prefix = paths::pages_prefix(peer_id, site_id);
    let mut slugs: BTreeSet<String> = BTreeSet::new();
    for entry in peers.tree_listing(peer_id, &prefix) {
        if let Some(rest) = entry.path.strip_prefix(&prefix) {
            let slug = rest.trim_start_matches('/');
            if !slug.is_empty() {
                slugs.insert(slug.to_string());
            }
        }
    }

    let mut pages = Vec::with_capacity(slugs.len());
    for slug in slugs {
        // Confirm the entity is actually a page (the prefix scan could in
        // principle surface a non-page entity placed under pages/); skip
        // anything that isn't, so export never emits a mis-typed body.
        if let Some(e) = peers.get_entity(peer_id, &paths::page_path(peer_id, site_id, &slug)) {
            if e.entity_type == SITE_PAGE_TYPE {
                pages.push((slug, SitePage::from_entity(&e)));
            }
        }
    }

    // The asset subgraph — `assets/{name}` entities (content-addressed bytes).
    // Same full-prefix scan as pages; the name is the remainder after the
    // prefix (`figures/x.png`). An embed `ref` resolves against this set.
    let aprefix = paths::assets_prefix(peer_id, site_id);
    let mut anames: BTreeSet<String> = BTreeSet::new();
    for entry in peers.tree_listing(peer_id, &aprefix) {
        if let Some(rest) = entry.path.strip_prefix(&aprefix) {
            let name = rest.trim_start_matches('/');
            if !name.is_empty() {
                anames.insert(name.to_string());
            }
        }
    }
    let mut assets = Vec::with_capacity(anames.len());
    for name in anames {
        if let Some(e) = peers.get_entity(peer_id, &paths::asset_path(peer_id, site_id, &name)) {
            if e.entity_type == SITE_ASSET_TYPE {
                assets.push((name, SiteAsset::from_entity(&e)));
            }
        }
    }

    Some(OwnedSite {
        peer_id: peer_id.to_string(),
        site_id: site_id.to_string(),
        manifest,
        pages,
        assets,
    })
}

/// Read **every** site under `peer_id` — the publish-all sweep. Discovers
/// site ids via the same `.list` primitive the site picker uses
/// ([`super::discovery::list_sites`]), then reads each. Sites that fail to
/// read (no manifest) are dropped. Order follows `list_sites` (sorted).
pub fn read_all_sites(peers: &Peers, peer_id: &str) -> Vec<OwnedSite> {
    super::discovery::list_sites(peers, peer_id)
        .into_iter()
        .filter_map(|site_id| read_site(peers, peer_id, &site_id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_site::format::NavItem;
    use crate::views::content_site::{ensure_demo_site, DEMO_SITE_ID};

    /// The bundled deep demo site, seeded into a real tree, must read back
    /// whole: the manifest, every flat page, AND the nested guide pages
    /// (`guide/intro`, `guide/install`, `guide/advanced/internals`). This is
    /// the recursive property the lazy `.list` browse path deliberately
    /// doesn't have — publishing needs the full closure.
    #[test]
    fn reads_bundled_demo_site_with_nested_pages_off_the_tree() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        ensure_demo_site(&peers, &pid);

        let site = read_site(&peers, &pid, DEMO_SITE_ID).expect("demo site reads");
        assert_eq!(site.manifest.title, "Entity Demo Site");
        assert_eq!(site.site_id, DEMO_SITE_ID);

        let slugs: Vec<&str> = site.pages.iter().map(|(s, _)| s.as_str()).collect();
        // Flat pages.
        assert!(slugs.contains(&"index"), "pages: {slugs:?}");
        assert!(slugs.contains(&"about"));
        assert!(slugs.contains(&"theory"));
        // Nested section pages — the recursive read (NOT one-level lazy).
        assert!(slugs.contains(&"guide/intro"), "nested guide page missing: {slugs:?}");
        assert!(slugs.contains(&"guide/install"));
        assert!(
            slugs.contains(&"guide/advanced/internals"),
            "3-level-deep page missing: {slugs:?}"
        );

        // Bodies actually decoded (not empty husks).
        let index = site.pages.iter().find(|(s, _)| s == "index").unwrap();
        assert!(index.1.body.contains("Welcome to the Entity Demo Site"));
    }

    #[test]
    fn missing_site_reads_none() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        assert!(read_site(&peers, &pid, "ghost").is_none());
    }

    #[test]
    fn read_all_sweeps_every_site_under_a_peer() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let ctx = peers.test_seed_ctx(&pid);

        // Two sites on one peer, seeded via real L0 writes.
        ensure_demo_site(&peers, &pid);
        let info = SiteManifest::new(
            "entity-info",
            "Entity Info",
            "index",
            vec![NavItem::new("Overview", "/index")],
        );
        ctx.store().put(&paths::manifest_path(&pid, "entity-info"), info.to_entity()).ok();
        ctx.store()
            .put(
                &paths::page_path(&pid, "entity-info", "index"),
                SitePage::markdown("Overview", "# Info\n\nBack to [Demo](site:demo/index).").to_entity(),
            )
            .ok();

        let mut sites = read_all_sites(&peers, &pid);
        sites.sort_by(|a, b| a.site_id.cmp(&b.site_id));
        let ids: Vec<&str> = sites.iter().map(|s| s.site_id.as_str()).collect();
        assert!(ids.contains(&DEMO_SITE_ID), "swept site ids: {ids:?}");
        assert!(ids.contains(&"entity-info"), "swept site ids: {ids:?}");

        // The second site's single page came through with its cross-link body.
        let info_site = sites.iter().find(|s| s.site_id == "entity-info").unwrap();
        assert_eq!(info_site.pages.len(), 1);
        assert!(info_site.pages[0].1.body.contains("site:demo/index"));
    }
}

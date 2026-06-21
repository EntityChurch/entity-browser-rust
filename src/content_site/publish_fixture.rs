//! Dev/test **publisher** — emit a content site as a static Amendment-5
//! directory (the shape `entity-publish` produces) so the HTTP-poll
//! consumer can be exercised end-to-end **without** workbench-go.
//!
//! Layout written (the v0.5 tree-path mirror the HTTP-poll consumer reads):
//! ```text
//! {dir}/
//!   content/{aa}/{bb}/{hex66}                ← bare hashable body (content-hash space)
//!   {peer_id}/sites/{site}/manifest.bin      ← system/hash pointer (site subgraph)
//!   {peer_id}/sites/{site}/pages/{slug}.bin
//! ```
//! Note `content/` is still the content-hash blob store (CONTENT extension,
//! `{hex(H)}` leaves) — the site subgraph moved OUT of it to a bare `sites/`
//! (v0.5 §2; was `content/sites/…`, the dropped layer violation).
//!
//! Native-only (writes files); never compiled into the wasm bundle. Used
//! by the e2e harness to drop a remote site into `dist/` and by the
//! round-trip test below. The bytes are produced with the **same**
//! `entity_ecf` encoders the consumer verifies against, so a successful
//! resolve here is a real proof of the on-disk wire shape.

#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)] // consumed by the e2e harness + the test below

use std::fs;
use std::path::Path;

use entity_entity::Entity;

use super::format::{SiteAsset, SiteManifest, SitePage};
use super::read::OwnedSite;

/// Emit a set of sites read off the **live tree** ([`OwnedSite`]) as the
/// entity-native content-data publish — the [A]→[B2] path. Writes the
/// content-addressed blob store (`content/{aa}/{bb}/{hex}`) plus the
/// peer-first `system/hash` pointer mirror (`{peer}/sites/{site}/….bin`) an
/// entity-aware peer / the HTTP-poll consumer ingests. Distinct top-level
/// dirs from the `.html` projection (`sites/{peer}/…`), so a unified publish
/// emits both into one `out_dir` without collision. Returns the site count.
///
/// **Scope: the `sites/` subgraph only** — `read_all_sites` reads a peer's
/// published sites, never its keys/app-state/private tree. Publishing the
/// *whole* peer tree is a separate, deliberately-gated capability (MAP §7),
/// not this. The content data here is exactly the site entities, content-
/// addressed and hash-verifiable.
///
/// `prefix` is the published-tree hosting scope (empty = root, byte-identical
/// to the un-prefixed layout). Everything — `content/` blobs and the `{peer}/`
/// pointer mirror — nests under `{dir}/{prefix}/…`; the registered HTTP origin
/// carries the same prefix, so the two-hop consumer resolves with no change.
pub fn emit_owned_sites(dir: &Path, sites: &[OwnedSite], prefix: &str) -> std::io::Result<usize> {
    let base = super::paths::prefixed_root(dir, prefix);
    for site in sites {
        let pages: Vec<(&str, SitePage)> =
            site.pages.iter().map(|(slug, page)| (slug.as_str(), page.clone())).collect();
        let assets: Vec<(&str, SiteAsset)> =
            site.assets.iter().map(|(name, a)| (name.as_str(), a.clone())).collect();
        emit_site(&base, &site.peer_id, &site.site_id, &site.manifest, &pages, &assets)?;
    }
    // The per-peer `sites.list` enumeration artifact — the PEER-level sibling of
    // each site's `pages.list`. Every site_id a peer hosts, one per line. The
    // remote cache-awareness consumer (`app::EntityApp::precache_peer_sites`)
    // fetches this on boot to pre-cache all of a peer's site manifests, so the
    // directory rail lists every site BEFORE any has been visited (pages still
    // fetch lazily on click). pages.list enumerates pages; sites.list enumerates
    // sites.
    write_sites_lists(&base, sites)?;
    Ok(sites.len())
}

/// Write `{dir}/{peer}/sites.list` for every peer represented in `sites` —
/// sorted, deduped site ids, one per line (trailing newline). Grouped by peer
/// so a multi-peer publish emits one list under each peer's directory.
fn write_sites_lists(dir: &Path, sites: &[OwnedSite]) -> std::io::Result<()> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut by_peer: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for site in sites {
        by_peer
            .entry(site.peer_id.as_str())
            .or_default()
            .insert(site.site_id.as_str());
    }
    for (peer, ids) in by_peer {
        let mut body = ids.into_iter().collect::<Vec<_>>().join("\n");
        if !body.is_empty() {
            body.push('\n');
        }
        let path = dir.join(peer).join("sites.list");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, body)?;
    }
    Ok(())
}

/// Emit `manifest` + `pages` as a published static site rooted at `dir`,
/// addressed under `peer_id`/`site_id`. Creates parent dirs as needed.
pub fn emit_site(
    dir: &Path,
    peer_id: &str,
    site_id: &str,
    manifest: &SiteManifest,
    pages: &[(&str, SitePage)],
    assets: &[(&str, SiteAsset)],
) -> std::io::Result<()> {
    write_entity(
        dir,
        peer_id,
        &format!("sites/{site_id}/manifest"),
        &manifest.to_entity(),
    )?;
    for (slug, page) in pages {
        write_entity(
            dir,
            peer_id,
            &format!("sites/{site_id}/pages/{slug}"),
            &page.to_entity(),
        )?;
    }
    // Asset blobs (content-addressed) + their `system/hash` pointers, at the
    // site's `assets/{name}` subgraph — the bytes an embed `ref` resolves to.
    // Same two-hop shape as a page, so the HTTP consumer's `fetch_asset` reads
    // them identically. Content-addressing dedups identical bytes across sites.
    for (name, asset) in assets {
        write_entity(
            dir,
            peer_id,
            &format!("sites/{site_id}/assets/{name}"),
            &asset.to_entity(),
        )?;
    }
    // The static `pages.list` listing artifact (the "static-origin floor" the
    // remote `.list` consumer reads — `discovery::children_from_slugs`). A
    // sibling of `manifest.bin`: every page slug, sorted, one per line. It is
    // NOT content-addressed (it's a derived index, not a tree entity), and is
    // covered by transport-trust like any served path. Lets a remote peer build
    // the sidebar + reach deep pages a CDN couldn't otherwise enumerate.
    write_pages_list(dir, peer_id, site_id, pages)?;
    Ok(())
}

/// Write `{dir}/{peer}/sites/{site}/pages.list` — sorted page slugs, one per
/// line (trailing newline). Empty-site → an empty file (still a valid listing).
fn write_pages_list(
    dir: &Path,
    peer_id: &str,
    site_id: &str,
    pages: &[(&str, SitePage)],
) -> std::io::Result<()> {
    let mut slugs: Vec<&str> = pages.iter().map(|(slug, _)| *slug).collect();
    slugs.sort_unstable();
    let mut body = slugs.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    let path = dir.join(peer_id).join(format!("sites/{site_id}/pages.list"));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, body)
}

/// Emit one app set (catalog + bundles) as published content data under
/// `{peer}/apps/{set}/…`, the same two-hop `.bin` shape as a site (so the HTTP
/// consumer resolves bundles exactly like assets). Generalizing the publish
/// emitter to a second data type: same `write_entity` primitive, a different
/// subgraph. `prefix` is the hosting scope (empty = root). Returns the bundle
/// count. Scope is the `apps/{set}/` subgraph only — never the whole peer tree.
pub fn emit_app_set(
    dir: &Path,
    peer_id: &str,
    set: &str,
    catalog: &crate::apps::format::AppCatalog,
    bundles: &[(String, crate::apps::format::AppBundle)],
    prefix: &str,
) -> std::io::Result<usize> {
    let base = super::paths::prefixed_root(dir, prefix);
    write_entity(&base, peer_id, &format!("apps/{set}/catalog"), &catalog.to_entity())?;
    for (id, bundle) in bundles {
        write_entity(
            &base,
            peer_id,
            &format!("apps/{set}/bundles/{id}"),
            &bundle.to_entity(),
        )?;
    }
    Ok(bundles.len())
}

/// Write one entity as a content blob (at its sharded content address)
/// plus a `system/hash` `.bin` pointer at its tree path.
fn write_entity(dir: &Path, peer_id: &str, tree_subpath: &str, ent: &Entity) -> std::io::Result<()> {
    let hex = ent.content_hash.to_hex();
    // content/{aa}/{bb}/{hex66} = the bare hashable body.
    let blob_dir = dir.join("content").join(&hex[0..2]).join(&hex[2..4]);
    fs::create_dir_all(&blob_dir)?;
    fs::write(blob_dir.join(&hex), entity_ecf::ecf_for_hash(&ent.entity_type, &ent.data))?;

    // {peer_id}/{tree_subpath}.bin = the 2-key system/hash pointer.
    let bin_path = dir.join(peer_id).join(format!("{tree_subpath}.bin"));
    if let Some(parent) = bin_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let pointer = entity_ecf::ecf_for_hash_value(
        "system/hash",
        &entity_ecf::Value::Bytes(ent.content_hash.to_bytes()),
    );
    fs::write(bin_path, pointer)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_site::format::NavItem;
    use crate::content_site::http_poll::{resolve_closure_via, BinSource, Freshness, PollError};
    use crate::content_site::location::Location;
    use std::future::Future;
    use std::path::PathBuf;
    use std::pin::Pin;

    /// A [`BinSource`] that reads files from an emitted fixture dir,
    /// resolving a URL `{origin}/{rel}` to `{root}/{rel}` — exactly what a
    /// same-origin static server does, but synchronous.
    struct FsBinSource {
        root: PathBuf,
        origin: String,
    }

    impl BinSource for FsBinSource {
        fn get(
            &self,
            url: String,
            _freshness: Freshness,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PollError>>>> {
            let rel = url.strip_prefix(&self.origin).unwrap_or(&url).trim_start_matches('/');
            let path = self.root.join(rel);
            let r = fs::read(&path).map_err(|e| PollError::Decode(format!("read {rel}: {e}")));
            Box::pin(std::future::ready(r))
        }
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        fn raw() -> RawWaker {
            fn noop(_: *const ()) {}
            fn clone(_: *const ()) -> RawWaker {
                raw()
            }
            RawWaker::new(std::ptr::null(), &RawWakerVTable::new(clone, noop, noop, noop))
        }
        let waker = unsafe { Waker::from_raw(raw()) };
        let mut cx = Context::from_waker(&waker);
        let mut future = Box::pin(future);
        loop {
            if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    /// Emit the e2e remote-fixture site into `dist/remote-fixture/` (the
    /// dir the e2e's static server serves). Run on demand by the e2e
    /// harness BEFORE serving — keeps the fixture in lockstep with the
    /// encoder (no checked-in binary artifacts). Mirrors the boot hook in
    /// `app.rs` (`bills-labs-peer` / `labs`, root = `index`).
    ///
    /// `cargo test --bin entity-browser emit_e2e_fixture -- --ignored`
    #[test]
    #[ignore = "fixture generator; run by the e2e harness, not the unit suite"]
    fn emit_e2e_fixture() {
        let dir = Path::new("dist/remote-fixture");
        let _ = fs::remove_dir_all(dir);
        fs::create_dir_all(dir).unwrap();

        let manifest = SiteManifest::new(
            "labs",
            "Bill's Labs",
            "index",
            vec![
                NavItem::new("Home", "/index"),
                NavItem::new("Guide", "/guide/intro"),
            ],
        );
        let pages = [
            (
                "index",
                SitePage::markdown(
                    "Bill's Labs — Home",
                    "# Bill's Labs\n\nThis page was **fetched over HTTP-poll** from another \
                     peer's published site — content-addressed, hash-verified, rendered inside \
                     the same overlay. See the [Guide](./guide/intro).",
                ),
            ),
            (
                "guide/intro",
                SitePage::markdown("Guide: Intro", "# Guide\n\nA nested remote page."),
            ),
        ];
        emit_site(dir, crate::app::REMOTE_FIXTURE_PEER, "labs", &manifest, &pages, &[]).unwrap();
        eprintln!("emitted e2e fixture → {}", dir.display());
    }

    #[test]
    fn emit_owned_sites_produces_resolvable_content_data() {
        // The [B2] owned bridge: an OwnedSite (as read off the live tree) emits
        // the same two-hop content data, resolvable through the consumer.
        let dir = std::env::temp_dir().join("entity-browser-publish-owned-bin-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let site = OwnedSite {
            peer_id: "PEERX".into(),
            site_id: "demo".into(),
            manifest: SiteManifest::new("demo", "Owned Demo", "index", vec![NavItem::new("Home", "/index")]),
            pages: vec![
                ("index".into(), SitePage::markdown("Home", "# Owned\n\nFrom the tree.")),
                ("guide/intro".into(), SitePage::markdown("Intro", "# Intro\n\nNested.")),
            ],
            assets: Vec::new(),
        };
        let n = emit_owned_sites(&dir, std::slice::from_ref(&site), "").unwrap();
        assert_eq!(n, 1);

        // Both the content blob store and the peer-first pointer mirror exist.
        assert!(dir.join("content").is_dir(), "content blob store missing");
        assert!(dir.join("PEERX/sites/demo/manifest.bin").exists(), "manifest pointer missing");
        assert!(dir.join("PEERX/sites/demo/pages/guide/intro.bin").exists(), "nested page pointer missing");

        // And it resolves end-to-end through the two-hop consumer.
        let origin = "http://localhost:8092/x";
        let src = FsBinSource { root: dir.clone(), origin: origin.to_string() };
        let loc = Location { peer_id: Some("PEERX".into()), site_id: "demo".into(), page: "guide/intro".into() };
        let rp = block_on(resolve_closure_via(&src, origin, &loc)).expect("owned .bin resolves");
        assert_eq!(rp.manifest.title, "Owned Demo");
        assert_eq!(rp.page.title(), "Intro");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn emit_app_set_produces_resolvable_bundles() {
        use crate::apps::format::{AppBundle, AppCatalog, AppEntry};
        use crate::content_site::http_poll::{fetch_app_bundle, fetch_app_catalog};

        let dir = std::env::temp_dir().join("entity-browser-publish-apps-bin-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let catalog = AppCatalog {
            entries: vec![AppEntry {
                id: "calc".into(),
                name: "Calc".into(),
                description: "d".into(),
                saves: false,
                ..Default::default()
            }],
        };
        let bundles = vec![("calc".to_string(), AppBundle::new("<html>calc</html>"))];
        // Emit under the non-games "apps" set — proves the set is parameterized.
        let n = emit_app_set(&dir, "PEERG", "apps", &catalog, &bundles, "").unwrap();
        assert_eq!(n, 1);
        assert!(dir.join("PEERG/apps/apps/catalog.bin").exists(), "catalog pointer missing");
        assert!(dir.join("PEERG/apps/apps/bundles/calc.bin").exists(), "bundle pointer missing");

        // Resolves end-to-end through the two-hop consumer.
        let origin = "http://localhost:8092/g";
        let src = FsBinSource { root: dir.clone(), origin: origin.to_string() };
        let cat_ent =
            block_on(fetch_app_catalog(&src, origin, "PEERG", "apps")).expect("catalog resolves");
        assert_eq!(AppCatalog::from_entity(&cat_ent).entries[0].id, "calc");
        let bundle_ent = block_on(fetch_app_bundle(&src, origin, "PEERG", "apps", "calc"))
            .expect("bundle resolves");
        assert_eq!(AppBundle::from_entity(&bundle_ent).html, "<html>calc</html>");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn emits_asset_blobs_that_fetch_back_through_the_consumer() {
        use crate::content_site::http_poll::fetch_asset;
        let dir = std::env::temp_dir().join("entity-browser-publish-assets-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let site = OwnedSite {
            peer_id: "PEERX".into(),
            site_id: "demo".into(),
            manifest: SiteManifest::new("demo", "Demo", "index", vec![]),
            pages: vec![(
                "index".into(),
                SitePage::markdown("Home", "# H\n\n::embed[Fig]{ref=assets/figures/d.svg}"),
            )],
            assets: vec![(
                "figures/d.svg".into(),
                SiteAsset::new("image/svg+xml", b"<svg/>".to_vec()),
            )],
        };
        emit_owned_sites(&dir, std::slice::from_ref(&site), "").unwrap();

        // The asset pointer + content blob landed at the assets/ leaf.
        assert!(
            dir.join("PEERX/sites/demo/assets/figures/d.svg.bin").exists(),
            "asset .bin pointer missing"
        );

        // And it round-trips through the consumer's asset two-hop.
        let origin = "http://localhost:8092/x";
        let src = FsBinSource { root: dir.clone(), origin: origin.to_string() };
        let ent = block_on(fetch_asset(&src, origin, "PEERX", "demo", "figures/d.svg"))
            .expect("asset two-hop resolves");
        let asset = SiteAsset::from_entity(&ent);
        assert_eq!(asset.media_type, "image/svg+xml");
        assert_eq!(asset.bytes, b"<svg/>");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn emits_pages_list_that_fetches_back_as_the_slug_set() {
        use crate::content_site::http_poll::fetch_pages_list;

        let dir = std::env::temp_dir().join("entity-browser-pages-list-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let manifest = SiteManifest::new("labs", "Labs", "index", vec![NavItem::new("Home", "/index")]);
        let pages = [
            ("index", SitePage::markdown("Home", "# Home")),
            ("guide/intro", SitePage::markdown("Intro", "# Intro")),
            ("guide/advanced/internals", SitePage::markdown("Internals", "# Deep")),
        ];
        emit_site(&dir, "PEERB", "labs", &manifest, &pages, &[]).unwrap();

        // The static listing exists alongside the manifest, sorted, one per line.
        let listing = fs::read_to_string(dir.join("PEERB/sites/labs/pages.list")).unwrap();
        assert_eq!(listing, "guide/advanced/internals\nguide/intro\nindex\n");

        // And it fetches back through the consumer as exactly the slug set.
        let origin = "http://localhost:8092/x";
        let src = FsBinSource { root: dir.clone(), origin: origin.to_string() };
        let slugs = block_on(fetch_pages_list(&src, origin, "PEERB", "labs")).unwrap();
        assert_eq!(slugs, vec!["guide/advanced/internals", "guide/intro", "index"]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn emits_sites_list_that_fetches_back_and_manifests_precache() {
        use crate::content_site::http_poll::{fetch_manifest, fetch_sites_list};

        let dir = std::env::temp_dir().join("entity-browser-sites-list-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // Two sites on ONE peer (the multi-site publish case) → one sites.list.
        let sites = vec![
            OwnedSite {
                peer_id: "PEERC".into(),
                site_id: "labs-main".into(),
                manifest: SiteManifest::new("labs-main", "Main", "index", vec![]),
                pages: vec![("index".into(), SitePage::markdown("Home", "# Main"))],
                assets: Vec::new(),
            },
            OwnedSite {
                peer_id: "PEERC".into(),
                site_id: "labs-research".into(),
                manifest: SiteManifest::new("labs-research", "Research", "index", vec![]),
                pages: vec![("index".into(), SitePage::markdown("Home", "# Research"))],
                assets: Vec::new(),
            },
        ];
        emit_owned_sites(&dir, &sites, "").unwrap();

        // The per-peer enumeration artifact: sorted site ids, one per line.
        let listing = fs::read_to_string(dir.join("PEERC/sites.list")).unwrap();
        assert_eq!(listing, "labs-main\nlabs-research\n");

        // It fetches back as the site set, and each listed manifest pre-caches
        // (the cache-awareness path: list → fetch manifest → directory shows it).
        let origin = "http://localhost:8092/x";
        let src = FsBinSource { root: dir.clone(), origin: origin.to_string() };
        let ids = block_on(fetch_sites_list(&src, origin, "PEERC")).unwrap();
        assert_eq!(ids, vec!["labs-main", "labs-research"]);
        for id in &ids {
            let m = block_on(fetch_manifest(&src, origin, "PEERC", id)).unwrap();
            assert_eq!(SiteManifest::from_entity(&m).site_id, *id, "manifest round-trips per listed site");
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prefix_nests_content_data_and_still_resolves() {
        // A non-empty prefix nests EVERY emitted path under `{dir}/{prefix}/…`,
        // and the consumer resolves it when the origin carries the same prefix.
        let dir = std::env::temp_dir().join("entity-browser-publish-owned-bin-prefix-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let site = OwnedSite {
            peer_id: "PEERX".into(),
            site_id: "demo".into(),
            manifest: SiteManifest::new("demo", "Owned Demo", "index", vec![NavItem::new("Home", "/index")]),
            pages: vec![("index".into(), SitePage::markdown("Home", "# Owned\n\nFrom the tree."))],
            assets: Vec::new(),
        };
        emit_owned_sites(&dir, std::slice::from_ref(&site), "hosted-peers/PEERX").unwrap();

        // Nothing at the un-prefixed root; everything under the prefix.
        assert!(!dir.join("content").exists(), "content leaked to root");
        assert!(!dir.join("PEERX").exists(), "pointer mirror leaked to root");
        assert!(dir.join("hosted-peers/PEERX/content").is_dir(), "content not nested");
        assert!(
            dir.join("hosted-peers/PEERX/PEERX/sites/demo/manifest.bin").exists(),
            "manifest pointer not nested"
        );

        // The origin carries the prefix → two-hop resolves unchanged.
        let origin = "http://localhost:8092/hosted-peers/PEERX";
        let src = FsBinSource { root: dir.join("hosted-peers/PEERX"), origin: origin.to_string() };
        let loc = Location { peer_id: Some("PEERX".into()), site_id: "demo".into(), page: String::new() };
        let rp = block_on(resolve_closure_via(&src, origin, &loc)).expect("prefixed .bin resolves");
        assert_eq!(rp.manifest.title, "Owned Demo");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn emit_to_disk_then_resolve_through_the_two_hop() {
        // Unique temp dir (no tempfile dep; clean + recreate).
        let dir = std::env::temp_dir().join("entity-browser-content-site-publish-fixture-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let (peer, site) = ("bills-labs-peer", "labs");
        let manifest = SiteManifest::new(
            "labs",
            "Bill's Labs",
            "index",
            vec![NavItem::new("Home", "/index"), NavItem::new("Deep", "/guide/intro")],
        );
        let pages = [
            ("index", SitePage::markdown("Home", "# Bill's Labs\n\nA **remote** site over HTTP-poll.")),
            ("guide/intro", SitePage::markdown("Guide", "# Guide\n\nA nested page.")),
        ];
        emit_site(&dir, peer, site, &manifest, &pages, &[]).unwrap();

        let origin = "http://localhost:8092/remote-fixture";
        let src = FsBinSource { root: dir.clone(), origin: origin.to_string() };

        // Root (empty page → manifest.root = index).
        let loc = Location { peer_id: Some(peer.into()), site_id: site.into(), page: String::new() };
        let rp = block_on(resolve_closure_via(&src, origin, &loc)).expect("root resolves from disk");
        assert_eq!(rp.manifest.title, "Bill's Labs");
        assert_eq!(rp.location.page, "index");
        assert!(rp.page.body.contains("remote"));

        // A nested page two-hop resolves from the on-disk layout.
        let loc = Location { peer_id: Some(peer.into()), site_id: site.into(), page: "guide/intro".into() };
        let rp = block_on(resolve_closure_via(&src, origin, &loc)).expect("nested page resolves");
        assert_eq!(rp.page.title(), "Guide");

        let _ = fs::remove_dir_all(&dir);
    }
}

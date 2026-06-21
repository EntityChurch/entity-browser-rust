//! Progressive (lazy) tree discovery — the `.list` primitive.
//!
//! Browsing a content site never materializes the whole tree (the
//! "download the index" anti-pattern at Wikipedia scale).
//! Discovery is **one level at a time**: list the IMMEDIATE children
//! under a pages prefix, **body-free**, on demand, cache. This is the
//! local/cached-tree form — a sync L0 `store().list(prefix)`, which scans
//! the location index (paths + hashes, **no entity bodies**) and we
//! reduce to the immediate child segments.
//!
//! The async remote form slots in behind this same shape via
//! [`children_from_slugs`]: a static `pages.list` listing artifact fetched
//! over HTTP (the static-origin floor — no query handler;
//! `publish_fixture::write_pages_list` emits it, `http_poll::fetch_pages_list`
//! reads it, the HTTP-poll resolver caches + reduces it). A cross-peer
//! dispatched list against a *live* peer is the remaining future form.
//! Browse-time discovery is `.list` only; **`query` is a cached-tree /
//! post-sync affordance, deliberately not on this path** (it would force
//! "sync the whole site to query").

#![allow(dead_code)] // the `.list` render consumer is deferred (review finding #4)

use std::collections::BTreeMap;

use super::paths;
use crate::peers::Peers;

/// One immediate child under a pages prefix — what a sidebar / tree-nav /
/// the offline-sync walk consumes. Body-free: we know the name and
/// whether it's a leaf page and/or a section, without loading any entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildEntry {
    /// The child's path segment (e.g. `guide`, `intro`).
    pub name: String,
    /// A page entity exists at exactly this child — a leaf you can open.
    pub is_page: bool,
    /// The child has descendants — a section you can descend into (lazily).
    pub is_section: bool,
}

/// List the IMMEDIATE children under `under` within a site, reading the
/// **local/cached tree only** (a body-free index scan). `under` is a
/// pages-relative prefix: `""` = the site's page root, `"guide/"` = the
/// Guide section.
///
/// Lazy + body-free: one level, no entity bodies loaded — so descending
/// into `guide/` does **not** materialize `guide/advanced/internals`.
/// Routed through [`Peers::tree_listing`], so it works on **both arms**:
/// Direct reads the store index; Worker reads the cache mirror
/// (`cache_list`) — which is fed only for **subscribed** prefixes, so the
/// reading surface must observe the site prefix (the overlay does). Returns
/// empty for a not-yet-local peer/site — a *remote* peer's `.list` comes from
/// the fetched `pages.list` via [`children_from_slugs`], routed by the
/// HTTP-poll resolver, not from this local scan.
pub fn list_child_pages(peers: &Peers, peer_id: &str, site_id: &str, under: &str) -> Vec<ChildEntry> {
    let prefix = format!("{}{}", paths::pages_prefix(peer_id, site_id), under);

    let mut children: BTreeMap<String, ChildEntry> = BTreeMap::new();
    for entry in peers.tree_listing(peer_id, &prefix) {
        let Some(rest) = entry.path.strip_prefix(&prefix) else {
            continue;
        };
        let rest = rest.trim_start_matches('/');
        if rest.is_empty() {
            continue;
        }
        let mut segs = rest.splitn(2, '/');
        let name = match segs.next() {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => continue,
        };
        // A deeper segment under this name → it's a section; an entry
        // exactly at `prefix + name` → a page leaf. A name can be both.
        let has_more = segs.next().is_some();
        let child = children.entry(name.clone()).or_insert(ChildEntry {
            name,
            is_page: false,
            is_section: false,
        });
        if has_more {
            child.is_section = true;
        } else {
            child.is_page = true;
        }
    }
    children.into_values().collect()
}

/// Reduce a **flat slug set** to the immediate children under `under` — the
/// same one-level shape as [`list_child_pages`], but computed from an explicit
/// list of full page slugs (`guide/intro`, `guide/advanced/internals`, …)
/// rather than a tree scan. This is the **remote** form: a static `pages.list`
/// artifact fetched over HTTP gives the slug set (the "static-origin floor"
/// listing the module header anticipates), and this turns it into the sidebar /
/// section-index children for a peer whose tree we can't scan locally. `under`
/// is a pages-relative prefix (`""` = root, `"guide/"` = the Guide section).
/// Body-free by nature (slugs carry no bodies). Sorted + deduped.
pub fn children_from_slugs(slugs: &[String], under: &str) -> Vec<ChildEntry> {
    let mut children: BTreeMap<String, ChildEntry> = BTreeMap::new();
    for slug in slugs {
        let Some(rest) = slug.strip_prefix(under) else {
            continue;
        };
        let rest = rest.trim_start_matches('/');
        if rest.is_empty() {
            continue;
        }
        let mut segs = rest.splitn(2, '/');
        let name = match segs.next() {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => continue,
        };
        let has_more = segs.next().is_some();
        let child = children.entry(name.clone()).or_insert(ChildEntry {
            name,
            is_page: false,
            is_section: false,
        });
        if has_more {
            child.is_section = true;
        } else {
            child.is_page = true;
        }
    }
    children.into_values().collect()
}

/// List the **site ids** under a peer — the immediate children of the peer's
/// `sites/` prefix. What the startup-surface settings control offers as
/// "boot into <site>" targets, and a future site picker. Body-free index scan,
/// same lazy `.list` shape as [`list_child_pages`]; arm-aware via
/// [`Peers::tree_listing`]. On the Worker arm the cache mirror only feeds
/// **subscribed** prefixes, so a consumer must observe
/// [`paths::sites_prefix`] for this to be non-empty
/// (`feedback_worker_cache_get_needs_subscription`). Returns sorted, deduped.
pub fn list_sites(peers: &Peers, peer_id: &str) -> Vec<String> {
    // Owned sites: scan `peer_id`'s OWN store under its own `sites/` prefix.
    sites_under(peers, peer_id, peer_id)
}

/// Scan `selector`'s store for the site ids under `site_peer`'s `sites/`
/// prefix. For owned sites `selector == site_peer` (the [`list_sites`] case);
/// for **cached foreign** sites the content lives at `/{foreign}/sites/` in MY
/// store, so `selector = me`, `site_peer = foreign` (the §2 selector/path
/// split). Body-free index scan; arm-aware via [`Peers::tree_listing`] (Worker
/// arm: the reader must observe the prefix). Sorted, deduped.
fn sites_under(peers: &Peers, selector: &str, site_peer: &str) -> Vec<String> {
    let prefix = paths::sites_prefix(site_peer);
    let mut ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for entry in peers.tree_listing(selector, &prefix) {
        let Some(rest) = entry.path.strip_prefix(&prefix) else {
            continue;
        };
        let rest = rest.trim_start_matches('/');
        // First segment = the site id (e.g. `demo` from `demo/manifest`).
        if let Some(id) = rest.split('/').next() {
            if !id.is_empty() {
                ids.insert(id.to_string());
            }
        }
    }
    ids.into_iter().collect()
}

/// Synchronously enumerate the sites **physically present in MY store** —
/// owned (`/{me}/sites/`) and cached-foreign (`/{foreign}/sites/` for each peer
/// I hold a route to). The DIRECT-READ counterpart to the async,
/// query-materialized [`read_site_index`]: the index lags (until the refresh
/// query lands) or stays empty (if the query fails), but a site whose manifest
/// is physically in my store is **browsable right now** — the same direct read
/// the browse area resolves through. The directory rail unions this with the
/// index so it can NEVER say "No sites yet" while the browse area renders a
/// site (BUG-3, the divergent-truths bug). A cached foreign site always has a
/// registered origin (the write-through runs only after an origin-routed
/// fetch), so the origins roster IS the foreign-peer set to scan. Arm-aware via
/// [`Peers::tree_listing`]; on the Worker arm the reader must observe the owned
/// + each foreign `sites/` prefix (the overlay/window factories do).
pub fn scan_local_sites(peers: &Peers, me: &str) -> Vec<SiteRef> {
    let mut out: std::collections::BTreeSet<SiteRef> = std::collections::BTreeSet::new();
    for site in sites_under(peers, me, me) {
        out.insert(SiteRef { peer: me.to_string(), site, owned: true });
    }
    for (foreign, _origin) in crate::content_site::origins::list_origins(peers, me) {
        for site in sites_under(peers, me, &foreign) {
            out.insert(SiteRef { peer: foreign.clone(), site, owned: false });
        }
    }
    out.into_iter().collect()
}

/// A site discoverable in MY store — owned (I authored it, `peer == me`) or
/// cached (fetched from a foreign peer via the P1 write-through, `peer != me`).
/// The peer-segment of the path *is* the owned-vs-cached signal (V7 §1.4); no
/// flag is stored — `owned` is derived here for the caller's convenience.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SiteRef {
    pub peer: String,
    pub site: String,
    pub owned: bool,
}

/// Enumerate EVERY site my store holds — owned and cached alike — the
/// universal-tree-native way: ask the query engine for every
/// [`SITE_MANIFEST_TYPE`](crate::content_site::format::SITE_MANIFEST_TYPE)
/// entity across the whole keyspace, then read the owning peer + site id off
/// each match's path ([`paths::parse_manifest_path`]). The tree is one keyspace
/// partitioned by peer, so we search by **type** and let the path say whose site
/// it is — no peer enumeration, no prefix filtering (contrast the single-peer
/// [`list_sites`], where you already hold the peer).
///
/// Works on **both arms** and — unlike a `tree_listing`/`cache_list` scan —
/// needs **no Worker-arm subscription coverage**: the Worker proxy runs `query`
/// *inside the worker against the real store* (`wasm-worker-host` `handle_query`
/// → `peer_ctx.query`), not over the main-thread cache mirror. Async because L1
/// `query` is dispatched; the selector is always MY peer.
///
/// > **wb-go discipline:** this `system/query` find is the L5 form for v1. The
/// > SDK-blessed shape is an index-/subscription-backed
/// > `peer.list_matching("/*/sites/*/manifest")` (Gap 2) — lift to it when the
/// > seam lands; the call site here is the one place that changes.
pub async fn list_all_sites(peers: &Peers, me: &str) -> Vec<SiteRef> {
    match peers.query(me, manifest_type_query()).await {
        Ok(results) => site_refs_from_query(&results, me),
        Err(_) => Vec::new(),
    }
}

/// Reduce manifest-type query matches to sorted, deduped [`SiteRef`]s, deriving
/// `owned` from the path's peer-segment vs `me`. Pure — shared by the async
/// [`list_all_sites`] and the index [`refresh_site_index`].
fn site_refs_from_query(results: &entity_sdk::QueryResults, me: &str) -> Vec<SiteRef> {
    let mut out: Vec<SiteRef> = results
        .matches
        .iter()
        .filter_map(|m| paths::parse_manifest_path(&m.path))
        .map(|(peer, site)| SiteRef {
            peer: peer.to_string(),
            site: site.to_string(),
            owned: peer == me,
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

// ---- Derived site index (async query → tree → sync read) -----------------
//
// `list_all_sites` is async (L1 query); surfaces that render synchronously (the
// Settings boot-target picker) can't await it. Per the entity-backed pattern,
// an async refresh materializes the query result into a small tree entity at
// `app_paths::site_index_path`; the sync surface reads that entity, and a
// normal subscription on the index path re-renders it when a refresh lands.
// The query stays the source of truth — this is just its subscribable cache.

/// Entity type for the derived site index (app-tier state).
const SITE_INDEX_TYPE: &str = "app/state/site-index";

/// Encode a site list as the index entity: `{ sites: [ {peer, site}, … ] }`.
/// `owned` is NOT stored — it's derived on read from the peer-segment vs the
/// reader's own id (the path already carries the partition).
pub fn site_index_entity(sites: &[SiteRef]) -> entity_entity::Entity {
    let arr: Vec<entity_ecf::Value> = sites
        .iter()
        .map(|s| {
            entity_ecf::Value::Map(vec![
                (entity_ecf::Value::Text("peer".into()), entity_ecf::text(&s.peer)),
                (entity_ecf::Value::Text("site".into()), entity_ecf::text(&s.site)),
            ])
        })
        .collect();
    let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![(
        entity_ecf::Value::Text("sites".into()),
        entity_ecf::Value::Array(arr),
    )]));
    entity_entity::Entity::new(SITE_INDEX_TYPE, data).expect("site index encodes")
}

/// Read the derived site index synchronously (L0). Returns owned + cached
/// [`SiteRef`]s (`owned` derived against `me`), sorted + deduped. Empty until a
/// refresh has populated it — the index-path subscription re-renders the reader
/// once it lands.
pub fn read_site_index(peers: &Peers, me: &str) -> Vec<SiteRef> {
    let path = crate::app_paths::site_index_path(crate::app_paths::APP_ID, me);
    let Some(entity) = peers.get_entity(me, &path) else {
        return Vec::new();
    };
    let value: entity_ecf::Value = match ciborium::from_reader(entity.data.as_slice()) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let sites_val = value
        .as_map()
        .and_then(|m| m.iter().find(|(k, _)| k.as_text() == Some("sites")).map(|(_, v)| v));
    let Some(arr) = sites_val.and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out: Vec<SiteRef> = arr
        .iter()
        .filter_map(|item| {
            let m = item.as_map()?;
            let field = |name: &str| {
                m.iter()
                    .find(|(k, _)| k.as_text() == Some(name))
                    .and_then(|(_, v)| v.as_text())
                    .map(str::to_string)
            };
            let peer = field("peer")?;
            let site = field("site")?;
            let owned = peer == me;
            Some(SiteRef { peer, site, owned })
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Every site selectable as a **boot target** — the union of the derived index
/// ([`read_site_index`], the async manifest-type query materialized for sync
/// reads) and the direct store scan ([`scan_local_sites`], owned + cached-foreign
/// by prefix over the origins roster). Unioning both sources mirrors the content
/// directory rail (BUG-3): the boot-target picker must never omit a cached site
/// that the rail / browse area can show — even before the async index refresh
/// lands, or when the type-query misses a freshly-cached manifest. A cached
/// foreign site reached only by browsing (resolver write-through, no
/// `sites.list` enumeration) is in my store at its natural `/{foreign}/sites/`
/// path, so the scan surfaces it whether or not the index query caught it.
/// Sorted, deduped (`(peer, site)` identity; `owned` derives identically in both
/// sources, so no split entries).
pub fn list_targetable_sites(peers: &Peers, me: &str) -> Vec<SiteRef> {
    let mut set: std::collections::BTreeSet<SiteRef> = std::collections::BTreeSet::new();
    set.extend(read_site_index(peers, me));
    set.extend(scan_local_sites(peers, me));
    set.into_iter().collect()
}

/// Kick a fire-and-forget refresh of the site index: run the manifest
/// type-query, write its result to [`crate::app_paths::site_index_path`]. A
/// subscription on that path re-renders any reader once it populates. Grabs a
/// `'static` query future + writer handle up front so nothing borrows `&Peers`
/// across the await (WASM `spawn_local`; native `tokio::spawn` for tests).
#[cfg(target_arch = "wasm32")]
pub fn refresh_site_index(peers: &Peers, me: &str) {
    let query_fut = peers.query(me, manifest_type_query());
    // PER-PEER write: the index path is `/{me}/app/.../site-index`, so it MUST
    // land in `me`'s own store — `writer_handle_for(me)`, not the primary-bound
    // `writer_handle()`. On the Worker arm a non-primary (backend) peer has its
    // own store; the old primary-bound writer wrote the index there while
    // `read_site_index(me)` read `me`'s store → empty rail ("No sites yet",
    // since fixed). Native shares one store, which is why no native test
    // caught it — the e2e regression pin runs on the Worker arm.
    let Some(writer) = peers.writer_handle_for(me) else {
        tracing::warn!(me = %me, "refresh_site_index: no writer handle for peer — site index NOT refreshed");
        return;
    };
    let me = me.to_string();
    wasm_bindgen_futures::spawn_local(async move {
        match query_fut.await {
            Ok(results) => {
                let refs = site_refs_from_query(&results, &me);
                let path = crate::app_paths::site_index_path(crate::app_paths::APP_ID, &me);
                tracing::debug!(
                    me = %me, sites = refs.len(), matches = results.matches.len(),
                    "refresh_site_index: writing site index"
                );
                writer.put(path, site_index_entity(&refs));
            }
            Err(e) => tracing::warn!(
                me = %me, error = %e,
                "refresh_site_index: site-manifest query FAILED — index stays empty ('No sites yet')"
            ),
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
pub fn refresh_site_index(peers: &Peers, me: &str) {
    let query_fut = peers.query(me, manifest_type_query());
    // Per-peer write — see the wasm arm for why this is `writer_handle_for(me)`
    // and not the primary-bound `writer_handle()`.
    let Some(writer) = peers.writer_handle_for(me) else {
        return;
    };
    let me = me.to_string();
    let task = async move {
        if let Ok(results) = query_fut.await {
            let path = crate::app_paths::site_index_path(crate::app_paths::APP_ID, &me);
            writer.put(path, site_index_entity(&site_refs_from_query(&results, &me)));
        }
    };
    // The frontend runs natively only under test (the native binary is a stub),
    // so spawn only when a Tokio runtime is present — a plain `#[test]` that
    // hits a refresh trigger (e.g. set_boot_kind→site) then no-ops instead of
    // panicking with "no reactor running".
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(task);
        }
        Err(_) => drop(task),
    }
}

/// A `system/query/expression` filtered to the site-manifest type with no path
/// prefix → the whole universal tree. Mirrors `query_console`'s
/// `build_expression_from_fields`; kept inline so `discovery` doesn't depend on
/// a view module.
fn manifest_type_query() -> entity_entity::Entity {
    let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![(
        entity_ecf::Value::Text("type_filter".into()),
        entity_ecf::text(crate::content_site::format::SITE_MANIFEST_TYPE),
    )]));
    entity_entity::Entity::new("system/query/expression", data)
        .expect("static manifest-type query expression encodes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_site::format::SiteManifest;
    use crate::views::content_site::{ensure_demo_site, DEMO_SITE_ID};

    /// Validate lazy one-level `.list` discovery against the deep demo
    /// site (the design's §12 validation): list a level, body-free, and
    /// confirm a deeper level is NOT materialized by listing the level
    /// above it.
    #[test]
    fn lazy_discovery_lists_one_level_at_a_time() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        ensure_demo_site(&peers, &pid);

        // Top level of the site's pages — Home/About/Theory + the Guide section.
        let top = list_child_pages(&peers, &pid, DEMO_SITE_ID, "");
        let names: Vec<&str> = top.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"index"), "top children: {names:?}");
        assert!(names.contains(&"about"));
        assert!(names.contains(&"theory"));
        assert!(names.contains(&"guide"));

        // `guide` is a SECTION (has descendants), not a direct page.
        let guide = top.iter().find(|c| c.name == "guide").expect("guide present");
        assert!(guide.is_section, "guide should be a section");
        assert!(!guide.is_page, "no page entity at pages/guide exactly");
        // `index` is a leaf page.
        let index = top.iter().find(|c| c.name == "index").expect("index present");
        assert!(index.is_page);

        // The top-level listing must NOT have pulled in the section's
        // children — that's the lazy property (no whole-index download).
        assert!(
            !names.contains(&"intro") && !names.contains(&"install"),
            "top-level listing leaked section children: {names:?}"
        );

        // Descend ONE level into the Guide section — only now.
        let guide_children = list_child_pages(&peers, &pid, DEMO_SITE_ID, "guide/");
        let gnames: Vec<&str> = guide_children.iter().map(|c| c.name.as_str()).collect();
        assert!(gnames.contains(&"intro"), "guide children: {gnames:?}");
        assert!(gnames.contains(&"install"));
        assert!(gnames.contains(&"advanced"));

        // `advanced` is itself a section; we know it exists WITHOUT having
        // listed its child (`internals`) — depth-bounded discovery.
        let advanced = guide_children.iter().find(|c| c.name == "advanced").expect("advanced present");
        assert!(advanced.is_section, "advanced should be a section");
        assert!(
            !gnames.contains(&"internals"),
            "2nd-level listing leaked the 3rd level: {gnames:?}"
        );
    }

    #[test]
    fn missing_peer_or_site_yields_empty_not_panic() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        // No demo seeded; an absent site lists empty (no panic).
        assert!(list_child_pages(&peers, &pid, "ghost-site", "").is_empty());
    }

    #[test]
    fn children_from_slugs_matches_the_tree_scan() {
        // The remote reducer must produce the SAME one-level children as the
        // local tree scan for the same site — so a remote sidebar looks
        // identical to a local one. Drive both off the demo site.
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        ensure_demo_site(&peers, &pid);

        // The flat slug set is what a static `pages.list` carries.
        let slugs: Vec<String> = read_demo_slugs(&peers, &pid);

        for under in ["", "guide/", "guide/advanced/"] {
            let mut local = list_child_pages(&peers, &pid, DEMO_SITE_ID, under);
            let mut remote = children_from_slugs(&slugs, under);
            local.sort_by(|a, b| a.name.cmp(&b.name));
            remote.sort_by(|a, b| a.name.cmp(&b.name));
            assert_eq!(local, remote, "mismatch under {under:?}");
        }

        // Spot-check the shape: top level has the Guide section + leaf pages.
        let top = children_from_slugs(&slugs, "");
        let guide = top.iter().find(|c| c.name == "guide").expect("guide");
        assert!(guide.is_section && !guide.is_page);
        assert!(top.iter().any(|c| c.name == "index" && c.is_page));
    }

    /// Read every page slug of the demo site (the data a static `pages.list`
    /// would carry) via the recursive reader.
    fn read_demo_slugs(peers: &Peers, pid: &str) -> Vec<String> {
        crate::content_site::read::read_site(peers, pid, DEMO_SITE_ID)
            .expect("demo reads")
            .pages
            .into_iter()
            .map(|(slug, _)| slug)
            .collect()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_all_sites_finds_owned_and_cached_by_type_query() {
        // The universal-tree approach: ONE query by manifest type returns sites
        // across every peer-segment in my store; the path says owned vs cached.
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();

        // Owned site: /{me}/sites/{DEMO}/manifest (+ pages).
        ensure_demo_site(&peers, &me);

        // Cached foreign site: a foreign manifest written at its NATURAL path in
        // MY store (what the P1 write-through does), under a real foreign id.
        let foreign = Peers::new_direct().primary_peer_id().to_string();
        peers.seed_write(
            &me,
            paths::manifest_path(&foreign, "labs"),
            SiteManifest::new("labs", "Labs", "index", vec![]).to_entity(),
        );

        let sites = list_all_sites(&peers, &me).await;

        // The owned demo site, tagged owned.
        assert!(
            sites.iter().any(|s| s.peer == me && s.site == DEMO_SITE_ID && s.owned),
            "owned demo site missing/mistagged: {sites:?}"
        );
        // The cached foreign site, tagged cached — selectable now (the gap the
        // single-peer, context-gated picker couldn't surface).
        assert!(
            sites.iter().any(|s| s.peer == foreign && s.site == "labs" && !s.owned),
            "cached foreign site missing/mistagged: {sites:?}"
        );
    }

    #[test]
    fn site_index_round_trips_and_derives_owned_on_read() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let foreign = Peers::new_direct().primary_peer_id().to_string();
        let refs = vec![
            SiteRef { peer: me.clone(), site: "mine".into(), owned: true },
            SiteRef { peer: foreign.clone(), site: "theirs".into(), owned: false },
        ];
        peers.seed_write(
            &me,
            crate::app_paths::site_index_path(crate::app_paths::APP_ID, &me),
            site_index_entity(&refs),
        );
        let got = read_site_index(&peers, &me);
        assert_eq!(got.len(), 2, "both entries read back: {got:?}");
        assert!(got.iter().any(|s| s.peer == me && s.site == "mine" && s.owned));
        // `owned` is derived on read from peer vs me — not stored.
        assert!(got.iter().any(|s| s.peer == foreign && s.site == "theirs" && !s.owned));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn refresh_site_index_materializes_owned_and_cached_for_sync_readers() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        ensure_demo_site(&peers, &me);
        let foreign = Peers::new_direct().primary_peer_id().to_string();
        peers.seed_write(
            &me,
            paths::manifest_path(&foreign, "labs"),
            SiteManifest::new("labs", "Labs", "index", vec![]).to_entity(),
        );

        // No refresh yet → the sync read is empty (the index-path subscription
        // re-renders the reader once a refresh lands).
        assert!(read_site_index(&peers, &me).is_empty());

        refresh_site_index(&peers, &me);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let idx = read_site_index(&peers, &me);
        assert!(
            idx.iter().any(|s| s.peer == me && s.site == DEMO_SITE_ID && s.owned),
            "owned demo missing from refreshed index: {idx:?}"
        );
        assert!(
            idx.iter().any(|s| s.peer == foreign && s.site == "labs" && !s.owned),
            "cached foreign missing from refreshed index: {idx:?}"
        );
    }

    #[test]
    fn scan_local_sites_finds_owned_and_cached_without_a_refresh() {
        // BUG-3: the rail must surface a physically-present site WITHOUT waiting
        // on the async index refresh — the divergence that showed "No sites yet"
        // while the browse area rendered a cached site. The direct scan is the
        // rail's other source, unioned with the (here-empty) index.
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        ensure_demo_site(&peers, &me);
        let foreign = Peers::new_direct().primary_peer_id().to_string();
        // Cached foreign manifest in MY store + its registered origin (the
        // write-through always records one; the scan keys off the origins
        // roster to find cached peers).
        peers.seed_write(
            &me,
            paths::manifest_path(&foreign, "labs"),
            SiteManifest::new("labs", "Labs", "index", vec![]).to_entity(),
        );
        crate::content_site::origins::set_origin(&peers, &me, &foreign, "http://labs.example");

        // The async index has NOT been refreshed → the derived read is empty.
        assert!(read_site_index(&peers, &me).is_empty(), "index empty without a refresh");

        // ...but the direct scan finds BOTH — so the rail never contradicts the
        // browse area.
        let scanned = scan_local_sites(&peers, &me);
        assert!(
            scanned.iter().any(|s| s.peer == me && s.site == DEMO_SITE_ID && s.owned),
            "owned demo missing from scan: {scanned:?}"
        );
        assert!(
            scanned.iter().any(|s| s.peer == foreign && s.site == "labs" && !s.owned),
            "cached foreign missing from scan: {scanned:?}"
        );
    }

    #[test]
    fn list_targetable_sites_includes_cached_via_scan_without_a_refresh() {
        // The Settings boot-target picker must offer CACHED foreign sites, not
        // just owned ones — and must do so even before the async index refresh
        // lands (the divergence that made a cached site browsable in the rail
        // but un-selectable as a boot target). `list_targetable_sites` unions
        // the (here-empty) index with the direct scan.
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        ensure_demo_site(&peers, &me);
        let foreign = Peers::new_direct().primary_peer_id().to_string();
        peers.seed_write(
            &me,
            paths::manifest_path(&foreign, "labs"),
            SiteManifest::new("labs", "Labs", "index", vec![]).to_entity(),
        );
        crate::content_site::origins::set_origin(&peers, &me, &foreign, "http://labs.example");

        // No index refresh → the query-derived index is empty.
        assert!(read_site_index(&peers, &me).is_empty());

        // ...but the targetable set surfaces BOTH the owned demo and the cached
        // foreign site, so both are selectable boot targets.
        let targetable = list_targetable_sites(&peers, &me);
        assert!(
            targetable.iter().any(|s| s.peer == me && s.site == DEMO_SITE_ID && s.owned),
            "owned demo missing from targetable: {targetable:?}"
        );
        assert!(
            targetable.iter().any(|s| s.peer == foreign && s.site == "labs" && !s.owned),
            "cached foreign site missing from targetable: {targetable:?}"
        );
    }

    #[test]
    fn parse_manifest_path_round_trips_and_rejects_non_manifests() {
        // The inverse the type-query enumeration relies on.
        let p = paths::manifest_path("PEER", "blog");
        assert_eq!(paths::parse_manifest_path(&p), Some(("PEER", "blog")));
        // Not a manifest path → None (so page/asset matches never masquerade).
        assert_eq!(paths::parse_manifest_path(&paths::page_path("PEER", "blog", "x")), None);
        assert_eq!(paths::parse_manifest_path("/PEER/sites/blog"), None);
        assert_eq!(paths::parse_manifest_path("/PEER/sites/blog/manifest/extra"), None);
    }

    #[test]
    fn list_sites_returns_site_ids_one_level() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        // Empty before any site is seeded.
        assert!(list_sites(&peers, &pid).is_empty());
        ensure_demo_site(&peers, &pid);
        let sites = list_sites(&peers, &pid);
        assert!(sites.contains(&DEMO_SITE_ID.to_string()), "sites: {sites:?}");
        // No page-level segments leak in — only the site id.
        assert!(!sites.iter().any(|s| s.contains('/')), "site ids only: {sites:?}");
    }
}

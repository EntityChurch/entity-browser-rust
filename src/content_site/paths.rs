//! Site tree-path helpers + the legacy-web URL projection.
//!
//! ## Tree placement (v0.5 — a free subgraph, NOT under `content`)
//!
//! A site is a **free subgraph** at a publisher-chosen tree path
//! (`APP-CONVENTION-SEMANTIC-CONTENT-SITE` v0.5 §2). This impl's publisher
//! convention places a site at a bare `sites/` subpath under the peer:
//!
//! ```text
//! /{peer_id}/sites/{site_id}/manifest
//! /{peer_id}/sites/{site_id}/pages/{page}
//! /{peer_id}/sites/{site_id}/assets/{name}
//! ```
//!
//! **Why not `content/sites/…` (the dropped v0.4.2 placement):**
//! `system/content/*` is the CONTENT extension's namespace for
//! capability-scoping the content-hash address space — its leaf is always
//! `{hex(H)}` (EXTENSION-CONTENT §6.4.2). An L5 application subgraph has no
//! business there; v0.5 corrects that layer violation. We also keep sites
//! **out** of the `app/entity-browser/…` namespace: a site is publishable
//! content readable by any impl, not frontend state tied to our app-id.
//!
//! The site's capability scope is its own subgraph root (`site_prefix`).
//! v1 placement is **deterministic** from `(peer_id, site_id)`; the fully
//! general "manifest names an arbitrary root path" case (e.g. importing a
//! foreign site published elsewhere) is a named follow-up, not built ahead
//! of the use case.
//!
//! ## URL projection (v0.5 §11 — the legacy-web surface)
//!
//! For permalinking, no-JS readers, and the SSG use case, a site projects
//! onto the legacy web at the **`sites` reserved first-segment literal**
//! (NETWORK §6.5.6 Amendment 9) — prefix-**FIRST**, peer-id **second**:
//!
//! ```text
//! {base}/sites/{peer_id}/{site_id}[/{page}]
//! ```
//!
//! This is a **publish-time projection, not a tree-storage rule**, and is
//! distinct from the live cross-peer HTTP-poll URL — which mirrors the raw
//! tree path (peer-first, served by the existing TREE_GET demux). See
//! [`site_url`] / [`parse_site_url`] for the projection and
//! [`super::http_poll`] for the live-poll mirror.

#![allow(dead_code)] // some helpers (assets, projection parse) are P1 consumers

/// Subpath under a peer where this publisher places its sites. A bare
/// `sites` — NOT `content/sites` (that squats the CONTENT-extension
/// namespace, v0.5 §2) and NOT `app/entity-browser/…` (sites are
/// publishable content, not app-id-tied state).
const SITES_SUBPATH: &str = "sites";

/// The SITE convention's reserved first-segment URL literal (NETWORK
/// §6.5.6 Amendment 9 / SITE v0.5 §11). Five chars, comfortably below the
/// Ed25519 peer-id minimum length, so the literal-then-parse-by-string
/// demux never mistakes it for a peer-id.
///
/// It happens to equal [`SITES_SUBPATH`] today, but the two are **distinct
/// concerns**: this is the spec-mandated web reserved word; that is our
/// publisher's tree-placement choice. Kept separate so changing one does
/// not silently move the other.
pub const SITE_URL_PREFIX: &str = "sites";

/// Prefix for **all** sites under a peer (trailing slash). Subscribing here
/// covers any configured `home_site` without the surface having to know
/// which site that is at construction time — which sidesteps the Worker-arm
/// cache-timing trap (the cache mirror only feeds *subscribed* prefixes, and
/// the config that names the site may not be cached yet when a window/overlay
/// is built — `feedback_worker_cache_get_needs_subscription`). Also covers a
/// future multi-site config / runtime site-switch for free.
pub fn sites_prefix(peer_id: &str) -> String {
    format!("/{}/{}/", peer_id, SITES_SUBPATH)
}

/// Prefix for everything in one site (trailing slash). This subgraph root
/// **is** the site's capability scope (v0.5 §7).
pub fn site_prefix(peer_id: &str, site_id: &str) -> String {
    format!("/{}/{}/{}/", peer_id, SITES_SUBPATH, site_id)
}

/// Path to a site's manifest entity.
pub fn manifest_path(peer_id: &str, site_id: &str) -> String {
    format!("/{}/{}/{}/manifest", peer_id, SITES_SUBPATH, site_id)
}

/// Inverse of [`manifest_path`]: parse `(peer_id, site_id)` out of a path iff it
/// is exactly a site manifest `/{peer}/sites/{site}/manifest`. The universal
/// tree carries the partition, so the **type-query** enumeration
/// ([`super::discovery::list_all_sites`]) reads the owning peer + site straight
/// off a matched manifest's path — it never filters by peer. Co-located with
/// `manifest_path` so the format and its inverse can't drift.
pub fn parse_manifest_path(path: &str) -> Option<(&str, &str)> {
    let mut segs = path.trim_start_matches('/').split('/');
    let peer = segs.next().filter(|s| !s.is_empty())?;
    if segs.next()? != SITES_SUBPATH {
        return None;
    }
    let site = segs.next().filter(|s| !s.is_empty())?;
    if segs.next()? != "manifest" || segs.next().is_some() {
        return None; // must end exactly at `.../manifest`
    }
    Some((peer, site))
}

/// Prefix for a site's pages (trailing slash).
pub fn pages_prefix(peer_id: &str, site_id: &str) -> String {
    format!("/{}/{}/{}/pages/", peer_id, SITES_SUBPATH, site_id)
}

/// Path to a single page entity within a site.
pub fn page_path(peer_id: &str, site_id: &str, page: &str) -> String {
    format!("/{}/{}/{}/pages/{}", peer_id, SITES_SUBPATH, site_id, page)
}

/// Prefix for a site's assets (trailing slash).
pub fn assets_prefix(peer_id: &str, site_id: &str) -> String {
    format!("/{}/{}/{}/assets/", peer_id, SITES_SUBPATH, site_id)
}

/// Path to a single asset entity within a site.
pub fn asset_path(peer_id: &str, site_id: &str, name: &str) -> String {
    format!("/{}/{}/{}/assets/{}", peer_id, SITES_SUBPATH, site_id, name)
}

/// Map an embed `ref` to the site-local asset **name** (the suffix after the
/// `assets/` prefix), or `None` if the ref is not a resolvable site-local
/// asset. This is the **security gate** for image resolution: only refs that
/// name a file inside the site's own `assets/` subgraph resolve — an external
/// URL (`https://…`, `//…`, `data:`), an absolute path, a parent escape
/// (`..`), or a non-`assets/` ref is rejected, so a hostile page body cannot
/// make the renderer fetch an arbitrary/tracking URL. `assets/figures/x.png`
/// → `Some("figures/x.png")`.
pub fn asset_name_from_ref(reference: &str) -> Option<String> {
    let r = reference.trim();
    if r.is_empty()
        || r.contains("://")
        || r.starts_with("//")
        || r.starts_with('/')
        || r.starts_with("data:")
        || r.split('/').any(|seg| seg == "..")
    {
        return None;
    }
    let name = r.strip_prefix("assets/")?;
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Recover a page slug from a full pages-prefixed path, if it belongs
/// to the given site.
pub fn page_from_path(peer_id: &str, site_id: &str, full_path: &str) -> Option<String> {
    full_path
        .strip_prefix(&pages_prefix(peer_id, site_id))
        .map(|s| s.to_string())
}

// ── Legacy-web URL projection (v0.5 §11) ────────────────────────────────
// `{base}/sites/{peer_id}/{site_id}[/{page}]` — prefix-FIRST. A publish-time
// projection for the no-JS / permalink / SSG surface; NOT a tree path.

/// Project `(peer_id, site_id, page)` onto the legacy-web URL path under a
/// base, prefix-first: `{base}/sites/{peer_id}/{site_id}[/{page}]`. An empty
/// `page` addresses the site root. `base` may carry a trailing slash (it is
/// trimmed) or be empty (a root-relative path).
pub fn site_url(base: &str, peer_id: &str, site_id: &str, page: &str) -> String {
    let base = base.trim_end_matches('/');
    let tail = if page.is_empty() {
        format!("{SITE_URL_PREFIX}/{peer_id}/{site_id}")
    } else {
        format!("{SITE_URL_PREFIX}/{peer_id}/{site_id}/{page}")
    };
    if base.is_empty() {
        format!("/{tail}")
    } else {
        format!("{base}/{tail}")
    }
}

/// Build a **static permalink** to the projected `.html` file a dumb server
/// serves: `{base}/sites/{peer_id}/{site_id}/{page}.html` (an empty `page` →
/// the site root `{base}/sites/{peer_id}/{site_id}/`). This is the *dumb-server
/// file* form the static export emits — distinct from [`site_url`], which is
/// the spec's clean (extension-less) projection URL. Use this for a "copy the
/// static permalink" affordance; it resolves wherever THIS peer's site is
/// published as static HTML (not the live SPA). `base` may be empty
/// (root-relative) or carry a trailing slash (trimmed).
pub fn static_permalink(base: &str, peer_id: &str, site_id: &str, page: &str) -> String {
    let base = base.trim_end_matches('/');
    let tail = if page.is_empty() {
        format!("{SITE_URL_PREFIX}/{peer_id}/{site_id}/")
    } else {
        format!("{SITE_URL_PREFIX}/{peer_id}/{site_id}/{page}.html")
    };
    if base.is_empty() {
        format!("/{tail}")
    } else {
        format!("{base}/{tail}")
    }
}

/// Parse a legacy-web projection **path** (no scheme/host — the base must
/// already be stripped) back into `(peer_id, site_id, page)`. The first
/// segment MUST be the reserved `sites` literal; a leading slash is
/// tolerated. `page` is the remaining segments rejoined (empty = site root).
/// Returns `None` if it is not a `sites/…` projection or lacks a peer+site.
pub fn parse_site_url(path: &str) -> Option<(String, String, String)> {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segs.first() != Some(&SITE_URL_PREFIX) || segs.len() < 3 {
        return None;
    }
    let peer_id = segs[1].to_string();
    let site_id = segs[2].to_string();
    let page = segs.get(3..).map(|s| s.join("/")).unwrap_or_default();
    Some((peer_id, site_id, page))
}

// ── Live deep link (the static↔live round-trip) ─────────────────────────
// `{base}/?site={peer_id}/{site_id}[/{page}]` — the query-param form a
// static page links to so a reader can jump from the no-JS snapshot INTO
// the live SPA, pre-navigated to the same page. The live app reads this
// param at boot (the [F3] consumer) and drives the content-site overlay to
// `(peer, site, page)`. Pairs with the static permalink [`site_url`]: the
// permalink is the archive-safe file; the deep link is the live on-ramp.

/// The boot query param the live SPA reads to deep-link into a site.
pub const SITE_QUERY_PARAM: &str = "site";

/// Sentinel peer segment meaning "the serving/live peer" — resolved at boot
/// to the system peer. A static export served **same-origin** with the SPA
/// (the `dist/` topology: WASM app at `/`, static tree in a subdir) doesn't
/// know the live peer's id at publish time, and that id differs from the
/// ephemeral publish peer anyway — so the banner deep-links to `self`, and
/// the live app opens the same site on whatever peer it booted. A cross-peer
/// *archive* deep link (a real, stable hosting peer-id, resolved via origins)
/// is the follow-up that pairs with stable hosting identity.
pub const SELF_PEER: &str = "self";

/// Build the live deep link **to the serving peer** (`self`): the same link
/// the static banner emits and the in-app "Share" control copies. Both halves
/// of the round-trip resolve `self` → the booting system peer, so the link is
/// **same-origin robust** regardless of which peer-id published the snapshot
/// (the publish peer is ephemeral and differs from the live peer; see
/// [`SELF_PEER`]). A cross-peer *archive* link with a real, stable peer-id is
/// the deferred hosting-identity follow-up. Kept here as the one place the
/// `self`-sharing decision lives, shared by the banner and the Share button.
pub fn self_deep_link(base: &str, site_id: &str, page: &str) -> String {
    site_deep_link(base, SELF_PEER, site_id, page)
}

/// Build the live deep link: `{base}/?site={peer_id}/{site_id}[/{page}]`.
/// `base` is the live origin (may carry a trailing slash; it is trimmed);
/// an empty `base` yields a root-relative `/?site=…`. An empty `page`
/// addresses the site root.
pub fn site_deep_link(base: &str, peer_id: &str, site_id: &str, page: &str) -> String {
    let base = base.trim_end_matches('/');
    let value = if page.is_empty() {
        format!("{peer_id}/{site_id}")
    } else {
        format!("{peer_id}/{site_id}/{page}")
    };
    format!("{base}/?{SITE_QUERY_PARAM}={value}")
}

/// Parse the **value** of the `site` query param (just the `{peer}/{site}
/// [/{page}]` part, already URL-decoded and with the `site=` stripped) into
/// `(peer_id, site_id, page)`. Returns `None` if it lacks a peer + site.
/// The [F3] boot consumer calls this after pulling the param off the URL.
pub fn parse_site_query(value: &str) -> Option<(String, String, String)> {
    let segs: Vec<&str> = value.split('/').filter(|s| !s.is_empty()).collect();
    if segs.len() < 2 {
        return None;
    }
    let peer_id = segs[0].to_string();
    let site_id = segs[1].to_string();
    let page = segs.get(2..).map(|s| s.join("/")).unwrap_or_default();
    Some((peer_id, site_id, page))
}

// ── Published-tree prefix (the per-peer hosting scope) ───────────────────
// A published peer is one isolated unit under a configurable `{PREFIX}` —
// `{domain}/{PREFIX}/…` — where its tree, content blobs, and `sites/`
// projection all live. The
// prefix is an **opaque host-base**, not a protocol reserved word: the
// reserved words (`sites`/`content`) appear *after* it, so the spec model is
// unchanged (host = `{domain}/{PREFIX}`). **Empty prefix = the domain root =
// today's layout, byte-identical** — the knob is purely additive. These
// helpers are publish-side only (native).

/// Validate + normalize a published-tree prefix at the CLI boundary. Empty →
/// `Ok("")` (root). A non-empty prefix must be a clean relative path: no
/// leading/trailing `/`, no empty/`.`/`..` segments, no whitespace/backslash,
/// and a first segment that is **not** a reserved word (`sites`/`content`) —
/// which would shadow the projection's own namespaces under the prefix. The
/// hosting layer additionally enforces cross-peer uniqueness + non-nesting
/// (a static-layout concern, not checkable from one publish run).
#[cfg(not(target_arch = "wasm32"))]
pub fn normalize_prefix(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Ok(String::new());
    }
    if raw.starts_with('/') || raw.ends_with('/') {
        return Err(format!("prefix {raw:?} must not start or end with '/'"));
    }
    let segs: Vec<&str> = raw.split('/').collect();
    for s in &segs {
        if s.is_empty() {
            return Err(format!("prefix {raw:?} has an empty path segment ('//')"));
        }
        if *s == "." || *s == ".." {
            return Err(format!("prefix {raw:?} must not contain '.' or '..' segments"));
        }
        if s.contains(|c: char| c.is_whitespace()) || s.contains('\\') {
            return Err(format!("prefix {raw:?} segment {s:?} has whitespace or a backslash"));
        }
    }
    if matches!(segs[0], SITE_URL_PREFIX | "content") {
        return Err(format!(
            "prefix {raw:?} first segment {:?} is a reserved word (sites/content) — \
             it would shadow the projection's own namespaces under the prefix",
            segs[0]
        ));
    }
    Ok(raw.to_string())
}

/// Join a validated prefix onto an output root: the root itself when empty
/// (so the un-prefixed layout is reproduced byte-for-byte), else
/// `out_dir/{prefix}`.
#[cfg(not(target_arch = "wasm32"))]
pub fn prefixed_root(out_dir: &std::path::Path, prefix: &str) -> std::path::PathBuf {
    if prefix.is_empty() {
        out_dir.to_path_buf()
    } else {
        out_dir.join(prefix)
    }
}

/// The absolute-href base for a validated prefix: `""` when empty (so a
/// `/sites/…` href is unchanged), else `/{prefix}` (so it becomes
/// `/{prefix}/sites/…`).
#[cfg(not(target_arch = "wasm32"))]
pub fn href_prefix(prefix: &str) -> String {
    if prefix.is_empty() {
        String::new()
    } else {
        format!("/{prefix}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_path_format() {
        assert_eq!(manifest_path("PEER1", "church"), "/PEER1/sites/church/manifest");
    }

    #[test]
    fn page_path_format() {
        assert_eq!(page_path("PEER1", "church", "about"), "/PEER1/sites/church/pages/about");
    }

    #[test]
    fn pages_prefix_format() {
        assert_eq!(pages_prefix("PEER1", "church"), "/PEER1/sites/church/pages/");
    }

    #[test]
    fn placement_is_not_under_content() {
        // v0.5: the layer violation is gone — no site path touches the
        // CONTENT-extension namespace.
        assert!(!manifest_path("P", "s").contains("/content/"));
        assert!(!site_prefix("P", "s").contains("/content/"));
    }

    #[test]
    fn asset_name_from_ref_accepts_site_local_and_rejects_external() {
        // Site-local refs resolve to their name under assets/.
        assert_eq!(asset_name_from_ref("assets/figures/x.png"), Some("figures/x.png".into()));
        assert_eq!(asset_name_from_ref("assets/demo.svg"), Some("demo.svg".into()));
        // Everything that could fetch off-site or escape the subgraph is rejected.
        assert_eq!(asset_name_from_ref("https://evil.test/x.png"), None);
        assert_eq!(asset_name_from_ref("//evil.test/x.png"), None);
        assert_eq!(asset_name_from_ref("/etc/passwd"), None);
        assert_eq!(asset_name_from_ref("data:image/png;base64,AAAA"), None);
        assert_eq!(asset_name_from_ref("assets/../../secret"), None);
        assert_eq!(asset_name_from_ref("figures/x.png"), None, "must be under assets/");
        assert_eq!(asset_name_from_ref("assets/"), None);
        assert_eq!(asset_name_from_ref(""), None);
    }

    #[test]
    fn page_from_path_round_trips() {
        let full = page_path("PEER1", "church", "docs/intro");
        assert_eq!(page_from_path("PEER1", "church", &full), Some("docs/intro".into()));
    }

    #[test]
    fn page_from_path_rejects_other_site() {
        let full = page_path("PEER1", "other", "x");
        assert_eq!(page_from_path("PEER1", "church", &full), None);
    }

    #[test]
    fn site_url_projects_prefix_first() {
        assert_eq!(site_url("https://ex.test", "PEER1", "church", ""), "https://ex.test/sites/PEER1/church");
        assert_eq!(
            site_url("https://ex.test/", "PEER1", "church", "about"),
            "https://ex.test/sites/PEER1/church/about"
        );
        // Empty base → root-relative permalink.
        assert_eq!(site_url("", "PEER1", "church", "guide/intro"), "/sites/PEER1/church/guide/intro");
    }

    #[test]
    fn parse_site_url_round_trips() {
        let path = site_url("", "PEER1", "church", "guide/intro");
        assert_eq!(
            parse_site_url(&path),
            Some(("PEER1".into(), "church".into(), "guide/intro".into()))
        );
        // Root (no page).
        assert_eq!(
            parse_site_url("/sites/PEER1/church"),
            Some(("PEER1".into(), "church".into(), String::new()))
        );
    }

    #[test]
    fn site_deep_link_round_trips() {
        let link = site_deep_link("https://ex.test", "PEER1", "church", "guide/intro");
        assert_eq!(link, "https://ex.test/?site=PEER1/church/guide/intro");
        // The consumer strips the `site=` and parses the value.
        let value = link.split("?site=").nth(1).unwrap();
        assert_eq!(
            parse_site_query(value),
            Some(("PEER1".into(), "church".into(), "guide/intro".into()))
        );
        // Root (no page) + empty base (root-relative).
        assert_eq!(site_deep_link("", "PEER1", "church", ""), "/?site=PEER1/church");
        assert_eq!(
            parse_site_query("PEER1/church"),
            Some(("PEER1".into(), "church".into(), String::new()))
        );
    }

    #[test]
    fn static_permalink_emits_html_file_form() {
        // A page → the `.html` file a dumb server serves.
        assert_eq!(
            static_permalink("https://ex.test", "PEER1", "church", "about"),
            "https://ex.test/sites/PEER1/church/about.html"
        );
        // Nested page stays depth-safe.
        assert_eq!(
            static_permalink("", "PEER1", "church", "guide/intro"),
            "/sites/PEER1/church/guide/intro.html"
        );
        // Empty page → the site root directory (served by its index.html).
        assert_eq!(
            static_permalink("https://ex.test/", "PEER1", "church", ""),
            "https://ex.test/sites/PEER1/church/"
        );
        // Distinct from the clean projection URL (no `.html`).
        assert_ne!(
            static_permalink("", "P", "s", "p"),
            site_url("", "P", "s", "p")
        );
    }

    #[test]
    fn self_deep_link_emits_the_self_sentinel() {
        // The shared `self`-sharing helper — the banner and the in-app Share
        // control must produce the identical same-origin link for a page.
        assert_eq!(
            self_deep_link("https://ex.test", "demo", "guide/intro"),
            "https://ex.test/?site=self/demo/guide/intro"
        );
        // It is exactly `site_deep_link` with the `self` peer.
        assert_eq!(
            self_deep_link("https://ex.test", "demo", "guide/intro"),
            site_deep_link("https://ex.test", SELF_PEER, "demo", "guide/intro")
        );
        // Root page (empty) → site root; empty base → root-relative.
        assert_eq!(self_deep_link("", "demo", ""), "/?site=self/demo");
    }

    #[test]
    fn parse_site_query_rejects_incomplete() {
        assert_eq!(parse_site_query("PEER1"), None); // missing site
        assert_eq!(parse_site_query(""), None);
    }

    #[test]
    fn parse_site_url_rejects_non_projection() {
        assert_eq!(parse_site_url("/PEER1/sites/church/manifest"), None); // peer-first tree path
        assert_eq!(parse_site_url("/content/aa/bb/hex"), None);
        assert_eq!(parse_site_url("/sites/PEER1"), None); // missing site
    }

    #[test]
    fn normalize_prefix_accepts_root_and_clean_paths() {
        assert_eq!(normalize_prefix(""), Ok(String::new())); // root
        assert_eq!(normalize_prefix("alice"), Ok("alice".into()));
        assert_eq!(normalize_prefix("hosted-peers/PEERX"), Ok("hosted-peers/PEERX".into()));
    }

    #[test]
    fn normalize_prefix_rejects_malformed() {
        assert!(normalize_prefix("/alice").is_err()); // leading slash
        assert!(normalize_prefix("alice/").is_err()); // trailing slash
        assert!(normalize_prefix("a//b").is_err()); // empty segment
        assert!(normalize_prefix("a/../b").is_err()); // dotdot
        assert!(normalize_prefix("a b").is_err()); // whitespace
        // Reserved first segments would shadow the projection's own namespaces.
        assert!(normalize_prefix("sites").is_err());
        assert!(normalize_prefix("content/x").is_err());
        // …but reserved words are fine as a NON-first segment.
        assert!(normalize_prefix("alice/sites").is_ok());
    }

    #[test]
    fn prefixed_root_is_identity_for_empty() {
        use std::path::Path;
        // Empty prefix → the root unchanged (byte-identical layout guarantee).
        assert_eq!(prefixed_root(Path::new("dist"), ""), Path::new("dist"));
        assert_eq!(prefixed_root(Path::new("dist"), "alice"), Path::new("dist/alice"));
        assert_eq!(prefixed_root(Path::new("dist"), "a/b"), Path::new("dist/a/b"));
    }

    #[test]
    fn href_prefix_is_empty_for_root() {
        assert_eq!(href_prefix(""), ""); // `/sites/…` stays `/sites/…`
        assert_eq!(href_prefix("alice"), "/alice");
        assert_eq!(href_prefix("a/b"), "/a/b");
    }
}

//! Site navigation addressing + the entity-native link classifier.
//!
//! A [`Location`] names a page within a site, possibly on a remote
//! peer. [`classify_link`] turns a raw link string (as written in
//! markdown) into a [`LinkTarget`], and [`resolve_target`] maps a
//! target back to a concrete [`Location`] for the resolver to fetch.
//!
//! Link forms (design doc §3.3):
//! - `./about`, `../x`, `intro`  → in-site, **directory-relative** to the
//!   current page (standard markdown/filesystem semantics)
//! - `/docs/intro`               → in-site, **root-absolute** (site root)
//! - `site:{id}/{page}`          → cross-site, same peer
//! - `entity://{peer}/sites/{id}/pages/{page}` → cross-peer (v0.5 tree path)
//! - `https://…`, `http://…`, `mailto:…` → external (leaves the system)
//!
//! In-site link convention (the "link resolution" decision):
//! **one** convention app-wide. **Body** links (authored markdown, including
//! ingested corpora like the papers content) resolve **directory-relative**
//! against the current page's directory — `../notes/x.md` from page
//! `research/model/grounding` → `research/notes/x` — with a trailing `.md`/
//! `.markdown` stripped. A leading `/` is **root-absolute**. **Nav targets and
//! all app-generated in-site links** (manifest nav, generated section-index
//! pages, breadcrumbs, sidebar) are authored root-absolute `/{slug}` so they
//! resolve identically from any page. [`resolve_in_site`] is the single point
//! where this lives, shared by the live overlay nav, static export, and the
//! demo content.

#![allow(dead_code)] // window/renderer consumers land in P1

/// A concrete page address: which peer (None = the bound/current
/// peer), which site, which page (empty = the site's root page).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Location {
    pub peer_id: Option<String>,
    pub site_id: String,
    pub page: String,
}

impl Location {
    /// A location on the current peer at the site root.
    pub fn site_root(site_id: impl Into<String>) -> Self {
        Self { peer_id: None, site_id: site_id.into(), page: String::new() }
    }
}

/// The classified meaning of a link string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// A page within the current site.
    InSite { page: String },
    /// A page in a different site on the same peer.
    CrossSite { site_id: String, page: String },
    /// A page in a site on another peer — the resolver fetches it
    /// behind the scenes.
    CrossPeer { peer_id: String, site_id: String, page: String },
    /// A link out of the entity system entirely.
    External { url: String },
}

/// Classify a raw link string written in markdown, relative to the
/// page currently being viewed.
pub fn classify_link(href: &str, current: &Location) -> LinkTarget {
    let h = href.trim();

    if h.starts_with("https://") || h.starts_with("http://") || h.starts_with("mailto:") {
        return LinkTarget::External { url: h.to_string() };
    }

    if let Some(rest) = h.strip_prefix("entity://") {
        if let Some(parsed) = parse_entity_link(rest) {
            return parsed;
        }
        // Unparseable entity:// — treat as external rather than guess.
        return LinkTarget::External { url: h.to_string() };
    }

    if let Some(rest) = h.strip_prefix("site:") {
        let (site_id, page) = split_first_segment(rest);
        return LinkTarget::CrossSite { site_id, page };
    }

    LinkTarget::InSite { page: resolve_in_site(h, &current.page) }
}

/// Map a classified target back to a concrete [`Location`] to fetch,
/// inheriting the current peer/site where the target is relative.
/// `External` targets resolve to `None` (they leave the system).
pub fn resolve_target(target: &LinkTarget, current: &Location) -> Option<Location> {
    match target {
        LinkTarget::InSite { page } => Some(Location {
            peer_id: current.peer_id.clone(),
            site_id: current.site_id.clone(),
            page: page.clone(),
        }),
        LinkTarget::CrossSite { site_id, page } => Some(Location {
            peer_id: current.peer_id.clone(),
            site_id: site_id.clone(),
            page: page.clone(),
        }),
        LinkTarget::CrossPeer { peer_id, site_id, page } => Some(Location {
            peer_id: Some(peer_id.clone()),
            site_id: site_id.clone(),
            page: page.clone(),
        }),
        LinkTarget::External { .. } => None,
    }
}

/// Parse the part after `entity://` into a cross-peer target.
/// Expected shape (v0.5): `{peer}/sites/{site_id}/pages/{page}`.
/// Tolerant: locates the `sites` and `pages` segments rather than
/// assuming fixed offsets — so the legacy `{peer}/content/sites/…` form
/// (pre-v0.5) still resolves. Returns `None` if it can't find a site.
fn parse_entity_link(rest: &str) -> Option<LinkTarget> {
    let segs: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    if segs.len() < 2 {
        return None;
    }
    let peer_id = segs[0].to_string();
    let sites_idx = segs.iter().position(|s| *s == "sites")?;
    let site_id = segs.get(sites_idx + 1)?.to_string();
    let page = match segs.iter().position(|s| *s == "pages") {
        Some(pages_idx) => segs.get(pages_idx + 1..).map(|s| s.join("/")).unwrap_or_default(),
        None => String::new(),
    };
    Some(LinkTarget::CrossPeer { peer_id, site_id, page })
}

/// Split `"{first}/{rest}"` into `(first, rest)`; rest may be empty.
fn split_first_segment(s: &str) -> (String, String) {
    match s.split_once('/') {
        Some((a, b)) => (a.to_string(), b.to_string()),
        None => (s.to_string(), String::new()),
    }
}

/// Title-case a path segment for display: `getting-started` →
/// `Getting started`. Shared by the breadcrumb trail, the section
/// sidebar, and generated section-index titles.
pub fn humanize(seg: &str) -> String {
    let spaced = seg.replace(['-', '_'], " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => spaced,
    }
}

/// Resolve an in-site link against the directory of the current page,
/// using standard markdown/filesystem semantics.
///
/// - leading `/`  → root-absolute (the current directory is ignored)
/// - `./`, bare, `../`, `../../` → relative to `current_page`'s directory
/// - `.` is a no-op, `..` pops one segment (clamped at the site root)
/// - a trailing `.md`/`.markdown` is stripped from the final slug
/// - any `#fragment` / `?query` is dropped (pages are whole-file)
/// - resolving to empty (the site root) yields `""` → the manifest root page
fn resolve_in_site(href: &str, current_page: &str) -> String {
    // Drop fragment/query first — they never participate in slug resolution.
    let href = href.split(['#', '?']).next().unwrap_or("");

    // Base directory: the dir portion of the current page slug.
    // `research/model/grounding` → ["research","model"]; `index` → [].
    let mut base: Vec<&str> = match current_page.rfind('/') {
        Some(slash) => current_page[..slash].split('/').filter(|s| !s.is_empty()).collect(),
        None => Vec::new(),
    };

    // A leading `/` is root-absolute: discard the current directory.
    let rel = match href.strip_prefix('/') {
        Some(stripped) => {
            base.clear();
            stripped
        }
        None => href,
    };

    // Walk the relative segments with `.`/`..` semantics.
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}          // empty (`//`, trailing `/`) and `.` are no-ops
            ".." => {
                base.pop(); // clamp at root: pop() on an empty Vec is a no-op
            }
            other => base.push(other),
        }
    }

    let slug = base.join("/");
    // Strip a markdown extension from the final slug only.
    slug.strip_suffix(".md")
        .or_else(|| slug.strip_suffix(".markdown"))
        .map(str::to_string)
        .unwrap_or(slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cur() -> Location {
        Location { peer_id: Some("HOME".into()), site_id: "church".into(), page: "index".into() }
    }

    /// A nested current page, dir = `research/model` — exercises the real
    /// directory-relative semantics the papers corpus depends on.
    fn cur_nested() -> Location {
        Location {
            peer_id: Some("HOME".into()),
            site_id: "lab".into(),
            page: "research/model/grounding".into(),
        }
    }

    #[test]
    fn external_links_classified() {
        assert!(matches!(classify_link("https://example.com", &cur()), LinkTarget::External { .. }));
        assert!(matches!(classify_link("http://x.test", &cur()), LinkTarget::External { .. }));
        assert!(matches!(classify_link("mailto:a@b.c", &cur()), LinkTarget::External { .. }));
    }

    #[test]
    fn in_site_links_resolve_from_root_page() {
        // From a root page (dir = ""), `./` and bare are siblings of root,
        // `/` is root-absolute, and `..` clamps at root.
        assert_eq!(classify_link("./about", &cur()), LinkTarget::InSite { page: "about".into() });
        assert_eq!(classify_link("about", &cur()), LinkTarget::InSite { page: "about".into() });
        assert_eq!(classify_link("/docs/intro", &cur()), LinkTarget::InSite { page: "docs/intro".into() });
        assert_eq!(classify_link("../theory", &cur()), LinkTarget::InSite { page: "theory".into() });
    }

    #[test]
    fn in_site_links_resolve_dir_relative_from_nested_page() {
        let c = cur_nested(); // dir = research/model
        let p = |h: &str| match classify_link(h, &c) {
            LinkTarget::InSite { page } => page,
            other => panic!("expected InSite, got {other:?}"),
        };
        // The canonical papers-corpus case: `../notes/x.md` from research/model.
        assert_eq!(p("../notes/abstract/analysis-abstract-bridge.md"), "research/notes/abstract/analysis-abstract-bridge");
        assert_eq!(p("sibling.md"), "research/model/sibling"); // bare = sibling
        assert_eq!(p("./sibling"), "research/model/sibling"); // ./ = no-op
        assert_eq!(p("../../top.md"), "top"); // two pops to root
        assert_eq!(p("../../../../escape"), "escape"); // `..` clamps at root
        assert_eq!(p("/abs/page.md"), "abs/page"); // leading / ignores current dir
        assert_eq!(p("notes/"), "research/model/notes"); // trailing slash → dir target
        assert_eq!(p("page.md#section"), "research/model/page"); // fragment dropped, .md stripped
        assert_eq!(p("/"), ""); // site root → empty → manifest root page
    }

    #[test]
    fn cross_site_links_classified() {
        assert_eq!(
            classify_link("site:labs/intro", &cur()),
            LinkTarget::CrossSite { site_id: "labs".into(), page: "intro".into() }
        );
    }

    #[test]
    fn cross_peer_link_parsed() {
        let t = classify_link("entity://PEERX/sites/labs/pages/post1", &cur());
        assert_eq!(
            t,
            LinkTarget::CrossPeer { peer_id: "PEERX".into(), site_id: "labs".into(), page: "post1".into() }
        );
    }

    #[test]
    fn cross_peer_nested_page_path() {
        let t = classify_link("entity://PEERX/sites/labs/pages/docs/deep", &cur());
        assert_eq!(
            t,
            LinkTarget::CrossPeer { peer_id: "PEERX".into(), site_id: "labs".into(), page: "docs/deep".into() }
        );
    }

    #[test]
    fn legacy_content_sites_link_still_parses() {
        // The tolerant parser locates the `sites` segment, so a pre-v0.5
        // `content/sites/…` link published before the placement erratum
        // still resolves to the same target (back-compat by construction).
        let t = classify_link("entity://PEERX/content/sites/labs/pages/post1", &cur());
        assert_eq!(
            t,
            LinkTarget::CrossPeer { peer_id: "PEERX".into(), site_id: "labs".into(), page: "post1".into() }
        );
    }

    #[test]
    fn malformed_entity_link_falls_back_to_external() {
        assert!(matches!(classify_link("entity://nope", &cur()), LinkTarget::External { .. }));
    }

    #[test]
    fn resolve_in_site_inherits_peer_and_site() {
        let t = LinkTarget::InSite { page: "about".into() };
        let loc = resolve_target(&t, &cur()).unwrap();
        assert_eq!(loc.peer_id, Some("HOME".into()));
        assert_eq!(loc.site_id, "church");
        assert_eq!(loc.page, "about");
    }

    #[test]
    fn resolve_cross_peer_switches_peer() {
        let t = LinkTarget::CrossPeer { peer_id: "PEERX".into(), site_id: "labs".into(), page: "intro".into() };
        let loc = resolve_target(&t, &cur()).unwrap();
        assert_eq!(loc.peer_id, Some("PEERX".into()));
        assert_eq!(loc.site_id, "labs");
    }

    #[test]
    fn resolve_external_is_none() {
        let t = LinkTarget::External { url: "https://x".into() };
        assert!(resolve_target(&t, &cur()).is_none());
    }
}

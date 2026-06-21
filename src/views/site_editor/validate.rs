//! Pure validation + render-health for the Site Editor.
//!
//! Two gates (defensive authoring):
//!
//! * **Input validation** (`validate_site_id` / `validate_page_slug`) — fail
//!   closed *before* a write, so the editor never lands a path-unsafe id.
//! * **Render health** (`site_health`) — does this site render in the (frozen)
//!   browser *right now*? It mirrors `resolver::resolve_local`'s root-page rule
//!   exactly, so `Renderable` is true iff the browser would render the root.
//!   It backs the D13 status line and lets create guarantee a Renderable result.
//!
//! All pure / native-testable; no WASM, no DOM.

use crate::content_site::{discovery, paths, SiteManifest};
use crate::peers::Peers;

/// Is `c` allowed in a single slug segment? Lowercase/uppercase letters,
/// digits, `-`, `_`. Deliberately conservative: no `.`, `/`, whitespace, or
/// anything that could escape or confuse a tree path / URL projection.
fn is_slug_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// Max length for a site id / page slug segment — generous for authoring,
/// bounded so a pathological id can't bloat a path.
const MAX_SEGMENT_LEN: usize = 64;

/// Validate a **site id** (the `{site}` segment of `/{peer}/sites/{site}/…`).
/// A single clean slug segment: non-empty, slug-chars only, length-bounded.
/// (No `/` — a site id is one segment; nesting is a page-slug concern.)
pub fn validate_site_id(site_id: &str) -> Result<(), String> {
    let s = site_id.trim();
    if s.is_empty() {
        return Err("Enter a site id.".into());
    }
    if s.len() > MAX_SEGMENT_LEN {
        return Err(format!("Site id is too long (max {MAX_SEGMENT_LEN})."));
    }
    if !s.chars().all(is_slug_char) {
        return Err("Site id may use only letters, digits, '-' and '_'.".into());
    }
    Ok(())
}

/// Validate a **page slug** (the `{page}` under `…/pages/`). May nest with
/// `/` (`guide/intro`); each segment must be a clean slug. Rejects empty
/// segments, leading/trailing `/`, and `.`/`..` (no parent escape).
pub fn validate_page_slug(slug: &str) -> Result<(), String> {
    let s = slug.trim();
    if s.is_empty() {
        return Err("Enter a page name.".into());
    }
    if s.starts_with('/') || s.ends_with('/') {
        return Err("Page name must not start or end with '/'.".into());
    }
    for seg in s.split('/') {
        if seg.is_empty() {
            return Err("Page name has an empty path segment ('//').".into());
        }
        if seg == "." || seg == ".." {
            return Err("Page name must not contain '.' or '..' segments.".into());
        }
        if seg.len() > MAX_SEGMENT_LEN {
            return Err(format!("A page-name segment is too long (max {MAX_SEGMENT_LEN})."));
        }
        if !seg.chars().all(is_slug_char) {
            return Err("Page name may use only letters, digits, '-', '_' and '/'.".into());
        }
    }
    Ok(())
}

/// Render-health of a site as the (frozen) browser sees it. `Renderable` iff
/// `resolver::resolve_local` would resolve the root page — same rule, so the
/// editor's "✓ renders" can't disagree with the browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SiteHealth {
    /// Manifest present + the root page resolves (a page entity, or a section
    /// path with child pages — the browser's generated section-index case).
    Renderable,
    /// Won't render; the reason is shown to the author (and may carry a fix).
    NotRenderable(String),
}

/// Compute [`SiteHealth`] for an **owned** site on `peer_id`. Reads are sync L0
/// against the bound peer's store (Direct/IDB arm). Mirrors
/// `resolver::resolve_local`: manifest → root page entity OR section-with-children.
pub fn site_health(peers: &Peers, peer_id: &str, site_id: &str) -> SiteHealth {
    let manifest = match peers.get_entity(peer_id, &paths::manifest_path(peer_id, site_id)) {
        Some(e) => SiteManifest::from_entity(&e),
        None => return SiteHealth::NotRenderable("no manifest for this site".into()),
    };
    let root = manifest.root().to_string();
    if peers.get_entity(peer_id, &paths::page_path(peer_id, site_id, &root)).is_some() {
        return SiteHealth::Renderable;
    }
    // A root that is a section (has child pages) renders a generated index.
    if !discovery::list_child_pages(peers, peer_id, site_id, &format!("{root}/")).is_empty() {
        return SiteHealth::Renderable;
    }
    SiteHealth::NotRenderable(format!("no '{root}' page — create it to make the site render"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_site::{paths, SitePage};

    #[test]
    fn site_id_accepts_clean_slugs_rejects_unsafe() {
        assert!(validate_site_id("mysite").is_ok());
        assert!(validate_site_id("my-site_2").is_ok());
        assert!(validate_site_id("  trimmed  ").is_ok());
        assert!(validate_site_id("").is_err());
        assert!(validate_site_id("a/b").is_err());
        assert!(validate_site_id("a b").is_err());
        assert!(validate_site_id("..").is_err());
        assert!(validate_site_id("x.md").is_err());
        assert!(validate_site_id(&"x".repeat(65)).is_err());
    }

    #[test]
    fn page_slug_allows_nesting_rejects_escape() {
        assert!(validate_page_slug("index").is_ok());
        assert!(validate_page_slug("guide/intro").is_ok());
        assert!(validate_page_slug("a/b/c").is_ok());
        assert!(validate_page_slug("").is_err());
        assert!(validate_page_slug("/x").is_err());
        assert!(validate_page_slug("x/").is_err());
        assert!(validate_page_slug("a//b").is_err());
        assert!(validate_page_slug("a/../b").is_err());
        assert!(validate_page_slug("a/./b").is_err());
        assert!(validate_page_slug("has space").is_err());
    }

    #[test]
    fn health_tracks_manifest_and_root_page() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();

        // No manifest → not renderable.
        assert!(matches!(site_health(&peers, &me, "s"), SiteHealth::NotRenderable(_)));

        // Manifest only (root 'index' missing, no children) → not renderable,
        // with the actionable reason. This is the state create must avoid.
        peers.seed_write(
            &me,
            paths::manifest_path(&me, "s"),
            crate::content_site::SiteManifest::new("s", "S", "index", vec![]).to_entity(),
        );
        match site_health(&peers, &me, "s") {
            SiteHealth::NotRenderable(r) => assert!(r.contains("index"), "reason names the page: {r}"),
            other => panic!("manifest-only should not be renderable: {other:?}"),
        }

        // Add the root page → renderable.
        peers.seed_write(
            &me,
            paths::page_path(&me, "s", "index"),
            SitePage::markdown("Home", "# Home").to_entity(),
        );
        assert_eq!(site_health(&peers, &me, "s"), SiteHealth::Renderable);
    }
}

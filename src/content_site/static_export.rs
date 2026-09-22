//! Static HTML export — project a site subgraph onto pre-rendered, no-JS
//! HTML for the legacy web (dumb CDN / permalink / SSG).
//!
//! This is the **publish-time projection** (`paths` §11 / SITE v0.5 §11):
//! a site's pages render to flat `.html` files under the prefix-first
//! `sites/{peer_id}/{site_id}/…` layout, with every entity-native link
//! rewritten to a static href a dumb browser can follow. There is no JS,
//! no SDK, no dispatch on the other end — just files. Real verification
//! (caps, signatures) happens in a live entity-aware peer; this surface is
//! for permalinking, no-JS readers, and the SSG use case.
//!
//! **Multi-site is first-class.** [`export_site_set`] takes a *set* of
//! sites and rewrites cross-site (`site:other/page`) and cross-peer
//! (`entity://…`) links to the other site's projection path — so a body of
//! sites exports as one navigable static tree, not isolated islands.
//!
//! Native-only (writes files); never compiled into the wasm bundle. Reuses
//! the live render path ([`render::render_page`]) and the link
//! classifier ([`location::classify_link`]) — it does **not** fork a second
//! renderer (the "shared render-lib" item, O2/A6).

#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)] // driven by the demo emitter + the test below

use std::fs;
use std::path::Path;

use super::format::{NavItem, SiteAsset, SiteManifest, SitePage};
use super::location::{self, LinkTarget};
use super::paths::SITE_URL_PREFIX;
use super::read::OwnedSite;

/// One site to export: its identity, manifest, and `(slug, page)` bodies.
pub struct ExportSite<'a> {
    pub peer_id: &'a str,
    pub site_id: &'a str,
    pub manifest: &'a SiteManifest,
    pub pages: &'a [(&'a str, SitePage)],
    /// `(name, asset)` — the site's embedded assets, written next to the pages
    /// so a dumb static server resolves `<img src="assets/…">` with no JS.
    pub assets: &'a [(String, SiteAsset)],
}

/// A link whose target is **not in the export set** — the defect this audit
/// exists for.
///
/// [`static_href`] resolves `site:other/page` against the **current** peer
/// unconditionally. That is right when the whole body of sites ships together,
/// and wrong under per-domain publishing, where each domain carries only its
/// own sites: the href is emitted, the file is not there, and a reader gets a
/// 404 that nothing on our side ever saw. Measured on production 2026-08-21
/// (`docs/status/FINDING-2026-08-21-cross-site-links-404-under-per-domain-publishing.md`).
///
/// **`CrossPeer` is affected identically, which the finding did not say.**
/// [`projection_href`] emits a root-absolute path with no host component at
/// all, so an explicit `entity://{other-peer}/sites/x` link also lands on the
/// *current* domain. The exporter cannot express a cross-domain link today;
/// this type is what makes that visible at publish time instead of on
/// production.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DanglingLink {
    /// The peer whose tree the source page was written into.
    pub from_peer: String,
    pub from_site: String,
    /// The page slug the link was authored on.
    pub from_page: String,
    /// The link as written, e.g. `site:billslab-entity-system/papers/07-deos`.
    pub target: String,
    /// Where it was rewritten to — the URL a reader actually requests, and
    /// 404s on. Carried because the authored form alone does not show the
    /// wrong peer the link was resolved against.
    pub href: String,
}

impl std::fmt::Display for DanglingLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let page = if self.from_page.is_empty() { "<root>" } else { &self.from_page };
        write!(
            f,
            "{}/{}/{} → {} (emitted {})",
            self.from_peer, self.from_site, page, self.target, self.href
        )
    }
}

/// The export set's membership, plus every out-of-set link found while
/// rewriting.
///
/// Interior mutability because the render path is a tree of `&`-taking
/// functions and threading `&mut` through [`render_nav_items`]' recursion
/// would be a larger change than the check it carries. The publish CLI is
/// single-threaded, so a `RefCell` is the right cell.
pub struct LinkAudit {
    /// `(peer_id, site_id)` present in this export.
    members: std::collections::HashSet<(String, String)>,
    dangling: std::cell::RefCell<Vec<DanglingLink>>,
}

impl LinkAudit {
    fn new(sites: &[ExportSite]) -> Self {
        Self {
            members: sites
                .iter()
                .map(|s| (s.peer_id.to_string(), s.site_id.to_string()))
                .collect(),
            dangling: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// Record `target` if the href just emitted for it points outside the set.
    ///
    /// Membership is asked of the peer the href was resolved **against**, not
    /// of the peer the author meant: for a `CrossSite` those differ precisely
    /// in the broken case, and the emitted path is what a reader requests.
    fn note(&self, target: &LinkTarget, current: &location::Location, raw: &str, href: &str) {
        let (peer, site) = match target {
            LinkTarget::CrossSite { site_id, .. } => {
                (current.peer_id.clone().unwrap_or_default(), site_id.clone())
            }
            LinkTarget::CrossPeer { peer_id, site_id, .. } => (peer_id.clone(), site_id.clone()),
            // An in-site target resolves within a site we are writing; an
            // external link passes through verbatim and is nobody's business
            // here.
            LinkTarget::InSite { .. } | LinkTarget::External { .. } => return,
        };
        if self.members.contains(&(peer.clone(), site.clone())) {
            return;
        }
        self.dangling.borrow_mut().push(DanglingLink {
            from_peer: current.peer_id.clone().unwrap_or_default(),
            from_site: current.site_id.clone(),
            from_page: current.page.clone(),
            target: raw.to_string(),
            href: href.to_string(),
        });
    }

    /// Every out-of-set link, deduplicated and ordered — the same authored link
    /// appears on every page that carries it, and an operator wants the list of
    /// things to fix, not a per-page tally.
    fn into_dangling(self) -> Vec<DanglingLink> {
        let mut out = self.dangling.into_inner();
        out.sort_by(|a, b| {
            (&a.from_site, &a.from_page, &a.target).cmp(&(&b.from_site, &b.from_page, &b.target))
        });
        out.dedup();
        out
    }
}

/// What an export produced.
///
/// `pages` is what the call sites already consumed; `dangling` is the new half.
/// Returned as a struct rather than a tuple so a future counter is a field
/// rather than a signature change at every caller.
#[derive(Debug, Default)]
pub struct ExportReport {
    pub pages: usize,
    /// Out-of-set links — see [`DanglingLink`]. **Empty is the only clean
    /// state**; a non-empty list is 404s that will ship.
    pub dangling: Vec<DanglingLink>,
}

/// How links project, and what to do about one that leaves the export set.
///
/// Bundled rather than passed as three positionals because they are one
/// concept — *where does an href point* — and because this is where a
/// cross-domain resolution map lands if it is ever built (backlog **B-4**,
/// `docs/plans/DESIGN-CROSS-DOMAIN-SITE-LINKS.md`): a field, not a fourth
/// argument threaded through every function in the render path.
#[derive(Clone, Copy)]
struct LinkCtx<'a> {
    layout: Layout,
    /// Per-peer hosting scope; empty is the domain root.
    prefix: &'a str,
    /// Present for a multi-site projection export; `None` for bare-root — a
    /// single site has no set to be outside of, and its cross-site links are
    /// deliberately namespaced outbound hrefs (see [`Layout::BareRoot`]).
    audit: Option<&'a LinkAudit>,
}

/// How a site projects onto the output tree + link space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Prefix-first projection: files at `sites/{peer}/{site}/{slug}.html`,
    /// in-site links root-absolute under that prefix, per-peer index, the
    /// entity footer. Multi-site, the legacy-web / permalink / CDN surface.
    Projection,
    /// Bare root: a **single** site rendered at the domain root — files at
    /// `{slug}.html`, in-site links `/{slug}.html`, no prefix, no peer index,
    /// no entity branding. The "just a site generator" on-ramp ([F1]). A
    /// cross-site/cross-peer link (rare in a standalone single site) still
    /// resolves to the projection layout, so a bare-root + projection export
    /// side by side stays internally linked; on its own it's a namespaced
    /// outbound href, not a silently-wrong root link.
    BareRoot,
}

/// Export a set of sites to static HTML under `out_dir`, at the prefix-first
/// projection layout `out_dir/sites/{peer_id}/{site_id}/{slug}.html`.
///
/// Cross-site / cross-peer links resolve across the whole set, so the
/// export is internally navigable. When `live_base` is `Some(origin)`, each
/// page carries a dismissable "open in the live entity browser" banner
/// deep-linking to `{origin}/?site=…` ([F2]).
///
/// Returns the pages written **and every link whose target was not in the set**
/// ([`ExportReport`]). The set was always in scope here and was never consulted;
/// a `site:` target belonging to another domain resolved to a path under *this*
/// peer and shipped as a 404. The caller decides what to do with the list —
/// this function still writes the tree either way, because refusing here would
/// break the per-domain builds that are shipping today.
pub fn export_site_set(
    out_dir: &Path,
    sites: &[ExportSite],
    prefix: &str,
    live_base: Option<&str>,
) -> std::io::Result<ExportReport> {
    let audit = LinkAudit::new(sites);
    let ctx = LinkCtx { layout: Layout::Projection, prefix, audit: Some(&audit) };
    let mut written = 0;
    for site in sites {
        let page_slugs: Vec<String> = site.pages.iter().map(|(s, _)| s.to_string()).collect();
        for (slug, page) in site.pages {
            let html = render_page(site, slug, page, ctx, live_base);
            let path = page_file_path(out_dir, site.peer_id, site.site_id, slug, prefix);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, html)?;
            written += 1;
        }
        // Generated section-index pages for intermediate directories that have
        // no page entity of their own (e.g. `research/notes` when only
        // `research/notes/x` pages exist). Static parity with the live overlay,
        // which synthesizes the same index on the fly
        // ([`super::resolver::section_index_page`]) — without these, a body/nav
        // link to a bare section dir 404s on the static surface.
        let have: std::collections::HashSet<&str> =
            page_slugs.iter().map(String::as_str).collect();
        for dir in section_dirs(&page_slugs) {
            if have.contains(dir.as_str()) {
                continue; // a real page already owns this slug
            }
            let children =
                super::discovery::children_from_slugs(&page_slugs, &format!("{dir}/"));
            if children.is_empty() {
                continue;
            }
            let idx = super::resolver::section_index_page(&dir, &children);
            let html = render_page(site, &dir, &idx, ctx, live_base);
            let path = page_file_path(out_dir, site.peer_id, site.site_id, &dir, prefix);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, html)?;
            written += 1;
        }
        // Write the site's raw asset bytes next to its pages, at
        // `sites/{peer}/{site}/assets/{name}`, so `<img src="assets/…">`
        // resolves on a dumb static server (the live app resolves the `.bin`
        // asset instead; this is the no-JS surface's copy).
        for (name, asset) in site.assets {
            let apath = asset_file_path(out_dir, site.peer_id, site.site_id, name, prefix);
            if let Some(parent) = apath.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&apath, &asset.bytes)?;
        }
        // A peer-level index that lists this peer's exported sites — the
        // static surface's answer to multi-site discovery.
        write_peer_index(out_dir, site.peer_id, sites, prefix)?;
    }
    // A landing index at `{out}/sites/index.html` listing every exported site —
    // so the static tree is reachable WITHOUT knowing the (ephemeral) publish
    // peer-id. It lives UNDER the `sites/` projection namespace (not `{out}/`)
    // so the export can be served at the SAME origin as the live SPA: the SPA
    // owns `/` (its own index.html), the static tree owns `/sites/…`, and the
    // root-absolute in-page links resolve. Serving `{out}/sites/` lands here.
    // (Bare-root export skips this — it IS the root.)
    write_root_index(out_dir, sites, prefix, live_base)?;
    // Land `/` somewhere sensible for a bare-dir preview (and clear any stale
    // origin SW); a no-op when a live SPA already owns `{out}/index.html`.
    write_landing_redirect(out_dir, prefix)?;
    Ok(ExportReport { pages: written, dangling: audit.into_dangling() })
}

/// Export sites read off the **live tree** ([`super::read::OwnedSite`]) to
/// static HTML — the [A]→[B1] path: one tree read, projected to no-JS pages.
/// Bridges the owned tree-read model onto the borrowing [`ExportSite`] the
/// renderer consumes (a per-page clone, negligible at publish time), so the
/// emitter and its tests stay one code path regardless of where the site
/// data came from. Cross-site/cross-peer links resolve across the whole set.
pub fn export_owned_sites(
    out_dir: &Path,
    sites: &[OwnedSite],
    prefix: &str,
    live_base: Option<&str>,
) -> std::io::Result<ExportReport> {
    // Materialize borrowed `(slug, page)` views first; `page_vecs` must
    // outlive the `ExportSite`s that borrow it (hence the separate binding).
    let page_vecs: Vec<Vec<(&str, SitePage)>> = sites
        .iter()
        .map(|s| s.pages.iter().map(|(slug, page)| (slug.as_str(), page.clone())).collect())
        .collect();
    let borrowed: Vec<ExportSite> = sites
        .iter()
        .zip(&page_vecs)
        .map(|(s, pv)| ExportSite {
            peer_id: &s.peer_id,
            site_id: &s.site_id,
            manifest: &s.manifest,
            pages: pv,
            assets: &s.assets,
        })
        .collect();
    export_site_set(out_dir, &borrowed, prefix, live_base)
}

/// Export **one** site at the domain root — the bare-root SSG mode ([F1]).
/// Files land at `{out}/{slug}.html` (no `sites/{peer}/{site}/` prefix), in-site
/// links are root-relative `/{slug}.html`, there is no peer index and no entity
/// branding: the output looks like any static site generator's, the "just a
/// site generator" on-ramp. Returns the number of pages written.
pub fn export_bare_root(
    out_dir: &Path,
    site: &OwnedSite,
    live_base: Option<&str>,
) -> std::io::Result<usize> {
    let pages: Vec<(&str, SitePage)> =
        site.pages.iter().map(|(slug, page)| (slug.as_str(), page.clone())).collect();
    let es = ExportSite {
        peer_id: &site.peer_id,
        site_id: &site.site_id,
        manifest: &site.manifest,
        pages: &pages,
        assets: &site.assets,
    };
    let mut written = 0;
    // No audit: a single site has no set to be outside of, and its cross-site
    // links are deliberately namespaced outbound hrefs (see [`Layout::BareRoot`]).
    // Auditing here would report every one of them as dangling, which is the
    // cry-wolf failure — the guard would be routed around within a day.
    let ctx = LinkCtx { layout: Layout::BareRoot, prefix: "", audit: None };
    for (slug, page) in es.pages {
        // Bare-root is the domain root itself — no hosting prefix applies.
        let html = render_page(&es, slug, page, ctx, live_base);
        let path = out_dir.join(format!("{slug}.html"));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, html)?;
        written += 1;
    }
    // Bare-root assets sit at the domain root: `{out}/assets/{name}`.
    for (name, asset) in es.assets {
        let apath = out_dir.join("assets").join(name);
        if let Some(parent) = apath.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&apath, &asset.bytes)?;
    }
    Ok(written)
}

/// The distinct intermediate directory prefixes across all page slugs —
/// every ancestor path that could be navigated to as a section. For slug
/// `research/notes/x`, yields `research` and `research/notes`. Sorted +
/// deduped (deterministic output). Used to emit generated section-index
/// pages for dirs that have no page entity of their own.
fn section_dirs(slugs: &[String]) -> Vec<String> {
    let mut dirs: Vec<String> = Vec::new();
    for slug in slugs {
        let segs: Vec<&str> = slug.split('/').collect();
        // All proper ancestors (exclude the leaf segment itself).
        for i in 1..segs.len() {
            dirs.push(segs[..i].join("/"));
        }
    }
    dirs.sort();
    dirs.dedup();
    dirs
}

/// Filesystem path for a page: `{out}/{prefix}/sites/{peer}/{site}/{slug}.html`
/// (the prefix segment is absent when empty).
fn page_file_path(
    out_dir: &Path,
    peer_id: &str,
    site_id: &str,
    slug: &str,
    prefix: &str,
) -> std::path::PathBuf {
    super::paths::prefixed_root(out_dir, prefix)
        .join(SITE_URL_PREFIX)
        .join(peer_id)
        .join(site_id)
        .join(format!("{slug}.html"))
}

/// Render one page to a complete standalone HTML document under `layout`.
/// When `live_base` is `Some`, a dismissable "open in live peer" banner is
/// injected at the top of the body, deep-linking to this page in the live SPA.
///
/// A **`format:html` page is emitted verbatim** and skips this template
/// entirely — see [`render::PageRender::Document`]. It is *already* a complete
/// standalone document (its own `<head>`, `<title>`, `<style>`), so wrapping it
/// would nest `<html>` inside `<body>` and leak its stylesheet onto our chrome;
/// and its hrefs/`<img src>` are its own, so the two rewriters below would
/// rewrite links that were never site-relative. The cost is honest and stated:
/// such a page carries no site nav in the static projection. The live surface
/// keeps its chrome, because there the document sits in a frame beside it.
fn render_page(
    site: &ExportSite,
    slug: &str,
    page: &SitePage,
    ctx: LinkCtx,
    live_base: Option<&str>,
) -> String {
    let current = location::Location {
        peer_id: Some(site.peer_id.to_string()),
        site_id: site.site_id.to_string(),
        page: slug.to_string(),
    };
    let rendered = super::render::render_page(&page.format, &page.body);
    let body = match rendered {
        super::render::PageRender::Document(doc) => return doc,
        super::render::PageRender::Markup(markup) => markup,
    };
    let body = rewrite_hrefs(&body, &current, ctx);
    // Images need their own pass — rewrite_hrefs only touches href=". Site-relative
    // `src="assets/…"` becomes a root-absolute static URL (depth-safe).
    let body = rewrite_img_srcs(&body, site.peer_id, site.site_id, ctx.layout, ctx.prefix);
    let nav = render_nav(&site.manifest.nav, &current, slug, ctx);
    let banner = live_base.map(|base| render_live_banner(base, site.peer_id, site.site_id, slug)).unwrap_or_default();
    let page_title = page.title();
    let site_title = &site.manifest.title;
    // The site-title link goes to the site root. Resolve it through the same
    // href logic (an empty-page in-site link) so it is correct per layout AND
    // from a nested page — a hardcoded "./" is wrong for `guide/intro.html`.
    let home_href = static_href(&LinkTarget::InSite { page: String::new() }, &current, ctx);
    // Bare-root carries no entity branding (the "just a site generator" pitch);
    // the projection surface names what it is.
    let footer = match ctx.layout {
        Layout::Projection => {
            "<footer class=\"site-footer\">Static export · entity content-site projection</footer>"
                .to_string()
        }
        Layout::BareRoot => String::new(),
    };

    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{page_title} — {site_title}</title>\n\
         <style>{css}</style>\n\
         </head>\n<body>\n\
         {banner}\
         <header class=\"site-header\"><a class=\"site-title\" href=\"{home}\">{site_title}</a>{nav}</header>\n\
         <main class=\"page\">\n{body}\n</main>\n\
         {footer}\n\
         </body>\n</html>\n",
        css = page_css(site_theme(site.manifest)),
        home = esc(&home_href),
    )
}

/// The site's effective own palette for export: its manifest-declared theme
/// when it names a **registered** theme, else `None` (the `SITE_TOKENS`
/// defaults). Mirrors the live "Site's theme" mode resolution (S-T2) — a
/// published page looks the way that mode shows the site in-app. Unknown
/// names warn loudly (the publish CLI's console) and export as today.
fn site_theme(manifest: &SiteManifest) -> Option<&'static crate::theme_tokens::Theme> {
    let name = manifest.params.get("theme").filter(|s| !s.is_empty())?;
    let theme = crate::theme_tokens::registered(name);
    if theme.is_none() {
        tracing::warn!(
            theme = %name,
            site = %manifest.site_id,
            "manifest declares unknown theme — exporting with the default site palette"
        );
    }
    theme
}

/// Render the dismissable "open in live peer" banner ([F2]). No-JS: the
/// dismiss is a pure-CSS checkbox toggle (a hidden checkbox + a `×` label;
/// `:checked` hides the banner — works on a dumb static host). The CTA
/// deep-links to `{live_base}/?site={peer}/{site}/{page}`
/// ([`paths::site_deep_link`]), which the live SPA reads at boot ([F3]).
fn render_live_banner(live_base: &str, peer_id: &str, site_id: &str, slug: &str) -> String {
    // Deep-link to the REAL publish peer-id, not the `self` sentinel. The
    // sentinel resolves to the live SPA's own system peer — correct ONLY for the
    // demo round-trip (the SPA seeds the demo into its own peer). For real
    // published content (an ingested corpus served same-origin at
    // `/{peer}/sites/…`), the content is NOT on the system peer; `self` lands on
    // "No site manifest". The peer-id IS the addressing key: the live SPA's
    // `?site=` boot routes a real peer-id through the resolver (it ensures a
    // same-origin origin entry → HTTP-poll fetches `/{peer}/sites/{site}/…bin`),
    // so the static→live round-trip resolves the actual published site.
    let href = super::paths::site_deep_link(live_base, peer_id, site_id, slug);
    format!(
        "<input type=\"checkbox\" id=\"live-banner-x\" class=\"live-banner-toggle\">\
         <aside class=\"live-banner\">\
         <span>You're viewing a static snapshot. \
         <a href=\"{href}\">Open in the live entity browser →</a></span>\
         <label for=\"live-banner-x\" class=\"live-banner-dismiss\" aria-label=\"Dismiss\">×</label>\
         </aside>\n",
        href = esc(&href),
    )
}

/// **`APP-CONVENTION-SEMANTIC-CONTENT-SITE` §4.1's max depth, expressed once.**
///
/// *"A renderer walking nav MUST maintain a visited-set (cycle detection),
/// enforce a max depth (recommend 32), and on either limit stop cleanly."*
/// **Two** functions here walk the authored nav tree — [`render_nav_items`] and
/// [`subtree_holds_active`] — and the bound was a `32` typed inline in one of
/// them, with the other uncapped. Two expressions of one rule is C15's defect;
/// one expression with no consumer is worse, so both read this.
///
/// **The cycle half of §4.1 is unreachable in this data model, and that is a
/// conformance statement rather than a gap.** [`NavItem::children`] is an owned
/// `Vec<NavItem>` — a Rust value cannot point back at an ancestor — and the
/// decode side builds it from CBOR, which has no back-reference. So an authored
/// cycle cannot survive into a tree we walk; what *is* reachable is pathological
/// depth, which is what this bounds. A visited-set here would be a guard against
/// a state the type system already forbids.
///
/// **Scope, stated because it is narrower than "the renderer":** the live DOM
/// app does not recurse over nav at all. `views/content_site/model.rs`'s
/// `output_from_resolved` maps `manifest.nav` **one level** into flat
/// `NavLink`s and never reads `children` (its own `GAP 3 (sub-nav)` note). So
/// §4.1's walking contract lands entirely on this static exporter today. If the
/// live renderer ever grows sub-nav, this constant moves somewhere both can see
/// it — and that move is the whole point of it being a constant.
const MAX_NAV_DEPTH: usize = 32;

/// Render the manifest nav menu to static `<a>` links (recursive). The
/// page currently being rendered is marked `aria-current`.
fn render_nav(
    nav: &[NavItem],
    current: &location::Location,
    current_slug: &str,
    ctx: LinkCtx,
) -> String {
    if nav.is_empty() {
        return String::new();
    }
    let mut out = String::from("<nav class=\"site-nav\">");
    render_nav_items(nav, current, current_slug, &mut out, 0, ctx);
    out.push_str("</nav>");
    out
}

fn render_nav_items(
    items: &[NavItem],
    current: &location::Location,
    current_slug: &str,
    out: &mut String,
    depth: usize,
    ctx: LinkCtx,
) {
    // §4.1 depth safety — see [`MAX_NAV_DEPTH`].
    if depth > MAX_NAV_DEPTH {
        return;
    }
    out.push_str("<ul>");
    for item in items {
        out.push_str("<li>");
        // A group renders as a native `<details>` disclosure — collapsible with
        // **no JavaScript**, which is the constraint this whole export exists
        // to satisfy, and still fully crawlable because the children stay in
        // the DOM whether or not the group is open.
        //
        // **Closed by default, and "you are here" is a HIGHLIGHT, not an open
        // panel.** The tempting move is to ship the current page's group `open`
        // — a static exporter knows the current page at render time, which the
        // live app does not. It is wrong here: the panel is absolutely
        // positioned so it does not shove the page down, which means an
        // auto-opened group would cover the article on every load, and without
        // JS a `<details>` cannot close on an outside click — so the reader
        // would have to dismiss a panel by hand on arrival at every page. The
        // render-time knowledge is still used; it just marks the group instead
        // of opening it.
        let group = !item.children.is_empty();
        if group {
            let here = if subtree_holds_active(&item.children, current, current_slug) {
                " class=\"here\""
            } else {
                ""
            };
            out.push_str(&format!("<details class=\"nav-group\"><summary{here}>"));
        }
        if item.target.is_empty() {
            // A section header — no link.
            out.push_str(&format!("<span class=\"nav-section\">{}</span>", esc(&item.label)));
        } else {
            let target = location::classify_link(&item.target, current);
            let href = static_href(&target, current, ctx);
            // Nav is audited too: the portal-index generator emits cross-site
            // nav links, so a domain router is exactly where an out-of-set
            // target shows up — and it would be invisible to a body-only check.
            if let Some(audit) = ctx.audit {
                audit.note(&target, current, &item.target, &href);
            }
            let active = matches!(&target, LinkTarget::InSite { page } if page == current_slug);
            let cls = if active { " class=\"active\"" } else { "" };
            out.push_str(&format!("<a href=\"{}\"{cls}>{}</a>", esc(&href), esc(&item.label)));
        }
        if group {
            out.push_str("</summary>");
        }
        if group {
            render_nav_items(&item.children, current, current_slug, out, depth + 1, ctx);
            out.push_str("</details>");
        }
        out.push_str("</li>");
    }
    out.push_str("</ul>");
}

/// Does the page being rendered live anywhere inside this subtree?
///
/// Whether this subtree contains the page being rendered — which decides
/// whether its group is **marked**, not whether it ships `open` (see
/// `render_nav_items`: an auto-opened group would cover the article on
/// arrival, and without JS the reader could not dismiss it). Recursive
/// because the active page may be several levels down.
///
/// **Takes no `LinkCtx`, and that absence is the point.** It classifies
/// targets a second time, over links the render pass has already noted —
/// so handing it the context would put `ctx.audit` within reach, and a
/// second `note()` reports every out-of-set nav link twice. Not passing
/// the audit sink is a weaker guarantee than not being able to reach it.
fn subtree_holds_active(
    items: &[NavItem],
    current: &location::Location,
    current_slug: &str,
) -> bool {
    subtree_holds_active_at(items, current, current_slug, 0)
}

/// **Depth-bounded, and it was not until 2026-09-09.** This is the *second*
/// walker over the authored nav tree and it had no bound at all, while its
/// sibling one screen up carried an inline `32` — so §4.1's max-depth MUST was
/// half-implemented, on the half nobody reads because it returns a `bool`
/// instead of markup. Bounded in practice by the decoder's own ceiling
/// (measured: an authored nav ≥127 deep does not decode at all), which is why
/// it was never a live overflow — a bound that holds by accident somewhere else
/// is not the bound the rule asks for.
fn subtree_holds_active_at(
    items: &[NavItem],
    current: &location::Location,
    current_slug: &str,
    depth: usize,
) -> bool {
    if depth > MAX_NAV_DEPTH {
        return false;
    }
    items.iter().any(|item| {
        if !item.target.is_empty() {
            let target = location::classify_link(&item.target, current);
            if matches!(&target, LinkTarget::InSite { page } if page == current_slug) {
                return true;
            }
        }
        subtree_holds_active_at(&item.children, current, current_slug, depth + 1)
    })
}

/// Rewrite every `href="…"` in a rendered HTML body from its entity-native
/// form to a static projection href, via the link classifier. External
/// links pass through untouched.
fn rewrite_hrefs(html: &str, current: &location::Location, ctx: LinkCtx) -> String {
    const NEEDLE: &str = "href=\"";
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(idx) = rest.find(NEEDLE) {
        let (before, after) = rest.split_at(idx + NEEDLE.len());
        out.push_str(before);
        match after.find('"') {
            Some(end) => {
                let raw = &after[..end];
                let target = location::classify_link(raw, current);
                let href = static_href(&target, current, ctx);
                if let Some(audit) = ctx.audit {
                    audit.note(&target, current, raw, &href);
                }
                out.push_str(&esc(&href));
                rest = &after[end..]; // leaves the closing quote for the next push
            }
            None => {
                // Malformed (no closing quote) — emit the remainder verbatim.
                out.push_str(after);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Rewrite site-relative image `src="assets/…"` refs to a root-absolute static
/// URL (depth-safe), leaving external / `data:` / already-absolute srcs alone.
/// A parallel of [`rewrite_hrefs`] for `src=` — needed because a nested page's
/// relative `assets/…` would otherwise resolve against the wrong directory.
fn rewrite_img_srcs(html: &str, peer_id: &str, site_id: &str, layout: Layout, prefix: &str) -> String {
    const NEEDLE: &str = "src=\"";
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(idx) = rest.find(NEEDLE) {
        let (before, after) = rest.split_at(idx + NEEDLE.len());
        out.push_str(before);
        match after.find('"') {
            Some(end) => {
                let raw = &after[..end];
                match raw.strip_prefix("assets/") {
                    Some(name) => {
                        out.push_str(&esc(&static_asset_href(name, peer_id, site_id, layout, prefix)))
                    }
                    None => out.push_str(raw), // external / data: — leave verbatim
                }
                rest = &after[end..];
            }
            None => {
                out.push_str(after);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Root-absolute URL for a site asset, resolvable from any page depth:
/// `/{prefix}/sites/{peer}/{site}/assets/{name}` (projection) or `/assets/{name}`
/// (bare-root). Mirrors [`projection_href`]/[`bare_href`] for assets.
fn static_asset_href(name: &str, peer_id: &str, site_id: &str, layout: Layout, prefix: &str) -> String {
    match layout {
        Layout::Projection => {
            let hp = super::paths::href_prefix(prefix);
            format!("{hp}/{SITE_URL_PREFIX}/{peer_id}/{site_id}/assets/{name}")
        }
        Layout::BareRoot => format!("/assets/{name}"),
    }
}

/// Filesystem path for a projection asset: `{out}/{prefix}/sites/{peer}/{site}/assets/{name}`.
fn asset_file_path(
    out_dir: &Path,
    peer_id: &str,
    site_id: &str,
    name: &str,
    prefix: &str,
) -> std::path::PathBuf {
    super::paths::prefixed_root(out_dir, prefix)
        .join(SITE_URL_PREFIX)
        .join(peer_id)
        .join(site_id)
        .join("assets")
        .join(name)
}

/// Map a classified link target to a static href under `layout`. Cross-site
/// and cross-peer always resolve to the projection path
/// `/sites/{peer}/{site}/{page}.html` (root-absolute, depth-safe); external
/// links pass through verbatim. Only the **in-site** href depends on layout:
/// projection keeps the prefixed path; bare-root drops to a root-relative
/// `/{page}.html` (the site IS the root).
/// **Both cross-* arms emit a host-less, root-absolute path**, so neither can
/// express a link to another *domain*. That is not an oversight to patch here:
/// there is no `peer_id → origin` input in scope, and inventing one per call
/// site is how two answers to one question ship. [`LinkAudit`] reports what
/// this cannot resolve; backlog **B-4** is where the input would come from.
fn static_href(target: &LinkTarget, current: &location::Location, ctx: LinkCtx) -> String {
    let cur_peer = current.peer_id.as_deref().unwrap_or("");
    let prefix = ctx.prefix;
    match target {
        LinkTarget::InSite { page } => match ctx.layout {
            Layout::Projection => projection_href(cur_peer, &current.site_id, page, prefix),
            Layout::BareRoot => bare_href(page),
        },
        LinkTarget::CrossSite { site_id, page } => projection_href(cur_peer, site_id, page, prefix),
        LinkTarget::CrossPeer { peer_id, site_id, page } => projection_href(peer_id, site_id, page, prefix),
        LinkTarget::External { url } => url.clone(),
    }
}

/// `/{prefix}/sites/{peer}/{site}/{page}.html`, or the site dir for an empty
/// page. The `{prefix}` segment is absent when empty (so a root deployment's
/// hrefs are byte-identical to before).
fn projection_href(peer_id: &str, site_id: &str, page: &str, prefix: &str) -> String {
    let hp = super::paths::href_prefix(prefix);
    if page.is_empty() {
        format!("{hp}/{SITE_URL_PREFIX}/{peer_id}/{site_id}/")
    } else {
        format!("{hp}/{SITE_URL_PREFIX}/{peer_id}/{site_id}/{page}.html")
    }
}

/// Bare-root in-site href: `/{page}.html`, or `/` for the site root.
fn bare_href(page: &str) -> String {
    if page.is_empty() {
        "/".to_string()
    } else {
        format!("/{page}.html")
    }
}

/// Write `{out}/sites/index.html` — the landing page listing EVERY exported
/// site across all peers, so the static tree is reachable without knowing the
/// publish peer-id. Placed under the `sites/` namespace (not `{out}/`) so it
/// never collides with a live SPA's `index.html` when both are served at one
/// origin. Each entry links to the site's projection root `/sites/{peer}/{site}/`.
fn write_root_index(
    out_dir: &Path,
    sites: &[ExportSite],
    prefix: &str,
    live_base: Option<&str>,
) -> std::io::Result<()> {
    let hp = super::paths::href_prefix(prefix);
    let mut items = String::new();
    for s in sites {
        items.push_str(&format!(
            "<li><a href=\"{hp}/{SITE_URL_PREFIX}/{peer}/{site}/\">{title}</a> \
             <span class=\"muted\">· {site} · {peer_short}…</span></li>",
            peer = esc(s.peer_id),
            site = esc(s.site_id),
            title = esc(&s.manifest.title),
            peer_short = esc(&s.peer_id.chars().take(12).collect::<String>()),
        ));
    }
    // When published with a live origin, point readers at the live app too.
    let live = live_base
        .map(|b| {
            format!(
                "<p class=\"muted\">This is a static snapshot. \
                 <a href=\"{}\">Open the live entity browser →</a></p>",
                esc(b)
            )
        })
        .unwrap_or_default();
    let html = format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>Published sites</title>\n<style>{css}</style>\n</head>\n<body>\n\
         <header class=\"site-header\"><span class=\"site-title\">Published sites</span></header>\n\
         <main class=\"page\">{live}<ul class=\"site-list\">{items}</ul></main>\n\
         <footer class=\"site-footer\">Static export · entity content-site projection</footer>\n\
         </body>\n</html>\n",
        css = page_css(None),
    );
    let dir = super::paths::prefixed_root(out_dir, prefix).join(SITE_URL_PREFIX);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("index.html"), html)
}

/// Write a root `{out}/index.html` that redirects to the `sites/` index —
/// **only if the output root has no `index.html` already**.
///
/// Two cases, one behavior:
/// - **Bare-dir preview / SSG projection** (e.g. `make publish-papers` into an
///   empty dir): without a root page, `/` falls through to the dev server's
///   directory listing. This lands the visitor on the sites index instead.
/// - **One-origin `site-serve`** (publish INTO the SPA's `dist/`): the WASM
///   build already wrote `dist/index.html`, so this is a **no-op** — the SPA
///   keeps `/`. The guard is what makes that safe; we never clobber the SPA.
///
/// The stub also **unregisters any service worker** on the origin. A prior SPA
/// deploy at the same `host:port` leaves an origin-scoped SW; served over a
/// plain static dir it keeps probing a now-absent `/sw.js` (404) and can shadow
/// the page with stale cache. Clearing it makes the preview self-heal rather
/// than inherit a previous deploy's worker.
fn write_landing_redirect(out_dir: &Path, prefix: &str) -> std::io::Result<()> {
    let path = out_dir.join("index.html");
    if path.exists() {
        return Ok(()); // a live SPA (or anything) already owns `/` — leave it.
    }
    // The published sites index lives at `{prefix}/sites/` (just `sites/` at
    // root). Redirect `/` there from the output root (a relative target, so it
    // works regardless of where the dir is served).
    let target = if prefix.is_empty() {
        format!("./{SITE_URL_PREFIX}/")
    } else {
        format!("./{prefix}/{SITE_URL_PREFIX}/")
    };
    let html = format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n\
         <meta charset=\"utf-8\">\n\
         <meta http-equiv=\"refresh\" content=\"0; url={target}\">\n\
         <title>Published sites</title>\n\
         <script>\n\
         if ('serviceWorker' in navigator) {{ navigator.serviceWorker.getRegistrations()\
         .then(function(rs){{ rs.forEach(function(r){{ r.unregister(); }}); }}); }}\n\
         location.replace('{target}');\n\
         </script>\n</head>\n<body>\n\
         <p>Redirecting to the <a href=\"{target}\">published sites</a>…</p>\n\
         </body>\n</html>\n",
    );
    fs::write(path, html)
}

/// Write `{out}/{prefix}/sites/{peer}/index.html` listing this peer's sites.
fn write_peer_index(
    out_dir: &Path,
    peer_id: &str,
    sites: &[ExportSite],
    prefix: &str,
) -> std::io::Result<()> {
    let hp = super::paths::href_prefix(prefix);
    let mut items = String::new();
    for s in sites.iter().filter(|s| s.peer_id == peer_id) {
        items.push_str(&format!(
            "<li><a href=\"{hp}/{SITE_URL_PREFIX}/{peer}/{site}/\">{title}</a></li>",
            peer = esc(peer_id),
            site = esc(s.site_id),
            title = esc(&s.manifest.title),
        ));
    }
    let html = format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>Sites — {peer}</title>\n<style>{css}</style>\n</head>\n<body>\n\
         <header class=\"site-header\"><span class=\"site-title\">Sites hosted by {peer}</span></header>\n\
         <main class=\"page\"><ul class=\"site-list\">{items}</ul></main>\n\
         <footer class=\"site-footer\">Static export · entity content-site projection</footer>\n\
         </body>\n</html>\n",
        peer = esc(peer_id),
        css = page_css(None),
    );
    let path = super::paths::prefixed_root(out_dir, prefix)
        .join(SITE_URL_PREFIX)
        .join(peer_id)
        .join("index.html");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, html)
}

/// Minimal HTML-attribute/text escaper (the bodies are already escaped by
/// the markdown renderer; this guards the values we interpolate ourselves —
/// titles, labels, hrefs).
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Self-contained stylesheet for exported pages: the page chrome (header /
/// nav / footer) with its palette **derived** from the site's effective own
/// palette (`doc_css::frozen` — the `SITE_TOKENS` defaults, or a
/// manifest-declared registered theme's resolved values), plus the shared
/// content-document rules (`doc_css::doc_css`) in their frozen-literal form
/// — the SAME rule table the live overlay renders as `var(--site-*, …)`, so
/// a published page and the in-app overlay cannot drift (S-T1/S-T2).
/// Published sites carry the site's own palette to other people's browsers;
/// no runtime token layer. Index pages (cross-site surfaces, not any one
/// site's) pass `None` — the default palette.
///
/// The live-mirror banner keeps its own fixed palette — a deliberately
/// distinct notice surface (own bg + own text), not part of the site theme.
///
/// **The nav-group marker is DRAWN WITH BORDERS, never a glyph — and the
/// reason is that `font-size` does not size a glyph.** The first fix for
/// "the arrows are tiny" replaced the native `<details>` marker with
/// `content:"\25BE"` at `font-size:13px`, and the operator reported back
/// that it was *still a tiny little triangle*. They were right, and it was
/// not a browser or font artifact: U+25BE is BLACK DOWN-POINTING **SMALL**
/// TRIANGLE — a subscript-sized mark that occupies a fraction of its em box,
/// so raising `font-size` scales the box the glyph sits in and barely moves
/// the ink. A 8×8 element with two 2px borders rotated 45° is sized in the
/// units the complaint was about, renders identically in every font, and
/// stays crisp. Generally: **if a control's size is the property under
/// review, do not express it as a character.**
///
/// The second half of that report was *"it's hard to see what the hitbox
/// is"* — a separate defect, and the more accurate one. The whole
/// `<summary>` has always toggled (everything but the `<a>`), but nothing
/// said so: no padding, no hover feedback, so the only thing that looked
/// pressable was the 10px mark. The summary is a pill now — hover and
/// `[open]` both raise a background and a border, so the target advertises
/// its own size. **An affordance that is bigger than it looks is a defect
/// even when every click lands.**
fn page_css(theme: Option<&'static crate::theme_tokens::Theme>) -> String {
    use super::doc_css::{doc_css, PaletteMode};
    let frozen = |token: &str| super::doc_css::frozen(theme, token);
    format!(
        ":root{{color-scheme:{scheme}}}\
         *{{box-sizing:border-box}}\
         body{{margin:0;background:{bg};color:{text};font:15px/1.6 system-ui,sans-serif}}\
         a{{color:{link};text-decoration:none}}a:hover{{text-decoration:underline}}\
         .site-header{{display:flex;flex-wrap:wrap;align-items:baseline;gap:16px;\
         padding:14px 20px;background:{nav_bg};border-bottom:1px solid {border};\
         position:relative}}\
         .site-title{{font-size:18px;font-weight:700;color:{text}}}\
         .site-nav>ul{{list-style:none;display:flex;flex-wrap:wrap;align-items:flex-start;\
         gap:6px 22px;margin:0;padding:0}}\
         .site-nav li{{display:flex;flex-direction:column;align-items:flex-start;gap:3px}}\
         .site-nav ul ul{{list-style:none;display:flex;flex-direction:column;\
         align-items:flex-start;gap:3px;margin:3px 0 0;padding:0}}\
         .site-nav ul ul a{{font-size:13px;color:{muted2}}}\
         .site-nav ul ul a:hover{{color:{link}}}\
         .site-nav details.nav-group>summary{{cursor:pointer;list-style:none;\
         display:flex;align-items:center;gap:9px;padding:5px 10px;\
         border:1px solid transparent;border-radius:7px}}\
         .site-nav details.nav-group>summary:hover,\
         .site-nav details.nav-group[open]>summary{{background:{panel};border-color:{border}}}\
         .site-nav details.nav-group>summary:focus-visible{{outline:2px solid {link};\
         outline-offset:2px}}\
         .site-nav details.nav-group>summary::-webkit-details-marker{{display:none}}\
         .site-nav details.nav-group>summary::after{{content:\"\";flex:none;\
         width:9px;height:9px;border-right:2px solid {muted};\
         border-bottom:2px solid {muted};transform:translateY(-3px) rotate(45deg);\
         transition:transform .12s ease,border-color .12s ease}}\
         .site-nav details.nav-group[open]>summary::after{{\
         transform:translateY(2px) rotate(-135deg);border-color:{accent}}}\
         .site-nav details.nav-group>summary:hover::after{{border-color:{link}}}\
         .site-nav details.nav-group>summary.here{{color:{accent};font-weight:600}}\
         .site-nav details.nav-group>summary.here>a{{color:{accent}}}\
         .site-nav details.nav-group>ul{{position:absolute;left:0;right:0;top:100%;\
         z-index:20;display:block;column-width:190px;column-gap:26px;\
         margin:0;padding:16px 20px;background:{nav_bg};\
         border-bottom:1px solid {border};box-shadow:0 14px 30px rgba(0,0,0,.45);\
         max-height:60vh;overflow:auto}}\
         .site-nav details.nav-group>ul li{{display:block;break-inside:avoid;margin:0 0 7px}}\
         .site-nav details.nav-group>ul a{{font-size:13.5px}}\
         .site-nav a.active{{color:{accent};font-weight:600}}\
         .nav-section{{color:{muted2};font-size:12px;text-transform:uppercase;letter-spacing:.05em}}\
         main.page{{max-width:760px;margin:0 auto;padding:28px 20px}}\
         {doc}\
         .site-list{{list-style:none;padding:0}}.site-list li{{margin:8px 0;font-size:17px}}\
         .muted{{color:{muted2};font-size:13px}}\
         .site-footer{{max-width:760px;margin:0 auto;padding:20px;color:{faint2};font-size:12px;\
         border-top:1px solid {border}}}\
         .live-banner-toggle{{position:absolute;opacity:0;pointer-events:none}}\
         .live-banner{{display:flex;align-items:center;justify-content:center;gap:14px;\
         padding:8px 16px;background:#16213e;border-bottom:1px solid #2a3a5e;\
         color:#cdd6f4;font-size:13px}}\
         .live-banner a{{color:{link};font-weight:600}}\
         .live-banner-dismiss{{cursor:pointer;color:#8892b0;font-size:18px;line-height:1;\
         padding:0 4px;user-select:none}}\
         .live-banner-toggle:checked + .live-banner{{display:none}}",
        scheme = theme.map(|t| t.scheme).unwrap_or("dark"),
        bg = frozen("--site-bg"),
        text = frozen("--site-text"),
        link = frozen("--site-link"),
        nav_bg = frozen("--site-nav-bg"),
        border = frozen("--site-border"),
        accent = frozen("--site-accent"),
        muted2 = frozen("--site-text-muted-2"),
        muted = frozen("--site-text-muted"),
        panel = frozen("--site-panel-bg"),
        faint2 = frozen("--site-text-faint-2"),
        doc = doc_css("main.page", PaletteMode::Frozen(theme)),
    )
}

#[cfg(test)]
mod nav_layout_tests {
    use super::*;

    /// A nested nav must render as a **tree**, and the stylesheet must lay that
    /// tree out as columns rather than flattening it into one row.
    ///
    /// **The bug this pins, reported from the outside as "the menu items just
    /// shoot off across the top".** The markup was always correct — a child
    /// `<ul>` nested inside its parent's `<li>`. The stylesheet was not: both
    /// `.site-nav ul` and `.site-nav li` were `display:flex` **unscoped by
    /// depth**, so a nested `<ul>` became a flex item laid out *beside* its
    /// parent label. Every descendant collapsed onto one line and a group
    /// header was indistinguishable from its own children.
    ///
    /// Measured in Firefox on a generated site, before and after, with only the
    /// stylesheet varying — a 4-item / 2-group nav went from **8 links on one
    /// 27px row** (and not even baseline-aligned: y=17 for leaves, y=20 for
    /// parents) to three rows with children x-aligned under their parent at a
    /// smaller muted size.
    ///
    /// **This is a spelling check standing in for a layout property**, and it is
    /// worth being honest that the real property is only observable in a
    /// browser: nothing here proves the page *renders* correctly. What it does
    /// prove is that the two rules whose absence caused the bug are still
    /// present and still scoped — a later edit that re-broadens `.site-nav ul`
    /// back to every depth fails here, which is exactly how the bug was
    /// introduced.
    #[test]
    fn a_nested_nav_renders_as_a_tree_and_the_css_lays_it_out_as_columns() {
        let nav = vec![
            NavItem::new("Home", "/index"),
            NavItem::section(
                "Research",
                "/research",
                vec![NavItem::new("Glossary", "/research/glossary")],
            ),
        ];
        let loc = location::Location::site_root("demo");
        let ctx = LinkCtx { layout: Layout::BareRoot, prefix: "", audit: None };
        let html = render_nav(&nav, &loc, "index", ctx);

        // The child list is INSIDE its parent's <li> — the tree, not a sibling.
        let research = html.find("Research").expect("parent rendered");
        let child_ul = html[research..].find("<ul>").expect("child <ul> rendered");
        let parent_li_end = html[research..].find("</li>").expect("parent <li> closes");
        assert!(
            child_ul < parent_li_end,
            "the child <ul> must nest inside its parent <li>, not follow it: {html}"
        );

        // A group is a native `<details>` — collapsible with NO JavaScript,
        // which is the property the whole static export exists for.
        assert!(
            html.contains("<details class=\"nav-group\"><summary"),
            "a group must render as a <details> disclosure: {html}"
        );
        // ...and it must ship CLOSED. The panel is absolutely positioned, so an
        // `open` group would cover the article on arrival, and without JS the
        // reader cannot dismiss it by clicking away. Mutating this to `open` is
        // the regression this line exists to catch.
        assert!(
            !html.contains("<details class=\"nav-group\" open"),
            "groups must ship closed — an open floating panel covers the page: {html}"
        );
        // The leaf with no children is NOT wrapped in a disclosure.
        let home = html.find("Home").expect("leaf rendered");
        assert!(
            html[..home].rfind("<details").is_none(),
            "a childless nav item must not become a group: {html}"
        );

        let css = page_css(None);
        // The horizontal row is scoped to the TOP level. Unscoped is the bug.
        assert!(
            css.contains(".site-nav>ul{list-style:none;display:flex"),
            "top-level nav row must be scoped with `>`: {css}"
        );
        // ...and every nested level stacks.
        assert!(
            css.contains(".site-nav ul ul{") && css.contains("flex-direction:column"),
            "nested nav lists must stack in a column: {css}"
        );
        // A bare `.site-nav ul{...display:flex` with no `>` is the regression.
        assert!(
            !css.contains(".site-nav ul{list-style:none;display:flex"),
            "an unscoped `.site-nav ul` flex rule reintroduces the flattening: {css}"
        );
    }

    /// The group holding the page being rendered is **marked**, not opened.
    ///
    /// This is the one thing a static exporter can do that the live app cannot
    /// — the current page is known at render time — and the whole value of it
    /// is lost if the marking is silently dropped, because then every group
    /// looks identical and the reader has to open three of them to find where
    /// they already are. Asserted on a page that is a **child** of the group,
    /// since that is the case the recursion exists for.
    #[test]
    fn the_group_holding_the_current_page_is_marked_but_not_opened() {
        let nav = vec![
            NavItem::new("Home", "/index"),
            NavItem::section(
                "Guides",
                "/guides",
                vec![NavItem::new("Start", "/guides/start")],
            ),
            NavItem::section(
                "Reference",
                "/reference",
                vec![NavItem::new("Tree", "/reference/tree")],
            ),
        ];
        let loc = location::Location::site_root("demo");
        let ctx = LinkCtx { layout: Layout::BareRoot, prefix: "", audit: None };
        let html = render_nav(&nav, &loc, "guides/start", ctx);

        assert!(
            html.contains("<summary class=\"here\"><a href=\"/guides.html\">Guides</a>"),
            "the group containing the current page must be marked: {html}"
        );
        // Exactly one — marking every group is the same as marking none.
        assert_eq!(
            html.matches("class=\"here\"").count(),
            1,
            "only the group holding the current page is marked: {html}"
        );
        // Marked, still closed.
        assert!(!html.contains(" open>") && !html.contains(" open "), "must stay closed: {html}");
    }

    /// The disclosure marker is **drawn**, and the group header is a **target**.
    ///
    /// Both halves are one operator report, made twice about the same control:
    /// *"it's still a tiny little triangle … it's hard to see what the hitbox
    /// is on it."* The first round of this fix hid the native marker and drew
    /// its own with `content:"\25BE"` at `font-size:13px` — which changed
    /// nothing visible, because U+25BE is BLACK DOWN-POINTING **SMALL**
    /// TRIANGLE and `font-size` sizes the em box, not the ink inside it. So
    /// the property this pins is not "there is a marker" (the broken version
    /// had one) but **"its size is expressed in the units the complaint was
    /// about"**: an empty `content` plus explicit `width`/`height`.
    ///
    /// It is a spelling check standing in for a rendered property, and it says
    /// so — the honest gate is a browser, which this export still has none of.
    /// What it *can* do is fail the specific regression that already shipped
    /// once: someone reaching for a character again.
    #[test]
    fn the_disclosure_marker_is_drawn_rather_than_typed() {
        let css = page_css(None);
        let marker = css
            .split(".site-nav details.nav-group>summary::after{")
            .nth(1)
            .and_then(|s| s.split('}').next())
            .expect("the nav group marker rule must exist");

        // Empty content + an explicit box: sized in px, in every font.
        assert!(
            marker.contains("content:\"\"") && marker.contains("width:") && marker.contains("height:"),
            "the marker must be a drawn box, not a glyph: {marker}"
        );
        // The regression, by name. A font-sized marker is the bug that shipped.
        assert!(
            !marker.contains("font-size"),
            "font-size does not size a glyph — that is what looked unchanged: {marker}"
        );

        // The header advertises its own hit area: hover and [open] both paint.
        assert!(
            css.contains(
                ".site-nav details.nav-group>summary:hover,\
                 .site-nav details.nav-group[open]>summary{background:"
            ),
            "the group header must show its target on hover and when open: {css}"
        );
        // ...and it is reachable by keyboard, visibly.
        assert!(
            css.contains(".site-nav details.nav-group>summary:focus-visible{outline:"),
            "a summary is focusable — the focus must be visible: {css}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two tiny sites on one peer, cross-linked both ways — the multi-site
    /// smoke test (cross-site link rewriting is where a static exporter
    /// breaks). Builds, exports to a temp dir, asserts the file layout, the
    /// rendered bodies, and that every link form rewrote to a static href.
    fn two_demo_sites() -> (SiteManifest, Vec<(&'static str, SitePage)>, SiteManifest, Vec<(&'static str, SitePage)>)
    {
        let demo_manifest = SiteManifest::new(
            "demo",
            "Entity Demo",
            "index",
            vec![NavItem::new("Home", "/index"), NavItem::new("About", "/about")],
        );
        let demo_pages = vec![
            (
                "index",
                SitePage::markdown(
                    "Welcome",
                    "# Welcome\n\nA tiny demo site. Read [About](./about), or visit the \
                     [Entity Info](site:entity-info/index) site.",
                ),
            ),
            ("about", SitePage::markdown("About", "# About\n\nBack to [Home](./index).")),
        ];

        let info_manifest = SiteManifest::new(
            "entity-info",
            "Entity Info",
            "index",
            vec![NavItem::new("Overview", "/index")],
        );
        let info_pages = vec![
            (
                "index",
                SitePage::markdown(
                    "What is the entity system?",
                    "# Entity System\n\nA content-addressed tree projected onto the web. \
                     Back to the [Demo](site:demo/index).",
                ),
            ),
        ];
        (demo_manifest, demo_pages, info_manifest, info_pages)
    }

    /// **Live-tree emitter** — the honest [A]→[B1] path. Seeds two
    /// cross-linked sites into a *real* `Peers` tree (the bundled deep demo
    /// + a second `entity-info` site that links into it), reads them back
    /// off the tree via [`read::read_all_sites`] (NOT in-memory fixtures),
    /// and exports the result. Proves multi-site browse / switch /
    /// click-through / cross-link works off the live tree as designed.
    ///
    /// Produces `dist/static-demo/` for eyeballing in a browser (serve
    /// `dist/` and open `/static-demo/sites/<PEER>/`). On demand, not in
    /// the suite.
    ///
    /// `cargo test --bin entity-browser emit_live_tree_demo -- --ignored --nocapture`
    #[test]
    #[ignore = "demo emitter; produces dist/static-demo, not a unit assertion"]
    fn emit_live_tree_demo() {
        use crate::content_site::publish::seed_demo_site_set;
        use crate::content_site::read;
        use crate::peers::Peers;

        // A real (Direct-arm, in-memory) peer tree — a live tree, just not
        // durable. Seed the demo site SET (bundled deep demo + a second site
        // cross-linking into it) via the same seeder `make site` uses, so
        // the emitter and the CLI exercise identical data.
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        seed_demo_site_set(&peers, &pid);

        // Read EVERY site off the live tree, then project to static HTML.
        let sites = read::read_all_sites(&peers, &pid);
        eprintln!(
            "read {} site(s) off the live tree: {:?}",
            sites.len(),
            sites.iter().map(|s| (&s.site_id, s.pages.len())).collect::<Vec<_>>()
        );

        let dir = Path::new("dist/static-demo");
        let _ = fs::remove_dir_all(dir);
        let n = export_owned_sites(dir, &sites, "", None).expect("live-tree export");
        eprintln!("emitted {} static pages → {} (peer {pid})", n.pages, dir.display());
    }

    #[test]
    fn exports_two_sites_with_cross_links_rewritten() {
        let (dm, dp, im, ip) = two_demo_sites();
        let sites = [
            ExportSite { peer_id: "PEER1", site_id: "demo", manifest: &dm, pages: &dp, assets: &[] },
            ExportSite { peer_id: "PEER1", site_id: "entity-info", manifest: &im, pages: &ip, assets: &[] },
        ];

        let dir = std::env::temp_dir().join("entity-browser-static-export-test");
        let _ = fs::remove_dir_all(&dir);
        let n = export_site_set(&dir, &sites, "", None).expect("export writes");
        assert_eq!(n.pages, 3, "two demo pages + one info page");
        // Both sites are in the set, so every cross-site link resolves — this
        // is the in-set control for `an_out_of_set_site_link_is_reported`.
        assert!(n.dangling.is_empty(), "a complete set has no out-of-set links: {:?}", n.dangling);

        // Projection file layout.
        let demo_index = dir.join("sites/PEER1/demo/index.html");
        let demo_about = dir.join("sites/PEER1/demo/about.html");
        let info_index = dir.join("sites/PEER1/entity-info/index.html");
        assert!(demo_index.exists() && demo_about.exists() && info_index.exists());

        let demo_html = fs::read_to_string(&demo_index).unwrap();
        // Body rendered (markdown → HTML).
        assert!(demo_html.contains("<h1>Welcome</h1>"));
        // In-site link `./about` → root-absolute projection .html.
        assert!(
            demo_html.contains(r#"href="/sites/PEER1/demo/about.html""#),
            "in-site link not rewritten: {demo_html}"
        );
        // Cross-site `site:entity-info/index` → the OTHER site's projection path.
        assert!(
            demo_html.contains(r#"href="/sites/PEER1/entity-info/index.html""#),
            "cross-site link not rewritten: {demo_html}"
        );
        // No entity-native link form leaked into the static output.
        assert!(!demo_html.contains("site:"), "raw site: link leaked: {demo_html}");
        assert!(!demo_html.contains("entity://"), "raw entity:// link leaked: {demo_html}");

        // The reverse cross-link resolves back to the demo site.
        let info_html = fs::read_to_string(&info_index).unwrap();
        assert!(
            info_html.contains(r#"href="/sites/PEER1/demo/index.html""#),
            "reverse cross-site link not rewritten: {info_html}"
        );

        // Per-peer multi-site index lists both sites.
        let peer_index = fs::read_to_string(dir.join("sites/PEER1/index.html")).unwrap();
        assert!(peer_index.contains("Entity Demo") && peer_index.contains("Entity Info"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifest_theme_freezes_that_palette_per_site() {
        // S-T2 export parity: a site whose manifest declares a registered
        // theme exports with THAT theme's frozen palette; a sibling site
        // without one keeps the default; the cross-site index pages (not any
        // one site's surface) keep the default too. Values must equal the
        // live strict-override resolution (`site_token_value` — one rule).
        use crate::theme_tokens::{site_token_value, LIGHT};
        let (mut dm, dp, im, ip) = two_demo_sites();
        dm.params.insert("theme".into(), "light".into());
        let sites = [
            ExportSite { peer_id: "PEER1", site_id: "demo", manifest: &dm, pages: &dp, assets: &[] },
            ExportSite { peer_id: "PEER1", site_id: "entity-info", manifest: &im, pages: &ip, assets: &[] },
        ];

        let dir = std::env::temp_dir().join("entity-browser-manifest-theme-test");
        let _ = fs::remove_dir_all(&dir);
        export_site_set(&dir, &sites, "", None).expect("export writes");

        let light_bg = site_token_value(&LIGHT, "--site-bg");
        let light_scheme = format!("color-scheme:{}", LIGHT.scheme);
        let default_bg =
            format!("background:{}", crate::content_site::doc_css::frozen(None, "--site-bg"));

        let themed = fs::read_to_string(dir.join("sites/PEER1/demo/index.html")).unwrap();
        assert!(themed.contains(&format!("background:{light_bg}")), "themed site: light bg");
        assert!(themed.contains(&light_scheme), "themed site: light color-scheme");
        assert!(!themed.contains("var("), "exported CSS stays self-contained");

        let plain = fs::read_to_string(dir.join("sites/PEER1/entity-info/index.html")).unwrap();
        assert!(plain.contains(&default_bg), "unthemed sibling keeps the default palette");

        let peer_index = fs::read_to_string(dir.join("sites/PEER1/index.html")).unwrap();
        assert!(peer_index.contains(&default_bg), "index pages keep the default palette");

        // An UNKNOWN theme name exports as today (warn is log-side).
        dm.params.insert("theme".into(), "lab".into());
        let sites =
            [ExportSite { peer_id: "PEER1", site_id: "demo", manifest: &dm, pages: &dp, assets: &[] }];
        let _ = fs::remove_dir_all(&dir);
        export_site_set(&dir, &sites, "", None).expect("export writes");
        let fallback = fs::read_to_string(dir.join("sites/PEER1/demo/index.html")).unwrap();
        assert!(fallback.contains(&default_bg), "unknown theme falls back to the default");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bare_root_renders_single_site_at_domain_root() {
        use crate::content_site::read::OwnedSite;

        // A small site with a nested page + an in-site, a cross-site, and an
        // external link — the three href classes bare-root must handle.
        let manifest = SiteManifest::new(
            "demo",
            "Demo",
            "index",
            vec![NavItem::new("Home", "/index"), NavItem::new("Deep", "/guide/intro")],
        );
        let site = OwnedSite {
            peer_id: "PEER1".into(),
            site_id: "demo".into(),
            manifest,
            pages: vec![
                (
                    "index".into(),
                    SitePage::markdown(
                        "Home",
                        "# Home\n\nGo [deep](./guide/intro), [out](site:other/x), or to [the web](https://example.com).",
                    ),
                ),
                ("guide/intro".into(), SitePage::markdown("Intro", "# Intro\n\nBack [home](../index).")),
            ],
            assets: Vec::new(),
        };

        let dir = std::env::temp_dir().join("entity-browser-bare-root-test");
        let _ = fs::remove_dir_all(&dir);
        let n = export_bare_root(&dir, &site, None).expect("bare-root export");
        assert_eq!(n, 2);

        // Files land at the ROOT — no sites/{peer}/{site}/ prefix.
        let index = dir.join("index.html");
        let deep = dir.join("guide/intro.html");
        assert!(index.exists() && deep.exists(), "bare-root files not at root");
        assert!(!dir.join("sites").exists(), "bare-root must not emit the projection prefix");

        let index_html = fs::read_to_string(&index).unwrap();
        // In-site link → root-relative `/{page}.html`, NOT the projection path.
        assert!(index_html.contains(r#"href="/guide/intro.html""#), "in-site bare href: {index_html}");
        assert!(
            !index_html.contains("/sites/PEER1/demo/"),
            "in-site link kept the projection prefix: {index_html}"
        );
        // Site-title home link is the root `/` (correct from the root page too).
        assert!(index_html.contains(r#"href="/""#), "home link not root: {index_html}");
        // External link passes through.
        assert!(index_html.contains(r#"href="https://example.com""#));
        // Cross-site link still resolves to the projection path (the documented
        // bare-root behavior — namespaced outbound, not a wrong root link).
        assert!(index_html.contains(r#"href="/sites/PEER1/other/x.html""#), "cross-site: {index_html}");
        // No entity branding in the footer.
        assert!(!index_html.contains("entity content-site projection"), "branding leaked: {index_html}");

        // A nested page's in-site link is ALSO root-absolute (depth-safe).
        let deep_html = fs::read_to_string(&deep).unwrap();
        assert!(deep_html.contains(r#"href="/index.html""#), "nested in-site bare href: {deep_html}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_banner_injected_only_when_live_base_given() {
        let m = SiteManifest::new("demo", "Demo", "index", vec![]);
        let pages = vec![("about", SitePage::markdown("About", "# About"))];
        let sites = [ExportSite { peer_id: "PEER1", site_id: "demo", manifest: &m, pages: &pages, assets: &[] }];
        let dir = std::env::temp_dir().join("entity-browser-banner-test");

        // With a live base → banner + deep link to this exact page.
        let _ = fs::remove_dir_all(&dir);
        export_site_set(&dir, &sites, "", Some("https://live.test")).unwrap();
        let html = fs::read_to_string(dir.join("sites/PEER1/demo/about.html")).unwrap();
        assert!(html.contains("class=\"live-banner\""), "banner missing: {html}");
        // Deep-links to the REAL publish peer-id (PEER1), so the live SPA's
        // `?site=` boot HTTP-polls `/{peer}/sites/…` and resolves the actual
        // published content — NOT the `self` sentinel (which resolves to the
        // SPA's own system peer and 404s for foreign/ingested content).
        assert!(
            html.contains(r#"href="https://live.test/?site=PEER1/demo/about""#),
            "deep link missing/wrong: {html}"
        );
        // No-JS dismiss: the CSS checkbox toggle is present.
        assert!(html.contains(r#"id="live-banner-x""#));

        // Without a live base → no banner at all.
        let _ = fs::remove_dir_all(&dir);
        export_site_set(&dir, &sites, "", None).unwrap();
        let plain = fs::read_to_string(dir.join("sites/PEER1/demo/about.html")).unwrap();
        // The element (not the always-present CSS) must be absent.
        assert!(!plain.contains(r#"id="live-banner-x""#), "banner element leaked: {plain}");
        assert!(!plain.contains("<aside class=\"live-banner\""), "banner element leaked: {plain}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn external_links_pass_through_unrewritten() {
        let m = SiteManifest::new("s", "S", "index", vec![]);
        let pages =
            vec![("index", SitePage::markdown("H", "[ext](https://example.com) and [m](mailto:a@b.c)"))];
        let sites = [ExportSite { peer_id: "P", site_id: "s", manifest: &m, pages: &pages, assets: &[] }];
        let dir = std::env::temp_dir().join("entity-browser-static-export-ext-test");
        let _ = fs::remove_dir_all(&dir);
        export_site_set(&dir, &sites, "", None).unwrap();
        let html = fs::read_to_string(dir.join("sites/P/s/index.html")).unwrap();
        assert!(html.contains(r#"href="https://example.com""#));
        assert!(html.contains(r#"href="mailto:a@b.c""#));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn raw_html_in_body_stays_neutralized_through_export() {
        // The render path escapes raw HTML; export must not re-introduce it.
        let m = SiteManifest::new("s", "S", "index", vec![]);
        let pages = vec![("index", SitePage::markdown("H", "<script>alert(1)</script>\n\nsafe"))];
        let sites = [ExportSite { peer_id: "P", site_id: "s", manifest: &m, pages: &pages, assets: &[] }];
        let dir = std::env::temp_dir().join("entity-browser-static-export-xss-test");
        let _ = fs::remove_dir_all(&dir);
        export_site_set(&dir, &sites, "", None).unwrap();
        let html = fs::read_to_string(dir.join("sites/P/s/index.html")).unwrap();
        assert!(!html.contains("<script>alert"), "raw script leaked into export: {html}");
        assert!(html.contains("&lt;script&gt;"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_html_page_exports_verbatim_and_skips_the_template() {
        // A `format:html` page is ALREADY a standalone document. Wrapping it in
        // the export template would nest <html> inside <body> and leak its
        // stylesheet onto our chrome, and the href/img rewriters would rewrite
        // links that were never site-relative. So it is written out byte-for-
        // byte — file-on-disk == body-in-tree, which is also what a consumer
        // fetching the permalink gets.
        let doc = "<!DOCTYPE html>\n<html lang=\"en\">\n<head><title>Paper 0</title>\n\
                   <style>body{font-family:serif}</style></head>\n\
                   <body><h1>Paper 0</h1><p>See <a href=\"./other.html\">other</a> \
                   and <img src=\"figures/x.png\" alt=\"f\"></p></body>\n</html>\n";
        let m = SiteManifest::new("s", "S", "index", vec![]);
        let pages = vec![
            ("index", SitePage::markdown("H", "# Home\n\nsee [paper](./paper)")),
            ("paper", SitePage::html("Paper", doc)),
        ];
        let sites = [ExportSite { peer_id: "P", site_id: "s", manifest: &m, pages: &pages, assets: &[] }];
        let dir = std::env::temp_dir().join("entity-browser-static-export-doc-test");
        let _ = fs::remove_dir_all(&dir);
        export_site_set(&dir, &sites, "", None).unwrap();

        let out = fs::read_to_string(dir.join("sites/P/s/paper.html")).unwrap();
        assert_eq!(out, doc, "a document page must export byte-for-byte");
        // The template's furniture must be absent — its presence would mean the
        // document got wrapped (two <html> elements in one file).
        assert!(!out.contains("site-header"), "export template wrapped a document: {out}");
        assert!(!out.contains("site-footer"), "export template wrapped a document: {out}");
        // And its own links/images are untouched by the rewriters.
        assert!(out.contains(r#"href="./other.html""#), "an href was rewritten: {out}");
        assert!(out.contains(r#"src="figures/x.png""#), "an img src was rewritten: {out}");

        // The markdown page beside it still gets the full treatment — the
        // change is per-page, not a mode switch for the whole site.
        let idx = fs::read_to_string(dir.join("sites/P/s/index.html")).unwrap();
        assert!(idx.contains("site-header"), "the markdown page lost its chrome: {idx}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bare_section_dirs_get_generated_index_pages() {
        // A site whose pages live under sections that have NO page of their
        // own (`guide`, `guide/advanced`). The live overlay generates a
        // section index on the fly; the static export must emit the same so a
        // link to the bare section doesn't 404.
        let m = SiteManifest::new("docs", "Docs", "guide/intro", vec![]);
        let pages = vec![
            ("guide/intro", SitePage::markdown("Intro", "# Intro\n\nSee [deep](advanced/deep).")),
            ("guide/advanced/deep", SitePage::markdown("Deep", "# Deep")),
        ];
        let sites = [ExportSite { peer_id: "P", site_id: "docs", manifest: &m, pages: &pages, assets: &[] }];
        let dir = std::env::temp_dir().join("entity-browser-static-export-section-test");
        let _ = fs::remove_dir_all(&dir);
        export_site_set(&dir, &sites, "", None).unwrap();

        // Both intermediate sections got an index page.
        let guide_idx = dir.join("sites/P/docs/guide.html");
        let adv_idx = dir.join("sites/P/docs/guide/advanced.html");
        assert!(guide_idx.exists(), "missing generated index for `guide`");
        assert!(adv_idx.exists(), "missing generated index for `guide/advanced`");

        // The `guide` index lists its children as root-absolute links to real files.
        let guide_html = fs::read_to_string(&guide_idx).unwrap();
        assert!(
            guide_html.contains(r#"href="/sites/P/docs/guide/intro.html""#),
            "section index should link to its child page: {guide_html}"
        );
        assert!(
            guide_html.contains(r#"href="/sites/P/docs/guide/advanced.html""#),
            "section index should link to its child section: {guide_html}"
        );

        // section_dirs is exact: only proper ancestors, deduped + sorted.
        assert_eq!(
            section_dirs(&["guide/intro".into(), "guide/advanced/deep".into()]),
            vec!["guide".to_string(), "guide/advanced".to_string()]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn intra_domain_cross_site_link_projects_to_sibling_site() {
        // The canonical intra-domain cross-site case (APP-CONVENTION-SEMANTIC-
        // CONTENT-SITE §4 `link-ref` + §11 URL projection): two sites under ONE
        // peer; a `site:{other}/{page}` body link from site A must project to
        // site B's URL `/sites/{peer}/{other}/{page}.html` — same peer, different
        // site. This is the form arch settled on; papers emit it
        // for cross-site links and we resolve it here. Permanent ratchet so the
        // intra-domain loop can't silently regress (papers-independent).
        let a = SiteManifest::new("alpha", "Alpha", "index", vec![]);
        let b = SiteManifest::new("beta", "Beta", "index", vec![]);
        let a_pages =
            vec![("index", SitePage::markdown("A", "See [Beta home](site:beta/index)."))];
        let b_pages = vec![("index", SitePage::markdown("B", "# Beta"))];
        let sites = [
            ExportSite { peer_id: "PEER", site_id: "alpha", manifest: &a, pages: &a_pages, assets: &[] },
            ExportSite { peer_id: "PEER", site_id: "beta", manifest: &b, pages: &b_pages, assets: &[] },
        ];
        let dir = std::env::temp_dir().join("entity-browser-static-export-xsite-test");
        let _ = fs::remove_dir_all(&dir);
        export_site_set(&dir, &sites, "", None).unwrap();

        // The cross-site link projects to the SIBLING site under the SAME peer…
        let alpha_html = fs::read_to_string(dir.join("sites/PEER/alpha/index.html")).unwrap();
        assert!(
            alpha_html.contains(r#"href="/sites/PEER/beta/index.html""#),
            "cross-site `site:` link should project to the sibling site: {alpha_html}"
        );
        // …and the target it points at actually exists (no dangling projection).
        assert!(dir.join("sites/PEER/beta/index.html").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    /// **The case that ships, and had no test.** Measured on production
    /// 2026-08-21: `entitychurchfoundation.org` served `site:` links to sites
    /// living on *other* domains, rewritten under foundation's own peer, 404.
    ///
    /// The sibling test above exports both sites, so resolving against the
    /// current peer *is* right there — which is exactly why it stayed green
    /// through the whole defect. The untested precondition was the one the
    /// deployment model deliberately violates: **each domain carries only its
    /// own sites** (operator decision `cgid-10-235`), so a `site:` target can
    /// be absent from the set.
    ///
    /// Two properties, and the second is the one that makes this a gate rather
    /// than a restatement: the out-of-set target is **reported**, and the
    /// in-set one beside it is **not**. A detector that flagged everything
    /// would satisfy the first assertion alone and be useless — the same
    /// cry-wolf shape that gets a guard routed around with `|| true`.
    #[test]
    fn an_out_of_set_site_link_is_reported_and_an_in_set_one_is_not() {
        let a = SiteManifest::new("alpha", "Alpha", "index", vec![]);
        let b = SiteManifest::new("beta", "Beta", "index", vec![]);
        let a_pages = vec![(
            "index",
            SitePage::markdown(
                "A",
                // One sibling (in the set) and one on another domain (not).
                "See [Beta](site:beta/index) and [Far](site:other-domain-main/papers/07).",
            ),
        )];
        let b_pages = vec![("index", SitePage::markdown("B", "# Beta"))];
        let sites = [
            ExportSite { peer_id: "PEER", site_id: "alpha", manifest: &a, pages: &a_pages, assets: &[] },
            ExportSite { peer_id: "PEER", site_id: "beta", manifest: &b, pages: &b_pages, assets: &[] },
        ];
        let dir = std::env::temp_dir().join("entity-browser-static-export-outofset-test");
        let _ = fs::remove_dir_all(&dir);
        let report = export_site_set(&dir, &sites, "", None).unwrap();

        assert_eq!(report.dangling.len(), 1, "exactly the out-of-set link: {:?}", report.dangling);
        let d = &report.dangling[0];
        assert_eq!(d.target, "site:other-domain-main/papers/07");
        assert_eq!(d.from_site, "alpha");
        // The href is the evidence: it names the peer the link was resolved
        // AGAINST, which is the whole defect — `PEER` never hosts that site.
        assert_eq!(d.href, "/sites/PEER/other-domain-main/papers/07.html");
        // …and that file genuinely is not there, so this is a real 404 and not
        // a bookkeeping complaint about a path that happens to resolve.
        assert!(!dir.join("sites/PEER/other-domain-main/papers/07.html").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    /// A cross-**peer** link is affected identically, which the production
    /// finding did not say and which matters for what a fix would have to be:
    /// [`projection_href`] emits a host-less root-absolute path, so naming the
    /// other peer explicitly still lands on the *current* domain. The exporter
    /// cannot express a cross-domain link at all — there is no host input.
    ///
    /// Kept separate from the `site:` case so that if a future change fixes one
    /// arm and not the other, exactly one test goes red.
    #[test]
    fn a_cross_peer_link_to_a_peer_outside_the_set_is_reported_too() {
        let a = SiteManifest::new("alpha", "Alpha", "index", vec![]);
        let a_pages = vec![(
            "index",
            SitePage::markdown("A", "See [Elsewhere](entity://OTHERPEER/sites/far/pages/index)."),
        )];
        let sites =
            [ExportSite { peer_id: "PEER", site_id: "alpha", manifest: &a, pages: &a_pages, assets: &[] }];
        let dir = std::env::temp_dir().join("entity-browser-static-export-xpeer-test");
        let _ = fs::remove_dir_all(&dir);
        let report = export_site_set(&dir, &sites, "", None).unwrap();

        assert_eq!(report.dangling.len(), 1, "the cross-peer target is out of set: {:?}", report.dangling);
        assert!(
            report.dangling[0].href.starts_with("/sites/OTHERPEER/"),
            "root-absolute on THIS domain — no host component exists to point elsewhere: {}",
            report.dangling[0].href
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// Bare-root exports one site, so a cross-site link there is a deliberate
    /// namespaced outbound href, not a defect ([`Layout::BareRoot`]). Auditing
    /// it would report every such link and train an operator to ignore the
    /// warning — so the audit is `None` on that path, and this pins it.
    #[test]
    fn bare_root_does_not_audit_links_it_is_not_the_authority_on() {
        let a = SiteManifest::new("solo", "Solo", "index", vec![]);
        let site = OwnedSite {
            peer_id: "PEER".into(),
            site_id: "solo".into(),
            manifest: a,
            pages: vec![("index".into(), SitePage::markdown("A", "[X](site:elsewhere/index)"))],
            assets: vec![],
        };
        let dir = std::env::temp_dir().join("entity-browser-static-export-bareroot-audit-test");
        let _ = fs::remove_dir_all(&dir);
        // Returns a plain count — there is no set, so there is nothing to report.
        let n = export_bare_root(&dir, &site, None).unwrap();
        assert_eq!(n, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn humanize_is_reused_not_reimplemented() {
        // Guard: we depend on the shared humanize helper (no parallel impl).
        assert_eq!(location::humanize("getting-started"), "Getting started");
    }
}

/// **`F-5` — the nav-cycle / max-depth vector.**
/// `APP-CONVENTION-SEMANTIC-CONTENT-SITE` §9 asks for *"a fixture of authored
/// depth 40 (> the max 32) with a cycle; assert the renderer stops at depth 32
/// and on the cycle without looping — **pinned depth so 'stop cleanly' is not
/// vacuously conformant**."*
///
/// That parenthetical is the whole design: a renderer that walked one level, or
/// none, or crashed, would satisfy *"does not loop"*. So the assertions here are
/// two-sided — it stops **at** 32 and it renders **through** 32.
///
/// **The cycle half is answered by the type, not by a guard** — see
/// [`MAX_NAV_DEPTH`]. `NavItem::children` is an owned `Vec<NavItem>` and CBOR
/// carries no back-reference, so a cycle cannot exist in a tree we walk. The
/// nearest reachable thing (the same label and target repeated at every level)
/// is exercised below and terminates on the depth bound, which is the behaviour
/// the vector is really asking about.
#[cfg(test)]
mod nav_depth_f5 {
    use super::*;
    use crate::content_site::format::{NavItem, SiteManifest};

    /// `depth` levels of nesting, every level carrying the **same** label and
    /// target — as close to an authored cycle as this data model admits.
    fn deep_nav(depth: usize) -> NavItem {
        let mut node = NavItem::new("Loop", "/loop");
        for _ in 0..depth {
            node = NavItem::section("Loop", "/loop", vec![node]);
        }
        node
    }

    fn render(depth: usize) -> String {
        let nav = vec![deep_nav(depth)];
        let current = location::Location {
            peer_id: Some("PEER".into()),
            site_id: "site".into(),
            page: "index".into(),
        };
        let ctx = LinkCtx { layout: Layout::BareRoot, prefix: "", audit: None };
        render_nav(&nav, &current, "index", ctx)
    }

    /// How many `<ul>` opens the emitted markup nests — one per level walked.
    fn nesting(html: &str) -> usize {
        let (mut depth, mut max) = (0usize, 0usize);
        let mut rest = html;
        while let Some(i) = rest.find('<') {
            rest = &rest[i..];
            if rest.starts_with("<ul>") {
                depth += 1;
                max = max.max(depth);
                rest = &rest[4..];
            } else if rest.starts_with("</ul>") {
                depth = depth.saturating_sub(1);
                rest = &rest[5..];
            } else {
                rest = &rest[1..];
            }
        }
        max
    }

    /// **The vector.** Authored depth 40, rendered, bounded at 32 — and the
    /// bound is asserted from **both** sides so "stops cleanly" cannot be
    /// satisfied by a renderer that stops early or not at all.
    #[test]
    fn an_authored_depth_of_forty_renders_through_thirty_two_and_stops_there() {
        let html = render(40);
        let levels = nesting(&html);

        assert_eq!(
            levels,
            MAX_NAV_DEPTH + 1,
            "§4.1 max depth is {MAX_NAV_DEPTH}, so a 40-deep nav must emit exactly {} nested \
             <ul> levels (0..={MAX_NAV_DEPTH}) — got {levels}",
            MAX_NAV_DEPTH + 1
        );
        // Two-sided, per §9's own "not vacuously conformant" clause: it did not
        // stop at one level, and it did not run to the authored 40.
        assert!(levels > 1, "a renderer that walked one level would pass a one-sided check");
        assert!(levels < 40, "the authored depth reached the output — the bound did nothing");
        // "Render what it has", not "render nothing".
        assert!(html.contains("Loop"), "the truncated nav still renders the levels it walked");
        assert!(html.ends_with("</nav>"), "the markup is closed — it stopped cleanly");
    }

    /// The **falsifier**, kept as a test rather than as a claim: the assertion
    /// above is only worth something if the authored depth is what decides the
    /// output below the bound.
    #[test]
    fn below_the_bound_the_authored_depth_is_what_renders() {
        for authored in [1usize, 5, 31] {
            assert_eq!(
                nesting(&render(authored)),
                authored + 1,
                "at authored depth {authored} — under the bound — the renderer must walk all of it"
            );
        }
    }

    /// **`subtree_holds_active` is bounded too, and this is the half that had
    /// no bound at all.** It returns a `bool`, so no markup assertion can see
    /// it: a deep tree whose active page sits **below** the bound must read as
    /// not-here rather than recursing to find it.
    #[test]
    fn the_second_nav_walker_is_bounded_by_the_same_constant() {
        let current = location::Location {
            peer_id: Some("PEER".into()),
            site_id: "site".into(),
            page: "index".into(),
        };

        // The active page at depth 2 — comfortably inside the bound.
        let shallow = vec![NavItem::section(
            "A",
            "",
            vec![NavItem::section("B", "", vec![NavItem::new("Here", "/index")])],
        )];
        assert!(
            subtree_holds_active(&shallow, &current, "index"),
            "inside the bound the walker must still find the active page"
        );

        // The same page buried past the bound: refused, not chased.
        let mut deep = NavItem::new("Here", "/index");
        for _ in 0..(MAX_NAV_DEPTH + 8) {
            deep = NavItem::section("A", "", vec![deep]);
        }
        assert!(
            !subtree_holds_active(&[deep], &current, "index"),
            "past §4.1's max depth the walker must stop rather than descend"
        );
    }

    /// **The decoder's own ceiling, measured and pinned — and the §4.1
    /// divergence it produces.**
    ///
    /// §4.1 says *stop cleanly (render what it has)*. At an authored nav depth
    /// of **127 or more** we do not render what we have: `SiteManifest::
    /// from_entity`'s `Err(_) => Self::default()` arm discards the **entire
    /// manifest**, so `site_id` and `title` go with the nav. Measured, not
    /// reasoned — ≤126 decodes fully, ≥127 yields a wholly empty manifest, and
    /// no depth up to 1,000,000 overflows the stack (the CBOR decoder's nesting
    /// limit is what stops it, not anything of ours).
    ///
    /// **The DoS half of §4.1 is therefore satisfied** — never an infinite loop,
    /// never a stack overflow — and the *"render what it has"* half is not, on
    /// inputs past the ceiling. Pinned here rather than fixed: the repair is a
    /// decoder change on the shipped bundle, and it is routed as a decision
    /// rather than taken quietly. **If this test starts failing, the CBOR
    /// decoder's nesting limit moved** — which is a supply-chain fact worth a
    /// red, not a number to update.
    #[test]
    fn a_nav_deeper_than_the_decoder_loses_the_whole_manifest_not_just_the_nav() {
        // Nested CBOR emitted by CONCATENATION, so nothing here recurses and
        // the only recursion measured is the decoder's.
        fn manifest_bytes(depth: usize) -> Vec<u8> {
            let mut level = vec![0xA1u8, 0x68];
            level.extend_from_slice(b"children");
            level.push(0x81);
            let mut leaf = vec![0xA1u8, 0x65];
            leaf.extend_from_slice(b"label");
            leaf.push(0x64);
            leaf.extend_from_slice(b"leaf");

            let mut nav = Vec::new();
            for _ in 0..depth {
                nav.extend_from_slice(&level);
            }
            nav.extend_from_slice(&leaf);

            let mut data = vec![0xA3u8, 0x63];
            data.extend_from_slice(b"nav");
            data.push(0x81);
            data.extend_from_slice(&nav);
            data.extend_from_slice(&[0x65]);
            data.extend_from_slice(b"title");
            data.extend_from_slice(&[0x61]);
            data.extend_from_slice(b"T");
            data.extend_from_slice(&[0x67]);
            data.extend_from_slice(b"site_id");
            data.extend_from_slice(&[0x61]);
            data.extend_from_slice(b"S");
            data
        }
        let decode = |depth: usize| {
            let e = entity_entity::Entity::new("app/site-manifest", manifest_bytes(depth)).unwrap();
            SiteManifest::from_entity(&e)
        };

        // §9's chosen fixture depth is inside the ceiling, which is what makes
        // the vector above reachable at all.
        assert_eq!(decode(40).title, "T", "an authored depth of 40 must decode — F-5 needs it to");
        assert_eq!(decode(126).title, "T", "126 is the last depth that decodes");

        let lost = decode(127);
        assert_eq!(lost.title, "", "measured: at 127 the decoder gives up");
        assert_eq!(
            lost.site_id, "",
            "and it takes the site's IDENTITY with it — this is the §4.1 divergence, not the nav"
        );
        assert!(lost.nav.is_empty());

        // No depth overflows: the DoS half of §4.1 holds, at any size.
        assert!(decode(1_000_000).title.is_empty(), "a million levels is refused, not fatal");
    }
}

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
//! the live render path ([`render::render_page_body`]) and the link
//! classifier ([`location::classify_link`]) — it does **not** fork a second
//! renderer (the "shared render-lib" item, O2/A6).

#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)] // driven by the demo emitter + the test below

use std::fs;
use std::path::Path;

use super::format::{NavItem, SiteManifest, SitePage};
use super::location::{self, LinkTarget};
use super::paths::SITE_URL_PREFIX;
use super::read::OwnedSite;
use super::render::render_page_body;

/// One site to export: its identity, manifest, and `(slug, page)` bodies.
pub struct ExportSite<'a> {
    pub peer_id: &'a str,
    pub site_id: &'a str,
    pub manifest: &'a SiteManifest,
    pub pages: &'a [(&'a str, SitePage)],
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
/// deep-linking to `{origin}/?site=…` ([F2]). Returns the number of HTML
/// pages written.
pub fn export_site_set(
    out_dir: &Path,
    sites: &[ExportSite],
    prefix: &str,
    live_base: Option<&str>,
) -> std::io::Result<usize> {
    let mut written = 0;
    for site in sites {
        let page_slugs: Vec<String> = site.pages.iter().map(|(s, _)| s.to_string()).collect();
        for (slug, page) in site.pages {
            let html = render_page(site, slug, page, Layout::Projection, prefix, live_base);
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
            let html = render_page(site, &dir, &idx, Layout::Projection, prefix, live_base);
            let path = page_file_path(out_dir, site.peer_id, site.site_id, &dir, prefix);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, html)?;
            written += 1;
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
    Ok(written)
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
) -> std::io::Result<usize> {
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
    };
    let mut written = 0;
    for (slug, page) in es.pages {
        // Bare-root is the domain root itself — no hosting prefix applies.
        let html = render_page(&es, slug, page, Layout::BareRoot, "", live_base);
        let path = out_dir.join(format!("{slug}.html"));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, html)?;
        written += 1;
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
fn render_page(
    site: &ExportSite,
    slug: &str,
    page: &SitePage,
    layout: Layout,
    prefix: &str,
    live_base: Option<&str>,
) -> String {
    let current = location::Location {
        peer_id: Some(site.peer_id.to_string()),
        site_id: site.site_id.to_string(),
        page: slug.to_string(),
    };
    let body = rewrite_hrefs(&render_page_body(&page.format, &page.body), &current, layout, prefix);
    let nav = render_nav(&site.manifest.nav, &current, slug, layout, prefix);
    let banner = live_base.map(|base| render_live_banner(base, site.peer_id, site.site_id, slug)).unwrap_or_default();
    let page_title = page.title();
    let site_title = &site.manifest.title;
    // The site-title link goes to the site root. Resolve it through the same
    // href logic (an empty-page in-site link) so it is correct per layout AND
    // from a nested page — a hardcoded "./" is wrong for `guide/intro.html`.
    let home_href = static_href(&LinkTarget::InSite { page: String::new() }, &current, layout, prefix);
    // Bare-root carries no entity branding (the "just a site generator" pitch);
    // the projection surface names what it is.
    let footer = match layout {
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
        css = PAGE_CSS,
        home = esc(&home_href),
    )
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

/// Render the manifest nav menu to static `<a>` links (recursive). The
/// page currently being rendered is marked `aria-current`.
fn render_nav(
    nav: &[NavItem],
    current: &location::Location,
    current_slug: &str,
    layout: Layout,
    prefix: &str,
) -> String {
    if nav.is_empty() {
        return String::new();
    }
    let mut out = String::from("<nav class=\"site-nav\">");
    render_nav_items(nav, current, current_slug, &mut out, 0, layout, prefix);
    out.push_str("</nav>");
    out
}

fn render_nav_items(
    items: &[NavItem],
    current: &location::Location,
    current_slug: &str,
    out: &mut String,
    depth: usize,
    layout: Layout,
    prefix: &str,
) {
    // Cycle/depth safety (SITE §4.1: recommend max depth 32).
    if depth > 32 {
        return;
    }
    out.push_str("<ul>");
    for item in items {
        out.push_str("<li>");
        if item.target.is_empty() {
            // A section header — no link.
            out.push_str(&format!("<span class=\"nav-section\">{}</span>", esc(&item.label)));
        } else {
            let target = location::classify_link(&item.target, current);
            let href = static_href(&target, current, layout, prefix);
            let active = matches!(&target, LinkTarget::InSite { page } if page == current_slug);
            let cls = if active { " class=\"active\"" } else { "" };
            out.push_str(&format!("<a href=\"{}\"{cls}>{}</a>", esc(&href), esc(&item.label)));
        }
        if !item.children.is_empty() {
            render_nav_items(&item.children, current, current_slug, out, depth + 1, layout, prefix);
        }
        out.push_str("</li>");
    }
    out.push_str("</ul>");
}

/// Rewrite every `href="…"` in a rendered HTML body from its entity-native
/// form to a static projection href, via the link classifier. External
/// links pass through untouched.
fn rewrite_hrefs(html: &str, current: &location::Location, layout: Layout, prefix: &str) -> String {
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
                out.push_str(&esc(&static_href(&target, current, layout, prefix)));
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

/// Map a classified link target to a static href under `layout`. Cross-site
/// and cross-peer always resolve to the projection path
/// `/sites/{peer}/{site}/{page}.html` (root-absolute, depth-safe); external
/// links pass through verbatim. Only the **in-site** href depends on layout:
/// projection keeps the prefixed path; bare-root drops to a root-relative
/// `/{page}.html` (the site IS the root).
fn static_href(target: &LinkTarget, current: &location::Location, layout: Layout, prefix: &str) -> String {
    let cur_peer = current.peer_id.as_deref().unwrap_or("");
    match target {
        LinkTarget::InSite { page } => match layout {
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
         <title>Published sites</title>\n<style>{PAGE_CSS}</style>\n</head>\n<body>\n\
         <header class=\"site-header\"><span class=\"site-title\">Published sites</span></header>\n\
         <main class=\"page\">{live}<ul class=\"site-list\">{items}</ul></main>\n\
         <footer class=\"site-footer\">Static export · entity content-site projection</footer>\n\
         </body>\n</html>\n",
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
/// - **One-origin `publish-serve`** (publish INTO the SPA's `dist/`): the WASM
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
         <title>Sites — {peer}</title>\n<style>{PAGE_CSS}</style>\n</head>\n<body>\n\
         <header class=\"site-header\"><span class=\"site-title\">Sites hosted by {peer}</span></header>\n\
         <main class=\"page\"><ul class=\"site-list\">{items}</ul></main>\n\
         <footer class=\"site-footer\">Static export · entity content-site projection</footer>\n\
         </body>\n</html>\n",
        peer = esc(peer_id),
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

/// Self-contained dark stylesheet, matching the app palette (`dom/theme`).
const PAGE_CSS: &str = "\
:root{color-scheme:dark}\
*{box-sizing:border-box}\
body{margin:0;background:#0e0e1e;color:#e0e0e0;font:15px/1.6 system-ui,sans-serif}\
a{color:#a6c0de;text-decoration:none}a:hover{text-decoration:underline}\
.site-header{display:flex;flex-wrap:wrap;align-items:baseline;gap:16px;\
padding:14px 20px;background:#0a0a1a;border-bottom:1px solid #333}\
.site-title{font-size:18px;font-weight:700;color:#e0e0e0}\
.site-nav ul{list-style:none;display:flex;flex-wrap:wrap;gap:12px;margin:0;padding:0}\
.site-nav li{display:flex;gap:12px;align-items:baseline}\
.site-nav a.active{color:#c0e0c0;font-weight:600}\
.nav-section{color:#888;font-size:12px;text-transform:uppercase;letter-spacing:.05em}\
main.page{max-width:760px;margin:0 auto;padding:28px 20px}\
main.page h1,main.page h2,main.page h3{line-height:1.25}\
main.page code{background:#0a0a1a;padding:1px 5px;border-radius:3px;font-size:90%}\
main.page pre{background:#0a0a1a;padding:12px;border-radius:6px;overflow:auto}\
main.page pre code{background:none;padding:0}\
main.page table{border-collapse:collapse}\
main.page td,main.page th{border:1px solid #333;padding:4px 8px}\
.site-list{list-style:none;padding:0}.site-list li{margin:8px 0;font-size:17px}\
.muted{color:#777;font-size:13px}\
.site-footer{max-width:760px;margin:0 auto;padding:20px;color:#666;font-size:12px;\
border-top:1px solid #222}\
.live-banner-toggle{position:absolute;opacity:0;pointer-events:none}\
.live-banner{display:flex;align-items:center;justify-content:center;gap:14px;\
padding:8px 16px;background:#16213e;border-bottom:1px solid #2a3a5e;\
color:#cdd6f4;font-size:13px}\
.live-banner a{color:#a6c0de;font-weight:600}\
.live-banner-dismiss{cursor:pointer;color:#8892b0;font-size:18px;line-height:1;\
padding:0 4px;user-select:none}\
.live-banner-toggle:checked + .live-banner{display:none}\
";

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
        // cross-linking into it) via the same seeder `make publish` uses, so
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
        eprintln!("emitted {n} static pages → {} (peer {pid})", dir.display());
    }

    #[test]
    fn exports_two_sites_with_cross_links_rewritten() {
        let (dm, dp, im, ip) = two_demo_sites();
        let sites = [
            ExportSite { peer_id: "PEER1", site_id: "demo", manifest: &dm, pages: &dp },
            ExportSite { peer_id: "PEER1", site_id: "entity-info", manifest: &im, pages: &ip },
        ];

        let dir = std::env::temp_dir().join("entity-browser-static-export-test");
        let _ = fs::remove_dir_all(&dir);
        let n = export_site_set(&dir, &sites, "", None).expect("export writes");
        assert_eq!(n, 3, "two demo pages + one info page");

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
        let sites = [ExportSite { peer_id: "PEER1", site_id: "demo", manifest: &m, pages: &pages }];
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
        let sites = [ExportSite { peer_id: "P", site_id: "s", manifest: &m, pages: &pages }];
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
        let sites = [ExportSite { peer_id: "P", site_id: "s", manifest: &m, pages: &pages }];
        let dir = std::env::temp_dir().join("entity-browser-static-export-xss-test");
        let _ = fs::remove_dir_all(&dir);
        export_site_set(&dir, &sites, "", None).unwrap();
        let html = fs::read_to_string(dir.join("sites/P/s/index.html")).unwrap();
        assert!(!html.contains("<script>alert"), "raw script leaked into export: {html}");
        assert!(html.contains("&lt;script&gt;"));
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
        let sites = [ExportSite { peer_id: "P", site_id: "docs", manifest: &m, pages: &pages }];
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
            ExportSite { peer_id: "PEER", site_id: "alpha", manifest: &a, pages: &a_pages },
            ExportSite { peer_id: "PEER", site_id: "beta", manifest: &b, pages: &b_pages },
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

    #[test]
    fn humanize_is_reused_not_reimplemented() {
        // Guard: we depend on the shared humanize helper (no parallel impl).
        assert_eq!(location::humanize("getting-started"), "Getting started");
    }
}

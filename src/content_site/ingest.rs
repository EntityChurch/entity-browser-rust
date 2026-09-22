//! disk → tree ingest: read a content-team `render/` output directory and
//! write it into a peer's tree as `SiteManifest` + `SitePage` entities.
//!
//! This is the inverse of [`super::read`]: where `read` walks the tree to
//! emit a static site, `ingest` walks a static emit to populate the tree. It
//! is the missing middle of the cross-team pipeline —
//!
//! ```text
//! papers render/  →  render/output/domains/<domain>/<site>/  →  INGEST  →  peer tree
//!   →  read_all_sites  →  static_export (.html) + publish_fixture (.bin)  →  serve
//! ```
//!
//! ## The emit contract we ingest (papers `render/`, deterministic)
//!
//! The render tool is **multi-site, multi-domain** (see
//! `entity-core-papers/docs/RENDER-TOOL-HANDOFF.md`). One run builds a single
//! site, a whole **domain**, or the whole **constellation** (all domains), and
//! the output is one directory PER DOMAIN with many sites under each:
//!
//! ```text
//! <root>/
//!   constellation.manifest.json   (domain/all builds — the multi-site index)
//!   <domain>/<site>/
//!     site.manifest.json   { site_id, title, tagline, theme, nav:[{title,path,children}] }
//!     run-manifest.json    (provenance — ignored here; their reproducibility proof)
//!     pages/**/*.md        markdown, each with a +++ TOML frontmatter block
//!     assets/figures/**    images — ingested as content-addressed `SiteAsset`
//!                          entities under the site's `assets/` subgraph
//! ```
//!
//! ## Two image grammars → one embed standard
//!
//! Page bodies carry images in two forms (see [`super::embed`]): the render
//! tool's `::embed[fallback]{ref=assets/…}` directive and plain markdown
//! `![alt](src)`. We **normalize markdown up into the embed directive at this
//! seam** (`markdown_to_embed`) so the stored body speaks one standard; the
//! referenced bytes are staged from `assets/**` into the asset subgraph. A
//! ref that points *outside* the site dir (an authored `![](../../output/…)`)
//! has no file under `assets/` to stage — it stays an unresolved embed (the
//! papers-side gap: such refs should be staged into the site's `assets/`).
//!
//! `site_id` is domain-prefixed (`billslab-research`), so ids stay unique
//! across a whole-constellation ingest with no collision. We discover sites by
//! a **recursive** scan for `site.manifest.json`-bearing directories, so a
//! single `--ingest=<path>` works for every render scope — a single site dir, a
//! legacy flat `sites/<id>/` parent, one `domains/<domain>/` directory, or the
//! whole `domains/` root.
//!
//! The mapping onto our tree model is mechanical (see the per-field notes
//! below). Where the two contracts differ we translate at this one seam
//! rather than carry an adapter through the rest of the pipeline — the goal
//! is to converge the contracts, not to accumulate translation cruft.
//!
//! Native-only: the publish surface is headless (`entity-browser publish`),
//! and the parsers (`serde_json`, `toml`) are non-wasm deps.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::format::{media_type_for_path, NavItem, SiteAsset, SiteManifest, SitePage};
use super::{embed, paths};
use crate::peers::Peers;

/// Ingest one or more sites from `path` into `peer_id`'s tree.
///
/// Discovers every `site.manifest.json`-bearing directory at any depth below
/// `path` (a leaf site dir is never descended into — its `pages/` are content,
/// not sub-sites). This single recursive scan handles every render-tool scope:
/// a lone site dir, a legacy flat `sites/<id>/` parent, one
/// `domains/<domain>/` directory, or the whole `domains/` constellation root.
/// Returns the ingested site ids, sorted (deterministic).
pub fn ingest_path(peers: &Peers, peer_id: &str, path: &Path) -> Result<Vec<String>, String> {
    let mut dirs = Vec::new();
    find_site_dirs(path, &mut dirs)?;
    dirs.sort();
    if dirs.is_empty() {
        return Err(format!(
            "no site.manifest.json at {} or anywhere below it",
            path.display()
        ));
    }
    let mut ids = Vec::new();
    for d in &dirs {
        ids.push(ingest_site_dir(peers, peer_id, d)?);
    }
    Ok(ids)
}

/// Recursively collect every directory holding a `site.manifest.json` under
/// `dir` (inclusive). A site directory is a leaf — we record it and stop
/// descending (its `pages/`/`assets/` subdirs are content, never sub-sites).
fn find_site_dirs(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if dir.join("site.manifest.json").is_file() {
        out.push(dir.to_path_buf());
        return Ok(());
    }
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("read dir {}: {e}", dir.display()))?;
    let mut subdirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    subdirs.sort();
    for sd in &subdirs {
        find_site_dirs(sd, out)?;
    }
    Ok(())
}

/// One site directory, read into entities and **not yet written anywhere**.
///
/// The disk→entity half of an ingest, split out from [`ingest_site_dir`] so it
/// can be measured without a `Peers`. That is not a testing convenience: it is
/// the first of `G-PIN-4`'s three links (*source fixture → site entity*), the
/// only one neither implementation had a fixture for, and the one both seats
/// independently named as the whole remaining risk. A link you cannot evaluate
/// without standing up a peer is a link nobody evaluates.
///
/// See `gpin4_joint_fixture.rs` and `tests/fixtures/gpin4-joint/source/`.
pub(crate) struct IngestedSite {
    pub site_id: String,
    pub manifest: SiteManifest,
    /// `(slug, page)`, slug-sorted by [`collect_pages`].
    pub pages: Vec<(String, SitePage)>,
    /// `(name, asset)` relative to `assets/`, name-sorted.
    pub assets: Vec<(String, SiteAsset)>,
}

/// Read one site directory into entities. Pure: touches no tree and no peer.
pub(crate) fn read_site_dir(dir: &Path) -> Result<IngestedSite, String> {
    let manifest_json = dir.join("site.manifest.json");
    let txt = std::fs::read_to_string(&manifest_json)
        .map_err(|e| format!("read {}: {e}", manifest_json.display()))?;
    let v: serde_json::Value =
        serde_json::from_str(&txt).map_err(|e| format!("parse {}: {e}", manifest_json.display()))?;

    let site_id = v["site_id"].as_str().unwrap_or("").to_string();
    if site_id.is_empty() {
        return Err(format!("{}: missing site_id", manifest_json.display()));
    }
    let title = v["title"].as_str().unwrap_or(&site_id).to_string();
    let nav = parse_nav(&v["nav"]);

    // Walk pages/ first: we need the slug set both to write the pages and to
    // pick a landing page (`root`) that actually exists.
    let pages_dir = dir.join("pages");
    let pages = collect_pages(&pages_dir)?;
    let root = pick_root(&pages);

    let mut manifest = SiteManifest::new(&site_id, &title, root, nav);
    // Carry the cosmetic cover fields into the open params bag — the tree
    // model has no dedicated tagline/theme field, and params is exactly the
    // spec's open attribute bag for this.
    if let Some(t) = v["tagline"].as_str().filter(|s| !s.is_empty()) {
        manifest.params.insert("tagline".into(), t.into());
    }
    if let Some(t) = v["theme"].as_str().filter(|s| !s.is_empty()) {
        manifest.params.insert("theme".into(), t.into());
    }

    // Stage the asset subgraph (images) before the pages, so a body's embed
    // ref has its bytes present in the same ingest. Content-addressed: the
    // store dedups identical bytes across sites.
    let assets = collect_assets(&dir.join("assets"))?;

    Ok(IngestedSite { site_id, manifest, pages, assets })
}

/// Ingest a single site directory (one that contains `site.manifest.json`)
/// into `peer_id`'s tree. The read half is [`read_site_dir`]; this is the
/// write half, and the split is deliberate — see that type's doc.
fn ingest_site_dir(peers: &Peers, peer_id: &str, dir: &Path) -> Result<String, String> {
    let site = read_site_dir(dir)?;
    let site_id = site.site_id;

    peers.seed_write(
        peer_id,
        paths::manifest_path(peer_id, &site_id),
        site.manifest.to_entity(),
    );
    // Assets before pages, so a body's embed ref has its bytes present in the
    // same ingest.
    for (name, asset) in &site.assets {
        peers.seed_write(
            peer_id,
            paths::asset_path(peer_id, &site_id, name),
            asset.to_entity(),
        );
    }
    for (slug, page) in &site.pages {
        peers.seed_write(
            peer_id,
            paths::page_path(peer_id, &site_id, slug),
            page.to_entity(),
        );
    }

    Ok(site_id)
}

/// Recursively read every file under `assets_dir` into `(name, SiteAsset)`,
/// where `name` is the path relative to `assets_dir` (`figures/x.png`) — the
/// suffix of the embed `ref` after the `assets/` prefix. Missing dir → empty
/// (a site need not have assets). Bytes are read raw; the media type is
/// inferred from the extension.
fn collect_assets(assets_dir: &Path) -> Result<Vec<(String, SiteAsset)>, String> {
    let mut out = Vec::new();
    if assets_dir.is_dir() {
        walk_assets(assets_dir, assets_dir, &mut out)?;
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn walk_assets(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, SiteAsset)>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read dir {}: {e}", dir.display()))?;
    for entry in entries.filter_map(Result::ok) {
        let p = entry.path();
        if p.is_dir() {
            walk_assets(root, &p, out)?;
            continue;
        }
        let rel = p.strip_prefix(root).map_err(|e| format!("strip prefix: {e}"))?;
        let name = rel.to_string_lossy().replace('\\', "/");
        // A render tool may stage a flagged placeholder when a pinned figure is
        // absent (`<id>.png.placeholder`); skip those — they're not real bytes.
        if name.ends_with(".placeholder") {
            continue;
        }
        let bytes = std::fs::read(&p).map_err(|e| format!("read {}: {e}", p.display()))?;
        out.push((name.clone(), SiteAsset::new(media_type_for_path(&name), bytes)));
    }
    Ok(())
}

/// The page source extensions we ingest, and the `SitePage.format` each
/// becomes. `.md` is the universal base (convention §3.1's default); `.html` is
/// the **web-tier escape hatch** the same section permits — a pre-rendered
/// document (a Pandoc paper/book) that we store verbatim and the app mounts in
/// a restricted sandbox. Order is the tie-break order for a slug collision
/// report, nothing more; a collision is refused, never resolved.
const PAGE_SOURCES: &[(&str, &str)] = &[("md", "markdown"), ("html", "html")];

/// Recursively read every page source under `pages_dir` into `(slug, SitePage)`.
/// Slug = path relative to `pages_dir`, extension stripped, slash-separated —
/// the exact form [`super::read`] recovers from the tree, so the round-trip
/// is identity.
///
/// **Two files that would claim one slug are refused, not ranked.** `foo.md`
/// beside `foo.html` is an authoring mistake with no correct answer: whichever
/// we picked, half the time we would publish the file the author did not mean,
/// and the tree records no trace of the one we dropped. So it fails here, where
/// the person who can fix it is standing — the same posture as the registry
/// emitter refusing a malformed bind target rather than signing it.
fn collect_pages(pages_dir: &Path) -> Result<Vec<(String, SitePage)>, String> {
    let mut out = Vec::new();
    let mut seen: BTreeMap<String, PathBuf> = BTreeMap::new();
    walk_pages(pages_dir, pages_dir, &mut out, &mut seen)?;
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn walk_pages(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, SitePage)>,
    seen: &mut BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read dir {}: {e}", dir.display()))?;
    for entry in entries.filter_map(Result::ok) {
        let p = entry.path();
        if p.is_dir() {
            walk_pages(root, &p, out, seen)?;
            continue;
        }
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
        let Some((_, format)) = PAGE_SOURCES.iter().find(|(e, _)| *e == ext) else {
            continue;
        };
        let rel = p.strip_prefix(root).map_err(|e| format!("strip prefix: {e}"))?;
        let slug = rel.to_string_lossy().replace('\\', "/");
        let slug = slug.strip_suffix(&format!(".{ext}")).unwrap_or(&slug).to_string();
        if let Some(first) = seen.get(&slug) {
            return Err(format!(
                "two page sources claim the slug '{slug}': {} and {} — \
                 rename one; a site page has exactly one source",
                first.display(),
                p.display()
            ));
        }
        seen.insert(slug.clone(), p.clone());
        let content =
            std::fs::read_to_string(&p).map_err(|e| format!("read {}: {e}", p.display()))?;
        let page = match *format {
            "html" => page_from_html(&content),
            _ => page_from_markdown(&content),
        };
        out.push((slug, page));
    }
    Ok(())
}

/// Build a `SitePage` from a pre-rendered HTML document.
///
/// The body is stored **byte-for-byte**. A Pandoc artifact is a complete
/// standalone file — its own `<head>`, `<title>`, inline stylesheet, internal
/// anchors — and every transform we apply to markdown here (frontmatter
/// stripping, `::embed` normalization) would corrupt it. There is deliberately
/// no sanitizing pass either: the safety boundary is the sandbox the app mounts
/// it in, not a rewrite at ingest, and doing both would mean *neither* is the
/// one place the property lives.
///
/// The only thing lifted out is the title, so the page has a name in the nav
/// and the breadcrumbs without the reader opening it.
fn page_from_html(content: &str) -> SitePage {
    let title = html_title(content);
    let mut page = SitePage::html(title.clone(), content);
    if title.is_empty() {
        // No `<title>`: leave the key absent rather than storing an empty
        // string, so the slug-humanizing fallback names the page (an empty
        // title would render as a blank breadcrumb, which reads as broken).
        page.frontmatter.remove("title");
    }
    page
}

/// The text of the document's first `<title>` element, trimmed and
/// entity-decoded for the handful of escapes a title realistically carries.
/// Empty when there is none — the caller then leaves `title` unset and the
/// existing humanize-the-slug fallback names the page, exactly as it does for a
/// markdown file with no frontmatter title.
fn html_title(content: &str) -> String {
    // Case-insensitive search without pulling in a regex: lowercase a copy for
    // locating, then slice the ORIGINAL so the title keeps its own casing.
    let lower = content.to_ascii_lowercase();
    let Some(open) = lower.find("<title") else { return String::new() };
    let Some(gt) = lower[open..].find('>').map(|i| open + i + 1) else { return String::new() };
    let Some(close) = lower[gt..].find("</title>").map(|i| gt + i) else { return String::new() };
    let raw = content[gt..close].trim();
    raw.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .trim()
        .to_string()
}

/// Build a `SitePage` from one emitted markdown file: strip the `+++` TOML
/// frontmatter block, lift `title` (and carry the provenance keys —
/// `content_class`, `source`, `recipe`, `status` — into the page
/// frontmatter so nothing is dropped), keep the rest as the markdown body.
fn page_from_markdown(content: &str) -> SitePage {
    let (fm, body) = split_frontmatter(content);
    let title = fm.get("title").cloned().unwrap_or_default();
    // Normalize markdown images UP into the canonical `::embed` directive so
    // the stored body speaks one standard (see [`super::embed`]). A body
    // already using `::embed` (the render tool's figures) is left unchanged.
    let body = embed::markdown_to_embed(&body);
    let mut page = SitePage::markdown(title, body);
    for (k, val) in fm {
        page.frontmatter.insert(k, val);
    }
    page
}

/// Split a leading `+++ … +++` TOML frontmatter block from a markdown body.
/// Returns the parsed string-valued frontmatter and the remaining body. A
/// file with no frontmatter yields an empty map and the whole content.
fn split_frontmatter(content: &str) -> (BTreeMap<String, String>, String) {
    let mut fm = BTreeMap::new();
    if let Some(rest) = content.strip_prefix("+++\n") {
        if let Some(end) = rest.find("\n+++\n") {
            let block = &rest[..end];
            let body = &rest[end + "\n+++\n".len()..];
            // The block is valid TOML; let the toml parser handle escaping.
            if let Ok(table) = block.parse::<toml::Table>() {
                for (k, val) in &table {
                    if let Some(s) = val.as_str() {
                        fm.insert(k.clone(), s.to_string());
                    }
                }
            }
            return (fm, body.trim_start_matches('\n').to_string());
        }
    }
    (fm, content.to_string())
}

/// Map their nav cover (`[{title, path, children}]`) onto ours
/// (`NavItem{label, target, children}`). `path` is an emitted page path
/// (`pages/research/index.md`); we project it to an in-site **root-absolute**
/// link (`/research/index`) — nav is a site-global menu, so it must resolve
/// identically from any page (see [`super::location`] convention).
fn parse_nav(v: &serde_json::Value) -> Vec<NavItem> {
    let Some(arr) = v.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .map(|n| {
            let label = n["title"].as_str().unwrap_or("").to_string();
            // Parse children first so a group header can derive its landing
            // section from them (depth-first; their targets are ready here).
            let children = parse_nav(&n["children"]);
            let raw = n["path"].as_str().unwrap_or("");
            let target = if raw.is_empty() {
                // A group header with no page of its own (`path: ""` in the
                // cover — e.g. billslab's "Papers"). Land it on its children's
                // section index, NOT the site root: the old `path_to_target("")`
                // → "/" silently aliased the home page, so clicking the header
                // navigated home and highlighted the home nav item instead.
                section_target_from_children(&children)
            } else {
                path_to_target(raw)
            };
            NavItem::section(label, target, children)
        })
        .collect()
}

/// `pages/research/index.md` → `/research/index` (in-site, root-absolute).
/// Empty in → empty out (a group header has no page; see [`parse_nav`]).
///
/// Every extension in [`PAGE_SOURCES`] is stripped, not just `.md` — a nav
/// entry naming a `.html` page must project to the same slug
/// [`collect_pages`] stored it under, or the menu links to a page that isn't
/// there. (Stripping only `.md` would leave `/papers/paper-00.html`, which
/// resolves to nothing and reads as a missing page rather than a bad link.)
fn path_to_target(page_path: &str) -> String {
    if page_path.is_empty() {
        return String::new();
    }
    let s = page_path.strip_prefix("pages/").unwrap_or(page_path);
    let s = PAGE_SOURCES
        .iter()
        .find_map(|(ext, _)| s.strip_suffix(&format!(".{ext}")))
        .unwrap_or(s);
    format!("/{s}")
}

/// The section landing target for a group-header nav node with no page of its
/// own — the longest common **directory** of its children's targets, so
/// clicking the header lands on that section's (synthesized) index, matching
/// the sidebar. `/papers/paper-00` (lone child) → `/papers`. Empty when there
/// are no targetable children or they share no directory.
fn section_target_from_children(children: &[NavItem]) -> String {
    // Directory segments of each child target (drop the leaf page segment).
    let dirs: Vec<Vec<&str>> = children
        .iter()
        .filter(|c| !c.target.is_empty())
        .map(|c| {
            let mut segs: Vec<&str> =
                c.target.split('/').filter(|s| !s.is_empty()).collect();
            segs.pop(); // the leaf page → its containing directory
            segs
        })
        .collect();
    let Some(first) = dirs.first() else {
        return String::new();
    };
    // Longest common segment prefix across all children's directories.
    let mut common = first.clone();
    for d in &dirs[1..] {
        let n = common.iter().zip(d).take_while(|(a, b)| a == b).count();
        common.truncate(n);
    }
    if common.is_empty() {
        String::new()
    } else {
        format!("/{}", common.join("/"))
    }
}

/// Pick the landing slug: prefer `index` if present, else the first page.
fn pick_root(pages: &[(String, SitePage)]) -> String {
    if pages.iter().any(|(s, _)| s == "index") {
        return "index".to_string();
    }
    pages.first().map(|(s, _)| s.clone()).unwrap_or_else(|| "index".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, content: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    /// A stand-in for a Pandoc artifact: standalone, own `<head>`/`<style>`,
    /// and the two things the real books carry that a naive ingest would
    /// mangle — a `+++` sequence in the prose and markdown-image syntax.
    const PANDOC_LIKE: &str = "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n\
        <meta charset=\"utf-8\">\n<title>Paper 0 &amp; the Entity System</title>\n\
        <style>body { font-family: serif; }</style>\n</head>\n<body>\n\
        <h1>Paper 0</h1>\n<p>A +++ sequence and ![not an image](x.png) in prose.</p>\n\
        </body>\n</html>\n";

    #[test]
    fn an_html_page_is_stored_verbatim_as_a_document() {
        // The whole contract of the html tier: the bytes that arrive are the
        // bytes that are stored. Every markdown transform in this module would
        // corrupt a standalone document, so none of them may run on it.
        let page = page_from_html(PANDOC_LIKE);
        assert_eq!(page.format, "html");
        assert_eq!(page.body, PANDOC_LIKE, "a document is stored byte-for-byte");
        // Specifically: frontmatter splitting did not eat anything, and the
        // `::embed` normalization did not rewrite the markdown-looking image.
        assert!(page.body.contains("+++"), "a +++ in prose survives");
        assert!(page.body.contains("![not an image](x.png)"), "no embed normalization ran");
        assert!(!page.body.contains("::embed"), "no embed directive was synthesized");
    }

    #[test]
    fn an_html_pages_title_comes_from_its_title_element() {
        // So the page has a name in the nav and breadcrumbs without opening it.
        // Entity-decoded, because a real title carries them (paper 0's does).
        let page = page_from_html(PANDOC_LIKE);
        assert_eq!(page.title(), "Paper 0 & the Entity System");
    }

    #[test]
    fn a_titleless_or_odd_document_still_ingests() {
        // Title extraction is best-effort — it must never be the thing that
        // fails an ingest, because the slug-humanizing fallback already names
        // a page fine. Each of these is a document we'd still want stored.
        for doc in [
            "<html><body><p>no title element at all</p></body></html>",
            "<html><head><TITLE>Shouty</TITLE></head><body>x</body></html>",
            "<html><head><title></title></head><body>x</body></html>",
            "<html><head><title>unclosed<body>x</body></html>",
            "",
        ] {
            let page = page_from_html(doc);
            assert_eq!(page.format, "html");
            assert_eq!(page.body, doc, "body is verbatim regardless of the title");
        }
        // Casing of the tag is ignored; casing of the TEXT is preserved.
        assert_eq!(
            page_from_html("<html><head><TITLE>Shouty</TITLE></head><body>x</body></html>")
                .title(),
            "Shouty"
        );
    }

    #[test]
    fn markdown_and_html_pages_coexist_in_one_site() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(
            dir,
            "site.manifest.json",
            r#"{ "site_id": "corpus", "title": "Corpus",
                 "nav": [ { "title": "Home", "path": "pages/index.md" },
                          { "title": "Paper 0", "path": "pages/papers/paper-00.html" } ] }"#,
        );
        write(dir, "pages/index.md", "+++\ntitle = \"Home\"\n+++\n\n# Welcome\n");
        write(dir, "pages/papers/paper-00.html", PANDOC_LIKE);

        let pages = collect_pages(&dir.join("pages")).expect("ingest");
        let by_slug: BTreeMap<_, _> = pages.into_iter().collect();
        assert_eq!(by_slug["index"].format, "markdown");
        assert_eq!(by_slug["papers/paper-00"].format, "html");
        assert_eq!(
            by_slug["papers/paper-00"].body, PANDOC_LIKE,
            "the html page is untouched by the markdown path"
        );

        // The nav must project the .html entry to the SAME slug the page was
        // stored under, or the menu links at a page that does not exist.
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("site.manifest.json")).unwrap())
                .unwrap();
        let nav = parse_nav(&manifest["nav"]);
        assert_eq!(nav[1].target, "/papers/paper-00", "nav strips .html, not just .md");
    }

    #[test]
    fn two_sources_claiming_one_slug_are_refused_by_name() {
        // There is no correct pick between `about.md` and `about.html`, so the
        // ingest refuses in front of the person who can rename one — rather
        // than publishing a coin-flip and recording nothing about the loser.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(dir, "pages/about.md", "+++\ntitle = \"About\"\n+++\n\nbody\n");
        write(dir, "pages/about.html", PANDOC_LIKE);

        let err = collect_pages(&dir.join("pages")).expect_err("a slug collision must refuse");
        assert!(err.contains("about"), "the message names the slug: {err}");
        assert!(err.contains("about.md"), "the message names the first file: {err}");
        assert!(err.contains("about.html"), "the message names the second file: {err}");
    }

    #[test]
    fn ingests_a_render_emit_and_reads_back_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(
            dir,
            "site.manifest.json",
            r#"{
              "site_id": "lab",
              "title": "Bill's Lab",
              "tagline": "R&D arm",
              "theme": "lab",
              "nav": [
                { "title": "Home", "path": "pages/index.md" },
                { "title": "Research", "path": "pages/research/index.md", "children": [
                  { "title": "Glossary", "path": "pages/research/glossary.md" }
                ] }
              ]
            }"#,
        );
        write(dir, "pages/index.md", "+++\ntitle = \"Home\"\ncontent_class = \"authored\"\n+++\n\n# Welcome\n");
        write(dir, "pages/research/index.md", "+++\ntitle = \"Research\"\n+++\n\nResearch body.\n");
        write(
            dir,
            "pages/research/glossary.md",
            "+++\ntitle = \"Glossary\"\nrecipe = \"glossary\"\n+++\n\nTerms.\n",
        );

        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let ids = ingest_path(&peers, &pid, dir).expect("ingest");
        assert_eq!(ids, vec!["lab"]);

        // Round-trip through the tree reader the publish pipeline uses.
        let site = super::super::read::read_site(&peers, &pid, "lab").expect("reads back");
        assert_eq!(site.manifest.title, "Bill's Lab");
        assert_eq!(site.manifest.root(), "index");
        // `"theme": "lab"` is a deliberately UNKNOWN theme name: ingest copies
        // the field verbatim (no validation at this layer — the render/export
        // consumers validate against the registry and warn loudly, S-T2), so
        // the params bag must carry it unchanged…
        assert_eq!(site.manifest.params.get("theme").map(String::as_str), Some("lab"));
        // …and the registry consumer must reject it (no silent dark restyle).
        assert_eq!(crate::theme_tokens::site_container_block("lab"), None);

        let slugs: Vec<&str> = site.pages.iter().map(|(s, _)| s.as_str()).collect();
        assert!(slugs.contains(&"index"));
        assert!(slugs.contains(&"research/index"), "nested page lost: {slugs:?}");
        assert!(slugs.contains(&"research/glossary"));

        // Frontmatter title lifted; provenance carried, not dropped.
        let glossary = site.pages.iter().find(|(s, _)| s == "research/glossary").unwrap();
        assert_eq!(glossary.1.title(), "Glossary");
        assert_eq!(glossary.1.frontmatter.get("recipe").map(String::as_str), Some("glossary"));
        assert!(glossary.1.body.contains("Terms."));
        assert!(!glossary.1.body.starts_with("+++"), "frontmatter not stripped from body");

        // Nav projected to in-site root-absolute links.
        let research_nav = site.manifest.nav.iter().find(|n| n.label == "Research").unwrap();
        assert_eq!(research_nav.target, "/research/index");
        assert_eq!(research_nav.children[0].target, "/research/glossary");
    }

    #[test]
    fn ingests_assets_and_normalizes_image_bodies_to_embeds() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(
            dir,
            "site.manifest.json",
            r#"{ "site_id": "figs", "title": "Figures", "nav": [] }"#,
        );
        // A page with a markdown image (authored form) + the render tool's
        // own ::embed directive (canonical form) — both must end up as embeds.
        write(
            dir,
            "pages/index.md",
            "+++\ntitle = \"Home\"\n+++\n\n# Figures\n\n![A landscape](assets/figures/landscape.svg)\n\n::embed[Topology figure]{ref=assets/figures/topology.png}\n",
        );
        // Two real asset files (one nested) + a placeholder that must be skipped.
        write(dir, "assets/figures/landscape.svg", "<svg xmlns=\"http://www.w3.org/2000/svg\"/>");
        write(dir, "assets/figures/topology.png", "fake-png-bytes");
        write(dir, "assets/figures/missing.png.placeholder", "PLACEHOLDER");

        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let ids = ingest_path(&peers, &pid, dir).expect("ingest");
        assert_eq!(ids, vec!["figs"]);

        let site = super::super::read::read_site(&peers, &pid, "figs").expect("reads back");

        // Body: BOTH image grammars are now the canonical embed directive.
        let index = site.pages.iter().find(|(s, _)| s == "index").unwrap();
        assert!(
            index.1.body.contains("::embed[A landscape]{ref=assets/figures/landscape.svg}"),
            "markdown image not normalized to embed: {}",
            index.1.body
        );
        assert!(
            index.1.body.contains("::embed[Topology figure]{ref=assets/figures/topology.png}"),
            "existing embed not preserved: {}",
            index.1.body
        );
        assert!(!index.1.body.contains("!["), "raw markdown image should be gone");

        // Assets: the two real files ingested, nested name preserved, media
        // type inferred; the placeholder skipped.
        let names: Vec<&str> = site.assets.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["figures/landscape.svg", "figures/topology.png"], "assets: {names:?}");
        let svg = site.assets.iter().find(|(n, _)| n == "figures/landscape.svg").unwrap();
        assert_eq!(svg.1.media_type, "image/svg+xml");
        assert!(svg.1.bytes.starts_with(b"<svg"));
        let png = site.assets.iter().find(|(n, _)| n == "figures/topology.png").unwrap();
        assert_eq!(png.1.media_type, "image/png");

        // The embed refs match the staged asset names (after the assets/ prefix).
        let refs = crate::content_site::embed::embed_refs(&index.1.body);
        assert!(refs.contains(&"assets/figures/landscape.svg".to_string()), "refs: {refs:?}");
        assert!(refs.contains(&"assets/figures/topology.png".to_string()));
    }

    #[test]
    fn ingests_a_registered_theme_name_that_resolves() {
        // The S-T2 happy path: a manifest declaring a REGISTERED theme name
        // ingests verbatim and resolves through the registry to a container
        // block (the render/export consumers apply it in "Site's theme" mode).
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(
            dir,
            "site.manifest.json",
            r#"{ "site_id": "lit", "title": "Lit", "theme": "light",
                 "nav": [ { "title": "Home", "path": "pages/index.md" } ] }"#,
        );
        write(dir, "pages/index.md", "+++\ntitle = \"Home\"\n+++\n\nBody.\n");

        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        ingest_path(&peers, &pid, dir).expect("ingest");
        let site = super::super::read::read_site(&peers, &pid, "lit").expect("reads back");
        assert_eq!(site.manifest.params.get("theme").map(String::as_str), Some("light"));
        let block = crate::theme_tokens::site_container_block("light")
            .expect("registered name resolves");
        assert!(block.contains("--site-bg:"), "container block: {block}");
    }

    #[test]
    fn ingests_a_whole_constellation_two_levels_deep() {
        // The render layout: domains/<domain>/<site>/ — sites live
        // TWO levels below the ingest root (not the old flat sites/<id>/). One
        // `--ingest=<domains-root>` must discover every site across every domain.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let site = |dom: &str, s: &str, id: &str| {
            write(
                &root.join(dom).join(s),
                "site.manifest.json",
                &format!(r#"{{ "site_id": "{id}", "title": "{id}", "nav": [] }}"#),
            );
            write(
                &root.join(dom).join(s),
                "pages/index.md",
                "+++\ntitle = \"Home\"\n+++\n\nBody.\n",
            );
        };
        site("billslab", "main", "billslab-main");
        site("billslab", "research", "billslab-research");
        site("entity-core-protocol", "conformance", "entity-core-protocol-conformance");
        // A stray non-site dir (e.g. the constellation manifest's siblings)
        // must be skipped, not error.
        std::fs::create_dir_all(root.join("billslab").join("assets-orphan")).unwrap();

        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let mut ids = ingest_path(&peers, &pid, root).expect("ingest constellation");
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "billslab-main",
                "billslab-research",
                "entity-core-protocol-conformance"
            ],
            "every site across every domain discovered, domain-prefixed ids unique"
        );
        // Each reads back as a real site through the publish-side reader.
        for id in &ids {
            let s = super::super::read::read_site(&peers, &pid, id).expect("reads back");
            assert!(s.pages.iter().any(|(slug, _)| slug == "index"));
        }
    }

    #[test]
    fn group_header_nav_lands_on_section_not_root() {
        // A nav cover node with `path: ""` is a pure group header (billslab's
        // "Papers"): derive its target from its children's section, NOT "/"
        // (the old behavior silently aliased the home page, so clicking the
        // header navigated home and highlighted the home nav item).
        let nav = parse_nav(&serde_json::json!([
            { "title": "Home", "path": "pages/index.md" },
            { "title": "Papers", "path": "", "children": [
                { "title": "Paper 0", "path": "pages/papers/paper-00.md" }
            ]},
            { "title": "Figures", "path": "", "children": [
                { "title": "Landscape", "path": "pages/research/figures/landscape.md" }
            ]},
            { "title": "Empty group", "path": "", "children": [] }
        ]));
        // Lone child `/papers/paper-00` → section `/papers`.
        let papers = nav.iter().find(|n| n.label == "Papers").unwrap();
        assert_eq!(papers.target, "/papers", "group header lands on its section, not /");
        assert_eq!(papers.children[0].target, "/papers/paper-00");
        // A deeper child derives the full common directory.
        assert_eq!(nav.iter().find(|n| n.label == "Figures").unwrap().target, "/research/figures");
        // A childless group has no section to land on → empty (non-navigable).
        assert_eq!(nav.iter().find(|n| n.label == "Empty group").unwrap().target, "");
        // A real page nav is unchanged (no regression).
        assert_eq!(nav.iter().find(|n| n.label == "Home").unwrap().target, "/index");
    }
}

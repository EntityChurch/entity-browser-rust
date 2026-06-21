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

/// Ingest a single site directory (one that contains `site.manifest.json`).
fn ingest_site_dir(peers: &Peers, peer_id: &str, dir: &Path) -> Result<String, String> {
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

    peers.seed_write(peer_id, paths::manifest_path(peer_id, &site_id), manifest.to_entity());
    for (name, asset) in &assets {
        peers.seed_write(
            peer_id,
            paths::asset_path(peer_id, &site_id, name),
            asset.to_entity(),
        );
    }
    for (slug, page) in &pages {
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

/// Recursively read every `*.md` under `pages_dir` into `(slug, SitePage)`.
/// Slug = path relative to `pages_dir`, `.md` stripped, slash-separated —
/// the exact form [`super::read`] recovers from the tree, so the round-trip
/// is identity.
fn collect_pages(pages_dir: &Path) -> Result<Vec<(String, SitePage)>, String> {
    let mut out = Vec::new();
    walk_md(pages_dir, pages_dir, &mut out)?;
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn walk_md(root: &Path, dir: &Path, out: &mut Vec<(String, SitePage)>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read dir {}: {e}", dir.display()))?;
    for entry in entries.filter_map(Result::ok) {
        let p = entry.path();
        if p.is_dir() {
            walk_md(root, &p, out)?;
            continue;
        }
        if p.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let rel = p.strip_prefix(root).map_err(|e| format!("strip prefix: {e}"))?;
        let slug = rel.to_string_lossy().replace('\\', "/");
        let slug = slug.strip_suffix(".md").unwrap_or(&slug).to_string();
        let content =
            std::fs::read_to_string(&p).map_err(|e| format!("read {}: {e}", p.display()))?;
        out.push((slug, page_from_markdown(&content)));
    }
    Ok(())
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
fn path_to_target(page_path: &str) -> String {
    if page_path.is_empty() {
        return String::new();
    }
    let s = page_path.strip_prefix("pages/").unwrap_or(page_path);
    let s = s.strip_suffix(".md").unwrap_or(s);
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
        assert_eq!(site.manifest.params.get("theme").map(String::as_str), Some("lab"));

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

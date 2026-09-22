//! **`G-PIN-4`, the site-vocabulary half — our side of the joint fixture.**
//!
//! [`super::crossimpl_reproducible_publish`] establishes that the *substrate*
//! agrees across independent cores: canonical ECF and the `EXTENSION-TREE`
//! v4.0.2 HAMT produce the same content hashes and the same root from the same
//! bindings, measured against `entity-workbench-go`'s vendored emission. What it
//! deliberately does **not** establish is the *vocabulary* — their fixture's
//! leaves are `test/note`, and `APP-CONVENTION-SEMANTIC-CONTENT-SITE` §9's cases
//! are about `app/site-manifest` and `app/site-page`.
//!
//! This module is the missing half, built so that the other seat can discharge
//! their side **without a rig, a network, or a coordination round-trip**:
//!
//! - `tests/fixtures/gpin4-joint/site.json` is the fixture as **authored
//!   input** — site id, title, params, a nested nav, and seven pages. Plain
//!   JSON, no Rust in it, readable by any implementation.
//! - `tests/fixtures/gpin4-joint/EXPECTED.json` is what **this** implementation
//!   computes from it: every site-relative key → content hash, and the trie root
//!   over that binding set.
//!
//! `G-PIN-4` is then literally what it says — one fixture, two publishers,
//! compare roots — and a red names the entity that diverged rather than only the
//! root. The protocol for the other seat is in that directory's `README.md`.
//!
//! ## Why the two key sets are directly comparable, measured rather than assumed
//!
//! Our site subgraph is at `/{peer}/sites/{site_id}/…` and theirs is at
//! `/{peer}/content/sites/{site_id}/…` (`entitysdk/site.go`'s `SiteManifestPath`
//! / `SitePagePath`). Both are conformant — §2's v0.5 ruling makes a site *"a
//! free subgraph at any publisher-chosen tree path"* — and **relative to the
//! site root both spell `manifest` and `pages/{slug}`**, which is what the trie
//! is keyed by. So placement, peer-id and keypair all fall out of the comparison
//! and neither side has to move.
//!
//! ## What is deliberately in the fixture, and why each row is there
//!
//! Every one of these is a case where an implementation can be internally
//! consistent and still disagree with another, so a fixture without them can go
//! green while the divergence is live:
//!
//! - **A page with NO frontmatter** (`glossary`). `frontmatter` is `omitempty`
//!   on their struct and conditional on ours; the key must be **absent**, not an
//!   empty map. An empty map encodes to different bytes and a different hash.
//! - **A page with TWO frontmatter keys** (`about`), so map key ordering is
//!   exercised. ECF orders canonically (length, then lexical) *"and the encoder
//!   gets no say"* — a `BTreeMap`'s natural order and the canonical order are
//!   not the same relation, which is the shape `AGENTS.md`'s `to_ecf`-vs-
//!   `into_writer` entry is about.
//! - **A nav section with no `target`** (`Reference`), the spec's
//!   `nav-node.? target` — the optional-absent case again, one level in.
//! - **A nested nav with `children`**, which both sides emit only when non-empty.
//! - **A non-default `format`** (`raw`, `html`), so `format` is not a constant
//!   that a hardcoded default could accidentally match.
//! - **Two levels of page nesting**, so the trie has interior nodes rather than
//!   one root bucket — a flat site can pass a build that a nested one fails.
//!
//! ## Regenerating
//!
//! `GPIN4_REGENERATE=1` rewrites `EXPECTED.json` from the current code. That is
//! the *only* way it should ever change, and changing it is a **wire event**:
//! the other seat is comparing against these bytes, so a regeneration that was
//! not accompanied by a deliberate `site.json` edit means our encoder moved, and
//! the question is which side is right — not which file to update.

#![cfg(all(test, not(target_arch = "wasm32")))]

use std::collections::BTreeMap;
use std::path::PathBuf;

use entity_hash::Hash;
use entity_store::MemoryContentStore;

use super::format::{NavItem, SiteManifest, SitePage};

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gpin4-joint")
}

/// Decode one nav node, recursively. Absent `target` is a section header and
/// absent `children` is a leaf — both are the optional-absent cases the fixture
/// exists to exercise, so neither is defaulted into existence here.
fn nav_from_json(v: &serde_json::Value) -> NavItem {
    let label = v.get("label").and_then(|x| x.as_str()).unwrap_or_default().to_string();
    let target = v.get("target").and_then(|x| x.as_str()).unwrap_or_default().to_string();
    let children = v
        .get("children")
        .and_then(|x| x.as_array())
        .map(|a| a.iter().map(nav_from_json).collect())
        .unwrap_or_default();
    NavItem { label, target, children }
}

fn string_map(v: Option<&serde_json::Value>) -> BTreeMap<String, String> {
    v.and_then(|x| x.as_object())
        .map(|o| {
            o.iter()
                .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// Author the whole fixture into the site-relative binding set a publisher
/// would commit: `manifest` plus one `pages/{slug}` per page.
///
/// **These are the keys relative to the site subgraph root**, which is the
/// operand `G-PIN-4` compares — not the peer-relative keys either publisher's
/// own published-root is keyed by.
fn author_fixture() -> (BTreeMap<String, Hash>, usize) {
    let raw = std::fs::read_to_string(fixture_dir().join("site.json"))
        .expect("the joint fixture is in the tree");
    let doc: serde_json::Value = serde_json::from_str(&raw).expect("the joint fixture is JSON");

    let manifest = SiteManifest {
        site_id: doc["site_id"].as_str().expect("site_id").to_string(),
        title: doc["title"].as_str().expect("title").to_string(),
        nav: doc["nav"].as_array().expect("nav").iter().map(nav_from_json).collect(),
        params: string_map(doc.get("params")),
    };

    let mut bindings = BTreeMap::new();
    bindings.insert("manifest".to_string(), manifest.to_entity().content_hash);

    let pages = doc["pages"].as_array().expect("pages");
    for p in pages {
        let page = SitePage {
            format: p["format"].as_str().expect("format").to_string(),
            body: p["body"].as_str().expect("body").to_string(),
            frontmatter: string_map(p.get("frontmatter")),
        };
        let slug = p["slug"].as_str().expect("slug");
        bindings.insert(format!("pages/{slug}"), page.to_entity().content_hash);
    }
    (bindings, pages.len())
}

/// `EXPECTED.json`, as `(key → wire-hex hash)` plus the trie root.
fn read_expected() -> (BTreeMap<String, String>, String) {
    let raw = std::fs::read_to_string(fixture_dir().join("EXPECTED.json"))
        .expect("EXPECTED.json is in the tree — run with GPIN4_REGENERATE=1 to mint it");
    let doc: serde_json::Value = serde_json::from_str(&raw).expect("EXPECTED.json is JSON");
    let bindings = doc["bindings"]
        .as_object()
        .expect("EXPECTED.json has a bindings map")
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().expect("a hex hash").to_string()))
        .collect();
    (bindings, doc["site_root"].as_str().expect("site_root").to_string())
}

fn write_expected(bindings: &BTreeMap<String, Hash>, root: &Hash) {
    let map: serde_json::Map<String, serde_json::Value> = bindings
        .iter()
        .map(|(k, h)| (k.clone(), serde_json::Value::String(h.to_hex())))
        .collect();
    let doc = serde_json::json!({
        "_comment": "COMPUTED by entity-browser-rust from site.json. Not hand-editable — \
                     see the module doc on src/content_site/gpin4_joint_fixture.rs. \
                     `site_root` is the EXTENSION-TREE v4.0.2 trie root over `bindings`, \
                     keyed relative to the site subgraph root. It is NOT a published-root \
                     head: that entity carries `published_at` and is not comparable.",
        "convention": "APP-CONVENTION-SEMANTIC-CONTENT-SITE §9 (G-PIN-4)",
        "types": { "manifest": "app/site-manifest", "pages/*": "app/site-page" },
        "site_root": root.to_hex(),
        "bindings": map,
    });
    std::fs::write(
        fixture_dir().join("EXPECTED.json"),
        format!("{}\n", serde_json::to_string_pretty(&doc).expect("serializes")),
    )
    .expect("EXPECTED.json is writable");
    eprintln!("GPIN4_REGENERATE: rewrote EXPECTED.json — site_root {}", root.to_hex());
}

// ---------------------------------------------------------------------------
// The gates
// ---------------------------------------------------------------------------

/// **Our half of `G-PIN-4`'s vocabulary case is pinned and does not drift.**
///
/// Every authored key's content hash and the trie root over them are compared
/// against `EXPECTED.json` — the file the other seat compares *their* publisher
/// against. So a change to our `app/site-manifest` / `app/site-page` encoding
/// reds here, in this repo, on the commit that makes it, rather than surfacing
/// as an unexplained cross-impl divergence later.
///
/// Per-key first and per-key named, for the same reason as the substrate gate:
/// a bare root mismatch costs a session to localize.
#[test]
fn the_joint_fixture_encodes_to_the_hashes_we_published_to_the_other_seat() {
    let (bindings, page_count) = author_fixture();
    let store = MemoryContentStore::new();
    let root = entity_tree::trie::build_trie(&store, &bindings).expect("the site trie builds");

    if std::env::var("GPIN4_REGENERATE").is_ok() {
        write_expected(&bindings, &root);
        return;
    }

    let (expected, expected_root) = read_expected();
    let mut drift = Vec::new();
    for (key, hash) in &bindings {
        match expected.get(key) {
            None => drift.push(format!("  {key:?} — authored {}, not in EXPECTED.json", hash.to_hex())),
            Some(e) if *e != hash.to_hex() => {
                drift.push(format!("  {key:?} — now {}, published {}", hash.to_hex(), e))
            }
            Some(_) => {}
        }
    }
    for key in expected.keys() {
        if !bindings.contains_key(key) {
            drift.push(format!("  {key:?} — in EXPECTED.json, no longer authored"));
        }
    }
    assert!(
        drift.is_empty(),
        "the joint fixture's encoding moved on {} key(s). The other seat is comparing against \
         EXPECTED.json, so regenerating is a WIRE EVENT, not a test fix — establish which side \
         is right first:\n{}",
        drift.len(),
        drift.join("\n")
    );
    assert_eq!(
        root.to_hex(),
        expected_root,
        "the site root moved without any binding moving — the trie construction changed"
    );

    // Anti-vacuity: an EXPECTED.json that had been emptied, or an authoring
    // pass that produced nothing, would satisfy every loop above.
    assert_eq!(
        bindings.len(),
        page_count + 1,
        "the fixture authored {} bindings for {page_count} pages + 1 manifest",
        bindings.len()
    );
    assert!(bindings.len() >= 8, "the joint fixture shrank below its designed coverage");

    eprintln!(
        "G-PIN-4 (vocabulary) — our half: {} bindings, site_root {}",
        bindings.len(),
        root.to_hex()
    );
}

/// **The optional keys are ABSENT, not empty — the divergence the fixture was
/// shaped around.**
///
/// `frontmatter`, `params`, `target` and `children` are all conditional on our
/// side and `omitempty` on theirs. Two implementations that disagree about
/// *empty vs absent* produce different bytes for the same authored content, and
/// nothing about either one looks wrong on its own. Asserted on the encoded
/// bytes rather than on a round-trip, because a round-trip is exactly what
/// cannot see this.
#[test]
fn an_absent_optional_is_not_an_empty_one() {
    let bare = SitePage {
        format: "markdown".into(),
        body: "# Bare\n".into(),
        frontmatter: BTreeMap::new(),
    };
    let mut with_empty_looking = bare.clone();
    with_empty_looking.frontmatter.insert(String::new(), String::new());

    assert_ne!(
        bare.to_entity().content_hash.to_hex(),
        with_empty_looking.to_entity().content_hash.to_hex(),
        "a page with no frontmatter hashes the same as one carrying a key — the optional \
         key is being emitted either way, and the fixture's `glossary` row measures nothing"
    );

    let encoded = bare.to_entity().data;
    let value: ciborium::Value =
        ciborium::from_reader(encoded.as_slice()).expect("the authored page decodes");
    let keys: Vec<String> = value
        .as_map()
        .expect("a map")
        .iter()
        .filter_map(|(k, _)| k.as_text().map(str::to_string))
        .collect();
    assert_eq!(
        keys,
        vec!["body".to_string(), "format".to_string()],
        "a page with no frontmatter must encode exactly `body` + `format`, in ECF's canonical \
         order (length, then lexical) — this is the byte sequence the other seat reproduces"
    );

    let header = NavItem { label: "Reference".into(), target: String::new(), children: Vec::new() };
    let value: ciborium::Value = ciborium::from_reader(
        SiteManifest {
            site_id: "x".into(),
            title: "x".into(),
            nav: vec![header],
            params: BTreeMap::new(),
        }
        .to_entity()
        .data
        .as_slice(),
    )
    .expect("the authored manifest decodes");
    let nav = value.as_map().expect("a map").iter().find(|(k, _)| k.as_text() == Some("nav"));
    let node = nav.expect("nav is present").1.as_array().expect("nav is an array")[0]
        .as_map()
        .expect("a nav node is a map");
    let node_keys: Vec<&str> = node.iter().filter_map(|(k, _)| k.as_text()).collect();
    assert_eq!(
        node_keys,
        vec!["label"],
        "a section header with no target must carry `label` alone (spec `nav-node.? target`)"
    );
}

// ---------------------------------------------------------------------------
// Link 1 — source directory → site entities
// ---------------------------------------------------------------------------

/// **`G-PIN-4`'s first link, the one neither implementation had a fixture for.**
///
/// Both seats decomposed `G-PIN-4` into three links independently and agreed on
/// the decomposition and on which link was unmeasured:
///
/// | link | state |
/// |---|---|
/// | source fixture → site entity | **this module** — was *"no fixture on either side"* |
/// | site entity → canonical bytes | pinned above; **independently confirmed** by `entity-workbench-go` decoding our August emission through their types and re-encoding byte-identically |
/// | binding set → CHAMP root | green above; **independently confirmed** — their reader rebuilt our signed root in a fresh store |
///
/// `entity-workbench-go` wrote *"we would rather take your directory layout than
/// propose one"*, and this is that layout: `tests/fixtures/gpin4-joint/source/`,
/// the content-team `render/` contract [`super::super::ingest`] already consumes
/// — a specified, deterministic emit shape rather than one invented for a gate.
///
/// **What this pins is OUR answer, not the agreed one.** `EXPECTED-INGEST.json`
/// records what our ingest makes of that directory, field by field, so their
/// ingest has something to disagree with. A red on their side is a divergence
/// to route, not a defect on either — which of the two readings is right is the
/// question the fixture exists to raise.
///
/// **Two roots, and the reason CHANGED on 2026-09-10 — read this before
/// collapsing them.** `site_root_pages` covers the manifest and pages;
/// `site_root_full` adds the asset.
///
/// They were split because *our own asset representation* was
/// unreproducible: bytes inline at any size, never through a chunker, so
/// §6.1's canonical `chunk_size` MUST was **bypassed** and no
/// reproducible-publish check could observe the path (arch made exactly that
/// argument normative in §4's `[MUST]`). That is fixed —
/// [`the_asset_takes_the_pointer_arm_and_its_blob_is_canonically_chunked`] —
/// and `site_root_full` is now a root a conforming publisher reproduces.
///
/// **They stay split anyway, for a different and weaker reason:**
/// `entity-workbench-go` does not model `app/site-asset` at all. So the
/// remaining incomparability is *the other seat has not implemented the
/// type*, not *our bytes are unreproducible* — and that distinction is the
/// whole value of keeping the pair. The plan that scheduled this work
/// predicted the roots would collapse into one; they do not, and one root
/// would delete the pages-only comparand that works today in exchange for a
/// full root nobody can currently compare against.
mod source_ingest {
    use super::*;
    use crate::content_site::asset_store;
    use crate::content_site::format::SitePage;
    use crate::content_site::ingest;
    use entity_store::ContentStore;
    use std::sync::Arc;

    fn source_dir() -> PathBuf {
        fixture_dir().join("source")
    }

    /// Ingest the source fixture, returning the site and the store its
    /// oversized assets were chunked into (the pointer arm needs both — the
    /// asset names a blob and the blob lives here).
    fn ingest_source() -> (ingest::IngestedSite, Arc<dyn ContentStore>) {
        let store: Arc<dyn ContentStore> = Arc::new(MemoryContentStore::new());
        let site = ingest::read_site_dir(&source_dir(), &store)
            .expect("the source fixture ingests");
        (site, store)
    }

    /// `(relative key → hash)` for the ingested site, site-subtree-scoped:
    /// `manifest`, `pages/{slug}`, `assets/{name}` — the keys
    /// `entity-workbench-go` confirmed are spelled identically on both sides
    /// once the peer prefix is trimmed. Returns `(pages-only, full)`.
    fn bindings(site: &ingest::IngestedSite) -> (BTreeMap<String, Hash>, BTreeMap<String, Hash>) {
        let mut pages = BTreeMap::new();
        pages.insert("manifest".to_string(), site.manifest.to_entity().content_hash);
        for (slug, page) in &site.pages {
            pages.insert(format!("pages/{slug}"), page.to_entity().content_hash);
        }
        let mut full = pages.clone();
        for (name, asset) in &site.assets {
            full.insert(format!("assets/{name}"), asset.to_entity().content_hash);
        }
        (pages, full)
    }

    fn root_of(b: &BTreeMap<String, Hash>) -> Hash {
        entity_tree::trie::build_trie(&MemoryContentStore::new(), b).expect("the site trie builds")
    }

    fn nav_json(n: &NavItem) -> serde_json::Value {
        let mut o = serde_json::Map::new();
        o.insert("label".into(), serde_json::Value::String(n.label.clone()));
        o.insert("target".into(), serde_json::Value::String(n.target.clone()));
        if !n.children.is_empty() {
            o.insert(
                "children".into(),
                serde_json::Value::Array(n.children.iter().map(nav_json).collect()),
            );
        }
        serde_json::Value::Object(o)
    }

    fn describe(site: &ingest::IngestedSite) -> serde_json::Value {
        let m = &site.manifest;
        serde_json::json!({
            "manifest": {
                "site_id": m.site_id,
                "title": m.title,
                "params": m.params,
                "nav": m.nav.iter().map(nav_json).collect::<Vec<_>>(),
            },
            "pages": site.pages.iter().map(|(slug, p)| serde_json::json!({
                "slug": slug,
                "format": p.format,
                "frontmatter": p.frontmatter,
                "body": p.body,
            })).collect::<Vec<_>>(),
            "assets": site.assets.iter().map(|(name, a)| serde_json::json!({
                "name": name,
                "media_type": a.media_type,
                // The payload arm and, for a pointer, the blob hash. **The
                // blob hash is the new cross-impl comparand for an asset's
                // bytes**: it is a function of the bytes and §6.1's canonical
                // FastCDC parameters alone, so a conforming publisher on any
                // core reproduces it. Recording only a byte count, as this
                // did while the path was all-inline, pins nothing a second
                // implementation could disagree with.
                "payload": match &a.payload {
                    crate::content_site::format::AssetPayload::Inline(b) => serde_json::json!({
                        "tag": "inline",
                        "bytes": b.len(),
                    }),
                    crate::content_site::format::AssetPayload::Pointer(h) => serde_json::json!({
                        "tag": "pointer",
                        "blob": h.to_hex(),
                    }),
                    // Not a wire arm. If it ever reaches the fixture our own
                    // ingest failed to encode an asset it just read off disk,
                    // and pinning that as an expected value would publish the
                    // failure to the other seat as if it were a decision.
                    crate::content_site::format::AssetPayload::Unsupported { tag } => panic!(
                        "the ingest produced a `{tag}` payload for {name} — our own stager emits \
                         only inline and pointer, so this cannot be a fixture value"
                    ),
                    crate::content_site::format::AssetPayload::InvalidForType { tag } => panic!(
                        "the ingest produced a `{tag}` payload for {name} — SITE §4 forbids it on \
                         an app/site-asset, so emitting one would publish a schema violation to \
                         the other seat as a fixture value"
                    ),
                    crate::content_site::format::AssetPayload::Unreadable => panic!(
                        "the ingest produced an unreadable payload for {name} — that is a defect \
                         here, not a fixture value"
                    ),
                },
            })).collect::<Vec<_>>(),
            // The blob + chunk entities the pointer assets resolve through,
            // by hash. Not trie keys — content-addressed, reached from inside
            // the asset entity — so they are recorded here rather than in
            // `bindings`, and a change in chunk boundaries shows up as a
            // changed list rather than silently inside one opaque root.
            "content": site.content.iter().map(|e| serde_json::json!({
                "type": e.entity_type,
                "hash": e.content_hash.to_hex(),
            })).collect::<Vec<_>>(),
        })
    }

    fn expected_path() -> PathBuf {
        fixture_dir().join("EXPECTED-INGEST.json")
    }

    /// **What our ingest makes of `source/`, pinned so the other seat has
    /// something to disagree with.**
    ///
    /// Regenerated with `GPIN4_REGENERATE=1`, and the same rule applies as to
    /// `EXPECTED.json`: **a regeneration not accompanied by a deliberate
    /// `source/` edit is a wire event, not a test fix.**
    #[test]
    fn our_ingest_of_the_source_directory_is_pinned_field_by_field() {
        let (site, _store) = ingest_source();
        let (pages, full) = bindings(&site);
        let (root_pages, root_full) = (root_of(&pages), root_of(&full));

        let doc = serde_json::json!({
            "_comment": "COMPUTED by entity-browser-rust's ingest from source/. Not hand-editable. \
                         This is OUR answer to G-PIN-4's first link, not the agreed one — a \
                         divergence here is routed, not corrected. See the module doc on \
                         src/content_site/gpin4_joint_fixture.rs.",
            "convention": "APP-CONVENTION-SEMANTIC-CONTENT-SITE §9 (G-PIN-4), link 1 of 3",
            "site_root_pages": root_pages.to_hex(),
            "site_root_full": root_full.to_hex(),
            "roots_are_split_because":
                "the asset row is now REPRODUCIBLE but not yet COMPARABLE, and those are \
                 different things. It was split because our asset was inline at any size, so \
                 §6.1's canonical chunking was bypassed rather than exercised and no \
                 reproducible-publish check could observe the path; that is fixed — the asset \
                 takes APP-CONVENTION-EMBED §3's pointer arm and `payload.blob` is 1 MiB-average \
                 FastCDC over the source bytes, which any conforming publisher reproduces. What \
                 remains is that entity-workbench-go does not model app/site-asset at all, so \
                 there is nothing on the other side to compare the row against. Compare \
                 site_root_pages until a second seat implements the type; compare \
                 site_root_full the day one does.",
            "entities": describe(&site),
            "bindings": full.iter().map(|(k, h)| (k.clone(), serde_json::Value::String(h.to_hex())))
                .collect::<serde_json::Map<_, _>>(),
        });

        if std::env::var("GPIN4_REGENERATE").is_ok() {
            std::fs::write(
                expected_path(),
                format!("{}\n", serde_json::to_string_pretty(&doc).expect("serializes")),
            )
            .expect("EXPECTED-INGEST.json is writable");
            eprintln!(
                "GPIN4_REGENERATE: rewrote EXPECTED-INGEST.json — pages-root {}",
                root_pages.to_hex()
            );
            return;
        }

        let raw = std::fs::read_to_string(expected_path())
            .expect("EXPECTED-INGEST.json is in the tree — GPIN4_REGENERATE=1 to mint it");
        let expected: serde_json::Value =
            serde_json::from_str(&raw).expect("EXPECTED-INGEST.json is JSON");

        assert_eq!(
            doc["entities"], expected["entities"],
            "our ingest of source/ produced different entities than the file the other seat is \
             comparing against. Regenerating is a WIRE EVENT — establish which reading is right first."
        );
        assert_eq!(doc["site_root_pages"], expected["site_root_pages"]);
        assert_eq!(doc["site_root_full"], expected["site_root_full"]);

        // Anti-vacuity: an emptied fixture, or a read that produced nothing,
        // would satisfy every comparison above.
        assert_eq!(site.pages.len(), 6, "the source fixture carries six pages");
        assert_eq!(site.assets.len(), 1, "and exactly one asset");
        assert_ne!(root_pages.to_hex(), root_full.to_hex(), "the asset must move the full root");

        eprintln!(
            "G-PIN-4 link 1 — our ingest: {} pages + {} asset -> pages-root {} / full-root {}",
            site.pages.len(),
            site.assets.len(),
            root_pages.to_hex(),
            root_full.to_hex()
        );
    }

    /// **The fixture's asset is over the ceiling, so it is the case §4's
    /// `[MUST]` governs — and this is the test that used to assert the
    /// opposite.**
    ///
    /// Its predecessor was `the_asset_is_one_inline_entity_at_any_size`, and
    /// it was a correct measurement of a non-conformant path: `SiteAsset` put
    /// the raw bytes inline at any size, so §6.1's canonical chunking was not
    /// *failed* but **unreachable**, and no reproducible-publish check could
    /// observe it. `entity-workbench-go` had diagnosed the symptom (*"nothing
    /// either seat holds exercises chunking"*); arch made the reasoning
    /// normative. **A test naming the behaviour we shipped is not evidence the
    /// behaviour is right** — AP45 — and this one is the reason the whole
    /// asset row was excluded from the comparison.
    ///
    /// What it pins now: the arm, the ceiling on the *right* side of the
    /// boundary, and the blob's reproducible identity. The byte-level
    /// two-sided boundary check is `asset_store`'s, where the decision lives.
    #[test]
    fn the_asset_takes_the_pointer_arm_and_its_blob_is_canonically_chunked() {
        let (site, store) = ingest_source();
        let (name, asset) = site.assets.first().expect("the fixture carries an asset");

        let source_len = std::fs::metadata(source_dir().join("assets").join(name))
            .expect("the fixture asset is on disk")
            .len() as usize;
        assert!(
            source_len > crate::content_site::format::INLINE_PAYLOAD_MAX,
            "the fixture's asset must clear the 16 KiB ceiling or this test measures nothing: \
             {name} is {source_len} bytes"
        );

        let blob = asset
            .pointer()
            .expect("an asset over the ceiling MUST use a pointer payload (content-site §4)");
        assert!(
            asset.inline_bytes().is_none(),
            "the pointer and inline arms are exclusive — carrying both would let two readers \
             disagree about which is authoritative"
        );

        // The bytes are in the content store, chunked at §6.1's canonical
        // parameters, and come back byte-exact. **`create_blob_fastcdc` at
        // CANONICAL_CHUNK_SIZE is re-derived here from the raw file rather
        // than read off the asset**, so this compares our pipeline's answer
        // against the spec's recipe instead of against itself.
        let raw = std::fs::read(source_dir().join("assets").join(name)).expect("readable");
        let recipe: Arc<dyn ContentStore> = Arc::new(MemoryContentStore::new());
        let recipe_blob = entity_content::create_blob_fastcdc(
            &recipe,
            &raw,
            crate::content_site::format::CANONICAL_CHUNK_SIZE,
        )
        .expect("the canonical chunker runs");
        assert_eq!(
            blob, recipe_blob,
            "the published blob hash must equal 1 MiB-average FastCDC over the source bytes — \
             that equality IS §6.1's 'same image → same site root' property, and it is what a \
             second implementation reproduces without seeing our code"
        );
        assert_eq!(
            asset_store::resolve_in(asset, &store).expect("resolves"),
            raw,
            "and it round-trips to the original bytes"
        );

        assert_eq!(site.assets.len(), 1, "anti-vacuity: one source file, one asset");
        assert!(
            !site.content.is_empty(),
            "anti-vacuity: the ingest carried the blob closure out with it"
        );
    }

    /// **A markdown page with no frontmatter stores `title: ""`; the HTML path
    /// deliberately does not.** Found while building the source fixture.
    ///
    /// `page_from_html` removes an empty title with the reason written down —
    /// *"leave the key absent rather than storing an empty string, so the
    /// slug-humanizing fallback names the page (an empty title would render as a
    /// blank breadcrumb, which reads as broken)"*. `page_from_markdown` does the
    /// opposite: `SitePage::markdown` inserts `title` unconditionally, so a
    /// frontmatter-less `.md` gets exactly the empty string the sibling path was
    /// fixed to avoid.
    ///
    /// **Pinned rather than fixed, and deliberately so on both counts.** It is a
    /// live rendering question *and* a cross-impl byte question — `{"title": ""}`
    /// and an absent key are different bytes, so whichever way it is resolved has
    /// to be resolved once, by both seats, not repaired here while the other
    /// side is comparing against these bytes. Routed with the fixture.
    #[test]
    fn a_frontmatterless_markdown_page_keeps_an_empty_title_and_the_html_path_does_not() {
        let (site, _store) = ingest_source();
        let by_slug: BTreeMap<&str, &SitePage> =
            site.pages.iter().map(|(s, p)| (s.as_str(), p)).collect();

        let md = by_slug["glossary"];
        assert_eq!(md.format, "markdown");
        assert_eq!(
            md.frontmatter.get("title").map(String::as_str),
            Some(""),
            "measured: the markdown path stores an EMPTY title for a frontmatter-less page"
        );

        let html = by_slug["report"];
        assert_eq!(html.format, "html");
        assert_eq!(
            html.frontmatter.get("title").map(String::as_str),
            Some("Report"),
            "the html path lifts <title>"
        );

        // The asymmetry, stated as the thing under review rather than left
        // implied by the two assertions above: same absence, two encodings.
        assert!(
            md.frontmatter.contains_key("title") && md.frontmatter["title"].is_empty(),
            "if this ever becomes an absent key, the fixture's bytes move and the other seat's \
             comparison moves with it — that is a routed decision, not a silent repair"
        );
    }

    /// **Our ingest cannot express a nav section with no `target`**, so §2's
    /// `nav-node.? target` optional-absent case is unreachable through this
    /// path — `parse_nav` derives a header's target from its children
    /// (`section_target_from_children`) rather than leaving it empty.
    ///
    /// Recorded because the hand-authored `site.json` fixture one module up
    /// *does* carry a target-less header, so the two halves of this fixture
    /// cover different things and neither is redundant. It also means a
    /// cross-impl check driven only from a source directory would never see
    /// that arm.
    #[test]
    fn no_nav_node_from_a_source_directory_lacks_a_target() {
        let (site, _store) = ingest_source();
        fn walk(items: &[NavItem], out: &mut Vec<String>) {
            for i in items {
                if i.target.is_empty() {
                    out.push(i.label.clone());
                }
                walk(&i.children, out);
            }
        }
        let mut empty = Vec::new();
        walk(&site.manifest.nav, &mut empty);
        assert!(
            empty.is_empty(),
            "a source-directory ingest produced target-less nav nodes {empty:?} — if that is now \
             possible the `site.json` half is no longer covering a case this half cannot"
        );
        assert!(!site.manifest.nav.is_empty(), "anti-vacuity: the fixture has a nav");
    }
}

//! Content-site entity types + CBOR codec.
//!
//! The cross-impl contract is **`APP-CONVENTION-SEMANTIC-CONTENT-SITE`
//! v0.4.2** (locked, in `../entity-core-architecture/.../applications/`).
//! Floor is **raw markdown in entities**: a page stores its markdown body
//! as a string + a `format` marker; the renderer translates
//! markdown→HTML at the last minute. Links use the entity-native scheme
//! classified in [`super::location`]. Assets are deferred (passive
//! `Embed`, post-v1).
//!
//! Migration note: the type tags moved
//! `content/site/*` → `app/site-*` (`F-8`, the `applications/` domain),
//! the manifest gained `site_id` + an open `params` bag (landing page =
//! `params.root`, our reasonable v1 choice for the spec's absent
//! top-level `root`), the page gained a `format` marker and relocated
//! its `title` into `frontmatter`, and `NavItem.target` is now optional
//! (section headers carry none).
//!
//! The codec mirrors the Knowledge Base pattern (`entity_ecf::to_ecf` to
//! encode, `ciborium` to decode, lossy `from_entity` returning a default
//! on malformed input). String-keyed maps (`params`, `frontmatter`) use
//! `BTreeMap` for deterministic, byte-stable output.

#![allow(dead_code)] // some accessors land with later renderer/edit surfaces

use std::collections::BTreeMap;

use entity_entity::Entity;

/// Entity type for a site manifest (the site's cover — identity + the
/// optional human nav menu). `app/site-*` per the locked convention §4.
pub const SITE_MANIFEST_TYPE: &str = "app/site-manifest";

/// Entity type for a single site page (markdown/html body + frontmatter).
pub const SITE_PAGE_TYPE: &str = "app/site-page";

/// Entity type for a site asset — the raw bytes of an embedded resource
/// (image/figure) plus its media type. Stored site-subgraph-bound at
/// [`super::paths::asset_path`]; the bytes content-address, so the same image
/// across sites dedups in the store (one blob, many tree refs). An embed's
/// `ref` (`assets/figures/x.png`) resolves to one of these.
pub const SITE_ASSET_TYPE: &str = "app/site-asset";

/// The landing page slug used when a manifest declares no `params.root`.
pub const DEFAULT_ROOT_PAGE: &str = "index";

/// Default base format for a page body.
pub const DEFAULT_PAGE_FORMAT: &str = "markdown";

/// One navigation/menu entry — "this is the menu, this is where the
/// links go." `target` is an entity-native link (see
/// [`super::location::classify_link`]); it is **optional** — an empty
/// `target` is a section header with no link (spec `nav-node.? target`).
///
/// `children` lets a top-level entry declare a section sub-menu (the
/// deep-site cycle's GAP3). It is **optional and
/// back-compatible**: a flat nav has no children and serializes
/// byte-identically to the pre-nesting format (the `children` key is
/// emitted only when non-empty), and an older flat reader ignores the
/// key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NavItem {
    pub label: String,
    /// Empty = section header (no link). Emitted only when non-empty.
    pub target: String,
    pub children: Vec<NavItem>,
}

impl NavItem {
    /// A leaf nav entry (no sub-menu).
    pub fn new(label: impl Into<String>, target: impl Into<String>) -> Self {
        Self { label: label.into(), target: target.into(), children: Vec::new() }
    }

    /// A section nav entry with a sub-menu of children.
    pub fn section(
        label: impl Into<String>,
        target: impl Into<String>,
        children: Vec<NavItem>,
    ) -> Self {
        Self { label: label.into(), target: target.into(), children }
    }

    /// Encode this entry (recursively) to a CBOR map value.
    fn to_value(&self) -> entity_ecf::Value {
        let mut pairs =
            vec![(entity_ecf::Value::Text("label".into()), entity_ecf::text(&self.label))];
        // `target` is optional (section headers have none); emit only
        // when present so a header carries no empty link.
        if !self.target.is_empty() {
            pairs.push((entity_ecf::Value::Text("target".into()), entity_ecf::text(&self.target)));
        }
        // Back-compat: only emit `children` when present, so a flat nav's
        // wire bytes are unchanged from the pre-nesting format.
        if !self.children.is_empty() {
            let kids: Vec<entity_ecf::Value> = self.children.iter().map(NavItem::to_value).collect();
            pairs.push((entity_ecf::Value::Text("children".into()), entity_ecf::Value::Array(kids)));
        }
        entity_ecf::Value::Map(pairs)
    }

    /// Decode an entry (recursively) from a CBOR map value; `None` if not
    /// a map. Missing `target` decodes to a section header (empty);
    /// missing `children` to an empty sub-menu.
    fn from_value(v: &ciborium::Value) -> Option<Self> {
        let map = v.as_map()?;
        let mut item = NavItem::default();
        for (k, val) in map {
            match k.as_text() {
                Some("label") => {
                    if let Some(s) = val.as_text() {
                        item.label = s.to_string();
                    }
                }
                Some("target") => {
                    if let Some(s) = val.as_text() {
                        item.target = s.to_string();
                    }
                }
                Some("children") => {
                    if let Some(arr) = val.as_array() {
                        item.children = arr.iter().filter_map(NavItem::from_value).collect();
                    }
                }
                _ => {}
            }
        }
        Some(item)
    }
}

/// A site manifest: the site's **cover** — stable id, title, the curated
/// nav menu, and an open `params` attribute bag.
///
/// Per the locked convention §4 the manifest holds **no** page-collection
/// field (that was the killed `pages`); discovery is lazy `.list`. The
/// spec has no top-level `root` (site identity is the subtree root hash);
/// we keep the landing-page pointer in `params.root` — a reasonable v1
/// choice to run by architecture, not a non-spec top-level key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SiteManifest {
    pub site_id: String,
    pub title: String,
    pub nav: Vec<NavItem>,
    /// Open string-keyed attribute bag (spec `params`). v1 carries
    /// `root` (the landing page slug). Sorted for byte-stable output.
    pub params: BTreeMap<String, String>,
}

impl SiteManifest {
    /// Build a manifest with the landing page recorded in `params.root`.
    pub fn new(
        site_id: impl Into<String>,
        title: impl Into<String>,
        root: impl Into<String>,
        nav: Vec<NavItem>,
    ) -> Self {
        let mut params = BTreeMap::new();
        params.insert("root".to_string(), root.into());
        Self { site_id: site_id.into(), title: title.into(), nav, params }
    }

    /// The landing page slug — `params.root`, defaulting to `index`.
    pub fn root(&self) -> &str {
        self.params.get("root").map(String::as_str).unwrap_or(DEFAULT_ROOT_PAGE)
    }

    pub fn from_entity(entity: &Entity) -> Self {
        let value: ciborium::Value = match ciborium::from_reader(entity.data.as_slice()) {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };
        let map = match value.as_map() {
            Some(m) => m,
            None => return Self::default(),
        };
        let mut out = Self::default();
        for (k, v) in map {
            match k.as_text() {
                Some("site_id") => {
                    if let Some(s) = v.as_text() {
                        out.site_id = s.to_string();
                    }
                }
                Some("title") => {
                    if let Some(s) = v.as_text() {
                        out.title = s.to_string();
                    }
                }
                Some("nav") => {
                    if let Some(arr) = v.as_array() {
                        out.nav = arr.iter().filter_map(NavItem::from_value).collect();
                    }
                }
                Some("params") => out.params = decode_string_map(v),
                _ => {}
            }
        }
        out
    }

    pub fn to_entity(&self) -> Entity {
        let nav_items: Vec<entity_ecf::Value> = self.nav.iter().map(NavItem::to_value).collect();
        let mut pairs = vec![
            (entity_ecf::Value::Text("site_id".into()), entity_ecf::text(&self.site_id)),
            (entity_ecf::Value::Text("title".into()), entity_ecf::text(&self.title)),
            (entity_ecf::Value::Text("nav".into()), entity_ecf::Value::Array(nav_items)),
        ];
        if !self.params.is_empty() {
            pairs.push((entity_ecf::Value::Text("params".into()), encode_string_map(&self.params)));
        }
        let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(pairs));
        Entity::new(SITE_MANIFEST_TYPE, data).unwrap()
    }
}

/// A single page: a base-format body + frontmatter (title required by
/// convention; derivable from the first H1 otherwise).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SitePage {
    /// Base format — `markdown` (default) | `html` (web escape hatch, §3.1).
    pub format: String,
    pub body: String,
    /// Frontmatter map; `title` is the one well-known key. Sorted for
    /// byte-stable output.
    pub frontmatter: BTreeMap<String, String>,
}

impl Default for SitePage {
    fn default() -> Self {
        Self {
            format: DEFAULT_PAGE_FORMAT.to_string(),
            body: String::new(),
            frontmatter: BTreeMap::new(),
        }
    }
}

impl SitePage {
    /// A markdown page with `frontmatter.title` set.
    pub fn markdown(title: impl Into<String>, body: impl Into<String>) -> Self {
        let mut frontmatter = BTreeMap::new();
        frontmatter.insert("title".to_string(), title.into());
        Self { format: DEFAULT_PAGE_FORMAT.to_string(), body: body.into(), frontmatter }
    }

    /// The page title — `frontmatter.title`, or empty if unset.
    pub fn title(&self) -> &str {
        self.frontmatter.get("title").map(String::as_str).unwrap_or("")
    }

    pub fn from_entity(entity: &Entity) -> Self {
        let value: ciborium::Value = match ciborium::from_reader(entity.data.as_slice()) {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };
        let map = match value.as_map() {
            Some(m) => m,
            None => return Self::default(),
        };
        let mut out = Self::default();
        for (k, v) in map {
            match k.as_text() {
                Some("format") => {
                    if let Some(s) = v.as_text() {
                        out.format = s.to_string();
                    }
                }
                Some("body") => {
                    if let Some(s) = v.as_text() {
                        out.body = s.to_string();
                    }
                }
                Some("frontmatter") => out.frontmatter = decode_string_map(v),
                _ => {}
            }
        }
        out
    }

    pub fn to_entity(&self) -> Entity {
        let mut pairs = vec![
            (entity_ecf::Value::Text("format".into()), entity_ecf::text(&self.format)),
            (entity_ecf::Value::Text("body".into()), entity_ecf::text(&self.body)),
        ];
        if !self.frontmatter.is_empty() {
            pairs.push((
                entity_ecf::Value::Text("frontmatter".into()),
                encode_string_map(&self.frontmatter),
            ));
        }
        let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(pairs));
        Entity::new(SITE_PAGE_TYPE, data).unwrap()
    }
}

/// A site asset — raw resource bytes + media type. The renderer resolves an
/// embed `ref` to one of these and builds a `data:` URL from `(media_type,
/// bytes)`; the publish/cache closure carries them as content-addressed blobs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SiteAsset {
    /// IANA media type (`image/png`, `image/svg+xml`, …).
    pub media_type: String,
    /// The raw resource bytes.
    pub bytes: Vec<u8>,
}

impl SiteAsset {
    pub fn new(media_type: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self { media_type: media_type.into(), bytes }
    }

    pub fn from_entity(entity: &Entity) -> Self {
        let value: ciborium::Value = match ciborium::from_reader(entity.data.as_slice()) {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };
        let map = match value.as_map() {
            Some(m) => m,
            None => return Self::default(),
        };
        let mut out = Self::default();
        for (k, v) in map {
            match k.as_text() {
                Some("media_type") => {
                    if let Some(s) = v.as_text() {
                        out.media_type = s.to_string();
                    }
                }
                Some("bytes") => {
                    if let Some(b) = v.as_bytes() {
                        out.bytes = b.clone();
                    }
                }
                _ => {}
            }
        }
        out
    }

    pub fn to_entity(&self) -> Entity {
        let pairs = vec![
            (entity_ecf::Value::Text("media_type".into()), entity_ecf::text(&self.media_type)),
            (entity_ecf::Value::Text("bytes".into()), entity_ecf::Value::Bytes(self.bytes.clone())),
        ];
        let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(pairs));
        Entity::new(SITE_ASSET_TYPE, data).unwrap()
    }
}

/// Best-effort IANA media type for an asset path, by extension. Unknown →
/// `application/octet-stream`. Covers the image set the content pipeline emits.
pub fn media_type_for_path(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

/// Encode a sorted string→string map to a CBOR map value (encode side).
fn encode_string_map(m: &BTreeMap<String, String>) -> entity_ecf::Value {
    let pairs = m
        .iter()
        .map(|(k, v)| (entity_ecf::Value::Text(k.clone()), entity_ecf::text(v)))
        .collect();
    entity_ecf::Value::Map(pairs)
}

/// Decode a CBOR map value into a string→string map (decode side). v1
/// keeps only string-valued keys (the open `any` value space is
/// string-only until a typed key needs more).
fn decode_string_map(v: &ciborium::Value) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some(map) = v.as_map() {
        for (k, val) in map {
            if let (Some(k), Some(val)) = (k.as_text(), val.as_text()) {
                out.insert(k.to_string(), val.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trips_through_entity() {
        let m = SiteManifest::new(
            "church",
            "Entity Church Foundation",
            "index",
            vec![
                NavItem::new("Home", "./index"),
                NavItem::new("About", "./about"),
                NavItem::new("Labs", "entity://PEERX/sites/labs/pages/intro"),
            ],
        );
        let m2 = SiteManifest::from_entity(&m.to_entity());
        assert_eq!(m2, m);
        assert_eq!(m2.site_id, "church");
        assert_eq!(m2.root(), "index", "landing page survives via params.root");
    }

    #[test]
    fn manifest_root_falls_back_to_index() {
        // A manifest with no params.root resolves the landing page to the
        // `index` convention rather than the empty string.
        let m = SiteManifest::default();
        assert_eq!(m.root(), "index");
    }

    #[test]
    fn nested_nav_round_trips_through_entity() {
        // A section entry with a sub-menu (GAP3 — the format allows
        // nesting). Round-trip preserves the whole tree.
        let m = SiteManifest::new(
            "docs",
            "Docs",
            "index",
            vec![
                NavItem::new("Home", "./index"),
                NavItem::section(
                    "Guide",
                    "./guide/intro",
                    vec![
                        NavItem::new("Intro", "./guide/intro"),
                        NavItem::new("Install", "./guide/install"),
                        NavItem::section(
                            "Advanced",
                            "./guide/advanced/internals",
                            vec![NavItem::new("Internals", "./guide/advanced/internals")],
                        ),
                    ],
                ),
            ],
        );
        let m2 = SiteManifest::from_entity(&m.to_entity());
        assert_eq!(m2, m, "nested nav (2 levels of children) survives the round-trip");
        assert_eq!(m2.nav[1].children.len(), 3);
        assert_eq!(m2.nav[1].children[2].children[0].label, "Internals");
    }

    #[test]
    fn flat_nav_is_wire_compatible_with_pre_nesting_format() {
        // Back-compat guarantee: a flat nav emits NO `children` keys, so a
        // flat manifest carries no nesting marker (the property older
        // readers depend on).
        let flat = SiteManifest::new(
            "flat",
            "Flat",
            "index",
            vec![NavItem::new("Home", "./index"), NavItem::new("About", "./about")],
        );
        let bytes = flat.to_entity().data;
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains("children"),
            "flat nav must not emit a `children` key (wire back-compat)"
        );
    }

    #[test]
    fn section_header_omits_target() {
        // A nav node with an empty target is a section header; its wire
        // form carries no `target` key (spec `nav-node.? target`).
        let m = SiteManifest::new(
            "s",
            "Sectioned",
            "index",
            vec![NavItem::section("Group", "", vec![NavItem::new("Leaf", "./leaf")])],
        );
        let bytes = m.to_entity().data;
        // The header has no target; the only `target` on the wire is the
        // leaf's. Round-trip preserves the empty header target.
        let m2 = SiteManifest::from_entity(&Entity::new(SITE_MANIFEST_TYPE, bytes).unwrap());
        assert_eq!(m2, m);
        assert_eq!(m2.nav[0].target, "", "section header has no link");
        assert_eq!(m2.nav[0].children[0].target, "./leaf");
    }

    #[test]
    fn manifest_entity_uses_correct_type() {
        let m = SiteManifest::default();
        assert_eq!(m.to_entity().entity_type, SITE_MANIFEST_TYPE);
        assert_eq!(SITE_MANIFEST_TYPE, "app/site-manifest");
    }

    #[test]
    fn page_round_trips_through_entity() {
        let p = SitePage::markdown("Welcome", "# Hello\n\nSome **markdown** with a [link](./about).");
        let p2 = SitePage::from_entity(&p.to_entity());
        assert_eq!(p2, p);
        assert_eq!(p2.title(), "Welcome");
        assert_eq!(p2.format, "markdown");
        assert_eq!(p.to_entity().entity_type, SITE_PAGE_TYPE);
    }

    #[test]
    fn page_format_defaults_to_markdown() {
        // A page entity carrying no `format` key decodes as markdown.
        let data = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
            "body" => entity_ecf::text("# Bare")
        });
        let p = SitePage::from_entity(&Entity::new(SITE_PAGE_TYPE, data).unwrap());
        assert_eq!(p.format, "markdown");
        assert_eq!(p.body, "# Bare");
    }

    #[test]
    fn malformed_entity_decodes_to_default() {
        let junk = Entity::new(SITE_PAGE_TYPE, vec![0xff, 0x00, 0x13]).unwrap();
        assert_eq!(SitePage::from_entity(&junk), SitePage::default());
    }

    #[test]
    fn asset_round_trips_through_entity() {
        let a = SiteAsset::new("image/png", vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a]);
        let a2 = SiteAsset::from_entity(&a.to_entity());
        assert_eq!(a2, a);
        assert_eq!(a.to_entity().entity_type, SITE_ASSET_TYPE);
    }

    #[test]
    fn identical_asset_bytes_produce_identical_entity_content() {
        // Content-addressing dedup property: two assets with the same
        // (media_type, bytes) encode to byte-identical entity data, so the
        // store collapses them to one blob regardless of which site refs them.
        let a = SiteAsset::new("image/svg+xml", b"<svg/>".to_vec());
        let b = SiteAsset::new("image/svg+xml", b"<svg/>".to_vec());
        assert_eq!(a.to_entity().data, b.to_entity().data);
    }

    #[test]
    fn media_type_is_inferred_by_extension() {
        assert_eq!(media_type_for_path("figures/x.png"), "image/png");
        assert_eq!(media_type_for_path("a/b/c.SVG"), "image/svg+xml");
        assert_eq!(media_type_for_path("photo.jpeg"), "image/jpeg");
        assert_eq!(media_type_for_path("noext"), "application/octet-stream");
    }
}

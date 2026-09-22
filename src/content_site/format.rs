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

use crate::embed::{EmbedPayload, PayloadError};

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

/// The web-tier base format (convention §3.1): a complete, pre-rendered HTML
/// document carried verbatim. **The exact string is load-bearing** — the
/// renderer fails closed on anything else (`render::render_page`), so a
/// near-miss like `"HTML"` or `"text/html"` silently becomes escaped markdown
/// rather than a document. Every producer and consumer names this constant.
pub const HTML_PAGE_FORMAT: &str = "html";

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

    /// A pre-rendered **HTML document** page (§3.1's web-tier escape hatch) —
    /// a complete standalone file (a Pandoc paper/book, an exported report)
    /// stored verbatim.
    ///
    /// The renderer hands this to a fully-restricted sandbox rather than to our
    /// own document; see `super::render::PageRender::Document`. The `title` is
    /// the *site's* name for the page (nav, breadcrumbs) and is independent of
    /// whatever `<title>` the document carries internally.
    pub fn html(title: impl Into<String>, body: impl Into<String>) -> Self {
        let mut frontmatter = BTreeMap::new();
        frontmatter.insert("title".to_string(), title.into());
        Self { format: HTML_PAGE_FORMAT.to_string(), body: body.into(), frontmatter }
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

/// The inline-payload ceiling — `APP-CONVENTION-EMBED` §3's
/// `inline-payload = { tag: "inline", bytes: bstr .size (1..16384) }`.
///
/// **Re-exported, not defined here.** §3.1 says the bound is *"a property of
/// the payload and not of the addressing"*, so it lives with the union in
/// [`crate::embed`] and every carrier reads the one constant. This alias stays
/// because the site's own callers and vectors name it, and because C15's rule
/// is one *expression*, not one spelling.
///
/// **Bytes at or below this go inline; bytes above it MUST take the pointer
/// arm** (content-site §4's `[MUST]`). Note which side of the boundary each
/// belongs to: 16,384 is the last inline size, 16,385 the first pointer size.
/// `F-5`'s lesson applies — a bound asserted from one side passes for a
/// producer that only ever looked one way, so
/// [`the_inline_ceiling_is_asserted_from_both_sides`] pins both.
///
/// [`the_inline_ceiling_is_asserted_from_both_sides`]: self::tests
pub use crate::embed::INLINE_PAYLOAD_MAX;

/// The canonical v1 publisher chunk size — content-site §6.1's
/// `[LOCKED — G-PIN-4]` default: **1 MiB FastCDC average** (min/avg/max =
/// 256 KiB / 1 MiB / 2 MiB, the shipped params).
///
/// **This constant is the whole reason the pointer arm is a conformance
/// concern rather than a size preference.** *"Same image → same site root"*
/// depends on every v1 publisher chunking identically; bytes that never enter
/// the content store never meet that rule, so an all-inline asset path does
/// not *fail* §6.1, it makes it **unreachable** — which is why our G-PIN-4
/// asset row was incomparable rather than merely large.
pub const CANONICAL_CHUNK_SIZE: usize = 1024 * 1024;

/// An asset's payload — `APP-CONVENTION-EMBED` §3's **tagged** union, reused
/// rather than restated (the content-site CDDL says so in as many words:
/// *"NOT a second payload union; the embed one, reused"*).
///
/// **The union itself now lives in [`crate::embed`], and this enum is the
/// SITE layer's reading of it.** Until phase 2a the sentence above was true of
/// the comment and false of the code: the union was implemented here, private
/// to a codec named for assets, which is why it could not be carried by
/// anything else. The split is the one the conventions already draw —
/// [`EmbedPayload`] is *what the wire admits*, this enum is *what this field
/// admits plus how each refusal is attributed*.
///
/// Two arms of the embed union are reachable for a site asset: `inline` for
/// bounded bytes and `pointer` for a content-store blob. `child` is not — see
/// [`InvalidForType`](Self::InvalidForType).
///
/// **The tag is load-bearing and a decoder MUST reject an untagged payload**
/// (EMBED §3's normative note — it is the silent-divergence risk `G-PIN-2`
/// closes). We refuse by returning `None` from [`AssetPayload::from_value`],
/// which surfaces as a defaulted [`SiteAsset`] rather than a guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetPayload {
    /// `{ tag: "inline", bytes: bstr .size (1..16384) }` — in-tree bytes.
    Inline(Vec<u8>),
    /// `{ tag: "pointer", hash: content-hash }` — a `system/content/blob` in
    /// the resolving peer's own content store, chunked at
    /// [`CANONICAL_CHUNK_SIZE`]. Same-peer by design (EMBED §3.4's
    /// implied-authority form), so it carries no authority term.
    Pointer(entity_hash::Hash),
    /// **NOT A WIRE ARM.** The decoder found no payload it could read: no
    /// `payload` key and no retired top-level `bytes`, or a `payload` that is
    /// untagged / carries an unknown tag (which EMBED §3 requires us to
    /// reject).
    ///
    /// **This variant exists because its absence was a shipped defect.** The
    /// default used to be `Inline(Vec::new())`, so an asset whose payload we
    /// could not read decoded as *an asset with zero bytes* and
    /// [`super::asset_store::resolve`] returned **`Ok(vec![])`** — a success
    /// carrying nothing, which no caller could tell from a publisher shipping
    /// an empty file. Measured on a real 30,778-byte published asset:
    /// `Inline(0 bytes)`, `Ok(0)`, and a renderer emitting
    /// `data:image/png;base64,` with no data. AP40 in the most literal form —
    /// two facts arriving as one value — and it is worse than the usual case
    /// because the collapsed value is on the *success* side.
    ///
    /// **`Inline(Vec::new())` is not even representable on the wire**:
    /// `inline-payload`'s `bstr .size (1..16384)` excludes zero, so the old
    /// default was an illegal state used as the safe one.
    Unreadable,
    /// **A tagged arm of `embed-payload` this build does not implement.**
    ///
    /// **Not `Unreadable`, and the difference is the same one this enum
    /// already exists to keep:** *"this entity is malformed"* and *"this
    /// publisher used a form we have not built"* license different sentences,
    /// and only the second is our gap rather than their defect.
    ///
    /// **`child` is NO LONGER this variant** — see
    /// [`InvalidForType`](Self::InvalidForType). This arm is now reachable
    /// only by a payload tag EMBED §3 does not yet define, i.e. a genuinely
    /// newer producer against an older reader. That is the case where naming
    /// our own gap is the correct attribution.
    Unsupported { tag: String },
    /// **A well-formed, correctly-tagged arm of `embed-payload` that
    /// `app/site-asset` does not admit** — today exactly `child`.
    ///
    /// **A-30, ruled 2026-09-10; `APP-CONVENTION-SEMANTIC-CONTENT-SITE`
    /// v0.5.1 §4 narrows the field to `inline-payload / pointer-payload` and
    /// makes the refusal a `[MUST]`** — *"a reader that decodes one MUST
    /// refuse the entity as invalid for its type, and MUST NOT report it as a
    /// malformed or untagged payload."*
    ///
    /// **The reason the arm is excluded is worth carrying, because it is not
    /// the one we argued.** We reasoned from §4's preamble — an asset is bytes
    /// with a name — and flagged it as an inference from prose. The argument
    /// that does not rest on prose is inside EMBED: `child-payload.ref`
    /// resolves to a sibling `Embed`, and **an `Embed`'s dispatch key IS its
    /// type tag** (`app/embed/{media_type}`), which is exactly why EMBED §3
    /// carries no `data.media_type` — *"a redundant second source of truth."*
    /// A site asset carries `media_type` in `data`. **So a `child`-payload
    /// asset holds two media types that can disagree**, with nothing to say
    /// which wins.
    ///
    /// **THIS VARIANT EXISTS BECAUSE THE OBVIOUS FIX WAS WRONG.** Our own
    /// plan said a "no" on A-30 would move `child` from
    /// [`Unsupported`](Self::Unsupported) to the untagged refusal and *"nothing
    /// else changes"*. That collapses two facts and mis-attributes one: EMBED
    /// §3's rule governs a payload with **no discriminator**, while a `child`
    /// payload here is correctly tagged, well-formed and unambiguous. Calling
    /// it malformed tells an operator the publisher emitted a corrupt byte
    /// when they emitted a **deliberate schema violation** — and those route
    /// to different people. We had drawn this exact distinction one convention
    /// over, for `SHARE-8`, in the same packet that proposed the collapse.
    ///
    /// So there are **four** outcomes and each names whose defect it is:
    /// resolved · our gap ([`Unsupported`](Self::Unsupported)) · **their
    /// schema violation (this)** · their malformed byte
    /// ([`Unreadable`](Self::Unreadable)).
    InvalidForType { tag: String },
}

impl Default for AssetPayload {
    /// [`AssetPayload::Unreadable`] — see that variant for why this is not
    /// an empty inline payload.
    fn default() -> Self {
        AssetPayload::Unreadable
    }
}

impl AssetPayload {
    /// Encode to the tagged CBOR map, or `None` for
    /// [`Unreadable`](AssetPayload::Unreadable) — which is not a wire arm and
    /// must not be fabricated into one.
    ///
    /// A re-encoded unreadable asset therefore carries **no `payload` key** and
    /// decodes back to `Unreadable`. That is deliberate: the alternative is
    /// emitting an empty inline payload, which is both non-conformant
    /// (`.size (1..16384)`) and the exact lie this variant exists to stop.
    fn to_value(&self) -> Option<entity_ecf::Value> {
        let shared = match self {
            AssetPayload::Inline(bytes) => EmbedPayload::Inline(bytes.clone()),
            AssetPayload::Pointer(hash) => EmbedPayload::Pointer(*hash),
            // None of these is a wire arm we may author. `Unsupported` and
            // `InvalidForType` in particular must NOT be re-emitted as an
            // absent payload and then treated as ours — the caller's job is to
            // not write them through at all (see `resolver::persist_to_cache`).
            AssetPayload::Unreadable
            | AssetPayload::Unsupported { .. }
            | AssetPayload::InvalidForType { .. } => return None,
        };
        Some(shared.to_value())
    }

    /// Decode from the tagged CBOR map. `None` for anything that is not a
    /// well-formed tagged arm — **including an untagged map that happens to
    /// carry `bytes`**, which is the pre-declaration shape this repo emitted
    /// and exactly what EMBED §3 requires a decoder to refuse.
    fn from_value(v: &ciborium::Value) -> Option<Self> {
        match EmbedPayload::from_value(v) {
            Ok(EmbedPayload::Inline(bytes)) => Some(AssetPayload::Inline(bytes)),
            Ok(EmbedPayload::Pointer(hash)) => Some(AssetPayload::Pointer(hash)),
            // A-30: `child` is a legal `embed-payload` arm and is NOT admitted
            // by `app/site-asset` (SITE v0.5.1 §4, `[MUST NOT]`). It is their
            // schema violation, not our gap and not a malformed byte, so it
            // gets its own outcome — see `AssetPayload::InvalidForType`.
            Ok(EmbedPayload::Child(_)) => {
                Some(AssetPayload::InvalidForType { tag: "child".to_string() })
            }
            // **A `child` whose reference is itself broken is still the excluded
            // arm.** The field refuses `child` whatever its `ref` says, and this
            // layer never resolves one — adjudicating an atom we would not
            // follow would report the publisher's smaller mistake and hide the
            // one that decides the outcome.
            Err(PayloadError::BadRef { .. }) => {
                Some(AssetPayload::InvalidForType { tag: "child".to_string() })
            }
            // A tag we do not implement is carried, not discarded.
            Err(PayloadError::UnknownTag { tag }) => Some(AssetPayload::Unsupported { tag }),
            // An UNTAGGED payload is `None` — EMBED §3's MUST — and so is a
            // malformed arm. The two are different from *unsupported* and stay
            // different; they are the same to this decoder because both mean
            // *we hold no payload*, which is what `Unreadable` says.
            Err(PayloadError::Untagged) | Err(PayloadError::Malformed { .. }) => None,
        }
    }
}

/// A site asset — a named, site-local binary resource (image, font,
/// stylesheet). The renderer resolves an embed `ref` to one of these and
/// builds a `data:` URL from `(media_type, bytes)`.
///
/// **The bytes are not necessarily here.** `payload` is EMBED §3's tagged
/// union: bounded bytes inline, anything larger as a `pointer` into the
/// content store. Reading an asset's bytes is therefore a *resolution*, not a
/// field access — [`SiteAsset::resolve`] is the one place that knows how, and
/// it takes the content lookup as an argument so every arm (publisher store,
/// browser store, HTTP origin) uses the same decision.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SiteAsset {
    /// IANA media type (`image/png`, `image/svg+xml`, …) — an asset has no
    /// dispatch tag of its own, so the media type is a field here rather than
    /// riding the entity type the way `app/embed/{media_type}` does.
    pub media_type: String,
    /// Inline bytes or a content-store pointer. See [`AssetPayload`].
    pub payload: AssetPayload,
    /// **Decode provenance, never emitted.** `true` when the payload was
    /// recovered from the retired top-level `bytes` key rather than from
    /// `payload` — see [`SiteAsset::from_entity`]'s legacy arm.
    ///
    /// It is here because a concession with no instrument behind it is one
    /// nobody can ever retire: without this, *"is the legacy arm still
    /// load-bearing?"* has no answer short of grepping every origin. Reported
    /// at the one-shot boundaries (an ingest read, an HTTP fetch), **never in
    /// the render loop** — a papers page has twenty figures and would emit
    /// twenty warnings a frame, which is how a real signal gets muted.
    ///
    /// Excluded from the wire in both directions: [`Self::to_entity`] never
    /// writes it, so a legacy asset re-encoded into the cache comes back out
    /// in the declared shape and the flag falls to `false` — the cache heals
    /// and says so.
    pub from_legacy_encoding: bool,
}

impl SiteAsset {
    /// An asset whose bytes are carried **inline**, unconditionally.
    ///
    /// Callers that may hand this bytes of any size want
    /// [`SiteAsset::stage`] instead — this one does not consult the ceiling,
    /// deliberately, so a fixture that means to build a specific arm can.
    pub fn inline(media_type: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            media_type: media_type.into(),
            payload: AssetPayload::Inline(bytes),
            from_legacy_encoding: false,
        }
    }

    /// The asset's inline bytes, if it carries any. `None` for a pointer —
    /// **not an empty slice**, because *"this asset holds no bytes"* and
    /// *"this asset's bytes live elsewhere"* decide different things at every
    /// call site that asked.
    pub fn inline_bytes(&self) -> Option<&[u8]> {
        match &self.payload {
            AssetPayload::Inline(b) => Some(b),
            AssetPayload::Pointer(_)
            | AssetPayload::Unreadable
            | AssetPayload::Unsupported { .. }
            | AssetPayload::InvalidForType { .. } => None,
        }
    }

    /// The blob hash this asset points at, if it is a pointer.
    pub fn pointer(&self) -> Option<entity_hash::Hash> {
        match &self.payload {
            AssetPayload::Inline(_)
            | AssetPayload::Unreadable
            | AssetPayload::Unsupported { .. }
            | AssetPayload::InvalidForType { .. } => None,
            AssetPayload::Pointer(h) => Some(*h),
        }
    }

    /// Decode an `app/site-asset`, **including the shape this repo emitted
    /// before `A-27` declared the type.**
    ///
    /// ## The legacy arm, why it exists, and how to retire it
    ///
    /// Until 2026-09-10 we encoded `{media_type, bytes}` — our own invention,
    /// declared nowhere, since `app/site-asset` was `implemented-undeclared`
    /// the whole time. The declared shape is `{media_type, payload}`, and a
    /// decoder that reads only the new key turns every already-published asset
    /// into an empty one.
    ///
    /// **That is not a migration inconvenience, it is a live regression, and
    /// the reason is the cache-arm ordering.** `MultiResolver::resolve_page`
    /// tries the durable cache **first**, with no network — so a returning
    /// visitor to any foreign site they have already viewed reads the
    /// old-shape entity out of their own tree and never issues a fetch that
    /// could heal it. Republishing the origin does not reach them. Measured
    /// on a real published asset: a 30,778-byte figure decoded to
    /// `Inline(0 bytes)` and rendered as `data:image/png;base64,`.
    ///
    /// **Deliberately narrow: the legacy arm fires only when `payload` is
    /// ABSENT ENTIRELY**, never when it is present and unreadable. An open
    /// type may grow a `bytes` field meaning something else, and this way the
    /// only entities it can claim are ones carrying the retired shape exactly.
    /// A present-but-untagged `payload` stays [`AssetPayload::Unreadable`],
    /// which is what EMBED §3's MUST asks for.
    ///
    /// **It is a READ, never a write** — [`Self::to_entity`] emits only the
    /// declared shape, so anything that re-encodes (the cache write-through)
    /// heals as it goes — and it is reported via
    /// [`Self::from_legacy_encoding`] so it can be shown to be dead before it
    /// is deleted.
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
        let mut saw_payload_key = false;
        let mut legacy_bytes: Option<Vec<u8>> = None;
        for (k, v) in map {
            match k.as_text() {
                Some("media_type") => {
                    if let Some(s) = v.as_text() {
                        out.media_type = s.to_string();
                    }
                }
                Some("payload") => {
                    saw_payload_key = true;
                    if let Some(p) = AssetPayload::from_value(v) {
                        out.payload = p;
                    }
                }
                Some("bytes") => legacy_bytes = v.as_bytes().cloned(),
                _ => {}
            }
        }
        if !saw_payload_key {
            if let Some(b) = legacy_bytes {
                out.payload = AssetPayload::Inline(b);
                out.from_legacy_encoding = true;
            }
        }
        out
    }

    /// Encode to the **declared** shape only — `{media_type, payload}`.
    ///
    /// Never the retired top-level `bytes`, even for an asset that was decoded
    /// from it: reading a legacy encoding is a concession, re-emitting one
    /// would be a second producer of it. And an
    /// [`Unreadable`](AssetPayload::Unreadable) payload emits **no `payload`
    /// key** rather than an empty inline one, which is both non-conformant and
    /// the collapse that variant exists to prevent.
    pub fn to_entity(&self) -> Entity {
        let mut pairs = vec![(
            entity_ecf::Value::Text("media_type".into()),
            entity_ecf::text(&self.media_type),
        )];
        if let Some(p) = self.payload.to_value() {
            pairs.push((entity_ecf::Value::Text("payload".into()), p));
        }
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
        let a = SiteAsset::inline("image/png", vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a]);
        let a2 = SiteAsset::from_entity(&a.to_entity());
        assert_eq!(a2, a);
        assert_eq!(a.to_entity().entity_type, SITE_ASSET_TYPE);
    }

    #[test]
    fn identical_asset_bytes_produce_identical_entity_content() {
        // Content-addressing dedup property: two assets with the same
        // (media_type, bytes) encode to byte-identical entity data, so the
        // store collapses them to one blob regardless of which site refs them.
        let a = SiteAsset::inline("image/svg+xml", b"<svg/>".to_vec());
        let b = SiteAsset::inline("image/svg+xml", b"<svg/>".to_vec());
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

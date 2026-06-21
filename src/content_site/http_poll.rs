//! HTTP-poll consumer — the two-hop fetch/verify **mechanism**.
//!
//! This is the "how a click becomes HTTP-poll" core, isolated from any
//! network or async so it is fully native-testable. The async `fetch()`
//! glue + the [`ContentResolver`](super::resolver::ContentResolver) impl
//! that drives it land in the next increment; this module owns the part
//! with all the protocol risk: the **two-hop indirection** (Amendment 6)
//! and the **content-hash auth gate**.
//!
//! ## Why a consumer at all (the layer boundary)
//!
//! A content site can be published as plain static files behind any HTTP
//! server (`entity-publish` → a `python -m http.server` dir). There is
//! **no live peer, no SDK, no dispatch** at the other end — just files.
//! Upstream `entity-core-rust` builds the HTTP-poll *server* but
//! deliberately leaves the browser consumer to this layer
//! (`core/peer/src/http_live.rs:85` — "the WASM target's HTTP path is the
//! browser's `fetch()` which lives in … entity-browser-rust"). So this is an L5 app
//! feature, ours to build — on the core codec primitives (`entity_ecf`,
//! `entity_hash`), reused, never reinvented.
//!
//! ## The wire shapes (verified against `http_live.rs` + `publish.go`)
//!
//! Both poll artifacts are **bare 2-key hashable ECF maps**
//! `ECF({data, type})` (`entity_ecf::ecf_for_hash`) — **not** the 3-key
//! wire entity (no `content_hash` field), so `entity_wire::decode_entity`
//! is the *wrong* tool and we decode with `ciborium` directly:
//!
//! - A tree leaf `.bin` is a **`system/hash` pointer** whose `data` is a
//!   CBOR bstr of the 33-byte wire hash `H` ([`crack_pointer`]). It is
//!   **not** the entity — `path → hash`, hop 1.
//! - The content body at `/content/{hex33(H)}` ([`content_url`],
//!   sharded-2-4) is the bare hashable pre-image of the real entity —
//!   `hash → bytes`, hop 2.
//!
//! ## The auth gate (`verify_and_decode`)
//!
//! A static origin signs nothing on reads, so the **content address is
//! the integrity proof**: `Hash::compute(type, data)` is precisely
//! `sha(ecf_for_hash(type, data))`, and the content body *is* that
//! pre-image. We re-derive the canonical `(type, data)`, recompute the
//! hash under `H`'s algorithm, and reject any body that does not hash to
//! `H` — the anti-graveyard "trust by hash-verify" contract. We render
//! the canonicalized entity (what we hashed), not the raw bytes.
//!
//! Implements network HTTP pathing (Amendment 5 + 6) and the
//! static-origin floor from the progressive-discovery render-closure design.

#![allow(dead_code)] // async resolver consumer lands in the next increment

use std::future::Future;
use std::pin::Pin;

use entity_entity::Entity;
use entity_hash::Hash;

use super::format::{SiteManifest, SitePage};
use super::location::Location;
use super::resolver::{ResolveError, ResolvedPage};

/// Entity type of an Amendment-6 tree-leaf pointer.
const HASH_POINTER_TYPE: &str = "system/hash";

/// Why a two-hop fetch/decode failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollError {
    /// The fetched bytes did not decode as a bare hashable ECF map.
    Decode(String),
    /// A `.bin` leaf was expected to be a `system/hash` pointer but
    /// carried a different type (e.g. a non-conformant one-hop body).
    NotAPointer,
    /// The pointer's `data` was not a 33-byte wire-hash bstr.
    BadPointer(String),
    /// The fetched content did **not** hash to the address we asked for —
    /// a corrupt or lying origin. The content-network auth gate.
    HashMismatch,
}

impl std::fmt::Display for PollError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PollError::Decode(e) => write!(f, "decode failed: {e}"),
            PollError::NotAPointer => write!(f, "tree leaf was not a system/hash pointer"),
            PollError::BadPointer(e) => write!(f, "malformed hash pointer: {e}"),
            PollError::HashMismatch => {
                write!(f, "content did not hash to its address (integrity check failed)")
            }
        }
    }
}

/// Build the content URL for a hash under an origin, **sharded-2-4**:
/// `{origin}/content/{hex[0:2]}/{hex[2:4]}/{hex}`. `hex` is the full wire
/// form (66 chars for SHA-256, `00`-prefixed — the algorithm byte is part
/// of the address, V7 §1.2), matching the `entity-publish` layout.
pub fn content_url(origin: &str, h: &Hash) -> String {
    let hex = h.to_hex();
    let origin = origin.trim_end_matches('/');
    // hex is always ≥ 4 chars (≥66 for any supported algorithm).
    format!("{}/content/{}/{}/{}", origin, &hex[0..2], &hex[2..4], hex)
}

/// HTTP URL of a site's `manifest.bin` leaf under a published origin —
/// `{origin}/{peer_id}/sites/{site_id}/manifest.bin`. This **mirrors the
/// raw tree path** ([`paths::manifest_path`](super::paths::manifest_path),
/// v0.5: `sites`, not `content/sites`), peer-first — it is what the
/// existing TREE_GET demux serves, NOT the prefix-first §11 URL projection
/// (`paths::site_url`, the legacy-web/static surface). Kept here (not in
/// `paths`) because it carries an http origin and appends the `.bin` poll
/// suffix.
pub fn manifest_bin_url(origin: &str, peer_id: &str, site_id: &str) -> String {
    format!("{}/{}/sites/{}/manifest.bin", origin.trim_end_matches('/'), peer_id, site_id)
}

/// HTTP URL of a site page's `.bin` leaf under a published origin —
/// `{origin}/{peer_id}/sites/{site_id}/pages/{slug}.bin` (tree-path mirror,
/// peer-first; see [`manifest_bin_url`]).
pub fn page_bin_url(origin: &str, peer_id: &str, site_id: &str, slug: &str) -> String {
    format!(
        "{}/{}/sites/{}/pages/{}.bin",
        origin.trim_end_matches('/'),
        peer_id,
        site_id,
        slug
    )
}

/// HTTP URL of a site asset's `.bin` leaf under a published origin —
/// `{origin}/{peer_id}/sites/{site_id}/assets/{name}.bin` (tree-path mirror,
/// peer-first; see [`manifest_bin_url`]). `name` is the asset's path under
/// `assets/` (`figures/x.png`), so the leaf is `assets/figures/x.png.bin`.
pub fn asset_bin_url(origin: &str, peer_id: &str, site_id: &str, name: &str) -> String {
    format!(
        "{}/{}/sites/{}/assets/{}.bin",
        origin.trim_end_matches('/'),
        peer_id,
        site_id,
        name
    )
}

/// Fetch + decode a site asset entity over `src` (the asset two-hop). Returns
/// the `app/site-asset` entity (media type + bytes); the caller decodes it with
/// [`SiteAsset::from_entity`](super::format::SiteAsset::from_entity). Same
/// verified two-hop the manifest/page closure runs.
pub async fn fetch_asset(
    src: &dyn BinSource,
    origin: &str,
    peer_id: &str,
    site_id: &str,
    name: &str,
) -> Result<Entity, PollError> {
    fetch_entity_two_hop(src, origin, &asset_bin_url(origin, peer_id, site_id, name)).await
}

/// HTTP URL of a site's static `pages.list` listing —
/// `{origin}/{peer_id}/sites/{site_id}/pages.list`. A **plain newline-delimited
/// slug list** (NOT a `.bin` entity — a one-hop fetch, no pointer/content
/// indirection), emitted by `publish_fixture::write_pages_list`. The remote
/// `.list` floor that lets a CDN-hosted site expose its sidebar + deep pages.
pub fn pages_list_url(origin: &str, peer_id: &str, site_id: &str) -> String {
    format!("{}/{}/sites/{}/pages.list", origin.trim_end_matches('/'), peer_id, site_id)
}

/// Fetch + parse a site's `pages.list` over `src` (one hop). Returns the page
/// slugs (blank lines dropped). A missing/empty listing yields an empty Vec —
/// a site simply exposes no remote sidebar, never an error (the listing is an
/// optional enrichment, not part of the render closure).
pub async fn fetch_pages_list(
    src: &dyn BinSource,
    origin: &str,
    peer_id: &str,
    site_id: &str,
) -> Result<Vec<String>, PollError> {
    // A `.list` enumeration is a mutable tree node at a stable path → fresh.
    let bytes = src.get(pages_list_url(origin, peer_id, site_id), Freshness::Mutable).await?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
}

/// HTTP URL of a peer's static `sites.list` listing —
/// `{origin}/{peer_id}/sites.list`. The PEER-level enumeration artifact (the
/// sibling of each site's `pages.list`): the set of site ids a peer hosts, so a
/// remote consumer can list a peer's sites before visiting any. Emitted by
/// `publish_fixture::write_sites_lists`.
pub fn sites_list_url(origin: &str, peer_id: &str) -> String {
    format!("{}/{}/sites.list", origin.trim_end_matches('/'), peer_id)
}

/// Fetch + parse a peer's `sites.list` over `src` (one hop). Returns the site
/// ids (blank lines dropped). A missing listing yields `Err` so the caller can
/// distinguish "peer exposes no enumeration" from "peer hosts zero sites" — the
/// pre-cache treats either as "nothing to pre-cache" (cache-awareness is an
/// optional enrichment, never required for a direct navigate to resolve).
pub async fn fetch_sites_list(
    src: &dyn BinSource,
    origin: &str,
    peer_id: &str,
) -> Result<Vec<String>, PollError> {
    // A `.list` enumeration is a mutable tree node at a stable path → fresh.
    let bytes = src.get(sites_list_url(origin, peer_id), Freshness::Mutable).await?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
}

/// HTTP URL of a peer's app-set catalog `.bin` leaf —
/// `{origin}/{peer_id}/apps/{set}/catalog.bin` (tree-path mirror, peer-first;
/// see [`manifest_bin_url`]). Same two-hop shape as a site manifest.
pub fn app_catalog_bin_url(origin: &str, peer_id: &str, set: &str) -> String {
    format!("{}/{}/apps/{}/catalog.bin", origin.trim_end_matches('/'), peer_id, set)
}

/// HTTP URL of an app-set bundle `.bin` leaf —
/// `{origin}/{peer_id}/apps/{set}/bundles/{id}.bin`. Same two-hop shape as a
/// site asset; the live Games/Apps window fetches this on click-through.
pub fn app_bundle_bin_url(origin: &str, peer_id: &str, set: &str, id: &str) -> String {
    format!("{}/{}/apps/{}/bundles/{}.bin", origin.trim_end_matches('/'), peer_id, set, id)
}

/// Fetch + decode an app-set catalog entity over `src` (the catalog two-hop).
/// Decode with [`AppCatalog::from_entity`](crate::apps::format::AppCatalog::from_entity).
pub async fn fetch_app_catalog(
    src: &dyn BinSource,
    origin: &str,
    peer_id: &str,
    set: &str,
) -> Result<Entity, PollError> {
    fetch_entity_two_hop(src, origin, &app_catalog_bin_url(origin, peer_id, set)).await
}

/// Fetch + decode an app-set bundle entity over `src` (the bundle two-hop).
/// Decode with [`AppBundle::from_entity`](crate::apps::format::AppBundle::from_entity).
pub async fn fetch_app_bundle(
    src: &dyn BinSource,
    origin: &str,
    peer_id: &str,
    set: &str,
    id: &str,
) -> Result<Entity, PollError> {
    fetch_entity_two_hop(src, origin, &app_bundle_bin_url(origin, peer_id, set, id)).await
}

/// Fetch + decode a site's manifest entity over `src` (the manifest two-hop).
/// The lightweight half of cache-awareness: pre-caching just the manifest is
/// enough for the directory to LIST a site; its pages fetch lazily on first
/// visit. Reuses the same verified two-hop the full closure resolve runs.
pub async fn fetch_manifest(
    src: &dyn BinSource,
    origin: &str,
    peer_id: &str,
    site_id: &str,
) -> Result<Entity, PollError> {
    fetch_entity_two_hop(src, origin, &manifest_bin_url(origin, peer_id, site_id)).await
}

/// Which half of the entity model a fetched URL belongs to — the cache policy
/// follows the model, not a blanket rule:
/// - [`Freshness::Mutable`] — a **tree node** at a stable path (a `system/hash`
///   pointer `.bin`, or a `.list` enumeration). Republishing changes its content
///   but NOT its URL, so a default-cached copy goes stale → fetch with
///   `cache: no-store`. This is the layer that must always be current.
/// - [`Freshness::Immutable`] — a **content-store blob** at `/content/{hash}`.
///   The URL *is* the content hash (and the body is hash-verified), so a cached
///   copy is always correct AND a changed entity gets a new URL anyway. Left
///   cacheable so content-addressing does its job (no needless re-download).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    Mutable,
    Immutable,
}

/// An async source of raw bytes for a URL — the seam between the
/// transport-free two-hop logic and the actual fetch. The live impl is a
/// browser `fetch()` (`FetchBinSource`, wasm); tests drive an in-memory
/// fixture. The future is intentionally **not** `Send` (a wasm `JsFuture`
/// isn't) so it composes with `spawn_local`. `freshness` lets the caller mark a
/// URL as a mutable tree node vs an immutable content blob (see [`Freshness`]).
pub trait BinSource {
    fn get(
        &self,
        url: String,
        freshness: Freshness,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PollError>>>>;
}

/// The live [`BinSource`] — a browser `fetch()` of the URL. **Simple GET
/// only** (no custom request headers) so a static CDN answers without a
/// CORS preflight (the origin must still emit `Access-Control-Allow-Origin`
/// for a cross-origin fetch). Mutable tree nodes fetch `cache: no-store`;
/// immutable content blobs use the default cache (see [`fetch_bytes`]).
/// Wasm-only; native builds drive a fixture.
#[cfg(target_arch = "wasm32")]
pub struct FetchBinSource;

#[cfg(target_arch = "wasm32")]
impl BinSource for FetchBinSource {
    fn get(
        &self,
        url: String,
        freshness: Freshness,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PollError>>>> {
        Box::pin(async move { fetch_bytes(&url, freshness).await })
    }
}

#[cfg(target_arch = "wasm32")]
async fn fetch_bytes(url: &str, freshness: Freshness) -> Result<Vec<u8>, PollError> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    let window = web_sys::window().ok_or_else(|| PollError::Decode("no window".into()))?;
    // Cache policy follows the model (see `Freshness`): a MUTABLE tree node
    // (pointer / `.list`) lives at a STABLE URL whose content changes on a
    // republish, so the browser's default cache would serve a stale copy (the
    // "republished but shows old apps" symptom) → `no-store`, always fresh from
    // the network. An IMMUTABLE content blob is keyed by its own hash, so a
    // cached copy is always correct → leave it cacheable (content-addressing
    // already gives a changed entity a new URL). Offline still works either way
    // via the durable IDB / SW fallback (a failed fetch keeps the cached entity).
    // Still a header-free simple GET, so no CORS preflight.
    let opts = web_sys::RequestInit::new();
    if freshness == Freshness::Mutable {
        opts.set_cache(web_sys::RequestCache::NoStore);
    }
    let request = web_sys::Request::new_with_str_and_init(url, &opts)
        .map_err(|e| PollError::Decode(format!("request build failed: {e:?}")))?;
    let resp_val = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| PollError::Decode(format!("fetch failed: {e:?}")))?;
    let resp: web_sys::Response =
        resp_val.dyn_into().map_err(|_| PollError::Decode("not a Response".into()))?;
    if !resp.ok() {
        return Err(PollError::Decode(format!("HTTP {} for {url}", resp.status())));
    }
    let buf_promise =
        resp.array_buffer().map_err(|e| PollError::Decode(format!("array_buffer: {e:?}")))?;
    let buf = JsFuture::from(buf_promise)
        .await
        .map_err(|e| PollError::Decode(format!("array_buffer await: {e:?}")))?;
    tracing::debug!(url, "http_poll fetch ok");
    Ok(js_sys::Uint8Array::new(&buf).to_vec())
}

/// Resolve the **render closure** (manifest + page) for `loc` over a
/// [`BinSource`] rooted at `origin` — the async orchestration an
/// `HttpPollResolver` runs in a `spawn_local`. Two two-hops: the manifest
/// (whose `params.root` names the landing page when `loc.page` is empty),
/// then the page, then the page's **embed-asset closure** (each `::embed` ref
/// in the page body → that asset's two-hop bytes). It deliberately fetches
/// **only** that closure — not siblings, not a path→hash index, not the site's
/// other assets (the scale discipline, closure design §2). Asset fetches are
/// **best-effort**: a missing/failed asset is skipped (the image degrades to
/// its alt text), never fatal to the page resolve.
pub async fn resolve_closure_via(
    src: &dyn BinSource,
    origin: &str,
    loc: &Location,
) -> Result<ResolvedPage, ResolveError> {
    let pid = loc.peer_id.clone().unwrap_or_default();

    let manifest_ent = fetch_entity_two_hop(src, origin, &manifest_bin_url(origin, &pid, &loc.site_id))
        .await
        .map_err(|_| ResolveError::ManifestMissing)?;
    let manifest = SiteManifest::from_entity(&manifest_ent);

    let page_slug = if loc.page.is_empty() { manifest.root().to_string() } else { loc.page.clone() };

    let page = match fetch_entity_two_hop(src, origin, &page_bin_url(origin, &pid, &loc.site_id, &page_slug)).await {
        Ok(page_ent) => SitePage::from_entity(&page_ent),
        Err(_) => {
            // No page entity here. If the static `pages.list` shows this slug
            // is a SECTION (has descendant pages), render a generated
            // section-index — same as the local resolver — so a remote
            // intermediate path (reached from a breadcrumb/sidebar) is a real
            // destination, not a dead end. Otherwise it's genuinely missing.
            let slugs = fetch_pages_list(src, origin, &pid, &loc.site_id).await.unwrap_or_default();
            let children =
                super::discovery::children_from_slugs(&slugs, &format!("{page_slug}/"));
            if children.is_empty() {
                return Err(ResolveError::PageMissing);
            }
            super::resolver::section_index_page(&page_slug, &children)
        }
    };

    // The embed-asset closure: each `::embed` ref in the body → the asset's
    // two-hop bytes, so the cache write-through can land them in MY store where
    // the renderer's resolver reads them. Site-local refs only
    // (`asset_name_from_ref` gates external/escaping refs). Best-effort.
    let mut assets = Vec::new();
    for reference in super::embed::embed_refs(&page.body) {
        let Some(name) = super::paths::asset_name_from_ref(&reference) else {
            continue;
        };
        if let Ok(ent) = fetch_asset(src, origin, &pid, &loc.site_id, &name).await {
            assets.push((name, super::format::SiteAsset::from_entity(&ent)));
        }
    }

    Ok(ResolvedPage {
        location: Location { peer_id: Some(pid), site_id: loc.site_id.clone(), page: page_slug },
        manifest,
        page,
        assets,
    })
}

/// Fetch one entity by its `.bin` leaf URL via the Amendment-6 two-hop:
/// GET the pointer → [`crack_pointer`] → GET `/content/{hex33}` →
/// [`verify_and_decode`].
async fn fetch_entity_two_hop(
    src: &dyn BinSource,
    origin: &str,
    bin_url: &str,
) -> Result<Entity, PollError> {
    // Hop 1: the pointer is a mutable tree node at a stable path → always fresh.
    let bin = src.get(bin_url.to_string(), Freshness::Mutable).await?;
    let h = crack_pointer(&bin)?;
    // Hop 2: the content blob is addressed by its hash → safe to cache.
    let body = src.get(content_url(origin, &h), Freshness::Immutable).await?;
    verify_and_decode(&body, &h)
}

/// Hop 1: decode a fetched `.bin` leaf as a `system/hash` pointer and
/// extract the content hash `H` it points at. The pointer is a bare
/// 2-key `ECF({type:"system/hash", data:<bstr 33B>})`.
pub fn crack_pointer(bin_bytes: &[u8]) -> Result<Hash, PollError> {
    let (etype, data_val) = decode_bare_pair(bin_bytes)?;
    if etype != HASH_POINTER_TYPE {
        return Err(PollError::NotAPointer);
    }
    let bytes = data_val
        .as_bytes()
        .ok_or_else(|| PollError::BadPointer("pointer data is not a bstr".into()))?;
    Hash::from_bytes(bytes).map_err(|e| PollError::BadPointer(format!("{e:?}")))
}

/// Hop 2: decode fetched content bytes into an [`Entity`], **verifying
/// the canonical `(type, data)` hashes to `expected`** before returning.
/// Recomputes the hash under `expected`'s algorithm; a tampered body is
/// rejected with [`PollError::HashMismatch`]. The returned entity carries
/// the canonical `data` (exactly what was verified).
pub fn verify_and_decode(body: &[u8], expected: &Hash) -> Result<Entity, PollError> {
    let (etype, data_val) = decode_bare_pair(body)?;
    // Canonical re-encode of the embedded data value (entity_ecf::Value IS
    // ciborium::Value, so no conversion). For canonical content this equals
    // the producer's original `data` bytes byte-for-byte.
    let data = entity_ecf::to_ecf(&data_val);
    Hash::validate(&etype, &data, expected).map_err(|_| PollError::HashMismatch)?;
    Entity::new(&etype, data).map_err(|e| PollError::Decode(format!("{e:?}")))
}

/// Decode a bare 2-key hashable ECF map `ECF({data, type})` into its
/// `(type, data-value)`. The body is `entity_ecf::ecf_for_hash(...)`
/// output — a plain CBOR map, decoded with `ciborium`.
fn decode_bare_pair(bytes: &[u8]) -> Result<(String, entity_ecf::Value), PollError> {
    let val: entity_ecf::Value =
        ciborium::from_reader(bytes).map_err(|e| PollError::Decode(format!("{e:?}")))?;
    let map = val.as_map().ok_or_else(|| PollError::Decode("not a CBOR map".into()))?;
    let mut etype: Option<String> = None;
    let mut data: Option<entity_ecf::Value> = None;
    for (k, v) in map {
        match k.as_text() {
            Some("type") => etype = v.as_text().map(str::to_string),
            Some("data") => data = Some(v.clone()),
            _ => {}
        }
    }
    let etype = etype.ok_or_else(|| PollError::Decode("missing 'type'".into()))?;
    let data = data.ok_or_else(|| PollError::Decode("missing 'data'".into()))?;
    Ok((etype, data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_site::format::{NavItem, SiteManifest, SitePage};
    use std::collections::HashMap;

    /// An in-memory stand-in for a published static dir: URL → bytes.
    /// Built with the **same** `entity_ecf` encoders the Go `entity-publish`
    /// producer uses (`ecf_for_hash` / `ecf_for_hash_value`), so the bytes
    /// have **cross-impl parity** — this proves the mechanism *and* the
    /// wire shape. (Parity against a real wb-go publish is the live proof.)
    struct PublishedFixture {
        files: HashMap<String, Vec<u8>>,
        origin: String,
    }

    impl PublishedFixture {
        fn new(origin: &str) -> Self {
            Self { files: HashMap::new(), origin: origin.trim_end_matches('/').to_string() }
        }

        fn get(&self, url: &str) -> Option<&[u8]> {
            self.files.get(url).map(Vec::as_slice)
        }

        fn put(&mut self, url: impl Into<String>, bytes: Vec<u8>) {
            self.files.insert(url.into(), bytes);
        }

        /// "Publish" an entity: store its bare hashable body at the
        /// content-addressed URL, and place a `system/hash` `.bin` pointer
        /// at `tree_path`.
        fn publish(&mut self, tree_path: &str, ent: &Entity) {
            let h = ent.content_hash.clone();
            // content body = bare 2-key hashable pre-image (Go EncodeHashable parity).
            self.put(content_url(&self.origin, &h), entity_ecf::ecf_for_hash(&ent.entity_type, &ent.data));
            // .bin = bare 2-key system/hash pointer, data = CBOR bstr of 33 bytes.
            let pointer = entity_ecf::ecf_for_hash_value(
                HASH_POINTER_TYPE,
                &entity_ecf::Value::Bytes(h.to_bytes()),
            );
            let url = format!("{}/{}", self.origin, tree_path.trim_start_matches('/'));
            self.put(url, pointer);
        }
    }

    /// The exact two-hop the async `HttpPollResolver` will run, driven
    /// synchronously over the fixture (no async, no network): fetch the
    /// `.bin` pointer → crack → fetch content → verify → entity.
    fn two_hop(fx: &PublishedFixture, tree_path: &str) -> Result<Entity, PollError> {
        let bin_url = format!("{}/{}", fx.origin, tree_path.trim_start_matches('/'));
        let bin = fx.get(&bin_url).ok_or_else(|| PollError::Decode("404 .bin".into()))?;
        let h = crack_pointer(bin)?;
        let body = fx
            .get(&content_url(&fx.origin, &h))
            .ok_or_else(|| PollError::Decode("404 content".into()))?;
        verify_and_decode(body, &h)
    }

    #[test]
    fn two_hop_resolves_published_manifest_and_page() {
        let origin = "http://localhost:9999";
        let (peer, site) = ("PEERB", "labs");
        let mut fx = PublishedFixture::new(origin);

        let manifest = SiteManifest::new(
            "labs",
            "Bill's Labs",
            "index",
            vec![NavItem::new("Home", "/index"), NavItem::new("Intro", "/intro")],
        );
        let page = SitePage::markdown("Intro", "# Intro\n\nWelcome to **Bill's Labs**.");

        fx.publish(&format!("{peer}/sites/{site}/manifest.bin"), &manifest.to_entity());
        fx.publish(&format!("{peer}/sites/{site}/pages/intro.bin"), &page.to_entity());

        // Manifest reconstitutes through the full two-hop + verify.
        let ent = two_hop(&fx, &format!("{peer}/sites/{site}/manifest.bin"))
            .expect("manifest two-hop resolves");
        assert_eq!(ent.entity_type, "app/site-manifest");
        assert_eq!(
            SiteManifest::from_entity(&ent),
            manifest,
            "reconstituted manifest equals the published one"
        );

        // Page too.
        let ent = two_hop(&fx, &format!("{peer}/sites/{site}/pages/intro.bin"))
            .expect("page two-hop resolves");
        assert_eq!(SitePage::from_entity(&ent), page);
    }

    #[test]
    fn canonical_reencode_matches_original_content_hash() {
        // The verify path canonical-re-encodes the embedded data value; for
        // our string/map/array content that MUST reproduce the producer's
        // hash exactly (no false rejection of valid content).
        let page = SitePage::markdown("Intro", "# Intro\n\nBody with a [link](./about).");
        let ent = page.to_entity();
        let body = entity_ecf::ecf_for_hash(&ent.entity_type, &ent.data);
        let decoded = verify_and_decode(&body, &ent.content_hash).expect("valid content verifies");
        assert_eq!(decoded.content_hash, ent.content_hash, "canonical re-encode preserves the hash");
        assert_eq!(SitePage::from_entity(&decoded), page);
    }

    #[test]
    fn tampered_content_is_rejected_by_hash_gate() {
        let origin = "http://localhost:9999";
        let mut fx = PublishedFixture::new(origin);
        let page = SitePage::markdown("Real", "# Real content");
        fx.publish("p.bin", &page.to_entity());

        // Learn the demanded hash + content URL from the pointer.
        let h = crack_pointer(fx.get(&format!("{origin}/p.bin")).unwrap()).unwrap();

        // A lying origin swaps the content for a different entity.
        let evil = SitePage::markdown("Evil", "# Malicious replacement").to_entity();
        fx.put(content_url(origin, &h), entity_ecf::ecf_for_hash(&evil.entity_type, &evil.data));

        // The hash gate rejects it — the bytes no longer hash to `h`.
        assert!(
            matches!(two_hop(&fx, "p.bin"), Err(PollError::HashMismatch)),
            "tampered content must fail the integrity gate"
        );
    }

    #[test]
    fn content_url_is_sharded_2_4() {
        let h = SitePage::markdown("x", "y").to_entity().content_hash;
        let hex = h.to_hex();
        assert_eq!(
            content_url("http://o/", &h),
            format!("http://o/content/{}/{}/{}", &hex[0..2], &hex[2..4], hex),
            "trailing origin slash trimmed; 2/2 shard prefix"
        );
        assert!(hex.starts_with("00"), "SHA-256 wire hex is 00-prefixed: {hex}");
        assert_eq!(hex.len(), 66, "33-byte wire form → 66 hex chars");
    }

    #[test]
    fn non_pointer_bin_is_rejected() {
        // A `.bin` that is a content body (the non-conformant one-hop) is
        // rejected — we require the system/hash pointer form.
        let ent = SitePage::markdown("x", "y").to_entity();
        let bytes = entity_ecf::ecf_for_hash(&ent.entity_type, &ent.data);
        assert!(matches!(crack_pointer(&bytes), Err(PollError::NotAPointer)));
    }

    #[test]
    fn garbage_bin_is_a_decode_error() {
        assert!(matches!(crack_pointer(&[0xff, 0x00, 0x13]), Err(PollError::Decode(_))));
    }

    // ===================================================================
    // Async closure orchestration — the path `HttpPollResolver` will run
    // inside `spawn_local`, here driven by a fixture BinSource + a minimal
    // native block_on (fixture futures are always Ready, so this never
    // truly pends).
    // ===================================================================

    /// A [`BinSource`] backed by a [`PublishedFixture`] — returns each
    /// URL's bytes as an immediately-ready future (404 → `Decode`).
    struct FixtureBinSource(PublishedFixture);

    impl BinSource for FixtureBinSource {
        fn get(
            &self,
            url: String,
            _freshness: Freshness,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PollError>>>> {
            let r = self
                .0
                .get(&url)
                .map(<[u8]>::to_vec)
                .ok_or_else(|| PollError::Decode(format!("404 {url}")));
            Box::pin(std::future::ready(r))
        }
    }

    /// Poll a future to completion on the current thread with a no-op
    /// waker. Sufficient because every fixture await is already Ready.
    fn block_on<F: Future>(future: F) -> F::Output {
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        fn raw() -> RawWaker {
            fn noop(_: *const ()) {}
            fn clone(_: *const ()) -> RawWaker {
                raw()
            }
            RawWaker::new(std::ptr::null(), &RawWakerVTable::new(clone, noop, noop, noop))
        }
        let waker = unsafe { Waker::from_raw(raw()) };
        let mut cx = Context::from_waker(&waker);
        let mut future = Box::pin(future);
        loop {
            if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    #[test]
    fn async_closure_resolves_remote_site_root() {
        let origin = "http://labs.example";
        let (peer, site) = ("PEERB", "labs");
        let mut fx = PublishedFixture::new(origin);

        let manifest = SiteManifest::new(
            "labs",
            "Bill's Labs",
            "index",
            vec![NavItem::new("Home", "/index"), NavItem::new("About", "/about")],
        );
        fx.publish(&format!("{peer}/sites/{site}/manifest.bin"), &manifest.to_entity());
        fx.publish(
            &format!("{peer}/sites/{site}/pages/index.bin"),
            &SitePage::markdown("Home", "# Welcome to the Labs").to_entity(),
        );

        let src = FixtureBinSource(fx);
        // Empty page → the resolver reads the manifest's params.root (index).
        let loc = Location { peer_id: Some(peer.into()), site_id: site.into(), page: String::new() };
        let rp = block_on(resolve_closure_via(&src, origin, &loc)).expect("remote root resolves");

        assert_eq!(rp.location.peer_id.as_deref(), Some("PEERB"));
        assert_eq!(rp.location.page, "index", "root page came from manifest params.root");
        assert_eq!(rp.manifest.title, "Bill's Labs");
        assert_eq!(rp.page.title(), "Home");
        assert!(rp.page.body.contains("Welcome"));
    }

    #[test]
    fn async_closure_missing_manifest_is_manifest_error() {
        let origin = "http://empty.example";
        let src = FixtureBinSource(PublishedFixture::new(origin));
        let loc = Location { peer_id: Some("P".into()), site_id: "ghost".into(), page: String::new() };
        assert_eq!(
            block_on(resolve_closure_via(&src, origin, &loc)),
            Err(ResolveError::ManifestMissing)
        );
    }

    #[test]
    fn async_closure_fetches_the_pages_embed_assets() {
        use crate::content_site::format::SiteAsset;
        let origin = "http://labs.example";
        let (peer, site) = ("PEERB", "labs");
        let mut fx = PublishedFixture::new(origin);
        fx.publish(
            &format!("{peer}/sites/{site}/manifest.bin"),
            &SiteManifest::new("labs", "Labs", "index", vec![]).to_entity(),
        );
        // The page body uses the canonical embed form (as ingest stores it).
        fx.publish(
            &format!("{peer}/sites/{site}/pages/index.bin"),
            &SitePage::markdown("Home", "# Home\n\n::embed[A figure]{ref=assets/figures/x.svg}")
                .to_entity(),
        );
        // …and its asset, at the assets/ leaf.
        fx.publish(
            &format!("{peer}/sites/{site}/assets/figures/x.svg.bin"),
            &SiteAsset::new("image/svg+xml", b"<svg/>".to_vec()).to_entity(),
        );

        let src = FixtureBinSource(fx);
        let loc = Location { peer_id: Some(peer.into()), site_id: site.into(), page: String::new() };
        let rp = block_on(resolve_closure_via(&src, origin, &loc)).expect("resolves with closure");

        assert_eq!(rp.assets.len(), 1, "the page's one embed asset was fetched");
        assert_eq!(rp.assets[0].0, "figures/x.svg", "asset name = ref minus the assets/ prefix");
        assert_eq!(rp.assets[0].1.media_type, "image/svg+xml");
        assert_eq!(rp.assets[0].1.bytes, b"<svg/>");
    }

    #[test]
    fn async_closure_missing_embed_asset_is_skipped_not_fatal() {
        let origin = "http://labs.example";
        let (peer, site) = ("PEERB", "labs");
        let mut fx = PublishedFixture::new(origin);
        fx.publish(
            &format!("{peer}/sites/{site}/manifest.bin"),
            &SiteManifest::new("labs", "Labs", "index", vec![]).to_entity(),
        );
        // Page references an asset that is NOT published.
        fx.publish(
            &format!("{peer}/sites/{site}/pages/index.bin"),
            &SitePage::markdown("Home", "::embed[Gone]{ref=assets/missing.png}").to_entity(),
        );

        let src = FixtureBinSource(fx);
        let loc = Location { peer_id: Some(peer.into()), site_id: site.into(), page: String::new() };
        // The page still resolves; the missing asset is simply absent (the image
        // degrades to its alt text), never a resolve failure.
        let rp = block_on(resolve_closure_via(&src, origin, &loc)).expect("page resolves anyway");
        assert!(rp.assets.is_empty(), "missing asset skipped, not fatal");
        assert!(rp.page.body.contains("::embed"), "page body intact");
    }

    #[test]
    fn async_closure_missing_page_is_page_error() {
        let origin = "http://labs.example";
        let (peer, site) = ("PEERB", "labs");
        let mut fx = PublishedFixture::new(origin);
        // Manifest present, but the requested page is not published.
        fx.publish(
            &format!("{peer}/sites/{site}/manifest.bin"),
            &SiteManifest::new("labs", "Labs", "index", vec![]).to_entity(),
        );
        let src = FixtureBinSource(fx);
        let loc = Location { peer_id: Some(peer.into()), site_id: site.into(), page: "ghost".into() };
        assert_eq!(
            block_on(resolve_closure_via(&src, origin, &loc)),
            Err(ResolveError::PageMissing)
        );
    }
}

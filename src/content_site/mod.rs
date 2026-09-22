//! Content sites — the format + resolver "spine" for Site Mode.
//!
//! A *content site* is a content subgraph (manifest + pages + assets +
//! nav) rooted at a manifest. The site DATA is a **free subgraph** at a
//! publisher-chosen tree path — this impl places it at
//! `/{pid}/sites/{site_id}/...` (v0.5 §2; NOT `content/sites/…`, the
//! dropped layer violation — `system/content/*` is the CONTENT
//! extension's hash-address namespace, see [`paths`]). The site VIEW
//! state (current location, history, mode) lives in the app namespace and
//! is owned by the window, not here.
//!
//! This module is the non-DOM, unit-testable core:
//! - [`format`]   — entity types (manifest / page / nav) + CBOR codec.
//! - [`paths`]    — site tree-path helpers + the legacy-web URL projection.
//! - [`location`] — [`Location`] + the entity-native link classifier.
//! - [`resolver`] — the [`ContentResolver`] seam + [`LocalTreeResolver`].
//!
//! **The seam (format ⊥ transport):** the renderer only ever reads a
//! resolved page from a cache; a [`ContentResolver`] fills it. The
//! local impl resolves synchronously from the tree; later HTTP-poll /
//! cross-peer impls slot in behind the same trait with the renderer
//! untouched.
//!
//! P0 = this spine + its tests (no DOM). Consumers (the
//! `views/content_site` window + `dom/content_site` renderer) land in P1.

/// Bytes ⇄ [`format::SiteAsset`] across EMBED §3's payload union — the
/// inline ceiling, the canonical chunker, and the resolution every consumer
/// arm shares.
pub mod asset_store;
pub mod cache;
pub mod discovery;
pub mod doc_css;
pub mod embed;
/// The one entry point for reading someone else's bytes — presence **and**
/// currency in one call, so a consumer cannot express "only if absent".
pub mod foreign_cache;
pub mod format;
pub mod http_poll;
#[cfg(not(target_arch = "wasm32"))]
pub mod ingest;
pub mod location;
/// The full path — a name resolved through a pinned registry to a verified page.
pub mod name_dispatch;
pub mod named_site;
pub mod origins;
pub mod paths;
pub mod prefs;
#[cfg(not(target_arch = "wasm32"))]
pub mod publish;
pub mod publish_fixture;
/// B16 — the static registry emitter (native publisher only).
#[cfg(not(target_arch = "wasm32"))]
pub mod registry_publish;
pub mod read;
pub mod render;
pub mod resolver;
/// The `http-poll` endpoint, **read rather than assumed** — the v1.8 MUST that
/// says a consumer never derives the manifest's location by convention.
pub mod publish_layout;
/// **The cross-implementation consume check** — a site published by
/// `entity-workbench-go`, walked by our reader. Tests only; ADR-0012's
/// cohort-consistent-vs-independent distinction is why it is not enough to
/// publish and consume with the same arm.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod crossimpl_go;
/// **The same check against a LIVE `entity-core-go` origin on another host** —
/// C-7 / `COHORT-OPEN-ITEMS` §1b. Skips loudly without the rig; `make crossimpl-go`.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod crossimpl_go_live;
/// **`G-PIN-4` — one fixture, two publishers, identical site root.** The other
/// direction from [`crossimpl_go`]: not *can we read their bytes* but *do the
/// two publishers produce the same root from the same content*. Runs against
/// the same vendored `entity-core-go` emission, with no rig and no
/// coordination. Tests only.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod crossimpl_reproducible_publish;
/// **`G-PIN-4`'s vocabulary half — our side of the joint fixture.** The
/// substrate gate above runs against `test/note` leaves; §9's cases are about
/// `app/site-manifest` / `app/site-page`. This pins what we compute from
/// `tests/fixtures/gpin4-joint/site.json` so `entity-workbench-go` can compare
/// their publisher against it with no rig. Tests only.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod gpin4_joint_fixture;
/// **C-2 — two devices, one key, one signed-root sequence.** A measurement of
/// arch's open multi-device-publishing item, not a fix: the sequence is anchored
/// in the output directory, so two out-dirs under one keypair are two
/// independent sequences. Tests only; the fix is `core/peer`'s.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod multi_device_sequence;
/// B15 — consume a signed published root over an async transport (both arches).
pub mod session_cache;
pub mod signed_fetch;
/// B14 — the signed root over a static publish (native publisher only).
#[cfg(not(target_arch = "wasm32"))]
pub mod signed_root;
#[cfg(not(target_arch = "wasm32"))]
pub mod static_export;

pub use format::{NavItem, SiteManifest, SitePage};
pub use location::{classify_link, humanize, resolve_target, LinkTarget, Location};
pub use render::{markdown_to_html, render_page, PageRender};
pub use resolver::{ContentResolver, MultiResolver, RepaintCell, ResolveError, ResolveOutcome};
// `http_poll::{content_url, crack_pointer, verify_and_decode, PollError}` and
// `resolver::{LocalTreeResolver, ResolvedPage}` are reachable by full path;
// the resolver drives them internally, so they aren't re-exported here.
// `discovery::{list_child_pages, ChildEntry}` (the lazy `.list` primitive)
// is likewise full-path-only until its render consumer lands — sitemap/
// listing rendering is deferred (review finding #4).

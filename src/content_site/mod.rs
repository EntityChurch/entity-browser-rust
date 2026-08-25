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

pub mod cache;
pub mod discovery;
pub mod doc_css;
pub mod embed;
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
pub use render::render_page_body;
pub use resolver::{ContentResolver, MultiResolver, RepaintCell, ResolveError, ResolveOutcome};
// `http_poll::{content_url, crack_pointer, verify_and_decode, PollError}` and
// `resolver::{LocalTreeResolver, ResolvedPage}` are reachable by full path;
// the resolver drives them internally, so they aren't re-exported here.
// `discovery::{list_child_pages, ChildEntry}` (the lazy `.list` primitive)
// is likewise full-path-only until its render consumer lands — sitemap/
// listing rendering is deferred (review finding #4).

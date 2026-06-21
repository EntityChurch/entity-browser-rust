//! The content resolver seam — format ⊥ transport.
//!
//! A [`ContentResolver`] turns a [`Location`] into a [`ResolvedPage`].
//! The model calls it on navigate; the renderer only ever reads the
//! resolved result (cached). This is the seam that lets transports swap
//! underneath without touching the renderer:
//!
//! - [`LocalTreeResolver`] (P0) reads the peer's own tree synchronously
//!   and returns [`ResolveOutcome::Ready`].
//! - A future cross-peer / HTTP-poll resolver kicks off an async fetch
//!   internally (cloning handles + `spawn_local`, the same way
//!   `Peers::put_and_wait` builds a `'static` future), writes the
//!   result into the shared site cache on completion, fires a repaint,
//!   and returns [`ResolveOutcome::Pending`]. The model treats the
//!   cache as the source of truth either way.
//!
//! Returning `Ready`/`Pending` (rather than a boxed future) matches
//! this codebase's idiom: a sync render loop reading a cache that
//! background work fills — exactly the Knowledge Base shape.

#![allow(dead_code)] // model consumer lands in P1

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use super::discovery::ChildEntry;
use super::format::{SiteManifest, SitePage};
use super::location::Location;
use super::paths;
use crate::peers::Peers;
use crate::window::RepaintFn;

/// A fully-resolved page ready to render: the page itself plus the
/// site manifest (for nav/chrome) and the concrete location reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPage {
    /// The concrete location reached (root resolved to a page slug).
    pub location: Location,
    pub manifest: SiteManifest,
    pub page: SitePage,
    /// The page's embed assets `(name, asset)` — the closure the HTTP arm
    /// fetched alongside the page so they can be written through to MY store
    /// (where [`crate::dom::content_site::make_asset_resolver`] finds them).
    /// **Empty for local/cached resolves**: those assets already live in the
    /// store (ingest / a prior write-through), so the renderer reads them
    /// directly — only the remote HTTP arm carries bytes here. Excluded from
    /// nothing (it's part of `Eq`), but it's transient: written through once
    /// then the durable store is the source of truth.
    pub assets: Vec<(String, super::format::SiteAsset)>,
}

/// Why a resolve failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveError {
    /// No manifest entity at the site's manifest path.
    ManifestMissing,
    /// No page entity at the requested (or root) page path.
    PageMissing,
    /// The Location names a remote peer with **no resolvable route** — no
    /// origin is registered for it and it isn't in the local tree — and the
    /// grace window for a route to appear has elapsed. Reported instead of
    /// spinning on `loading` forever, so an unknown/unreachable peer is an
    /// honest, bounded error (the predictability fix). Distinct from a
    /// *registered* origin that fails to fetch (that path errors via the
    /// HTTP-poll failure backoff).
    Unreachable,
}

/// The result of beginning a resolve.
///
/// `Ready` is a large variant (a whole `ResolvedPage` — manifest + page,
/// each with their string-keyed maps) next to an empty `Pending`. We
/// intentionally do **not** box it: this enum is a **transient
/// per-navigation return** — built once when the user navigates,
/// immediately matched, and dropped. It is never stored in bulk, so the
/// variant-size optimization clippy suggests would only add a heap
/// allocation on the resolve path for no benefit.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum ResolveOutcome {
    /// Resolved synchronously (local transport).
    Ready(Result<ResolvedPage, ResolveError>),
    /// An async transport began a fetch; the cache will be filled and a
    /// repaint fired when it completes.
    Pending,
}

/// The transport seam. Implementors fetch a page for a location.
pub trait ContentResolver {
    /// Begin resolving `loc`. See [`ResolveOutcome`].
    fn resolve_page(&self, peers: &Peers, loc: &Location) -> ResolveOutcome;

    /// List the immediate children of `loc`'s site under the page-prefix
    /// `under` (`""` = page root, `"guide/"` = the Guide section),
    /// body-free — for the sidebar + section-index listings. Default:
    /// empty; the local resolver overrides with a tree scan, and the
    /// HTTP-poll arm with the fetched `pages.list` (`children_from_slugs`).
    fn list_children(&self, _peers: &Peers, _loc: &Location, _under: &str) -> Vec<ChildEntry> {
        Vec::new()
    }

    /// The site manifest for `loc` if it is durably available **without** the
    /// page — for the O3 manifest-pinned shell (render the chrome when a
    /// page is ephemeral + offline). Default `None` (no shell); the
    /// [`MultiResolver`] overrides it to read from the durable cache.
    fn manifest_only(&self, _peers: &Peers, _loc: &Location) -> Option<SiteManifest> {
        None
    }
}

/// Resolves pages from the bound peer's own tree (sync L0 reads).
pub struct LocalTreeResolver;

impl ContentResolver for LocalTreeResolver {
    fn resolve_page(&self, peers: &Peers, loc: &Location) -> ResolveOutcome {
        ResolveOutcome::Ready(resolve_local(peers, loc))
    }

    fn list_children(&self, peers: &Peers, loc: &Location, under: &str) -> Vec<ChildEntry> {
        let pid: &str = loc.peer_id.as_deref().unwrap_or_else(|| peers.primary_peer_id());
        super::discovery::list_child_pages(peers, pid, &loc.site_id, under)
    }
}

fn resolve_local(peers: &Peers, loc: &Location) -> Result<ResolvedPage, ResolveError> {
    let pid: &str = loc.peer_id.as_deref().unwrap_or_else(|| peers.primary_peer_id());

    let manifest = peers
        .get_entity(pid, &paths::manifest_path(pid, &loc.site_id))
        .map(|e| SiteManifest::from_entity(&e))
        .ok_or(ResolveError::ManifestMissing)?;

    // Empty page → the manifest's declared root page (params.root).
    let page_slug = if loc.page.is_empty() {
        manifest.root().to_string()
    } else {
        loc.page.clone()
    };

    let page = match peers.get_entity(pid, &paths::page_path(pid, &loc.site_id, &page_slug)) {
        Some(e) => SitePage::from_entity(&e),
        None => {
            // No page entity here. If the path is a *section* (has child
            // pages), render a generated section-index listing them instead
            // of a not-found error — so an intermediate path (e.g. `guide`,
            // reached from a breadcrumb or sidebar) is a real destination.
            // Flows through the normal page path, so it gets the site's nav
            // + breadcrumbs + sidebar for free, and its links rewrite like
            // any markdown.
            let children =
                super::discovery::list_child_pages(peers, pid, &loc.site_id, &format!("{page_slug}/"));
            if children.is_empty() {
                return Err(ResolveError::PageMissing);
            }
            section_index_page(&page_slug, &children)
        }
    };

    Ok(ResolvedPage {
        location: Location {
            peer_id: Some(pid.to_string()),
            site_id: loc.site_id.clone(),
            page: page_slug,
        },
        manifest,
        page,
        assets: Vec::new(), // local: assets already in the store (ingest)
    })
}

/// Build a synthetic "section index" page: a heading + a markdown list of
/// links to the section's immediate children. The renderer turns it into
/// HTML and rewrites the links like any page. Shared by the local resolver
/// (children from the tree scan) and the HTTP-poll arm (children from a
/// fetched `pages.list`), so a remote intermediate path is a real destination
/// too — identical chrome either way.
pub(super) fn section_index_page(section_slug: &str, children: &[ChildEntry]) -> SitePage {
    let title = section_slug
        .rsplit('/')
        .next()
        .map(super::location::humanize)
        .unwrap_or_else(|| section_slug.to_string());
    let mut body = format!("# {title}\n\n");
    for child in children {
        let label = super::location::humanize(&child.name);
        // Root-absolute in-site link: this index is rendered *on* the section
        // page, so a `./`-relative link would double the section prefix under
        // directory-relative resolution. `/{slug}` is depth-safe from any page.
        body.push_str(&format!("- [{label}](/{section_slug}/{})\n", child.name));
    }
    SitePage::markdown(&title, &body)
}

/// A shared, late-bound repaint handle. The model owns the cell and fills
/// it from the render path (the handle isn't available at construction);
/// the async resolver fires it when a remote fetch lands.
pub type RepaintCell = Rc<RefCell<Option<RepaintFn>>>;

/// The transport router (mirrors `transport::MultiConnector`). It does
/// **not** dispatch — it classifies a [`Location`] and picks a resolver:
///
/// - a peer with a **registered HTTP origin** (the site-origin registry,
///   [`super::origins`]) → [`HttpPollResolver`] (async, `Pending`);
/// - everything else (the local/bound peer, or an unreachable remote) →
///   [`LocalTreeResolver`] (sync `Ready`).
///
/// This IS the "translate a click into HTTP-poll" decision, made entirely
/// at the app layer — the engine is downstream (the cache), never the
/// router. A live cross-peer (ws) arm slots in here later (P3).
pub struct MultiResolver {
    our_peer_id: String,
    local: LocalTreeResolver,
    http: HttpPollResolver,
    /// When we first saw each unregistered-remote Location with no local hit.
    /// After [`UNREACHABLE_GRACE_MS`] with still no route, the Location reports
    /// [`ResolveError::Unreachable`] instead of an endless `Pending`.
    pending_since: RefCell<HashMap<Location, f64>>,
    /// Page Locations whose body we've already **decided** about this session
    /// (written through if kept, or left ephemeral if manifest-pinned). Guards
    /// against re-deciding the same page every frame while the Worker-arm cache
    /// mirror catches up to the async `dispatch_write` — the in-memory HTTP
    /// cache keeps serving `Ready` in that window, and without this guard each
    /// frame would re-`seed_write` (or re-read the keep pref). Session-scoped,
    /// so a `keep_offline` toggle takes effect on the next session/reload or on
    /// not-yet-visited pages — not retroactively mid-session.
    persisted: RefCell<HashSet<Location>>,
    /// `(foreign, site)` pairs whose **manifest** + provenance we've written
    /// through. The manifest is the mutable ref + the enumerable anchor, so it
    /// is persisted **unconditionally** (O3 manifest-pinned, §5) — once per
    /// site, not once per page or per frame.
    manifest_persisted: RefCell<HashSet<(String, String)>>,
    /// Shared repaint signal — used to DRIVE the unreachable grace: the
    /// origin-less Pending branch has no fetch to self-fire a repaint, so it
    /// schedules a one-shot repaint past the grace to bound the spinner (BUG-2,
    /// [`Self::schedule_grace_repaint`]). The same cell the `http` arm holds.
    repaint: RepaintCell,
}

impl MultiResolver {
    pub fn new(our_peer_id: String, repaint: RepaintCell) -> Self {
        Self {
            our_peer_id,
            local: LocalTreeResolver,
            http: HttpPollResolver::new(repaint.clone()),
            pending_since: RefCell::new(HashMap::new()),
            persisted: RefCell::new(HashSet::new()),
            manifest_persisted: RefCell::new(HashSet::new()),
            repaint,
        }
    }

    /// Schedule a one-shot repaint just past the unreachable grace so a Pending
    /// origin-less Location is actually re-evaluated and bounded to
    /// `Unreachable` (→ cached shell) by the reactive frame loop, instead of
    /// spinning on "Loading…" forever (BUG-2). One-shot via
    /// `Closure::once_into_js` (self-cleaning, no `forget()` leak). Native has
    /// no timer/clock — no-op (it never trips the grace anyway).
    #[cfg(target_arch = "wasm32")]
    fn schedule_grace_repaint(&self) {
        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsCast;
        let repaint = self.repaint.clone();
        let cb = Closure::once_into_js(move || {
            if let Some(rp) = repaint.borrow().clone() {
                rp();
            }
        });
        if let Some(win) = web_sys::window() {
            // A small margin past the grace so `grace_elapsed` is definitely
            // true when the repaint-driven frame re-reads the clock.
            let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(
                cb.unchecked_ref(),
                UNREACHABLE_GRACE_MS as i32 + 250,
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn schedule_grace_repaint(&self) {}

    /// Write a freshly-fetched remote page through to MY durable store — the
    /// **manifest-pinned** caching browse (§3 + O3, §5). Foreign authored
    /// content (Category A) lands at its natural universal path
    /// `/{foreign}/sites/{S}/...` in my store (§2, byte-faithful), with the
    /// **manifest** always persisted (the mutable ref + the enumerable anchor —
    /// it keeps a visited site in `list_all_sites` + navigable across reloads)
    /// and the **page body persisted only when the site is "kept offline"**
    /// (the `keep_offline` preference). Default manifest-pinned leaves page
    /// bodies ephemeral — re-fetched on demand off the browser HTTP cache (§10).
    /// The SDK-tier provenance record (under `/{me}/system/cache/...`, the
    /// field-split §4) rides with the always-on manifest write. Guarded so each
    /// step runs at most once per session, not once per frame.
    fn persist_to_cache(&self, peers: &Peers, loc: &Location, rp: &ResolvedPage, origin: &str) {
        if self.persisted.borrow().contains(loc) {
            return;
        }
        let Some(foreign) = rp.location.peer_id.as_deref() else {
            return; // not a foreign page — nothing to cache cross-namespace
        };
        let site = &rp.location.site_id;
        let me = &self.our_peer_id;

        // §4b loud guard ("silent write-drops are the dangerous failure mode").
        // A cross-namespace cache write needs a well-formed foreign peer-id: the
        // Worker-arm L1 `tree.put` REJECTS a malformed peer-segment and
        // `seed_write`/`dispatch_write` then drop the failure silently — so the
        // cache would quietly stop persisting and look fine until a reload loses
        // the site (exactly the trap P1 closed, sneaking back invisibly). Real
        // cross-peer links always carry real ids, so this never fires in
        // production; if it ever does, make it LOUD and skip — marking the
        // Location persisted so it doesn't re-fire every frame.
        // (`is_peer_id` is the same Base58/46-char rule the write path enforces:
        // `entity_entity::EntityUri` core/entity/src/lib.rs.)
        if !entity_entity::EntityUri::is_peer_id(foreign) {
            tracing::error!(
                foreign = %foreign,
                site = %site,
                "site-cache write SKIPPED — foreign peer-id is not valid \
                 Base58/46-char; a malformed cross-namespace write would be \
                 dropped silently (DESIGN-PEER-GENERAL-SITE-CACHE §4b)"
            );
            self.persisted.borrow_mut().insert(loc.clone());
            return;
        }

        // Manifest + provenance: ALWAYS (manifest-pinned), once per site.
        let manifest_entity = rp.manifest.to_entity();
        let site_key = (foreign.to_string(), site.clone());
        if self.manifest_persisted.borrow_mut().insert(site_key) {
            peers.seed_write(me, paths::manifest_path(foreign, site), manifest_entity.clone());
            // The SDK-tier freshness ledger (under MY namespace; never synced).
            let prov = super::cache::CacheProvenance {
                last_reconciled: now_ms() as u64,
                pinned_root_hash: super::cache::manifest_hash_hex(&manifest_entity),
                source_transport: origin.to_string(),
            };
            super::cache::write_provenance(peers, me, foreign, site, &prov);
            // NOTE: we deliberately do NOT seed the origin registry here. The
            // handoff suggested it, but `persist_to_cache` is only reached from
            // the HTTP-fetch success path, which already required a registered
            // origin (`get_origin` Some at the `resolve_page` route split) — so
            // at cache time the origin is ALWAYS present and a seed would be a
            // guaranteed no-op. The real "loading forever" fix is driving the
            // grace + rendering the cached shell while Pending (BUG-2, see
            // `MultiResolver::schedule_grace_repaint` + the model's Pending
            // branch), not a redundant origin write.
        }

        // Embed assets the page pulls in (its closure, fetched by the HTTP arm).
        // Written through ALWAYS — not gated on keep_offline — because the
        // renderer resolves `<img>` from the durable store
        // ([`crate::dom::content_site::make_asset_resolver`]), so a
        // manifest-pinned site must still have its visible images present to
        // render this frame. Site-subgraph-bound at the foreign's natural
        // `/{foreign}/sites/{site}/assets/{name}` path (in MY store);
        // content-addressed, so identical bytes dedup. Bounded: just the
        // current page's embeds, not the whole site.
        for (name, asset) in &rp.assets {
            peers.seed_write(me, paths::asset_path(foreign, site, name), asset.to_entity());
        }

        // Page body: only when this site is "kept offline" (full caching, O3).
        // The pref is read once per page-Location then guarded; a later keep
        // toggle takes effect on the next session/reload or unvisited pages.
        if super::prefs::read_prefs(peers, me, foreign, site).keep_offline {
            peers.seed_write(
                me,
                paths::page_path(foreign, site, &rp.location.page),
                rp.page.to_entity(),
            );
        }

        self.persisted.borrow_mut().insert(loc.clone());
    }
}

/// Resolve a foreign [`Location`] from **MY** store — selector = my peer, path =
/// the foreign peer's natural `/{foreign}/sites/...` address (the §2
/// selector/path split). This is the durable **cache hit**: when the
/// write-through (§3) has landed the manifest + page, a navigate / reload /
/// offline read resolves here without touching the network. Returns `Err` on
/// any miss so the caller falls through to an HTTP fetch — no section-index
/// synthesis here (a cache miss just re-fetches online; the synthesized index
/// is the owned-tree resolver's job). Mirrors [`resolve_local`]'s empty-page →
/// `manifest.root()` rule.
fn resolve_from_my_store(
    peers: &Peers,
    my_peer_id: &str,
    loc: &Location,
) -> Result<ResolvedPage, ResolveError> {
    let Some(foreign) = loc.peer_id.as_deref() else {
        // A bound-peer (None) Location is the owned-tree case, not a foreign
        // cache read — let the normal local arm handle it.
        return Err(ResolveError::ManifestMissing);
    };
    let manifest = peers
        .get_entity(my_peer_id, &paths::manifest_path(foreign, &loc.site_id))
        .map(|e| SiteManifest::from_entity(&e))
        .ok_or(ResolveError::ManifestMissing)?;
    let page_slug = if loc.page.is_empty() {
        manifest.root().to_string()
    } else {
        loc.page.clone()
    };
    let page = peers
        .get_entity(my_peer_id, &paths::page_path(foreign, &loc.site_id, &page_slug))
        .map(|e| SitePage::from_entity(&e))
        .ok_or(ResolveError::PageMissing)?;
    Ok(ResolvedPage {
        location: Location {
            peer_id: Some(foreign.to_string()),
            site_id: loc.site_id.clone(),
            page: page_slug,
        },
        manifest,
        page,
        assets: Vec::new(), // cached foreign: assets already written through to my store
    })
}

/// Grace before an unregistered remote (no origin ever resolved, not local)
/// is reported [`ResolveError::Unreachable`] rather than spinning on `loading`.
/// Long enough to absorb the Worker-arm async origin-registry write (finding
/// #2 — sub-second); short enough that a genuinely-unknown peer surfaces an
/// honest error promptly.
const UNREACHABLE_GRACE_MS: f64 = 8000.0;

/// Has the unreachable grace elapsed? Pure (testable without a clock).
fn grace_elapsed(first_seen_ms: f64, now: f64) -> bool {
    now - first_seen_ms >= UNREACHABLE_GRACE_MS
}

impl ContentResolver for MultiResolver {
    fn resolve_page(&self, peers: &Peers, loc: &Location) -> ResolveOutcome {
        // Remote peer. We never look up an origin for our own peer (that
        // content is local).
        if let Some(pid) = loc.peer_id.as_deref() {
            if pid != self.our_peer_id {
                // Durable cache hit FIRST — the foreign site, written through to
                // my store on a prior fetch (§3), resolves from there with no
                // network. Read-before-route: a cache hit needs no origin, so a
                // reload/offline read works even if the route was since dropped.
                // This is what dissolves the "exit site ⇒ can't get back" trap.
                if let Ok(rp) = resolve_from_my_store(peers, &self.our_peer_id, loc) {
                    self.pending_since.borrow_mut().remove(loc);
                    return ResolveOutcome::Ready(Ok(rp));
                }
                if let Some(origin) = super::origins::get_origin(peers, &self.our_peer_id, pid) {
                    self.pending_since.borrow_mut().remove(loc); // a route appeared
                    // Cache miss → fetch over HTTP-poll; on a completed fetch,
                    // write the result through to the durable tree cache so the
                    // next read short-circuits above (reload-safe + offline).
                    let outcome = self.http.resolve(loc, &origin);
                    if let ResolveOutcome::Ready(Ok(rp)) = &outcome {
                        self.persist_to_cache(peers, loc, rp, &origin);
                    }
                    return outcome;
                }
                // Remote peer, no origin registered (yet). It may still be a
                // locally-hosted backend peer whose tree we can read, so try
                // local first. If that misses, we hold `Pending` (loading) for
                // a GRACE window — NOT a hard error: on the Worker arm the
                // origin-registry write is async, so the first frames see no
                // origin and a hard error would flash a misleading "manifest
                // missing" before the write lands. But we no longer wait
                // forever: once the grace elapses with still no route, we
                // report `Unreachable` so an unknown/unreachable peer is an
                // honest, bounded error rather than an endless spinner
                // (the predictability fix; finding #2 + the review). Native
                // (now_ms()==0) never trips the grace, matching the no-clock
                // convention the HTTP cache uses.
                return match self.local.resolve_page(peers, loc) {
                    ResolveOutcome::Ready(Ok(rp)) => {
                        self.pending_since.borrow_mut().remove(loc);
                        ResolveOutcome::Ready(Ok(rp))
                    }
                    _ => {
                        let now = now_ms();
                        let (first, newly_pending) = {
                            let mut pending = self.pending_since.borrow_mut();
                            match pending.get(loc).copied() {
                                Some(first) => (first, false),
                                None => {
                                    pending.insert(loc.clone(), now);
                                    (now, true)
                                }
                            }
                        };
                        // First time this Location goes Pending — DRIVE the
                        // grace. The reactive frame loop only re-renders on a
                        // trigger (subscription fire / repaint); the HTTP arm
                        // self-fires on fetch completion, but this origin-less
                        // branch has none, so without a nudge the window would
                        // sit on "Loading…" forever and never reach the grace
                        // bound (BUG-2). Schedule a one-shot repaint past the
                        // grace so the next frame flips Pending → Unreachable
                        // (→ cached shell). No-op on native (no timer/clock).
                        if newly_pending {
                            self.schedule_grace_repaint();
                        }
                        if now > 0.0 && grace_elapsed(first, now) {
                            ResolveOutcome::Ready(Err(ResolveError::Unreachable))
                        } else {
                            ResolveOutcome::Pending
                        }
                    }
                };
            }
        }
        self.local.resolve_page(peers, loc)
    }

    fn list_children(&self, peers: &Peers, loc: &Location, under: &str) -> Vec<ChildEntry> {
        // Same arm routing as resolve_page: a registered remote → the
        // HTTP-poll arm (now backed by the fetched `pages.list`); everything
        // else → the local tree scan.
        if let Some(pid) = loc.peer_id.as_deref() {
            if pid != self.our_peer_id {
                if let Some(origin) = super::origins::get_origin(peers, &self.our_peer_id, pid) {
                    return self.http.list_children(loc, under, &origin);
                }
            }
        }
        self.local.list_children(peers, loc, under)
    }

    /// The manifest for `loc` if durably available **without** the page — MY
    /// store for a cached foreign site (the §2 selector/path split), or the
    /// bound peer's tree for a local site. Backs the O3 manifest-pinned shell:
    /// when a manifest-pinned site's page is ephemeral + the origin is
    /// unreachable, the surface still renders the chrome (title + nav) instead
    /// of a bare error — a visited site never strands you on its shell.
    fn manifest_only(&self, peers: &Peers, loc: &Location) -> Option<SiteManifest> {
        let (selector, site_peer) = match loc.peer_id.as_deref() {
            Some(foreign) if foreign != self.our_peer_id => (self.our_peer_id.as_str(), foreign),
            _ => (self.our_peer_id.as_str(), self.our_peer_id.as_str()),
        };
        peers
            .get_entity(selector, &paths::manifest_path(site_peer, &loc.site_id))
            .map(|e| SiteManifest::from_entity(&e))
    }
}

/// Backoff before a failed Location is re-fetched. A *transient* failure
/// (network blip, origin not ready, momentary 404) must not permanently
/// brick a Location: we hold the error for this window (so we render it +
/// don't hammer the origin every frame), then the next resolve retries.
/// Short enough that a user re-click a moment later retries; long enough
/// that an idle error page doesn't storm the origin. Report finding #1.
const RETRY_BACKOFF_MS: f64 = 4000.0;

/// `Date.now()` millis (wall clock). Native has no `fetch()`/clock and
/// never runs the live path, so it reads 0.0 — tests drive the state
/// machine via [`HttpPollResolver::seed`]/[`seed_failed`](HttpPollResolver::seed_failed).
#[cfg(target_arch = "wasm32")]
fn now_ms() -> f64 {
    js_sys::Date::now()
}
#[cfg(not(target_arch = "wasm32"))]
fn now_ms() -> f64 {
    0.0
}

/// One entry in the HTTP-poll closure cache. `Done` is large (a whole
/// `ResolvedPage`) next to the small `Loading`/`Failed` variants, but
/// boxing would not help steady state: a `Loading` entry converges to
/// `Done` within a frame or two, so the map is dominated by `Done`
/// regardless — same rationale as [`ResolveOutcome`].
#[allow(clippy::large_enum_variant)]
enum CacheState {
    /// A `spawn_local` fetch is in flight; render `loading`.
    Loading,
    /// The fetch succeeded; render the page (cached for the session).
    Done(ResolvedPage),
    /// The fetch failed. We render the error until `retry_at_ms`, then the
    /// next resolve drops back to a fresh fetch — so a transient failure
    /// is recoverable rather than permanent (report finding #1). The error
    /// is **not** cached past the backoff.
    Failed { error: ResolveError, retry_at_ms: f64 },
}

/// One entry in the per-site `pages.list` cache. The slug list powers the
/// remote sidebar + section indexes (`children_from_slugs`); like the page
/// cache it's `Loading → Done`, with a short backoff on failure so a missing
/// listing (an optional enrichment) is retryable without storming the origin.
enum ListState {
    Loading,
    Done(Vec<String>),
    Failed { retry_at_ms: f64 },
}

/// Key for the `pages.list` cache: `(peer_id, site_id)`. A site's listing is
/// shared across all its pages, so it is keyed by the site, not the Location.
type SiteKey = (String, String);

/// Resolves a remote site over static HTTP-poll. On a cache miss it marks
/// the Location `Loading`, kicks off the two-hop closure fetch in a
/// `spawn_local`, and returns `Pending`; on completion it fills the cache
/// and fires the repaint, so the next frame reads `Ready` from cache. The
/// model is unchanged — it just re-reads the outcome each frame. The
/// cache is in-memory (per surface); promoting it to a tree-backed cache
/// under the remote qualified path (closure design §5) is a later
/// durability step.
///
/// A second cache (`lists`) holds each site's fetched `pages.list` slug set,
/// filled lazily the first time the sidebar asks for children — same
/// background-fetch-then-repaint shape as the page cache.
pub struct HttpPollResolver {
    cache: Rc<RefCell<HashMap<Location, CacheState>>>,
    lists: Rc<RefCell<HashMap<SiteKey, ListState>>>,
    repaint: RepaintCell,
}

impl HttpPollResolver {
    fn new(repaint: RepaintCell) -> Self {
        Self {
            cache: Rc::new(RefCell::new(HashMap::new())),
            lists: Rc::new(RefCell::new(HashMap::new())),
            repaint,
        }
    }

    fn resolve(&self, loc: &Location, origin: &str) -> ResolveOutcome {
        // Decide from the current cache state whether to serve a cached
        // result or (re)start a fetch. The immutable borrow is dropped
        // before any mutable re-borrow below.
        let start_fetch = {
            let cache = self.cache.borrow();
            match cache.get(loc) {
                Some(CacheState::Loading) => return ResolveOutcome::Pending,
                Some(CacheState::Done(rp)) => return ResolveOutcome::Ready(Ok(rp.clone())),
                Some(CacheState::Failed { error, retry_at_ms }) => {
                    if now_ms() < *retry_at_ms {
                        // Still within backoff — show the error, don't refetch.
                        return ResolveOutcome::Ready(Err(*error));
                    }
                    true // backoff elapsed → retry
                }
                None => true, // first sight of this Location → fetch
            }
        };
        if start_fetch {
            self.cache.borrow_mut().insert(loc.clone(), CacheState::Loading);
            self.spawn_fetch(loc.clone(), origin.to_string());
        }
        ResolveOutcome::Pending
    }

    /// Children under `under` for a remote site, from its fetched `pages.list`.
    /// Cache hit (`Done`) → reduce the slug set with
    /// [`super::discovery::children_from_slugs`]; miss → kick a background
    /// fetch + repaint and return empty for now (next frame fills the sidebar),
    /// mirroring [`Self::resolve`]'s Pending shape. Loading / in-backoff failure
    /// → empty (no sidebar yet / this listing isn't available).
    fn list_children(&self, loc: &Location, under: &str, origin: &str) -> Vec<ChildEntry> {
        let key: SiteKey = (loc.peer_id.clone().unwrap_or_default(), loc.site_id.clone());
        let have: Option<Vec<String>> = {
            let lists = self.lists.borrow();
            match lists.get(&key) {
                Some(ListState::Done(slugs)) => Some(slugs.clone()),
                Some(ListState::Loading) => return Vec::new(),
                Some(ListState::Failed { retry_at_ms }) if now_ms() < *retry_at_ms => {
                    return Vec::new()
                }
                _ => None, // absent, or an expired failure → (re)fetch
            }
        };
        match have {
            Some(slugs) => super::discovery::children_from_slugs(&slugs, under),
            None => {
                self.lists.borrow_mut().insert(key.clone(), ListState::Loading);
                self.spawn_list_fetch(key, origin.to_string());
                Vec::new()
            }
        }
    }

    /// Fetch the site's `pages.list` (browser only); on completion fill the
    /// list cache and fire the repaint so the sidebar re-renders populated.
    #[cfg(target_arch = "wasm32")]
    fn spawn_list_fetch(&self, key: SiteKey, origin: String) {
        let lists = self.lists.clone();
        let repaint = self.repaint.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let (peer, site) = (&key.0, &key.1);
            let state =
                match super::http_poll::fetch_pages_list(&super::http_poll::FetchBinSource, &origin, peer, site)
                    .await
                {
                    Ok(slugs) => ListState::Done(slugs),
                    Err(_) => ListState::Failed { retry_at_ms: now_ms() + RETRY_BACKOFF_MS },
                };
            lists.borrow_mut().insert(key, state);
            if let Some(rp) = repaint.borrow().clone() {
                rp();
            }
        });
    }

    /// Native builds have no `fetch()` — tests pre-seed the list cache via
    /// [`Self::seed_list`].
    #[cfg(not(target_arch = "wasm32"))]
    fn spawn_list_fetch(&self, _key: SiteKey, _origin: String) {}

    #[cfg(test)]
    fn seed_list(&self, peer_id: &str, site_id: &str, slugs: Vec<String>) {
        self.lists
            .borrow_mut()
            .insert((peer_id.to_string(), site_id.to_string()), ListState::Done(slugs));
    }

    /// Spawn the two-hop closure fetch (browser only). On completion fill
    /// the cache and fire the repaint so the render loop re-reads `Ready`.
    #[cfg(target_arch = "wasm32")]
    fn spawn_fetch(&self, loc: Location, origin: String) {
        let cache = self.cache.clone();
        let repaint = self.repaint.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result =
                super::http_poll::resolve_closure_via(&super::http_poll::FetchBinSource, &origin, &loc)
                    .await;
            let state = match result {
                Ok(rp) => CacheState::Done(rp),
                // Don't cache the error past a short backoff — a transient
                // failure must be retryable (finding #1).
                Err(error) => CacheState::Failed { error, retry_at_ms: now_ms() + RETRY_BACKOFF_MS },
            };
            cache.borrow_mut().insert(loc, state);
            if let Some(rp) = repaint.borrow().clone() {
                rp();
            }
        });
    }

    /// Native builds have no `fetch()`/executor — the live path is
    /// browser-only. The Location stays `Loading`; tests pre-seed the
    /// cache via [`Self::seed`] to exercise the Ready/Pending machine.
    #[cfg(not(target_arch = "wasm32"))]
    fn spawn_fetch(&self, _loc: Location, _origin: String) {}

    #[cfg(test)]
    fn seed(&self, loc: Location, result: Result<ResolvedPage, ResolveError>) {
        let state = match result {
            Ok(rp) => CacheState::Done(rp),
            // Seeded errors never expire (INFINITY backoff) so a test can
            // assert the in-backoff render; the live path uses a real clock.
            Err(error) => CacheState::Failed { error, retry_at_ms: f64::INFINITY },
        };
        self.cache.borrow_mut().insert(loc, state);
    }

    #[cfg(test)]
    fn seed_failed(&self, loc: Location, error: ResolveError, retry_at_ms: f64) {
        self.cache.borrow_mut().insert(loc, CacheState::Failed { error, retry_at_ms });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_site::format::{NavItem, SiteAsset, SiteManifest, SitePage};

    /// Seed a minimal site (manifest + two pages) into a peer's tree via
    /// L0 store puts, then return the peer id.
    fn seed_church_site(peers: &Peers) -> String {
        let pid = peers.primary_peer_id().to_string();
        let ctx = peers.test_seed_ctx(&pid);

        let manifest = SiteManifest::new(
            "church",
            "Entity Church Foundation",
            "index",
            vec![NavItem::new("Home", "/index"), NavItem::new("About", "/about")],
        );
        ctx.store().put(&paths::manifest_path(&pid, "church"), manifest.to_entity()).ok();
        ctx.store()
            .put(
                &paths::page_path(&pid, "church", "index"),
                SitePage::markdown("Home", "# Welcome\n\nSee [About](./about).").to_entity(),
            )
            .ok();
        ctx.store()
            .put(
                &paths::page_path(&pid, "church", "about"),
                SitePage::markdown("About", "# About us").to_entity(),
            )
            .ok();
        pid
    }

    #[test]
    fn resolves_root_page_when_page_empty() {
        let peers = Peers::new_direct();
        let pid = seed_church_site(&peers);

        let loc = Location { peer_id: Some(pid), site_id: "church".into(), page: String::new() };
        match LocalTreeResolver.resolve_page(&peers, &loc) {
            ResolveOutcome::Ready(Ok(rp)) => {
                assert_eq!(rp.location.page, "index", "empty page resolves to manifest root");
                assert_eq!(rp.manifest.title, "Entity Church Foundation");
                assert_eq!(rp.manifest.nav.len(), 2);
                assert_eq!(rp.page.title(), "Home");
                assert!(rp.page.body.contains("Welcome"));
            }
            other => panic!("expected Ready(Ok), got {other:?}"),
        }
    }

    #[test]
    fn resolves_named_page() {
        let peers = Peers::new_direct();
        let pid = seed_church_site(&peers);

        let loc = Location { peer_id: Some(pid), site_id: "church".into(), page: "about".into() };
        match LocalTreeResolver.resolve_page(&peers, &loc) {
            ResolveOutcome::Ready(Ok(rp)) => {
                assert_eq!(rp.page.title(), "About");
                assert!(rp.page.body.contains("About us"));
            }
            other => panic!("expected Ready(Ok), got {other:?}"),
        }
    }

    #[test]
    fn missing_manifest_errors() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let loc = Location { peer_id: Some(pid), site_id: "nonexistent".into(), page: String::new() };
        match LocalTreeResolver.resolve_page(&peers, &loc) {
            ResolveOutcome::Ready(Err(ResolveError::ManifestMissing)) => {}
            other => panic!("expected ManifestMissing, got {other:?}"),
        }
    }

    #[test]
    fn missing_page_errors() {
        let peers = Peers::new_direct();
        let pid = seed_church_site(&peers);
        let loc = Location { peer_id: Some(pid), site_id: "church".into(), page: "ghost".into() };
        match LocalTreeResolver.resolve_page(&peers, &loc) {
            ResolveOutcome::Ready(Err(ResolveError::PageMissing)) => {}
            other => panic!("expected PageMissing, got {other:?}"),
        }
    }

    // -- MultiResolver routing --------------------------------------------

    fn repaint_cell() -> RepaintCell {
        Rc::new(RefCell::new(None))
    }

    #[test]
    fn multi_resolver_uses_local_for_bound_peer_and_none() {
        let peers = Peers::new_direct();
        let pid = seed_church_site(&peers);
        let mr = MultiResolver::new(pid.clone(), repaint_cell());

        // peer == our peer → local sync resolve.
        let loc = Location { peer_id: Some(pid.clone()), site_id: "church".into(), page: String::new() };
        assert!(matches!(mr.resolve_page(&peers, &loc), ResolveOutcome::Ready(Ok(_))));

        // peer == None → local.
        let loc = Location { peer_id: None, site_id: "church".into(), page: String::new() };
        assert!(matches!(mr.resolve_page(&peers, &loc), ResolveOutcome::Ready(Ok(_))));
    }

    #[test]
    fn multi_resolver_routes_registered_remote_to_http_arm() {
        let peers = Peers::new_direct();
        let our = peers.primary_peer_id().to_string();
        // A remote peer with a registered origin.
        crate::content_site::origins::set_origin(&peers, &our, "PEERB", "http://labs.example");
        let mr = MultiResolver::new(our, repaint_cell());

        let loc = Location { peer_id: Some("PEERB".into()), site_id: "labs".into(), page: String::new() };
        // First touch → HTTP arm marks Loading + (native) no fetch → Pending.
        assert!(
            matches!(mr.resolve_page(&peers, &loc), ResolveOutcome::Pending),
            "a registered remote routes to the HTTP-poll arm (Pending)"
        );

        // Simulate the fetch landing by seeding the http cache, then the
        // next resolve reads Ready from cache — the model's per-frame
        // re-read path.
        let resolved = ResolvedPage {
            location: loc.clone(),
            manifest: SiteManifest::new("labs", "Labs", "index", vec![]),
            page: SitePage::markdown("Home", "# hi"),
            assets: Vec::new(),
        };
        mr.http.seed(loc.clone(), Ok(resolved));
        match mr.resolve_page(&peers, &loc) {
            ResolveOutcome::Ready(Ok(rp)) => assert_eq!(rp.manifest.title, "Labs"),
            other => panic!("expected Ready from cache, got {other:?}"),
        }
    }

    #[test]
    fn multi_resolver_unregistered_remote_is_pending_not_local_error() {
        let peers = Peers::new_direct();
        let our = peers.primary_peer_id().to_string();
        let mr = MultiResolver::new(our, repaint_cell());
        // No origin registered for PEERX and the remote site isn't in our
        // local tree → `Pending` (loading), NOT a misleading local
        // ManifestMissing. This avoids the Worker-arm "local-error flash"
        // before the async origin-registry write lands (finding #2). Native
        // has no clock (now_ms()==0), so the grace never trips here.
        let loc = Location { peer_id: Some("PEERX".into()), site_id: "x".into(), page: String::new() };
        assert!(matches!(mr.resolve_page(&peers, &loc), ResolveOutcome::Pending));
    }

    // -- Peer-general durable cache (write-through + cache-read, §2/§3) ----

    #[test]
    fn cached_foreign_site_resolves_from_my_store_without_origin() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let mr = MultiResolver::new(me.clone(), repaint_cell());
        // A REAL foreign peer-id: tree paths validate the peer-segment format,
        // so the cache lives at a real `/{foreign}/...` path (as in production).
        let foreign = Peers::new_direct().primary_peer_id().to_string();
        let foreign = foreign.as_str();

        // Seed a foreign peer's site INTO MY store at its natural universal
        // path — exactly what the write-through lands. (L0 seed; the store
        // keys on the full path with no peer-segment validation, §2.)
        let ctx = peers.test_seed_ctx(&me);
        ctx.store()
            .put(
                &paths::manifest_path(foreign, "blog"),
                SiteManifest::new("blog", "Bob's Blog", "index", vec![NavItem::new("Home", "/index")])
                    .to_entity(),
            )
            .ok();
        ctx.store()
            .put(
                &paths::page_path(foreign, "blog", "index"),
                SitePage::markdown("Home", "# Hi from Bob").to_entity(),
            )
            .ok();

        // No origin registered — a durable cache hit needs none (read-before-
        // route). This is the reload / exit-and-return / offline path.
        let loc = Location { peer_id: Some(foreign.into()), site_id: "blog".into(), page: String::new() };
        match mr.resolve_page(&peers, &loc) {
            ResolveOutcome::Ready(Ok(rp)) => {
                assert_eq!(rp.manifest.title, "Bob's Blog");
                assert_eq!(rp.location.page, "index", "empty page → manifest root from the cached manifest");
                assert!(rp.page.body.contains("Bob"));
            }
            other => panic!("expected a durable cache hit, got {other:?}"),
        }
    }

    /// Seed an HTTP-arm fetch result for a labs/index Location into `mr` and
    /// drive the resolve that fires the write-through. Shared by the
    /// manifest-pinned-default and kept-offline tests.
    fn drive_labs_fetch(mr: &MultiResolver, peers: &Peers, foreign: &str) -> Location {
        let loc = Location { peer_id: Some(foreign.to_string()), site_id: "labs".into(), page: String::new() };
        assert!(matches!(mr.resolve_page(peers, &loc), ResolveOutcome::Pending));
        mr.http.seed(
            loc.clone(),
            Ok(ResolvedPage {
                location: Location { peer_id: Some(foreign.to_string()), site_id: "labs".into(), page: "index".into() },
                manifest: SiteManifest::new("labs", "Labs", "index", vec![]),
                page: SitePage::markdown("Home", "# hi from labs"),
                assets: Vec::new(),
            }),
        );
        assert!(matches!(mr.resolve_page(peers, &loc), ResolveOutcome::Ready(Ok(_))));
        loc
    }

    #[test]
    fn fetch_writes_manifest_through_but_page_ephemeral_by_default() {
        // O3 manifest-pinned DEFAULT: the manifest (the enumerable anchor +
        // mutable ref) + provenance write through, the PAGE BODY does NOT.
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let foreign = Peers::new_direct().primary_peer_id().to_string();
        crate::content_site::origins::set_origin(&peers, &me, &foreign, "http://labs.example");
        let mr = MultiResolver::new(me.clone(), repaint_cell());
        let loc = drive_labs_fetch(&mr, &peers, &foreign);

        assert!(
            peers.get_entity(&me, &paths::manifest_path(&foreign, "labs")).is_some(),
            "manifest ALWAYS written through (manifest-pinned)"
        );
        assert!(
            peers.get_entity(&me, &paths::page_path(&foreign, "labs", "index")).is_none(),
            "page body NOT written by default — ephemeral, re-fetched on demand"
        );
        let prov = crate::content_site::cache::read_provenance(&peers, &me, &foreign, "labs")
            .expect("provenance recorded with the always-on manifest write");
        assert_eq!(prov.source_transport, "http://labs.example");
        assert!(!prov.pinned_root_hash.is_empty(), "manifest hash pinned");

        // The site is durably ENUMERABLE (manifest present), and the manifest
        // resolves on a cold resolver — but the ephemeral page re-fetches
        // (Pending here: fresh in-memory cache, native = no fetch).
        let mr2 = MultiResolver::new(me.clone(), repaint_cell());
        assert!(
            mr2.manifest_only(&peers, &loc).is_some(),
            "manifest durably enumerable on reload (the shell anchor)"
        );
        assert!(
            matches!(mr2.resolve_page(&peers, &loc), ResolveOutcome::Pending),
            "ephemeral page re-fetches on a cold resolver (manifest-pinned)"
        );
    }

    #[test]
    fn kept_offline_site_writes_page_through_and_serves_fully_on_reload() {
        // "Keep this site" (keep_offline pref) promotes to full caching: the
        // page body persists too, so a cold resolver serves the whole page
        // offline — the original write-through proof, now opt-in.
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let foreign = Peers::new_direct().primary_peer_id().to_string();
        crate::content_site::origins::set_origin(&peers, &me, &foreign, "http://labs.example");
        // Keep BEFORE browsing → the page body is written on fetch.
        crate::content_site::prefs::update_prefs(&peers, &me, &foreign, "labs", |p| {
            p.keep_offline = true
        });

        let mr = MultiResolver::new(me.clone(), repaint_cell());
        let loc = drive_labs_fetch(&mr, &peers, &foreign);

        assert!(
            peers.get_entity(&me, &paths::page_path(&foreign, "labs", "index")).is_some(),
            "kept site persists the page body (full offline)"
        );
        // A cold resolver serves the entire page from the durable tree — no net.
        let mr2 = MultiResolver::new(me.clone(), repaint_cell());
        match mr2.resolve_page(&peers, &loc) {
            ResolveOutcome::Ready(Ok(rp)) => assert!(rp.page.body.contains("labs")),
            other => panic!("kept site serves fully offline on reload, got {other:?}"),
        }
    }

    #[test]
    fn fetched_embed_assets_write_through_to_my_store_always() {
        // The Stage-4 asset closure: a remote page's embed assets (carried in
        // ResolvedPage.assets by the HTTP arm) are written through to MY store at
        // the foreign's natural assets path — UNCONDITIONALLY (not gated on
        // keep_offline), because the renderer resolves <img> from the store, so a
        // manifest-pinned site still needs its images present.
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let foreign = Peers::new_direct().primary_peer_id().to_string();
        crate::content_site::origins::set_origin(&peers, &me, &foreign, "http://labs.example");
        let mr = MultiResolver::new(me.clone(), repaint_cell());
        let loc = Location { peer_id: Some(foreign.clone()), site_id: "labs".into(), page: String::new() };
        mr.http.seed(
            loc.clone(),
            Ok(ResolvedPage {
                location: Location { peer_id: Some(foreign.clone()), site_id: "labs".into(), page: "index".into() },
                manifest: SiteManifest::new("labs", "Labs", "index", vec![]),
                page: SitePage::markdown("Home", "# hi\n\n::embed[Fig]{ref=assets/figures/x.svg}"),
                assets: vec![(
                    "figures/x.svg".into(),
                    SiteAsset::new("image/svg+xml", b"<svg/>".to_vec()),
                )],
            }),
        );
        // Drive the resolve that fires the write-through.
        assert!(matches!(mr.resolve_page(&peers, &loc), ResolveOutcome::Ready(Ok(_))));

        // The asset landed in MY store at the foreign's natural assets path, so
        // the renderer's store-backed resolver will find it.
        let cached = peers
            .get_entity(&me, &paths::asset_path(&foreign, "labs", "figures/x.svg"))
            .expect("embed asset written through to my store");
        let asset = SiteAsset::from_entity(&cached);
        assert_eq!(asset.media_type, "image/svg+xml");
        assert_eq!(asset.bytes, b"<svg/>");
        // …even though the page body itself is ephemeral (manifest-pinned default).
        assert!(
            peers.get_entity(&me, &paths::page_path(&foreign, "labs", "index")).is_none(),
            "page body still ephemeral by default; only the asset closure persists"
        );
    }

    #[test]
    fn write_through_runs_once_not_every_frame() {
        // The `persisted` guard: re-resolving a cached Location must not keep
        // re-seeding. We assert idempotence via the guard set indirectly — a
        // second resolve still returns the same Ready without error, and the
        // provenance timestamp path stays stable (one record).
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let foreign = Peers::new_direct().primary_peer_id().to_string();
        crate::content_site::origins::set_origin(&peers, &me, &foreign, "http://labs.example");
        let mr = MultiResolver::new(me.clone(), repaint_cell());
        let loc = Location { peer_id: Some(foreign.clone()), site_id: "labs".into(), page: String::new() };
        mr.http.seed(
            loc.clone(),
            Ok(ResolvedPage {
                location: Location { peer_id: Some(foreign.clone()), site_id: "labs".into(), page: "index".into() },
                manifest: SiteManifest::new("labs", "Labs", "index", vec![]),
                page: SitePage::markdown("Home", "# hi"),
                assets: Vec::new(),
            }),
        );
        // Drive several frames; the guard makes the persist a no-op after #1.
        for _ in 0..3 {
            assert!(matches!(mr.resolve_page(&peers, &loc), ResolveOutcome::Ready(Ok(_))));
        }
        assert!(mr.persisted.borrow().contains(&loc), "Location marked persisted once");
        assert_eq!(mr.persisted.borrow().len(), 1, "exactly one cached Location");
    }

    #[test]
    fn malformed_foreign_peer_id_skips_cache_write_loudly_not_silently() {
        // §4b: reads DON'T validate the peer-segment but the Worker-arm write
        // path does — a malformed foreign id would have its cross-namespace
        // write dropped SILENTLY (the dangerous failure mode: cache looks fine
        // until a reload loses it). The `persist_to_cache` guard turns that into
        // a skip (the render still serves the in-memory HTTP cache) + a loud
        // error log, and marks the Location persisted so it doesn't re-fire.
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let foreign = "bills-labs-peer".to_string(); // NOT 46-char Base58
        crate::content_site::origins::set_origin(&peers, &me, &foreign, "http://labs.example");
        let mr = MultiResolver::new(me.clone(), repaint_cell());
        let loc = Location { peer_id: Some(foreign.clone()), site_id: "labs".into(), page: String::new() };
        mr.http.seed(
            loc.clone(),
            Ok(ResolvedPage {
                location: Location { peer_id: Some(foreign.clone()), site_id: "labs".into(), page: "index".into() },
                manifest: SiteManifest::new("labs", "Labs", "index", vec![]),
                page: SitePage::markdown("Home", "# hi"),
                assets: Vec::new(),
            }),
        );

        // The render still succeeds off the in-memory HTTP cache (reads tolerate
        // the malformed id)…
        assert!(matches!(mr.resolve_page(&peers, &loc), ResolveOutcome::Ready(Ok(_))));

        // …but NOTHING was written cross-namespace under the malformed segment.
        assert!(
            peers.get_entity(&me, &paths::manifest_path(&foreign, "labs")).is_none(),
            "no cross-namespace write under a malformed foreign peer-id"
        );
        // Provenance (within MY valid namespace) is skipped too — the persist
        // short-circuited before any write.
        assert!(
            crate::content_site::cache::read_provenance(&peers, &me, &foreign, "labs").is_none(),
            "provenance skipped along with the content write"
        );
        // Guarded: marked persisted so the skip doesn't re-fire each frame.
        assert!(mr.persisted.borrow().contains(&loc), "guard marks the Location persisted once");
    }

    #[test]
    fn unreachable_grace_is_bounded_not_infinite() {
        // The grace decision is pure + clock-injected (the live path feeds
        // js_sys::Date::now()). Under the window → keep loading; past it →
        // report Unreachable instead of spinning forever (predictability fix).
        let t0 = 1_000_000.0;
        assert!(!grace_elapsed(t0, t0), "fresh: still loading");
        assert!(!grace_elapsed(t0, t0 + UNREACHABLE_GRACE_MS - 1.0), "within grace: loading");
        assert!(grace_elapsed(t0, t0 + UNREACHABLE_GRACE_MS), "past grace: unreachable");
    }

    #[test]
    fn multi_resolver_remote_list_children_uses_fetched_pages_list() {
        let peers = Peers::new_direct();
        let our = peers.primary_peer_id().to_string();
        crate::content_site::origins::set_origin(&peers, &our, "PEERB", "http://labs.example");
        let mr = MultiResolver::new(our, repaint_cell());
        let loc = Location { peer_id: Some("PEERB".into()), site_id: "labs".into(), page: String::new() };

        // First touch with no listing cached → empty (a background fetch is
        // kicked; native is a no-op, so it stays empty here — Loading shape).
        assert!(mr.list_children(&peers, &loc, "").is_empty());

        // Simulate the `pages.list` fetch landing.
        mr.http.seed_list(
            "PEERB",
            "labs",
            vec!["index".into(), "guide/intro".into(), "guide/advanced/internals".into()],
        );

        // Top level: the `guide` section + the `index` leaf, from the slugs.
        let top = mr.list_children(&peers, &loc, "");
        let guide = top.iter().find(|c| c.name == "guide").expect("guide section");
        assert!(guide.is_section && !guide.is_page, "guide is a section");
        assert!(top.iter().any(|c| c.name == "index" && c.is_page), "index leaf: {top:?}");

        // Descend one level — the nested page is reachable without a local tree.
        let guide_children = mr.list_children(&peers, &loc, "guide/");
        assert!(guide_children.iter().any(|c| c.name == "intro" && c.is_page));
        assert!(guide_children.iter().any(|c| c.name == "advanced" && c.is_section));
    }

    #[test]
    fn http_failed_within_backoff_renders_error_then_retries_after() {
        let repaint = repaint_cell();
        let http = HttpPollResolver::new(repaint);
        let loc = Location { peer_id: Some("PEERB".into()), site_id: "labs".into(), page: String::new() };

        // Within backoff (retry_at in the future) → render the cached error,
        // do NOT refetch.
        http.seed_failed(loc.clone(), ResolveError::PageMissing, f64::INFINITY);
        assert!(matches!(
            http.resolve(&loc, "http://labs.example"),
            ResolveOutcome::Ready(Err(ResolveError::PageMissing)),
        ));

        // Backoff elapsed (retry_at in the past) → drop back to a fresh
        // fetch (Loading → Pending), so a transient failure is recoverable.
        http.seed_failed(loc.clone(), ResolveError::PageMissing, -1.0);
        assert!(
            matches!(http.resolve(&loc, "http://labs.example"), ResolveOutcome::Pending),
            "an expired failure retries instead of staying a permanent error"
        );
    }

    #[test]
    fn section_index_links_are_root_absolute() {
        // The generated section index is rendered *on* the section page; its
        // child links MUST be root-absolute (`/{section}/{child}`) so that
        // directory-relative body resolution does not double the prefix on a
        // nested section (e.g. `guide/advanced` → `guide/guide/advanced/...`).
        let children = vec![
            ChildEntry { name: "intro".into(), is_page: true, is_section: false },
            ChildEntry { name: "advanced".into(), is_page: false, is_section: true },
        ];
        let page = section_index_page("guide", &children);
        assert!(page.body.contains("(/guide/intro)"), "child link not root-absolute: {}", page.body);
        assert!(page.body.contains("(/guide/advanced)"), "section link not root-absolute: {}", page.body);
        assert!(!page.body.contains("(./"), "no `./`-relative links may leak: {}", page.body);
    }
}

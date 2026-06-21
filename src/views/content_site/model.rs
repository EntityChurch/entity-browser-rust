//! Content Site model — long-lived data layer for one site window.
//!
//! Mirrors the Knowledge Base shape: `Arc<Mutex<Inner>>` interior
//! mutability (all methods `&self`), `&Peers`-taking action methods,
//! and a pure `render_output(&Peers)` the renderer consumes. Navigation
//! state (current location + back-history) is persisted to the window's
//! app-namespace state path so it survives reload and so a write fires
//! the window-state watch → re-render.
//!
//! Resolution goes through the [`ContentResolver`] seam. P1 holds a
//! [`LocalTreeResolver`]; swapping in cross-peer / HTTP-poll resolvers
//! later doesn't touch this model (it just reads the cache / outcome).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use entity_entity::Entity;

use super::output::{
    Crumb, NavLink, RailFilter, SectionLink, SiteDirectory, SiteEntry, SiteRenderOutput,
};
use crate::content_site::resolver::ResolvedPage;
use crate::content_site::{
    classify_link, humanize, render_page_body, resolve_target, ContentResolver, Location,
    MultiResolver, NavItem, RepaintCell, ResolveError, ResolveOutcome, SitePage,
};
use crate::peers::Peers;
use crate::window::{RepaintFn, WindowId};

/// Entity type for the window's persisted navigation state.
const STATE_TYPE: &str = "app/state/content_site";

/// Active-trail test for a nav item: is `current_page` the nav target's
/// `page`, or a descendant within the same top-level section? A real
/// site header keeps a section highlighted across its whole subtree, not
/// only on the exact landing page (the deep-site cycle's GAP1).
/// Section = the first path segment of the slug (`guide/advanced/caching`
/// → `guide`); an empty section never matches by prefix (only exactly).
fn in_section(target_page: &str, current_page: &str) -> bool {
    if target_page == current_page {
        return true;
    }
    fn section(p: &str) -> &str {
        p.split('/').next().unwrap_or("")
    }
    let s = section(target_page);
    !s.is_empty() && s == section(current_page)
}

/// Map a [`NavItem`] to a render-ready [`NavLink`] (the top nav bar),
/// computing the active-trail flag relative to the current location. The
/// section sidebar is built separately from the live tree ([`build_sidebar`]).
/// Assemble the peer-free part of a [`SiteRenderOutput`] from a resolved
/// closure (manifest + page). `nav_base` is the location relative links are
/// classified against (the requested location for the window/overlay; the
/// home location for fast-paint). `sidebar` is the one peer-dependent
/// enrichment ([`build_sidebar`] reads `.list` from the live tree) — callers
/// without a peer pass `vec![]` and the renderer keeps the single-pane layout.
///
/// Reused by the pre-peer **fast-paint** boot path (cut 2c): it renders the
/// remote home over HTTP into `#site-layer` *before* the local peer exists,
/// using the SAME render output the live overlay produces — so the static
/// paint and the live page are identical (no flash of a different version).
pub fn output_from_resolved(
    rp: &crate::content_site::resolver::ResolvedPage,
    nav_base: &Location,
    can_go_back: bool,
    sidebar: Vec<SectionLink>,
) -> SiteRenderOutput {
    let nav = rp
        .manifest
        .nav
        .iter()
        .map(|item| nav_link(item, nav_base, &rp.location.site_id, &rp.location.page))
        .collect();
    let breadcrumbs = breadcrumbs(
        &rp.manifest.title,
        rp.manifest.root(),
        &rp.location.page,
        rp.page.title(),
    );
    SiteRenderOutput {
        site_title: rp.manifest.title.clone(),
        nav,
        breadcrumbs,
        sidebar,
        can_go_back,
        page_title: rp.page.title().to_string(),
        // F-CONTENT-1: format-aware, but `html` is NOT raw passthrough
        // (no sanitizer in the tree) — render_page_body escapes it.
        body_html: render_page_body(&rp.page.format, &rp.page.body),
        peer: rp.location.peer_id.clone(),
        site_id: rp.location.site_id.clone(),
        current_page: rp.location.page.clone(),
        error: None,
        loading: false,
    }
}

fn nav_link(item: &NavItem, loc: &Location, cur_site: &str, cur_page: &str) -> NavLink {
    let active = resolve_target(&classify_link(&item.target, loc), loc)
        .map(|t| t.site_id == cur_site && in_section(&t.page, cur_page))
        .unwrap_or(false);
    NavLink { label: item.label.clone(), target: item.target.clone(), active }
}

/// Build the breadcrumb trail to `current_page` from its slug segments.
/// Pure presentation (the deep-site cycle's GAP2). On the root page there
/// is no trail (empty). Otherwise: a clickable site-root crumb, then one
/// crumb per path segment — **intermediate segments are clickable**
/// (they navigate to that section path, which renders a section-index
/// listing), and the last is the page title ("you are here", not
/// clickable).
fn breadcrumbs(site_title: &str, root_slug: &str, current_page: &str, page_title: &str) -> Vec<Crumb> {
    if current_page.is_empty() || current_page == root_slug {
        return Vec::new();
    }
    let mut out =
        vec![Crumb { label: site_title.to_string(), target: Some("/".to_string()) }];
    let mut segs: Vec<&str> = current_page.split('/').filter(|s| !s.is_empty()).collect();
    // A `…/index` page IS the landing page of its section — collapse the
    // trailing `index` so the trail reads `… › Research` (the section, current),
    // not `… › Research › Research` (the dir crumb, then its like-named index).
    if segs.len() > 1 && *segs.last().unwrap() == "index" {
        segs.pop();
    }
    for (i, seg) in segs.iter().enumerate() {
        if i + 1 == segs.len() {
            // The current page — a label, not a link.
            out.push(Crumb { label: page_title.to_string(), target: None });
        } else {
            // An ancestor section — link to its path (→ section index).
            let path = segs[..=i].join("/");
            out.push(Crumb { label: humanize(seg), target: Some(format!("/{path}")) });
        }
    }
    out
}

/// Build the tree-driven section sidebar from the live page listing
/// (`.list`, via the resolver seam). Shows the top-level entries and
/// expands the active top-level section one level (its child pages) — the
/// standard docs sidebar. Empty when the site is flat or listing is
/// unavailable (remote HTTP — finding #4), so the renderer falls back to
/// the simple single-pane layout. Works on both arms (Direct + Worker;
/// the Worker cache mirror is fed by the overlay's site-prefix subscribe).
fn build_sidebar(
    resolver: &dyn ContentResolver,
    peers: &Peers,
    loc: &Location,
    current_page: &str,
) -> Vec<SectionLink> {
    let top = resolver.list_children(peers, loc, "");
    if top.is_empty() {
        return Vec::new();
    }
    let cur_section = current_page.split('/').next().unwrap_or("");
    let mut out = Vec::new();
    for e in &top {
        out.push(SectionLink {
            label: humanize(&e.name),
            target: format!("/{}", e.name),
            active: in_section(&e.name, current_page),
            depth: 0,
            is_section: e.is_section,
        });
        // Expand the active top-level section one level (its child pages).
        if e.is_section && !cur_section.is_empty() && e.name == cur_section {
            for k in resolver.list_children(peers, loc, &format!("{}/", e.name)) {
                let kpath = format!("{}/{}", e.name, k.name);
                let active = current_page == kpath || current_page.starts_with(&format!("{kpath}/"));
                out.push(SectionLink {
                    label: humanize(&k.name),
                    target: format!("/{kpath}"),
                    active,
                    depth: 1,
                    is_section: k.is_section,
                });
            }
        }
    }
    out
}

/// Persisted per-window navigation state: which location we're viewing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentSiteState {
    /// Owning peer (`None` = the window's bound peer).
    pub peer: Option<String>,
    pub site_id: String,
    /// Page slug; empty = the site's root page.
    pub page: String,
}

impl ContentSiteState {
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
                Some("peer") => {
                    if let Some(s) = v.as_text() {
                        if !s.is_empty() {
                            out.peer = Some(s.to_string());
                        }
                    }
                }
                Some("site_id") => {
                    if let Some(s) = v.as_text() {
                        out.site_id = s.to_string();
                    }
                }
                Some("page") => {
                    if let Some(s) = v.as_text() {
                        out.page = s.to_string();
                    }
                }
                _ => {}
            }
        }
        out
    }

    pub fn to_entity(&self) -> Entity {
        let data = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
            "peer" => entity_ecf::text(self.peer.clone().unwrap_or_default()),
            "site_id" => entity_ecf::text(&self.site_id),
            "page" => entity_ecf::text(&self.page)
        });
        Entity::new(STATE_TYPE, data).unwrap()
    }

    fn location(&self) -> Location {
        Location {
            peer_id: self.peer.clone(),
            site_id: self.site_id.clone(),
            page: self.page.clone(),
        }
    }
}

#[derive(Debug, Default)]
struct Inner {
    state: ContentSiteState,
    /// Back-history of visited states (most recent last).
    history: Vec<ContentSiteState>,
    /// The directory rail's view filter (My / All / External). Session-only,
    /// window-surface only (the overlay has no rail) — a presentation choice,
    /// never persisted to the tree.
    rail_filter: RailFilter,
}

/// Long-lived model for one Content Site surface — a window section or
/// the Site Mode overlay. The only thing that differs between the two is
/// where the navigation state persists ([`state_path`](Self::state_path));
/// everything else (resolver, render, navigate) is shared.
pub struct ContentSiteModel {
    peer_id: String,
    /// Tree path where this surface's navigation state persists. Per-
    /// window for a window ([`new`](Self::new)); app-level for the
    /// overlay ([`new_overlay`](Self::new_overlay)).
    state_path: String,
    inner: Arc<Mutex<Inner>>,
    resolver: Box<dyn ContentResolver>,
    /// The site this surface points at by default — the configured
    /// `home_site` ([`session_config`](crate::session_config)), hydrated in
    /// [`initialize`](Self::initialize). Replaces the old hard-coded
    /// `DEMO_SITE_ID` on the boot/default path: which-site is config, not a
    /// constant. (The *current* location the user browsed to persists in the
    /// nav state; this is only the default the state seeds to.)
    default_site_id: String,
    /// The peer the default `home_site` lives on (config `home_site.peer_id`),
    /// hydrated in [`initialize`](Self::initialize). `None` = a local site on
    /// this surface's own peer (the common case — empty config peer). `Some`
    /// threads the peer dimension so a `Site` boot / home target can point at a
    /// site on another peer (`entity://{peer}/sites/{id}`).
    default_site_peer: Option<String>,
    /// Late-bound repaint handle the async (HTTP-poll) resolver fires when
    /// a remote fetch lands. The render path fills it each frame (the
    /// handle isn't available at model construction); the local resolver
    /// never reads it.
    repaint: RepaintCell,
}

impl std::fmt::Debug for ContentSiteModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContentSiteModel")
            .field("peer_id", &self.peer_id)
            .field("state_path", &self.state_path)
            .field("inner", &self.inner)
            .finish()
    }
}

impl ContentSiteModel {
    /// A window-bound model — navigation state persists at the window's
    /// per-window state path.
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        let state_path =
            crate::app_paths::window_state_path(crate::app_paths::APP_ID, &peer_id, window_id);
        Self::with_state_path(peer_id, state_path)
    }

    /// The Site Mode overlay's model — navigation state persists at an
    /// app-level (not per-window) path, because the overlay is a distinct
    /// surface, not a window.
    pub fn new_overlay(peer_id: String) -> Self {
        let state_path = crate::session_config::overlay_location_path(&peer_id);
        Self::with_state_path(peer_id, state_path)
    }

    fn with_state_path(peer_id: String, state_path: String) -> Self {
        let repaint: RepaintCell = Rc::new(RefCell::new(None));
        let resolver = Box::new(MultiResolver::new(peer_id.clone(), repaint.clone()));
        Self {
            peer_id,
            state_path,
            inner: Arc::new(Mutex::new(Inner::default())),
            resolver,
            // Hydrated from config in `initialize`; the demo site is the
            // build-default home, so this is a safe pre-hydration fallback.
            default_site_id: crate::session_config::DEMO_SITE_ID.to_string(),
            default_site_peer: None,
            repaint,
        }
    }

    /// Provide the repaint handle the async resolver fires when a remote
    /// HTTP-poll fetch completes. Called each frame from the render path
    /// (cheap — overwrites the cell); a no-op for purely-local sites.
    pub fn set_repaint(&self, repaint: RepaintFn) {
        *self.repaint.borrow_mut() = Some(repaint);
    }

    /// Ensure this surface's nav state exists and hydrate the in-memory
    /// location from the persisted state / session config.
    ///
    /// **Content seeding is no longer done here (boot-closure reframe).** The
    /// bundled demo is provisioned by the owned, awaited
    /// [`boot_load`](crate::app::EntityApp::boot_load) step — gated on
    /// `home_site` being the local demo — so construction has no synchronous
    /// tree-seed side-effect, and a non-demo / remote home site (a real
    /// content deployment) is never clobbered with the demo. Tests that want
    /// the demo present call `ensure_demo_site` explicitly.
    pub fn initialize(&mut self, peers: &Peers) {
        // Hydrate the default site from the session config's `home_site`
        // (which-site is config, not a constant). Defaults to the demo site
        // when config is absent (the `Full` build default).
        let home = crate::session_config::read(peers, &self.peer_id).home_site;
        self.default_site_id = home.id;
        // Empty config peer = local to this surface's own peer (`None`); a
        // non-empty peer threads the cross-peer dimension into the seed state.
        self.default_site_peer = Some(home.peer_id).filter(|p| !p.is_empty());
        self.ensure_state_in_tree(peers);
        let state = self.read_state(peers);
        self.inner.lock().unwrap().state = state;
    }

    fn state_path(&self) -> String {
        self.state_path.clone()
    }

    fn ensure_state_in_tree(&self, peers: &Peers) {
        let path = self.state_path();
        if peers.get_entity(&self.peer_id, &path).is_none() {
            let default = ContentSiteState {
                peer: self.default_site_peer.clone(),
                site_id: self.default_site_id.clone(),
                page: String::new(),
            };
            peers.dispatch_write(&self.peer_id, path, default.to_entity());
        }
    }

    fn read_state(&self, peers: &Peers) -> ContentSiteState {
        peers
            .get_entity(&self.peer_id, &self.state_path())
            .map(|e| ContentSiteState::from_entity(&e))
            .unwrap_or_else(|| ContentSiteState {
                peer: self.default_site_peer.clone(),
                site_id: self.default_site_id.clone(),
                page: String::new(),
            })
    }

    fn persist_state(&self, peers: &Peers) {
        let entity = self.inner.lock().unwrap().state.to_entity();
        peers.dispatch_write(&self.peer_id, self.state_path(), entity);
    }

    fn current_location(&self) -> Location {
        self.inner.lock().unwrap().state.location()
    }

    // -- Actions --

    /// Navigate to a raw link target (classified + resolved). External
    /// links never reach here. Persists the new location, which fires
    /// the window-state watch → re-render.
    pub fn navigate(&self, target: &str, peers: &Peers) {
        let current = self.current_location();
        let classified = classify_link(target, &current);
        let Some(loc) = resolve_target(&classified, &current) else {
            return; // external / unresolvable — renderer owns those
        };
        self.go_to(loc, peers);
    }

    /// Move to a concrete [`Location`], pushing the prior state onto the
    /// back-history and persisting (which fires the window-state watch →
    /// re-render). The shared core of [`navigate`](Self::navigate) and
    /// [`open_site`](Self::open_site).
    fn go_to(&self, loc: Location, peers: &Peers) {
        {
            let mut inner = self.inner.lock().unwrap();
            let prev = inner.state.clone();
            inner.state = ContentSiteState {
                peer: loc.peer_id.clone(),
                site_id: loc.site_id.clone(),
                page: loc.page.clone(),
            };
            inner.history.push(prev);
        }
        self.persist_state(peers);
    }

    /// Open a site from the directory rail at its root page. `peer` empty =
    /// an owned site on this window's bound peer (`Location.peer_id = None`);
    /// a non-empty `peer` = a cached foreign site (the §2 selector/path split
    /// resolves it from my store). Bumps the site's `visit_count` (a recency
    /// signal the directory surfaces) — the one place we count a visit, so it
    /// counts explicit opens, not per-frame renders.
    pub fn open_site(&self, peer: &str, site: &str, peers: &Peers) {
        let peer_id = Some(peer.to_string()).filter(|p| !p.is_empty());
        // The provenance/prefs ledger keys by the concrete owning peer; an
        // owned site keys by my own id (`peer` empty → my bound peer).
        let key_peer = peer_id.clone().unwrap_or_else(|| self.peer_id.clone());
        crate::content_site::prefs::update_prefs(
            peers,
            &self.peer_id,
            &key_peer,
            site,
            |p| p.visit_count = p.visit_count.saturating_add(1),
        );
        self.go_to(
            Location { peer_id, site_id: site.to_string(), page: String::new() },
            peers,
        );
    }

    /// Toggle the bookmark flag for a site (owned or cached). Read-modify-write
    /// of the app-tier preference record under MY namespace; the prefs-prefix
    /// subscription re-renders the directory.
    pub fn toggle_bookmark(&self, peer: &str, site: &str, peers: &Peers) {
        self.toggle_pref(peer, site, peers, |p| p.bookmarked = !p.bookmarked);
    }

    /// Toggle "keep offline" for a cached site (O3): on = full page-body
    /// caching, off = manifest-pinned. Takes effect for pages fetched after the
    /// toggle (the resolver reads the pref at fetch time) — a full back-fill of
    /// already-visited pages happens on the next reload / re-browse.
    pub fn toggle_keep_offline(&self, peer: &str, site: &str, peers: &Peers) {
        self.toggle_pref(peer, site, peers, |p| p.keep_offline = !p.keep_offline);
    }

    fn toggle_pref(
        &self,
        peer: &str,
        site: &str,
        peers: &Peers,
        mutate: impl FnOnce(&mut crate::content_site::prefs::SitePrefs),
    ) {
        let key_peer = if peer.is_empty() { self.peer_id.clone() } else { peer.to_string() };
        crate::content_site::prefs::update_prefs(peers, &self.peer_id, &key_peer, site, mutate);
    }

    /// Assemble the directory rail: every site my store holds (the derived
    /// index), enriched with provenance (cached sites) + preferences, with the
    /// current location flagged. Bookmarked sites sort first, then owned before
    /// cached, each group alphabetical — a stable, scannable order. Reads are
    /// all sync L0 against MY store; on the Worker arm the window must observe
    /// the index / provenance / prefs prefixes (the factory subscribes them).
    pub fn site_directory(&self, peers: &Peers) -> SiteDirectory {
        use crate::content_site::{cache, discovery, prefs};
        let me = &self.peer_id;
        let cur = self.current_location();
        let cur_peer = cur.peer_id.clone().unwrap_or_else(|| me.clone());

        // Union the async, query-materialized index with a SYNC direct scan of
        // sites physically in my store. The index lags/fails; a site whose
        // manifest is present is browsable now (the same direct read the browse
        // area resolves), so the rail must show it — never "No sites yet" while
        // a site renders (BUG-3, the divergent-truths bug). Dedup by (peer,
        // site); the index still contributes any query-only refs.
        let mut seen = std::collections::HashSet::new();
        let refs: Vec<discovery::SiteRef> = discovery::read_site_index(peers, me)
            .into_iter()
            .chain(discovery::scan_local_sites(peers, me))
            .filter(|r| seen.insert((r.peer.clone(), r.site.clone())))
            .collect();
        let mut entries: Vec<SiteEntry> = refs
            .into_iter()
            .map(|r| {
                let prefs = prefs::read_prefs(peers, me, &r.peer, &r.site);
                // Provenance only exists for cached foreign content; owned sites
                // were authored locally, never fetched + recorded.
                let prov = if r.owned {
                    None
                } else {
                    cache::read_provenance(peers, me, &r.peer, &r.site)
                };
                SiteEntry {
                    is_current: r.site == cur.site_id && r.peer == cur_peer,
                    bookmarked: prefs.bookmarked,
                    keep_offline: prefs.keep_offline,
                    visit_count: prefs.visit_count,
                    last_reconciled: prov.as_ref().map(|p| p.last_reconciled).unwrap_or(0),
                    source_transport: prov.map(|p| p.source_transport).unwrap_or_default(),
                    peer: r.peer,
                    site: r.site,
                    owned: r.owned,
                }
            })
            .collect();
        // Bookmarked first, then owned before cached, then alphabetical by site.
        entries.sort_by(|a, b| {
            b.bookmarked
                .cmp(&a.bookmarked)
                .then(b.owned.cmp(&a.owned))
                .then(a.site.cmp(&b.site))
                .then(a.peer.cmp(&b.peer))
        });
        // Apply the session view filter last (All keeps every entry — the
        // historical behaviour).
        let filter = self.inner.lock().unwrap().rail_filter;
        entries.retain(|e| filter.keeps(e.owned));
        SiteDirectory { entries, filter }
    }

    /// Set the directory rail's view filter (My / All / External). In-memory,
    /// session-only — the caller marks the window dirty to rebuild (there's no
    /// tree write to drive a subscription).
    pub fn set_rail_filter(&self, filter: RailFilter) {
        self.inner.lock().unwrap().rail_filter = filter;
    }

    /// Go back to the previous location (pop the back-history). No-op at
    /// the start of history. The pop is *not* re-pushed, so this is a true
    /// "back". Persisting fires the window-state watch → re-render. History
    /// is in-memory (session-scoped) and does not survive a reload.
    pub fn back(&self, peers: &Peers) {
        let went_back = {
            let mut inner = self.inner.lock().unwrap();
            match inner.history.pop() {
                Some(prev) => {
                    inner.state = prev;
                    true
                }
                None => false,
            }
        };
        if went_back {
            self.persist_state(peers);
        }
    }

    // -- Pure read --

    pub fn render_output(&self, peers: &Peers) -> SiteRenderOutput {
        let (loc, can_go_back) = {
            let inner = self.inner.lock().unwrap();
            (inner.state.location(), !inner.history.is_empty())
        };
        match self.resolver.resolve_page(peers, &loc) {
            ResolveOutcome::Ready(Ok(rp)) => {
                // The section sidebar is the one peer-dependent enrichment
                // (`.list` read from the live tree); everything else is built
                // purely from the resolved closure by `output_from_resolved`,
                // which the pre-peer fast-paint boot path (cut 2c) reuses.
                let sidebar =
                    build_sidebar(&*self.resolver, peers, &rp.location, &rp.location.page);
                output_from_resolved(&rp, &loc, can_go_back, sidebar)
            }
            ResolveOutcome::Ready(Err(e)) => {
                // O3 manifest-pinned shell: a cached foreign site whose page is
                // ephemeral + whose origin is now unreachable still renders its
                // chrome (title + nav) from the durable manifest, with a notice
                // — instead of a bare error. A local page-miss is a genuine
                // not-found (no shell).
                self.shell_output(peers, &loc, e, can_go_back)
                    .unwrap_or_else(|| SiteRenderOutput { can_go_back, ..self.error_output(&loc, e) })
            }
            ResolveOutcome::Pending => {
                // While the live page is still resolving, show the cached
                // OUTLINE if we hold a durable manifest for this foreign site
                // (BUG-2): the user sees the real site chrome (nav / sidebar)
                // plus a "loading…" note in the content pane, instead of a
                // full-pane blank spinner that — on an origin-less cached site —
                // would otherwise never complete. The resolver's grace driver
                // bounds the underlying resolve to Unreachable→shell; this just
                // makes the wait non-blank and navigable. No cached manifest
                // ⇒ the bare loading state (first-ever visit, still fetching).
                self.shell_from_manifest(peers, &loc, can_go_back, "_Loading the live page…_")
                    .unwrap_or(SiteRenderOutput {
                        site_id: loc.site_id.clone(),
                        current_page: loc.page.clone(),
                        peer: loc.peer_id.clone(),
                        can_go_back,
                        loading: true,
                        ..Default::default()
                    })
            }
        }
    }

    fn error_output(&self, loc: &Location, err: ResolveError) -> SiteRenderOutput {
        let msg = match err {
            ResolveError::ManifestMissing => {
                format!("No site manifest at '{}' (peer: {}).", loc.site_id, loc.peer_id.clone().unwrap_or_else(|| self.peer_id.clone()))
            }
            ResolveError::PageMissing => {
                let page = if loc.page.is_empty() { "<root>" } else { &loc.page };
                format!("Page '{}' not found in site '{}'.", page, loc.site_id)
            }
            ResolveError::Unreachable => {
                format!(
                    "Couldn't reach peer '{}' — no route is registered for it.",
                    loc.peer_id.clone().unwrap_or_else(|| self.peer_id.clone())
                )
            }
        };
        SiteRenderOutput {
            site_id: loc.site_id.clone(),
            current_page: loc.page.clone(),
            peer: loc.peer_id.clone(),
            error: Some(msg),
            ..Default::default()
        }
    }

    /// Build the O3 manifest-pinned **shell** for a cached foreign site whose
    /// page couldn't resolve (ephemeral page + unreachable origin). Returns
    /// `None` when there's no shell to show — a local site (a page-miss there is
    /// a genuine not-found), or no durable manifest. Reuses
    /// [`output_from_resolved`] with a synthetic notice page so the chrome (nav
    /// / breadcrumbs / title) is identical to a live render.
    fn shell_output(
        &self,
        peers: &Peers,
        loc: &Location,
        err: ResolveError,
        can_go_back: bool,
    ) -> Option<SiteRenderOutput> {
        let notice = match err {
            // We hold the page's site but not its body, and the origin answered:
            // a genuinely-missing page (not offline).
            ResolveError::PageMissing => {
                "_This page isn't kept for offline viewing — reconnect to load it._"
            }
            // The live source is unreachable (origin down / 404'd the fetch),
            // but we have the cached outline.
            ResolveError::Unreachable | ResolveError::ManifestMissing => {
                "_This site's source is unreachable. Showing its cached outline._"
            }
        };
        self.shell_from_manifest(peers, loc, can_go_back, notice)
    }

    /// Build the manifest-pinned **shell** — the cached site chrome (nav /
    /// breadcrumbs / title) with a synthetic `notice` page body — for a cached
    /// FOREIGN site, or `None` when there's nothing to shell (a local site,
    /// where a page-miss is a genuine not-found, or no durable manifest). The
    /// shell is gated on whether we HOLD the manifest, NOT on the live error
    /// type: a manifest-pinned site whose page is ephemeral re-fetches over
    /// HTTP, and an unreachable origin 404s the *manifest fetch* too — so the
    /// live error is `ManifestMissing` even though we have a durable cached
    /// copy. Reuses [`output_from_resolved`] so the chrome is identical to a
    /// live render. Shared by the offline/error shell ([`shell_output`]) and
    /// the still-loading shell (the Pending branch, BUG-2) so a visited foreign
    /// site always shows its outline instead of a bare spinner.
    fn shell_from_manifest(
        &self,
        peers: &Peers,
        loc: &Location,
        can_go_back: bool,
        notice: &str,
    ) -> Option<SiteRenderOutput> {
        let _foreign = loc.peer_id.as_deref().filter(|p| *p != self.peer_id.as_str())?;
        let manifest = self.resolver.manifest_only(peers, loc)?;
        let page_slug = if loc.page.is_empty() {
            manifest.root().to_string()
        } else {
            loc.page.clone()
        };
        let title = if loc.page.is_empty() {
            manifest.title.clone()
        } else {
            humanize(page_slug.rsplit('/').next().unwrap_or(&page_slug))
        };
        let rp = ResolvedPage {
            location: Location {
                peer_id: loc.peer_id.clone(),
                site_id: loc.site_id.clone(),
                page: page_slug,
            },
            manifest,
            page: SitePage::markdown(&title, notice),
            assets: Vec::new(),
        };
        // No sidebar — offline, the `.list` isn't available.
        Some(output_from_resolved(&rp, loc, can_go_back, Vec::new()))
    }

    #[cfg(test)]
    pub fn state_snapshot(&self) -> ContentSiteState {
        self.inner.lock().unwrap().state.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::content_site::DEMO_SITE_ID;

    fn pm() -> Peers {
        Peers::new_direct()
    }

    fn model(peers: &Peers) -> ContentSiteModel {
        let pid = peers.primary_peer_id().to_string();
        // Seeding is now owned by `boot_load` in production; seed explicitly
        // here so these unit tests have the demo content to render.
        crate::views::content_site::ensure_demo_site(peers, &pid);
        let mut m = ContentSiteModel::new(1, pid);
        m.initialize(peers);
        m
    }

    #[test]
    fn state_round_trips_through_entity() {
        let s = ContentSiteState { peer: Some("P".into()), site_id: "demo".into(), page: "about".into() };
        assert_eq!(ContentSiteState::from_entity(&s.to_entity()), s);
    }

    #[test]
    fn state_empty_peer_round_trips_as_none() {
        let s = ContentSiteState { peer: None, site_id: "demo".into(), page: String::new() };
        assert_eq!(ContentSiteState::from_entity(&s.to_entity()), s);
    }

    #[test]
    fn initialize_seeds_demo_site_and_renders_root() {
        let peers = pm();
        let m = model(&peers);
        let out = m.render_output(&peers);
        assert!(out.error.is_none(), "root should resolve: {:?}", out.error);
        assert_eq!(out.current_page, "index");
        assert!(!out.site_title.is_empty());
        assert!(!out.nav.is_empty());
        assert!(out.body_html.contains("<h1>"), "rendered markdown: {}", out.body_html);
    }

    #[test]
    fn navigate_in_site_changes_page() {
        let peers = pm();
        let m = model(&peers);
        m.navigate("./about", &peers);
        assert_eq!(m.state_snapshot().page, "about");
        let out = m.render_output(&peers);
        assert_eq!(out.current_page, "about");
        assert!(out.error.is_none());
    }

    #[test]
    fn nav_active_flag_tracks_current_page() {
        let peers = pm();
        let m = model(&peers);
        m.navigate("./about", &peers);
        let out = m.render_output(&peers);
        let about = out.nav.iter().find(|n| n.target.contains("about")).expect("about nav item");
        assert!(about.active, "About should be active on the about page");
    }

    #[test]
    fn navigate_external_is_ignored() {
        let peers = pm();
        let m = model(&peers);
        let before = m.state_snapshot();
        m.navigate("https://example.com", &peers);
        assert_eq!(m.state_snapshot(), before, "external nav must not change location");
    }

    #[test]
    fn missing_page_yields_error_output() {
        let peers = pm();
        let m = model(&peers);
        m.navigate("./ghost", &peers);
        let out = m.render_output(&peers);
        assert!(out.error.is_some());
    }

    // ===================================================================
    // DEEP-SITE DISCOVERY PROBE
    //
    // Seeds a genuinely nested docs-style site (2- and 3-level page
    // paths, deep→deep links, nav pointing into subsections) and drives
    // the model against it. The PURPOSE is to find what the nav model +
    // format actually need at depth before we freeze §3. Each assertion
    // is labelled WORKS (confirmed support) or GAP (asserts the *current*
    // broken/absent behavior so the finding is explicit + regression-
    // pinned). Run with: cargo test --bins probe_deep_site -- --nocapture
    // ===================================================================

    /// Seed a deep `docs` site into the primary peer's tree (Direct L0).
    fn seed_deep_docs_site(peers: &Peers) -> String {
        use crate::content_site::paths;
        use crate::content_site::{NavItem, SiteManifest, SitePage};
        let pid = peers.primary_peer_id().to_string();
        let ctx = peers.test_seed_ctx(&pid);

        // Nav points INTO subsections — the realistic case.
        let manifest = SiteManifest::new(
            "docs",
            "Entity Docs",
            "index",
            vec![
                NavItem::new("Home", "/index"),
                NavItem::new("Guide", "/guide/intro"),
                NavItem::new("Reference", "/reference/api"),
            ],
        );
        ctx.store().put(&paths::manifest_path(&pid, "docs"), manifest.to_entity()).ok();

        let pages: &[(&str, &str, &str)] = &[
            ("index", "Home", "# Docs\n\nStart with the [Guide](./guide/intro)."),
            ("guide/intro", "Guide: Intro",
                "# Intro\n\nNext: [Install](install), or jump to [Caching](advanced/caching)."),
            ("guide/install", "Guide: Install", "# Install\n\nBack to [Intro](intro)."),
            ("guide/advanced/caching", "Guide: Caching (deep)",
                "# Caching\n\nThree levels deep. Back to [Intro](../intro)."),
            ("reference/api", "Reference: API", "# API\n\nThe reference section."),
        ];
        for (slug, title, body) in pages {
            ctx.store()
                .put(
                    &paths::page_path(&pid, "docs", slug),
                    SitePage::markdown(*title, *body).to_entity(),
                )
                .ok();
        }
        pid
    }

    /// Find a nav link by label in the rendered output.
    fn nav_active(out: &SiteRenderOutput, label: &str) -> bool {
        out.nav.iter().find(|n| n.label == label).map(|n| n.active).unwrap_or(false)
    }

    #[test]
    fn output_from_resolved_builds_peer_free_output() {
        // The fast-paint contract: a render output assembled from a resolved
        // closure alone, no peer / no sidebar — identical shape to what the
        // live overlay produces for the same page.
        use crate::content_site::resolver::ResolvedPage;
        use crate::content_site::{Location, NavItem, SiteManifest, SitePage};
        let manifest = SiteManifest::new(
            "labs",
            "Bill's Labs",
            "index",
            vec![NavItem::new("Home", "/index"), NavItem::new("Guide", "/guide/intro")],
        );
        let rp = ResolvedPage {
            location: Location {
                peer_id: Some("labs-host".into()),
                site_id: "labs".into(),
                page: "index".into(),
            },
            manifest,
            page: SitePage::markdown("Home", "# Welcome\n\nHello from the labs."),
            assets: Vec::new(),
        };
        let out = output_from_resolved(&rp, &rp.location, false, vec![]);
        assert_eq!(out.site_title, "Bill's Labs");
        assert_eq!(out.site_id, "labs");
        assert_eq!(out.current_page, "index");
        assert_eq!(out.peer.as_deref(), Some("labs-host"));
        assert_eq!(out.nav.len(), 2);
        assert!(nav_active(&out, "Home"), "Home nav active on the root page");
        assert!(!nav_active(&out, "Guide"));
        assert!(out.sidebar.is_empty(), "no sidebar without a peer");
        assert!(out.breadcrumbs.is_empty(), "root page has no trail");
        assert!(out.error.is_none() && !out.loading);
        assert!(out.body_html.contains("Welcome"), "body rendered: {}", out.body_html);
    }

    #[test]
    fn in_section_matches_section_trail_not_just_exact() {
        // exact match
        assert!(in_section("about", "about"));
        assert!(in_section("guide/intro", "guide/intro"));
        // descendant within the same top-level section (the trail)
        assert!(in_section("guide/intro", "guide/install"));
        assert!(in_section("guide/intro", "guide/advanced/caching"));
        // different sections never match
        assert!(!in_section("index", "guide/intro"));
        assert!(!in_section("guide/intro", "reference/api"));
        assert!(!in_section("about", "theory"));
        // empty section only matches exactly (root link)
        assert!(in_section("", ""));
        assert!(!in_section("", "guide/intro"));
    }

    #[test]
    fn probe_deep_site_resolution_and_nav() {
        let peers = pm();
        let m = model(&peers); // seeds + sits on demo/index
        seed_deep_docs_site(&peers);

        // -- WORKS: cross-site jump into the deep docs site --
        m.navigate("site:docs/index", &peers);
        let out = m.render_output(&peers);
        assert_eq!(out.site_id, "docs", "WORKS: cross-site nav switches site");
        assert_eq!(out.current_page, "index");
        assert!(out.error.is_none(), "WORKS: docs root resolves: {:?}", out.error);
        assert_eq!(out.site_title, "Entity Docs");
        assert!(nav_active(&out, "Home"), "WORKS: Home nav active on index");

        // -- WORKS: 2-level-deep page resolves + renders --
        m.navigate("./guide/intro", &peers);
        let out = m.render_output(&peers);
        assert_eq!(out.current_page, "guide/intro", "WORKS: 2-level slug resolves");
        assert!(out.error.is_none(), "WORKS: deep page resolves: {:?}", out.error);
        assert!(out.body_html.contains("Intro"), "WORKS: deep page body renders");
        assert!(nav_active(&out, "Guide"), "WORKS: Guide nav active on exact deep target");

        // -- WORKS: 3-level-deep page resolves (deep→deep link) --
        // Click the body link from guide/intro (dir-relative `advanced/caching`).
        m.navigate("advanced/caching", &peers);
        let out = m.render_output(&peers);
        assert_eq!(out.current_page, "guide/advanced/caching", "WORKS: 3-level slug resolves");
        assert!(out.error.is_none(), "WORKS: 3-deep page resolves: {:?}", out.error);
        assert!(out.body_html.contains("Caching"));

        // -- GAP1 active-trail: FIXED. On a child of the
        // Guide section, the "Guide" nav item (./guide/intro) stays
        // highlighted because `in_section` matches the top-level section,
        // not just the exact page. Home/Reference stay dark.
        assert!(
            nav_active(&out, "Guide"),
            "GAP1 FIXED: Guide nav active on a child page (active-trail)"
        );
        assert!(!nav_active(&out, "Home"), "GAP1: Home stays dark in the Guide section");
        assert!(!nav_active(&out, "Reference"), "GAP1: Reference stays dark in the Guide section");

        // -- GAP 2 (breadcrumbs): the output carries no trail/ancestor
        // info. The renderer can only show a flat current_page slug; there
        // is no breadcrumb structure to render "Docs / Guide / Caching".
        assert_eq!(
            out.current_page, "guide/advanced/caching",
            "GAP2 breadcrumbs: only a flat slug is exposed, no ancestor trail"
        );
        eprintln!("FINDING GAP2: breadcrumbs — output exposes only the flat slug, no ancestor trail");

        // -- GAP 3 (sub-nav): manifest.nav is a flat Vec<NavItem>; there
        // is no way to express the Guide section's children (Intro/
        // Install/Caching) as a sub-menu. Confirm the nav is flat.
        assert!(
            out.nav.iter().all(|_| true) && out.nav.len() == 3,
            "GAP3 sub-nav: nav is a flat list of top-level items only"
        );
        eprintln!("FINDING GAP3: sub-nav — nav is flat (3 top-level items); no section children");
    }

    #[test]
    fn probe_deep_site_back_history() {
        let peers = pm();
        let m = model(&peers);
        seed_deep_docs_site(&peers);

        m.navigate("site:docs/index", &peers);
        m.navigate("./guide/intro", &peers);
        m.navigate("install", &peers); // dir-relative click from guide/intro → guide/install

        // History IS accumulating (model.rs ~214 pushes prev on navigate).
        {
            let inner = m.inner.lock().unwrap();
            assert_eq!(inner.history.len(), 3, "history accumulates: demo/index, docs/index, guide/intro");
        }

        // -- GAP4 (back) FIXED: back() pops the history and returns to the
        // previous location. Walk all the way back to the start.
        m.back(&peers);
        assert_eq!(m.state_snapshot().page, "guide/intro", "back → previous page");
        m.back(&peers);
        assert_eq!(m.state_snapshot().page, "index", "back again → docs index");
        m.back(&peers);
        // Back to where we started: the demo site's root page (empty slug).
        assert_eq!(m.state_snapshot().page, "", "back to the demo start (root)");
        assert_eq!(m.state_snapshot().site_id, super::super::DEMO_SITE_ID);
        // History exhausted → back() is a no-op (stays put).
        let before = m.state_snapshot();
        m.back(&peers);
        assert_eq!(m.state_snapshot(), before, "back at history start is a no-op");
    }

    #[test]
    fn back_is_reported_in_output_and_returns() {
        let peers = pm();
        let m = model(&peers);
        // Root: no history yet.
        assert!(!m.render_output(&peers).can_go_back);
        m.navigate("./about", &peers);
        let out = m.render_output(&peers);
        assert!(out.can_go_back, "after a navigate, back is available");
        m.back(&peers);
        assert!(!m.render_output(&peers).can_go_back, "back consumed the only history entry");
        assert_eq!(m.state_snapshot().page, "", "returned to the root location");
    }

    #[test]
    fn pending_foreign_page_renders_cached_shell_not_blank_spinner() {
        // BUG-2: a cached foreign site whose page is still resolving (origin
        // registered → HTTP arm Pending on native; no fetch executor) must
        // render the CACHED OUTLINE (title + nav) instead of a full-pane blank
        // "Loading…" that, on an origin-less reload, would otherwise never
        // complete. Pairs with the resolver's grace-driver (the wasm side that
        // bounds the underlying resolve to Unreachable→shell).
        use crate::content_site::paths;
        use crate::content_site::{NavItem, SiteManifest};

        let peers = pm();
        let me = peers.primary_peer_id().to_string();
        // A real foreign peer-id (tree paths validate the peer-segment).
        let foreign = Peers::new_direct().primary_peer_id().to_string();

        // Seed ONLY the foreign MANIFEST into my store (manifest-pinned default
        // — the page body stays ephemeral) and register the origin so resolve
        // routes to the HTTP arm → Pending on native (no fetch lands).
        let ctx = peers.test_seed_ctx(&me);
        ctx.store()
            .put(
                &paths::manifest_path(&foreign, "labs"),
                SiteManifest::new(
                    "labs",
                    "Bob's Labs",
                    "index",
                    vec![NavItem::new("Home", "/index"), NavItem::new("Notes", "/notes")],
                )
                .to_entity(),
            )
            .ok();
        crate::content_site::origins::set_origin(&peers, &me, &foreign, "http://labs.example");

        let mut m = ContentSiteModel::new_overlay(me.clone());
        m.initialize(&peers);
        m.open_site(&foreign, "labs", &peers);

        let out = m.render_output(&peers);
        assert!(!out.loading, "a cached manifest renders the outline, not a blank spinner");
        assert!(out.error.is_none(), "Pending is not an error");
        assert_eq!(out.site_title, "Bob's Labs", "the cached site title shows while loading");
        assert_eq!(out.nav.len(), 2, "the cached nav shows so the site stays navigable");
        assert!(
            out.body_html.to_lowercase().contains("loading"),
            "the content pane carries the loading note, got: {}",
            out.body_html
        );
    }

    #[test]
    fn breadcrumbs_trail_deep_page() {
        let peers = pm();
        let m = model(&peers);
        seed_deep_docs_site(&peers);

        // Root page → no breadcrumbs.
        m.navigate("site:docs/index", &peers);
        assert!(m.render_output(&peers).breadcrumbs.is_empty(), "root page has no trail");

        // Three levels deep → root crumb (clickable) + segment labels + page title.
        m.navigate("./guide/advanced/caching", &peers);
        let out = m.render_output(&peers);
        let labels: Vec<&str> = out.breadcrumbs.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["Entity Docs", "Guide", "Advanced", "Guide: Caching (deep)"]);
        // Root + intermediate-section crumbs are clickable (→ section path);
        // only the current page is a plain label.
        assert_eq!(out.breadcrumbs[0].target.as_deref(), Some("/"));
        assert_eq!(out.breadcrumbs[1].target.as_deref(), Some("/guide"));
        assert_eq!(out.breadcrumbs[2].target.as_deref(), Some("/guide/advanced"));
        assert_eq!(out.breadcrumbs[3].target, None);
    }

    #[test]
    fn breadcrumbs_collapse_trailing_index() {
        // A `section/index` page IS that section's landing page — the trail
        // must read `Site › Section` (current), not `Site › Section › Section`
        // (the dir crumb, then its like-named index). billslab research/index.
        let c = breadcrumbs("Bill's Lab", "index", "research/index", "Research");
        let labels: Vec<&str> = c.iter().map(|x| x.label.as_str()).collect();
        assert_eq!(labels, vec!["Bill's Lab", "Research"]);
        assert_eq!(c[0].target.as_deref(), Some("/"));
        assert_eq!(c[1].target, None, "the section index IS the current page (no self-link)");

        // A deep index collapses only the trailing segment; ancestors still link.
        let deep = breadcrumbs("Bill's Lab", "index", "research/model/index", "Model Data");
        let dl: Vec<&str> = deep.iter().map(|x| x.label.as_str()).collect();
        assert_eq!(dl, vec!["Bill's Lab", "Research", "Model Data"]);
        assert_eq!(deep[1].target.as_deref(), Some("/research"));
        assert_eq!(deep[2].target, None);

        // A non-index leaf is unchanged; the site root still has no trail.
        assert_eq!(
            breadcrumbs("S", "index", "research/grounding", "Grounding")
                .iter()
                .map(|x| x.label.clone())
                .collect::<Vec<_>>(),
            vec!["S", "Research", "Grounding"]
        );
        assert!(breadcrumbs("S", "index", "index", "Home").is_empty());
    }

    #[test]
    fn sidebar_lists_tree_and_expands_active_section() {
        let peers = pm();
        let m = model(&peers);
        seed_deep_docs_site(&peers);
        m.navigate("site:docs/guide/intro", &peers);
        let out = m.render_output(&peers);

        // Top-level entries come from the live tree (`.list`), NOT the flat
        // manifest nav — so the sidebar shows even for a flat-nav site.
        let labels: Vec<&str> = out.sidebar.iter().map(|s| s.label.as_str()).collect();
        assert!(labels.contains(&"Guide"), "sidebar shows tree sections: {labels:?}");
        assert!(labels.contains(&"Reference"));

        // The active Guide section is expanded one level to its child pages.
        let guide = out.sidebar.iter().find(|s| s.label == "Guide").unwrap();
        assert!(guide.active && guide.depth == 0 && guide.is_section);
        let intro = out.sidebar.iter().find(|s| s.label == "Intro").expect("Guide expands → Intro");
        assert_eq!(intro.depth, 1);
        assert!(intro.active, "Intro is the current page");

        // Reference is NOT the active section, so it is not expanded.
        assert!(!out.sidebar.iter().any(|s| s.label == "Api"));
    }

    // ===================================================================
    // SITE-AWARE WINDOW DIRECTORY (P3)
    // ===================================================================

    /// Seed a cached foreign site (manifest + root page at its natural path in
    /// MY store, the P1 write-through shape) + its provenance ledger, and return
    /// the foreign peer-id. Mirrors what `persist_to_cache` lands.
    fn seed_cached_foreign_site(peers: &Peers, me: &str) -> String {
        use crate::content_site::{cache, paths, SiteManifest, SitePage};
        let foreign = Peers::new_direct().primary_peer_id().to_string();
        peers.seed_write(
            me,
            paths::manifest_path(&foreign, "labs"),
            SiteManifest::new("labs", "Bill's Labs", "index", vec![]).to_entity(),
        );
        peers.seed_write(
            me,
            paths::page_path(&foreign, "labs", "index"),
            SitePage::markdown("Home", "# Labs\n\nHello.").to_entity(),
        );
        cache::write_provenance(
            peers,
            me,
            &foreign,
            "labs",
            &cache::CacheProvenance {
                last_reconciled: 1_700_000_000_123,
                pinned_root_hash: "h".into(),
                source_transport: "http://labs.example/x".into(),
            },
        );
        foreign
    }

    /// Materialize the derived site index for a set of refs (the rail reads the
    /// index, not the live async query — the refresh→index→subscription bridge).
    fn seed_index(peers: &Peers, me: &str, refs: &[crate::content_site::discovery::SiteRef]) {
        peers.seed_write(
            me,
            crate::app_paths::site_index_path(crate::app_paths::APP_ID, me),
            crate::content_site::discovery::site_index_entity(refs),
        );
    }

    #[test]
    fn site_directory_assembles_owned_and_cached_rows() {
        use crate::content_site::discovery::SiteRef;
        let peers = pm();
        let m = model(&peers); // owned demo, current = demo/index
        let me = peers.primary_peer_id().to_string();
        let foreign = seed_cached_foreign_site(&peers, &me);
        // Bookmark the cached site so it sorts to the top (bookmark beats owned).
        m.toggle_bookmark(&foreign, "labs", &peers);
        seed_index(
            &peers,
            &me,
            &[
                SiteRef { peer: me.clone(), site: DEMO_SITE_ID.into(), owned: true },
                SiteRef { peer: foreign.clone(), site: "labs".into(), owned: false },
            ],
        );

        let dir = m.site_directory(&peers);
        assert_eq!(dir.entries.len(), 2);
        // Bookmarked cached site first.
        assert_eq!(dir.entries[0].site, "labs");
        assert!(dir.entries[0].bookmarked && !dir.entries[0].owned);
        assert_eq!(dir.entries[0].last_reconciled, 1_700_000_000_123);
        assert_eq!(dir.entries[0].source_transport, "http://labs.example/x");
        assert!(!dir.entries[0].is_current);
        // The owned demo is current (the model sits on demo/index) + no provenance.
        let demo = dir.entries.iter().find(|e| e.site == DEMO_SITE_ID).unwrap();
        assert!(demo.owned && demo.is_current);
        assert_eq!(demo.last_reconciled, 0, "owned sites carry no provenance");
    }

    #[test]
    fn site_directory_view_filter_narrows_to_owned_or_external() {
        use crate::content_site::discovery::SiteRef;
        let peers = pm();
        let m = model(&peers); // owned demo
        let me = peers.primary_peer_id().to_string();
        let foreign = seed_cached_foreign_site(&peers, &me);
        seed_index(
            &peers,
            &me,
            &[
                SiteRef { peer: me.clone(), site: DEMO_SITE_ID.into(), owned: true },
                SiteRef { peer: foreign.clone(), site: "labs".into(), owned: false },
            ],
        );

        // Default (All): owned + cached together — the historical behaviour.
        let all = m.site_directory(&peers);
        assert_eq!(all.filter, RailFilter::All);
        assert_eq!(all.entries.len(), 2);

        // Mine: only the owned site.
        m.set_rail_filter(RailFilter::Mine);
        let mine = m.site_directory(&peers);
        assert_eq!(mine.filter, RailFilter::Mine, "the active filter is surfaced");
        assert!(mine.entries.iter().all(|e| e.owned), "only owned rows: {:?}", mine.entries);
        assert!(mine.entries.iter().any(|e| e.site == DEMO_SITE_ID));

        // External: only the cached foreign site.
        m.set_rail_filter(RailFilter::External);
        let ext = m.site_directory(&peers);
        assert!(ext.entries.iter().all(|e| !e.owned), "only cached rows: {:?}", ext.entries);
        assert!(ext.entries.iter().any(|e| e.site == "labs"));

        // Wire token round-trips (the action carries the string form).
        assert_eq!(RailFilter::parse(RailFilter::Mine.as_str()), RailFilter::Mine);
        assert_eq!(RailFilter::parse(RailFilter::External.as_str()), RailFilter::External);
        assert_eq!(RailFilter::parse("bogus"), RailFilter::All);
    }

    #[test]
    fn open_cached_site_switches_location_counts_visit_and_resolves() {
        use crate::content_site::prefs;
        let peers = pm();
        let m = model(&peers);
        let me = peers.primary_peer_id().to_string();
        let foreign = seed_cached_foreign_site(&peers, &me);

        m.open_site(&foreign, "labs", &peers);
        let st = m.state_snapshot();
        assert_eq!(st.site_id, "labs");
        assert_eq!(st.peer.as_deref(), Some(foreign.as_str()), "cached open threads the peer");
        assert_eq!(st.page, "", "opens at the site root");
        assert_eq!(prefs::read_prefs(&peers, &me, &foreign, "labs").visit_count, 1);
        // It resolves from MY store (the §2 cache hit) — no network needed.
        let out = m.render_output(&peers);
        assert!(out.error.is_none(), "cached site opens + renders: {:?}", out.error);
        assert_eq!(out.site_title, "Bill's Labs");
    }

    #[test]
    fn open_owned_site_clears_peer_dimension() {
        use crate::content_site::prefs;
        let peers = pm();
        let m = model(&peers);
        let me = peers.primary_peer_id().to_string();
        m.navigate("./about", &peers); // move off the root first
        m.open_site("", DEMO_SITE_ID, &peers);
        let st = m.state_snapshot();
        assert_eq!(st.peer, None, "an owned open clears the peer dimension (bound peer)");
        assert_eq!(st.site_id, DEMO_SITE_ID);
        assert_eq!(st.page, "");
        // An owned open counts under MY id.
        assert_eq!(prefs::read_prefs(&peers, &me, &me, DEMO_SITE_ID).visit_count, 1);
    }

    #[test]
    fn toggle_bookmark_flips_and_persists() {
        use crate::content_site::prefs;
        let peers = pm();
        let m = model(&peers);
        let me = peers.primary_peer_id().to_string();
        assert!(!prefs::read_prefs(&peers, &me, &me, DEMO_SITE_ID).bookmarked);
        m.toggle_bookmark("", DEMO_SITE_ID, &peers);
        assert!(prefs::read_prefs(&peers, &me, &me, DEMO_SITE_ID).bookmarked);
        m.toggle_bookmark("", DEMO_SITE_ID, &peers);
        assert!(!prefs::read_prefs(&peers, &me, &me, DEMO_SITE_ID).bookmarked, "second toggle clears");
    }

    #[test]
    fn shell_output_renders_chrome_for_offline_cached_page() {
        use crate::content_site::{paths, SiteManifest};
        let peers = pm();
        let m = model(&peers);
        let me = peers.primary_peer_id().to_string();
        let foreign = Peers::new_direct().primary_peer_id().to_string();
        // Manifest-pinned: the manifest is durably cached, the page is not.
        peers.seed_write(
            &me,
            paths::manifest_path(&foreign, "labs"),
            SiteManifest::new(
                "labs",
                "Bill's Labs",
                "index",
                vec![NavItem::new("Home", "/index"), NavItem::new("Guide", "/guide/intro")],
            )
            .to_entity(),
        );
        let loc = Location { peer_id: Some(foreign.clone()), site_id: "labs".into(), page: String::new() };

        // A cached foreign site with an unresolvable page → the SHELL: chrome
        // from the durable manifest, NOT a bare error.
        let shell = m
            .shell_output(&peers, &loc, ResolveError::PageMissing, false)
            .expect("a cached foreign site yields a manifest shell");
        assert!(shell.error.is_none(), "the shell is chrome, not an error");
        assert_eq!(shell.site_title, "Bill's Labs");
        assert_eq!(shell.nav.len(), 2, "nav comes from the durable manifest");
        assert!(
            shell.body_html.to_lowercase().contains("reconnect")
                || shell.body_html.to_lowercase().contains("offline"),
            "a notice explains the page isn't loaded: {}",
            shell.body_html
        );

        // THE REGRESSION (Phase 21b): an unreachable origin 404s the manifest
        // *fetch* too, so the live error is `ManifestMissing` even though we
        // hold the cached manifest. The shell must still render from the cache —
        // gated on `manifest_only`, not on the error type.
        let shell_mm = m
            .shell_output(&peers, &loc, ResolveError::ManifestMissing, false)
            .expect("cached manifest + live ManifestMissing still yields a shell");
        assert_eq!(shell_mm.site_title, "Bill's Labs");
        assert!(shell_mm.error.is_none());
        assert!(
            shell_mm.body_html.to_lowercase().contains("unreachable"),
            "ManifestMissing notice says the source is unreachable: {}",
            shell_mm.body_html
        );

        // A LOCAL page-miss gets NO shell — that's a genuine not-found.
        let local = Location { peer_id: None, site_id: DEMO_SITE_ID.into(), page: "ghost".into() };
        assert!(m.shell_output(&peers, &local, ResolveError::PageMissing, false).is_none());
        // A foreign site we DON'T hold a manifest for → no shell (the real
        // ManifestMissing: we genuinely don't have it).
        let absent = Location { peer_id: Some(foreign), site_id: "absent".into(), page: String::new() };
        assert!(m.shell_output(&peers, &absent, ResolveError::ManifestMissing, false).is_none());
    }

    #[test]
    fn site_directory_shows_local_sites_before_the_index_populates() {
        // BUG-3: the rail unions the async, query-materialized index with a SYNC
        // direct scan of my store, so a physically-present site shows
        // IMMEDIATELY — never "No sites yet" while the browse area renders one.
        // `model()` seeds the demo site, so even with the index unmaterialized
        // the rail lists it (previously this asserted the buggy empty rail).
        let peers = pm();
        let me = peers.primary_peer_id().to_string();
        let m = model(&peers);
        // The async index has not been refreshed in this unit context...
        assert!(
            crate::content_site::discovery::read_site_index(&peers, &me).is_empty(),
            "precondition: the derived index is unmaterialized"
        );
        // ...yet the rail shows the seeded demo site via the direct scan.
        let dir = m.site_directory(&peers);
        assert!(
            dir.entries
                .iter()
                .any(|e| e.site == super::super::DEMO_SITE_ID && e.peer == me && e.owned),
            "the seeded demo site shows in the rail before any index refresh: {:?}",
            dir.entries.iter().map(|e| &e.site).collect::<Vec<_>>()
        );
    }

    #[test]
    fn section_path_renders_generated_index() {
        let peers = pm();
        let m = model(&peers);
        seed_deep_docs_site(&peers);
        // `reference` is a section path with no page entity of its own.
        m.navigate("site:docs/reference", &peers);
        let out = m.render_output(&peers);
        assert!(out.error.is_none(), "section path renders an index, not an error: {:?}", out.error);
        assert_eq!(out.current_page, "reference");
        // The generated index links to the section's child page(s).
        assert!(
            out.body_html.contains("reference/api"),
            "section index lists children as links: {}",
            out.body_html
        );
    }
}

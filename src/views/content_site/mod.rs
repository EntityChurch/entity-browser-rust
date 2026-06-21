//! Content Site window — renders a content-addressed site as navigable
//! HTML (Site Mode, P1).
//!
//! Thin controller around a long-lived [`ContentSiteModel`] (same split
//! as Knowledge Base: controller marshals actions → model; model owns
//! data + resolution; the pure DOM renderer in `dom::content_site`
//! reads the model's output). The renderer renders into the passed
//! container element — a window section now, the full-screen
//! `#site-layer` overlay later (P2). Nothing else changes when the host
//! swaps.

pub mod model;
pub mod output;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use crate::content_site::format::{SiteAsset, SITE_MANIFEST_TYPE};
use crate::content_site::{paths, NavItem, SiteManifest, SitePage};
use crate::window_watch::WindowWatch;
use model::ContentSiteModel;

/// The bundled demo site id (seeded on first window open). Canonical home
/// is [`crate::session_config::DEMO_SITE_ID`] — re-exported here for the
/// content seeder + tests. This is the demo *content* id, not a boot-path
/// pointer; the app reaches its home site through config (`home_site`).
pub use crate::session_config::DEMO_SITE_ID;

/// Content Site window — peer-bound, thin controller.
pub struct ContentSiteWindow {
    window_id: WindowId,
    peer_id: String,
    model: ContentSiteModel,
    watch: WindowWatch,
}

impl ContentSiteWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        let model = ContentSiteModel::new(window_id, peer_id.clone());
        Self {
            window_id,
            peer_id,
            model,
            watch: WindowWatch::new(),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Site Browser",
            description: "Browse content-addressed sites (Site Mode)",
            scope: crate::window::WindowScope::Peer,
            create: |id, peer_id, pm| {
                let mut window = ContentSiteWindow::new(id, peer_id.to_string());
                window.model.initialize(pm);
                // Re-render when our navigation state changes (navigate
                // persists the location) ...
                pm.watch_prefix(
                    &mut window.watch,
                    &window.peer_id,
                    crate::app_paths::window_state_path(
                        crate::app_paths::APP_ID,
                        &window.peer_id,
                        window.window_id,
                    ),
                );
                // ... and when site content changes (seed lands, future
                // edits/publishes). Subscribe to ALL sites under the peer —
                // which-site is config now (`home_site`), and the Worker-arm
                // cache mirror only feeds subscribed prefixes, so we cannot
                // depend on knowing the configured site id here at build time.
                pm.watch_prefix(
                    &mut window.watch,
                    &window.peer_id,
                    paths::sites_prefix(&window.peer_id),
                );

                // -- Site-aware directory wiring (P3) --
                //
                // The window's directory rail reads three ledgers that, on the
                // Worker arm, the cache mirror only feeds for SUBSCRIBED prefixes
                // (`feedback_worker_cache_get_needs_subscription`). Observe each
                // one the rail reads:
                //   - the derived site-index (the rail's site list);
                //   - the SDK-tier provenance ledger (cached-site freshness);
                //   - the app-tier preferences ledger (bookmarks / visit count).
                pm.watch_prefix(
                    &mut window.watch,
                    &window.peer_id,
                    crate::app_paths::site_index_path(crate::app_paths::APP_ID, &window.peer_id),
                );
                pm.watch_prefix(
                    &mut window.watch,
                    &window.peer_id,
                    crate::content_site::cache::provenance_prefix(&window.peer_id),
                );
                pm.watch_prefix(
                    &mut window.watch,
                    &window.peer_id,
                    crate::app_paths::site_cache_prefix(crate::app_paths::APP_ID, &window.peer_id),
                );
                // The site-origins registry feeds `list_origins` (the foreign
                // peers we route to) below + the resolver's `get_origin`. Both
                // read it via the Worker-arm cache mirror, so observe the
                // registry prefix — parity with the overlay
                // (`site_overlay.rs`), without which an origin registered after
                // this window opens never re-renders the rail (the resolver
                // can't reach a freshly-added foreign peer's sites).
                pm.watch_prefix(
                    &mut window.watch,
                    &window.peer_id,
                    crate::app_paths::site_origins_prefix(crate::app_paths::APP_ID, &window.peer_id),
                );
                // Cached foreign content lives at `/{P}/sites/` for each routable
                // peer P. Subscribe every one we already hold a route to so the
                // rail can OPEN a cached site (read it from my store) on the
                // Worker arm. A foreign site cached AFTER this window opens needs
                // a re-open to be browsable here — the window has no &mut
                // per-frame hook to grow its watch set the way the overlay's
                // `ensure_foreign_watches` does (handoff §3 follow-up #2 accepts
                // the same "re-open catches it" bound for the Settings picker).
                for (foreign, _origin) in
                    crate::content_site::origins::list_origins(pm, &window.peer_id)
                {
                    pm.watch_prefix(
                        &mut window.watch,
                        &window.peer_id,
                        paths::sites_prefix(&foreign),
                    );
                }
                // Kick a fire-and-forget index refresh so the rail populates
                // (the type-query → index entity → index-path subscription →
                // re-render bridge; same as the Settings picker).
                crate::content_site::discovery::refresh_site_index(pm, &window.peer_id);

                Box::new(window)
            },
        }
    }
}

impl WindowView for ContentSiteWindow {
    fn title(&self) -> String {
        "Site Browser".into()
    }

    fn type_name(&self) -> &'static str {
        "Site Browser"
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, action: &Action, peers: &Peers) {
        match action {
            Action::SiteNavigate { window_id, target } if *window_id == self.window_id => {
                self.model.navigate(target, peers);
            }
            Action::SiteBack { window_id } if *window_id == self.window_id => {
                self.model.back(peers);
            }
            Action::SiteOpen { window_id, peer, site } if *window_id == self.window_id => {
                self.model.open_site(peer, site, peers);
            }
            Action::SiteBookmarkToggle { window_id, peer, site } if *window_id == self.window_id => {
                self.model.toggle_bookmark(peer, site, peers);
            }
            Action::SiteKeepToggle { window_id, peer, site } if *window_id == self.window_id => {
                self.model.toggle_keep_offline(peer, site, peers);
            }
            Action::SiteRailFilter { window_id, filter } if *window_id == self.window_id => {
                // In-memory view change (no tree write) → no subscription would
                // fire; mark dirty so the rail rebuilds with the new filter.
                self.model
                    .set_rail_filter(crate::views::content_site::output::RailFilter::parse(filter));
                self.watch.mark_dirty();
            }
            _ => {}
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        peers: &Peers,
        ctx: &crate::dom::DomCtx,
    ) {
        use crate::dom::util;
        // Hand the async (HTTP-poll) resolver a repaint handle so a remote
        // fetch completion redraws this window. It must ALSO mark this window
        // dirty, not merely request a frame: a frame only rebuilds DIRTY
        // windows, and the HTTP-poll resolver caches its result in-memory (no
        // tree write), so a bare repaint fires a frame in which this window is
        // still clean → its section isn't rebuilt → it never re-resolves and
        // stays stuck on "Loading the live page…". The overlay is immune (it
        // re-renders every active frame). This regressed when the boot
        // cache-awareness pre-seeded the manifest up front: previously the
        // first manifest write on HTTP success dirtied the window, but a
        // pre-cached manifest re-write is a no-op change → no notification →
        // no dirty. Compose the dirty-mark in so completion always rebuilds.
        let dirty = self.watch.flag();
        let rp = ctx.repaint.clone();
        self.model.set_repaint(std::rc::Rc::new(move || {
            dirty.mark();
            rp();
        }));
        let directory = self.model.site_directory(peers);
        let output = self.model.render_output(peers);

        // The site-aware window is the directory rail (window-only) + the
        // shared single-site browse view. The overlay uses the same browse
        // renderer with NO rail — so the rail wrap lives here, not in the
        // shared `dom::content_site::render`.
        util::clear_children(container);
        // `.cs-window-row` is a row on desktop (rail | content); on mobile it
        // stacks so the directory rail collapses above the content rather than
        // eating a side column (responsive CSS injected by `content_site::render`,
        // which is root-scoped so it reaches this rail too).
        let row = util::create_element_with_class("div", "cs-window-row");

        let rail = util::create_element("nav");
        crate::dom::site_directory::render(&rail, &directory, ctx, self.window_id);
        util::append(&row, &rail);

        let content = util::create_element("div");
        // `min-height:0` lets it shrink in the mobile column layout (where the
        // rail stacks above); `min-width:0` is the desktop-row analogue.
        util::set_attr(&content, "style", "flex:1;min-width:0;min-height:0;overflow:hidden;");
        let resolve_asset =
            crate::dom::content_site::make_asset_resolver(peers, &self.peer_id, &output);
        crate::dom::content_site::render(
            &content,
            &output,
            ctx,
            crate::dom::content_site::SiteNavHost::Window(self.window_id),
            &resolve_asset,
        );
        util::append(&row, &content);
        util::append(container, &row);
    }
}

/// Seed the bundled demo site into `peer_id`'s tree if it isn't there
/// yet. Idempotent (gated on the manifest's presence). Synchronous L0
/// writes so the first render resolves without waiting on a dispatch
/// round-trip — the Direct-arm path the Content Site window opens on.
/// (Worker-arm-bound sites are a later concern, like the rest of
/// cross-peer/transport work.)
pub fn ensure_demo_site(peers: &Peers, peer_id: &str) {
    // Re-seed when the manifest is absent OR carries the OLD type tag.
    // Worker mode (default) is durable: pre-migration demo entities
    // persist in OPFS under the old `content/site/*` tags at this same
    // path. Gating on path-presence alone (the original guard) would
    // serve a stale-typed ghost forever; gate on the CURRENT type so the
    // rename actually takes (D16 — the durable-Worker orphan trap).
    let up_to_date = peers
        .get_entity(peer_id, &paths::manifest_path(peer_id, DEMO_SITE_ID))
        .map(|e| e.entity_type == SITE_MANIFEST_TYPE)
        .unwrap_or(false);
    if up_to_date {
        return;
    }

    // A genuinely *deep* demo site (2- and 3-level page paths under a
    // "Guide" section) so the overlay exercises nested navigation +
    // active-trail, not just flat pages — the deep-site cycle's live
    // surface. Top-level nav still carries Home/About/Theory
    // (Phase 19/20 assert these) plus the Guide section.
    let manifest = SiteManifest::new(
        DEMO_SITE_ID,
        "Entity Demo Site",
        "index",
        vec![
            // Nav targets are root-absolute (`/slug`): nav is a site-global
            // menu rendered on every page, so it must resolve identically from
            // any current page (the link-resolution convention, location.rs).
            NavItem::new("Home", "/index"),
            // Guide is a section (its pages nest under guide/*); the format
            // can now carry a children sub-menu (GAP3), but until a sidebar
            // renderer consumes children we keep the demo's declared nav ==
            // what's rendered (a flat top bar) — no unrendered data (AP10).
            NavItem::new("Guide", "/guide/intro"),
            NavItem::new("About", "/about"),
            NavItem::new("Theory", "/theory"),
        ],
    );

    let pages = [
        (
            "index",
            SitePage::markdown(
                "Welcome",
                "# Welcome to the Entity Demo Site\n\nThis page is a **content-addressed entity** rendered as HTML — you're browsing it inside a full entity peer, but it looks like any other site.\n\n::embed[Entity Demo Figure — a content-addressed SVG asset, embedded via the ::embed directive]{ref=assets/figures/demo.svg}\n\n- It's just markdown stored in the tree.\n- Links navigate within the entity system.\n- The overlay toggle reveals the peer underneath.\n\nStart with the [Guide](./guide/intro), read [About](./about) or the [Theory](./theory), or visit [the web](https://example.com).\n",
            ),
        ),
        (
            "guide/intro",
            SitePage::markdown(
                "Guide — Intro",
                "# Guide: Intro\n\nThis page lives at `guide/intro` — a **nested** content entity. The *Guide* nav item stays highlighted across the whole section (active-trail).\n\nNext: [Install](install), or jump straight to the [Internals](advanced/internals).\n\nBack to [Home](../index).\n",
            ),
        ),
        (
            "guide/install",
            SitePage::markdown(
                "Guide — Install",
                "# Guide: Install\n\nStill in the Guide section (`guide/install`). Notice *Guide* is still the active nav item.\n\nBack to the [Intro](intro), or deeper to [Internals](advanced/internals).\n",
            ),
        ),
        (
            "guide/advanced/internals",
            SitePage::markdown(
                "Guide — Internals",
                "# Guide: Internals\n\nThree levels deep (`guide/advanced/internals`) and still resolving from the tree by path. The *Guide* section nav stays lit the whole way down.\n\nBack to the [Intro](../intro).\n",
            ),
        ),
        (
            "about",
            SitePage::markdown(
                "About",
                "# About\n\nThe Entity Demo Site is a tiny showcase of **Site Mode**: content-addressed static sites with reactivity, served from the entity system.\n\n```\nsite/demo/\n  manifest\n  pages/{index,about,theory}\n  pages/guide/{intro,install}\n  pages/guide/advanced/internals\n```\n\nBack to [Home](./index).\n",
            ),
        ),
        (
            "theory",
            SitePage::markdown(
                "Theory",
                "# Theory\n\nA *site* is a content subgraph rooted at a signed manifest. Pages are markdown entities; links are entity-native and resolve across sites and peers.\n\n> Format ⊥ transport: the same page renders from the local tree, a peer, or a CDN.\n\nBack to [Home](./index).\n",
            ),
        ),
    ];

    // Arm-aware seed write via the blessed `Peers::seed_write` router
    // method: Direct (native, Tauri WebView, tests) → synchronous L0 put
    // so the demo is readable in the same render pass (the sync `#[test]`s
    // depend on it); Worker (browser) → async `dispatch_write`, the
    // resolver's `Pending`/repaint seam absorbs the delay. This replaced
    // an open-coded `direct_peer_context` reach-through whose original
    // unconditional `store().put()` panicked on Worker spawn and froze the
    // rAF loop — routing it removes the hatch *and* the panic class.
    peers.seed_write(peer_id, paths::manifest_path(peer_id, DEMO_SITE_ID), manifest.to_entity());
    // The figure the index page embeds — a small, human-authored SVG so the
    // asset path has a *visible* artifact end-to-end (independent of the
    // papers compute-figure pipeline, whose PNGs may be unpinned placeholders).
    // SVG is text, content-addressed like any asset, and safe in an <img>.
    peers.seed_write(
        peer_id,
        paths::asset_path(peer_id, DEMO_SITE_ID, "figures/demo.svg"),
        SiteAsset::new("image/svg+xml", DEMO_FIGURE_SVG.as_bytes().to_vec()).to_entity(),
    );
    for (slug, page) in pages {
        peers.seed_write(peer_id, paths::page_path(peer_id, DEMO_SITE_ID, slug), page.to_entity());
    }
}

/// A small authored SVG figure the demo site's index page embeds — proof of
/// the embed→asset→`<img>` path with a visible artifact.
const DEMO_FIGURE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="480" height="240" viewBox="0 0 480 240">
  <rect width="480" height="240" rx="12" fill="#15151f"/>
  <rect x="1" y="1" width="478" height="238" rx="12" fill="none" stroke="#2a2a3e"/>
  <circle cx="120" cy="150" r="44" fill="#3a6ea5"/>
  <rect x="200" y="100" width="84" height="96" rx="6" fill="#9fd0ff"/>
  <polygon points="330,196 392,100 440,196" fill="#6ad0a0"/>
  <text x="24" y="46" fill="#cfe3ff" font-family="system-ui,sans-serif" font-size="22" font-weight="700">Entity Demo Figure</text>
  <text x="24" y="72" fill="#9aa3b2" font-family="system-ui,sans-serif" font-size="13">A content-addressed SVG asset, embedded via ::embed</text>
</svg>"##;

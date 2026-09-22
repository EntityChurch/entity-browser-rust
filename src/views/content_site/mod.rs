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

mod demo_content;
pub mod model;
pub mod output;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use crate::content_site::paths;
use crate::window_watch::WindowWatch;
use model::ContentSiteModel;

/// The bundled demo site id (seeded on first window open). Canonical home
/// is [`crate::session_config::DEMO_SITE_ID`] — re-exported here for the
/// content seeder + tests. This is the demo *content* id, not a boot-path
/// pointer; the app reaches its home site through config (`home_site`).
pub use crate::session_config::DEMO_SITE_ID;

/// The companion demo site — a second owned site on the SAME peer that the
/// primary demo cross-site-links to, so the bundled demo exercises `site:`
/// cross-site navigation (location.rs) end to end and the directory rail has
/// more than one owned entry.
pub const DEMO_NOTES_SITE_ID: &str = "demo-notes";

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
            name: "Site Browser", // i18n-ignore — identity key; display via window.site_browser
            description: "Browse content-addressed sites (Site Mode)", // i18n-ignore — dead_code
            scope: crate::window::WindowScope::Peer,
            create: |id, peer_id, pm| {
                let mut window = ContentSiteWindow::new(id, peer_id.to_string());
                window.model.initialize(pm);
                // That read is best-effort and on the Worker arm reads NOTHING:
                // `initialize`'s `get_entity` answers from the per-prefix cache
                // mirror, and the only subscription that would cover this
                // window's state path is the one registered below — after the
                // read, and asynchronously even then. So a Site Browser window
                // restored on the Worker arm opened at the configured home
                // instead of where it was left, on every boot.
                //
                // The correction is `WindowView::hydrate_durable`, and it is
                // NOT called here: `WindowManager::spawn` calls it for every
                // window it creates, which is what makes the step structural
                // rather than something the next window author has to remember.
                // (`docs/plans/AUDIT-WORKER-ARM-NAVIGATION-2026-08-30.md`.)
                //
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
        crate::i18n::window_title("Site Browser") // i18n-ignore — lookup key
    }

    fn type_name(&self) -> &'static str {
        "Site Browser" // i18n-ignore — stable type identifier, not UI text
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    /// AP41's original repair, now reached through the class hook. The model
    /// owns the three properties (`Unheard` keeps what we have, the
    /// `nav_generation` guard refuses to clobber a navigation that landed
    /// during the await, and a sync read that already answered short-circuits
    /// the round-trip entirely).
    ///
    /// **The site-origin mirror is deliberately NOT here**, though this is where
    /// it belongs on paper. A window bound to a non-system peer reads an empty
    /// origin registry (`origins::mirror_origins`), and the repair has to be a
    /// *conversation* — list the source, then read each destination record
    /// before deciding to write — which a task spawned from a factory cannot
    /// hold, because `Peers` is a non-`Clone` router and the borrow dies at the
    /// first await. It runs from `boot_load` instead
    /// (`origins::mirror_to_all_local_peers`), which owns the borrow long
    /// enough; the residual bound is written down there.
    fn hydrate_durable(&self, _peers: &Peers) {
        #[cfg(target_arch = "wasm32")]
        self.model.spawn_hydrate_durable(_peers);
    }

    /// ⭐ **Open showing a named site — bound to MY store, aimed at THEIR
    /// content.**
    ///
    /// The two peers in play are the whole history of this surface:
    /// `open_target`'s doc records that binding this window to the publisher
    /// produced a real window with an empty rail, because cached foreign content
    /// lives at `/{foreign}/sites/…` in *my* store and no local SDK hosts a
    /// publisher's id. So the target's peer goes into the **location** — which is
    /// exactly where `Action::SiteOpen`'s `peer` already goes — and the binding is
    /// untouched.
    ///
    /// The `Unusable` arm is the live one and is not an edge case: a name that
    /// resolved to a publisher and an origin says *whose sites*, never *which*,
    /// and this window opens as it otherwise would. Reporting that honestly is
    /// what keeps the Registry Browser's remaining guess visible instead of
    /// dressing it as a hit.
    fn aim(&mut self, target: &crate::entity_ref::EntityRef, peers: &Peers) -> crate::window::Aim {
        use crate::window::Aim;
        let Some(payload) =
            crate::open_target::payload(target, crate::open_target::SITES_SEGMENT)
        else {
            return Aim::NotMine;
        };
        match crate::content_site::location::Location::from_target_payload(target.peer(), payload) {
            Some(loc) => {
                self.model.open_page(
                    loc.peer_id.as_deref().unwrap_or(""),
                    &loc.site_id,
                    &loc.page,
                    peers,
                );
                self.watch.mark_dirty();
                Aim::Aimed
            }
            // A D13 log field, not a label; see `Aim::Unusable`.
            None => Aim::Unusable("names this publisher's sites but not which one"), // i18n-ignore
        }
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

/// Seed the bundled demo site into `peer_id`'s tree if it isn't there yet.
///
/// **Lives in [`demo_content`]**, with the page bodies it writes — a demo
/// site's manifest title, nav labels and page titles are that site's
/// content, authored by whoever published it, and the app does not
/// translate the pages it renders. See that module's header.
pub use demo_content::ensure_demo_site;

#[cfg(test)]
mod aim_tests {
    use super::*;
    use crate::window::Aim;

    fn peer_id(seed: u8) -> String {
        let kp = entity_crypto::Keypair::from_seed([seed; 32]);
        entity_crypto::PeerId::from_public_key(&kp.public_key_bytes()).to_string()
    }

    fn window(peers: &Peers) -> ContentSiteWindow {
        ContentSiteWindow::new(1, peers.primary_peer_id().to_string())
    }

    /// A site page address opens the window **at that page of that publisher's
    /// site**, and the binding is untouched — the subject travels in the
    /// location, exactly where `Action::SiteOpen`'s `peer` already travels.
    #[test]
    fn a_site_page_address_opens_that_page_and_leaves_the_binding_alone() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let publisher = peer_id(21);
        let mut w = window(&peers);
        let target = crate::open_target::site(&publisher, "demo", "about");
        assert_eq!(w.aim(&target, &peers), Aim::Aimed);
        let state = w.model.state_snapshot();
        assert_eq!(state.peer.as_deref(), Some(publisher.as_str()));
        assert_eq!(state.site_id, "demo");
        assert_eq!(state.page, "about", "the page is the half a site-root open would drop");
        assert_eq!(w.peer_id, me, "the store this window reads must not move");
    }

    /// ⭐ **The Registry Browser's live case: routable, not aimable.** A name that
    /// resolved to a publisher says *whose* sites and never *which*, so the aim
    /// reports `Unusable` and the window opens exactly as it otherwise would.
    /// Asserting the state is **unchanged** is the half that matters — a version
    /// that "helpfully" opened some site would be a guess (AP54) wearing a
    /// feature's name.
    #[test]
    fn a_publishers_site_directory_is_unusable_and_moves_nothing() {
        let peers = Peers::new_direct();
        let mut w = window(&peers);
        let before = w.model.state_snapshot();
        let target = crate::open_target::site_directory(&peer_id(22));
        assert!(matches!(w.aim(&target, &peers), Aim::Unusable(_)));
        assert_eq!(w.model.state_snapshot(), before, "an unusable aim must move nothing");
    }

    /// Another convention's address is `NotMine` and moves nothing.
    #[test]
    fn a_feed_address_is_not_this_windows() {
        let peers = Peers::new_direct();
        let mut w = window(&peers);
        let before = w.model.state_snapshot();
        assert_eq!(w.aim(&crate::open_target::feed(&peer_id(23)), &peers), Aim::NotMine);
        assert_eq!(w.model.state_snapshot(), before);
    }

    /// An address naming something real **inside** a site lands you in that site
    /// rather than nowhere — `Location::from_target_payload`'s deliberate
    /// charity. Refusing would report *"nothing to open"* about a site that is
    /// sitting right there.
    #[test]
    fn an_address_deeper_than_a_page_still_lands_in_the_site() {
        let peers = Peers::new_direct();
        let publisher = peer_id(24);
        let mut w = window(&peers);
        let asset = crate::entity_ref::EntityRef::live(
            &publisher,
            "/sites/demo/assets/figure-1.png",
        );
        assert_eq!(w.aim(&asset, &peers), Aim::Aimed);
        let state = w.model.state_snapshot();
        assert_eq!(state.site_id, "demo");
        assert_eq!(state.page, "", "an asset is not a page; the site root is the honest landing");
    }
}

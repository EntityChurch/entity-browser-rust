//! The Site Mode overlay controller — renders the active content site
//! into the light-DOM `#site-layer` (a full-pane sibling of `#dom-layer`).
//!
//! This is the overlay counterpart to a Content Site *window*: it owns a
//! [`ContentSiteModel`] (app-level nav state, not per-window) and reuses
//! the **same** host-agnostic renderer (`dom::content_site::render`) — the
//! only difference is [`SiteNavHost::Overlay`], which routes link clicks
//! to [`Action::SiteOverlayNavigate`].
//!
//! Reactivity: [`render`](SiteOverlay::render) is called once per frame
//! while the overlay is active and rebuilds the DOM **only when the
//! rendered output changes** (a `SiteRenderOutput` equality guard) — so a
//! navigation persists → next frame the output differs → the pane
//! rebuilds; an idle frame is a cheap compare with no DOM churn. The
//! overlay's listener closures live in [`closures`](Self::closures),
//! cleared on each rebuild so old listeners are dropped (D12).

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use crate::action::Action;
use crate::dom::content_site::SiteNavHost;
use crate::dom::{util, DomCtx};
use crate::peers::Peers;
use crate::views::content_site::model::ContentSiteModel;
use crate::views::content_site::output::SiteRenderOutput;
use crate::window::{new_closure_vec, ClosureVec, RepaintFn};
use crate::window_watch::WindowWatch;

/// Sentinel window id for the overlay's `DomCtx` — the overlay is not a
/// window, and its renderer never reads `window_id` (nav routes via
/// [`SiteNavHost::Overlay`], not the window id).
const OVERLAY_CTX_WINDOW_ID: crate::window::WindowId = 0;

/// Drives the Site Mode overlay surface.
pub struct SiteOverlay {
    model: ContentSiteModel,
    /// This surface's bound peer — the store selector for the cached foreign
    /// content reads and the prefix anchor for the foreign-site subscriptions.
    peer_id: String,
    /// Subscriptions that keep the Worker-arm cache mirror fed for the
    /// paths the overlay reads (the mode state + the site content). Held
    /// for the overlay's lifetime; dropping it cancels the observes (D9).
    /// Never read — the subscription *existing* is the point. Grows over the
    /// overlay's life as foreign sites are cached ([`Self::ensure_foreign_watches`]).
    watch: WindowWatch,
    /// Foreign peers whose `/{P}/sites/` prefix is already subscribed — so we
    /// add each watch exactly once. Cached foreign content lives at
    /// `/{foreign}/sites/...` in MY store; on the Worker arm the cache mirror
    /// only feeds *subscribed* prefixes, so a reload reads `None` for a cached
    /// site unless its prefix is observed (which is what strands you on exit —
    /// `feedback_worker_cache_get_needs_subscription`).
    foreign_watched: HashSet<String>,
    /// Listener closures for the currently-mounted overlay DOM. Cleared
    /// (dropping the JS-side closures) on each rebuild.
    closures: ClosureVec,
    /// Typing-draft map for the renderer's `DomCtx`. The site renderer
    /// doesn't use drafts today, but `DomCtx` requires one.
    drafts: util::DraftsMap,
    /// Last output rendered into the pane — the rebuild guard.
    last_output: Option<SiteRenderOutput>,
    /// Last `can_exit` (exposed chrome↔site toggle) rendered — folded into the
    /// rebuild guard so flipping `show_toggle`/`enabled` in Settings while the
    /// overlay is live updates the Exit affordance even when `output` is
    /// unchanged. Derived from `site_mode.show_toggle && enabled` (BUG-1).
    last_can_exit: Option<bool>,
}

impl SiteOverlay {
    /// Construct the overlay for `peer_id`, seed/hydrate its state
    /// (idempotent), and subscribe so the Worker-arm cache mirror is fed.
    pub fn new(peer_id: &str, peers: &Peers) -> Self {
        let mut model = ContentSiteModel::new_overlay(peer_id.to_string());
        model.initialize(peers);

        // CRITICAL (Worker arm = the browser default): `get_entity` reads
        // a cache mirror that is populated ONLY for SUBSCRIBED prefixes
        // (peers_worker::cache_get). Without these observes, in Worker
        // mode both the session-config read (apply_site_mode) and the
        // content resolve return None — so the toggle silently does nothing
        // until another window's broad subscription happens to flush the
        // cache. That was the reported "View Site does nothing until I open a
        // window" bug. We subscribe to exactly what we read:
        //   - the session config path → apply_site_mode's per-frame read
        //     reflects the toggle write;
        //   - the site content prefix → the page/manifest resolve.
        let mut watch = WindowWatch::new();
        peers.watch_prefix(&mut watch, peer_id, crate::session_config::state_path(peer_id));
        // ALL sites under the peer — which-site is config (`home_site`), and
        // the Worker-arm cache mirror only feeds subscribed prefixes, so we
        // can't depend on the configured site id being known/cached here.
        peers.watch_prefix(
            &mut watch,
            peer_id,
            crate::content_site::paths::sites_prefix(peer_id),
        );
        // The site-origin registry (peer_id → http origin) is read per
        // frame by the MultiResolver when a Location names a remote peer;
        // on the Worker arm that read hits the cache mirror, so we must
        // subscribe its prefix or cross-peer origins resolve to None
        // ([[feedback_worker_cache_get_needs_subscription]]).
        peers.watch_prefix(
            &mut watch,
            peer_id,
            crate::app_paths::site_origins_prefix(crate::app_paths::APP_ID, peer_id),
        );
        // The SDK-tier provenance ledger for cached foreign sites lives under
        // my own `/{me}/system/cache/` — observe it so a Worker-arm read of
        // "when did I last reconcile site S" reflects the write-through.
        peers.watch_prefix(
            &mut watch,
            peer_id,
            crate::content_site::cache::provenance_prefix(peer_id),
        );
        // Foreign cached content lives at `/{P}/sites/` for each peer P we hold
        // a route to (you can only have cached a peer you have an origin for).
        // Subscribe each known one now; `ensure_foreign_watches` (per frame)
        // picks up any that warm in later — a fresh Worker boot's cache mirror
        // may not have the origins registry yet at construction.
        let mut foreign_watched = HashSet::new();
        for (foreign, _origin) in crate::content_site::origins::list_origins(peers, peer_id) {
            peers.watch_prefix(
                &mut watch,
                peer_id,
                crate::content_site::paths::sites_prefix(&foreign),
            );
            foreign_watched.insert(foreign);
        }

        Self {
            model,
            peer_id: peer_id.to_string(),
            watch,
            foreign_watched,
            closures: new_closure_vec(),
            drafts: Rc::new(RefCell::new(std::collections::HashMap::new())),
            last_output: None,
            last_can_exit: None,
        }
    }

    /// Ensure a `/{foreign}/sites/` subscription exists for every peer we hold a
    /// route to, so the Worker-arm cache mirror feeds cached foreign content.
    /// Idempotent + cheap (a roster read + set membership); called per frame so
    /// a route that warms *after* construction — the common fresh-boot case —
    /// still gets observed, and a newly-followed cross-peer link is covered.
    fn ensure_foreign_watches(&mut self, peers: &Peers) {
        for (foreign, _origin) in crate::content_site::origins::list_origins(peers, &self.peer_id) {
            if self.foreign_watched.insert(foreign.clone()) {
                peers.watch_prefix(
                    &mut self.watch,
                    &self.peer_id,
                    crate::content_site::paths::sites_prefix(&foreign),
                );
            }
        }
    }

    /// Navigate the overlay to a raw link `target` (classified + resolved
    /// by the model; external links are ignored). The new location
    /// persists, so the next [`render`](Self::render) rebuilds the pane.
    pub fn navigate(&self, target: &str, peers: &Peers) {
        self.model.navigate(target, peers);
    }

    /// Go back to the overlay's previous location (the back affordance).
    pub fn back(&self, peers: &Peers) {
        self.model.back(peers);
    }

    /// Render the active site into `#site-layer`, rebuilding only when the
    /// output changed since the last frame. `sink`/`repaint` are the
    /// renderer's shared action sink + repaint signal (nav clicks push
    /// [`Action::SiteOverlayNavigate`] there).
    pub fn render(
        &mut self,
        peers: &Peers,
        sink: Rc<RefCell<Vec<Action>>>,
        repaint: RepaintFn,
    ) {
        let Some(layer) = util::get_element_by_id("site-layer") else {
            return;
        };
        // Give the model (its async HTTP-poll resolver) a repaint handle so
        // a completed remote fetch can trigger a redraw → next-frame
        // re-read of the now-filled closure cache. Cheap; overwrites.
        self.model.set_repaint(repaint.clone());
        // Keep the Worker-arm cache mirror fed for every routable foreign peer
        // (routes can warm after construction on a fresh boot).
        self.ensure_foreign_watches(peers);
        let output = self.model.render_output(peers);
        // The overlay-side "Exit Site" affordance mirrors the chrome status-bar
        // toggle: shown iff the deployment exposes the chrome↔site toggle. In a
        // locked/strict-site deployment this is `false`, so no exit is rendered
        // and the user can't strand themselves in chrome (BUG-1). Read per frame
        // (cheap L0 store read; `apply_site_mode` already reads the same config
        // each frame).
        let cfg = crate::session_config::read(peers, &self.peer_id);
        let can_exit = cfg.site_mode.exposes_toggle();
        if self.last_output.as_ref() == Some(&output) && self.last_can_exit == Some(can_exit) {
            return; // unchanged — leave the mounted DOM (and its scroll) alone
        }

        // Rebuild: drop the previous frame's listeners (D12), then render.
        // `content_site::render` clears the container first.
        self.closures.borrow_mut().clear();
        let ctx = DomCtx {
            window_id: OVERLAY_CTX_WINDOW_ID,
            actions: sink,
            repaint,
            closures: self.closures.clone(),
            drafts: self.drafts.clone(),
        };
        let resolve_asset =
            crate::dom::content_site::make_asset_resolver(peers, &self.peer_id, &output);
        crate::dom::content_site::render(
            &layer,
            &output,
            &ctx,
            SiteNavHost::Overlay { can_exit },
            &resolve_asset,
        );
        self.last_output = Some(output);
        self.last_can_exit = Some(can_exit);
    }
}

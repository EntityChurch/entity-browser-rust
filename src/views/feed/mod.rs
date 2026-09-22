//! **Feed window** — follow a publisher by peer id and read what they posted.
//!
//! The first product surface over `APP-CONVENTION-FEED`. The pieces beneath it
//! were built in order and each has its own module doc: [`crate::feed`] is the
//! codec, [`crate::feed_publish`] the emitter, [`crate::feed_read`] the walk,
//! [`crate::feed_fetch`] the poll-shaped adapter over it, and
//! [`crate::feed_follows`] the durable follow registry, and
//! [`crate::feed_gatherers`] the peers this profile reads others THROUGH.
//!
//! ## What this window does NOT do, stated rather than implied
//!
//! - **It holds nothing durable of anybody else's.** The fetched feed is
//!   in-memory and per-window, so there is no offline read and D24 does not
//!   engage. [`crate::feed_fetch`]'s module doc carries the three-way currency
//!   analysis for when that changes.
//! - ~~**It cannot post.**~~ **It can, as of 2026-09-15** —
//!   [`crate::feed_compose`]'s four verbs, wired as the *Your feed* section.
//!   Writing `app/feed/entry` into your own tree **is** publishing on the live
//!   road; the static emit is a separate operational act and is not this
//!   window's business. What it still cannot do is author a **collection** —
//!   the verbs exist and are gated, and nothing on screen calls them.
//! - **It persists no window state**, deliberately — see [`model`]. Who you
//!   follow is app-scoped; which of them you are looking at is not a fact about
//!   the profile.
//!
//! ## The subscription is the follow prefix, and that is the only one
//!
//! Change detection here is `WindowWatch` over
//! [`crate::app_paths::feed_follows_prefix`] — the one thing on this surface
//! that lives in the tree. The *entries* are not in our tree at all, so nothing
//! can subscribe to them; the poller fires the repaint itself when a walk lands,
//! which is `HttpPollResolver`'s arrangement for exactly the same reason.

pub mod model;
pub mod output;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use crate::window_watch::WindowWatch;
use model::FeedModel;

pub struct FeedWindow {
    #[allow(dead_code)]
    model: FeedModel,
    watch: WindowWatch,
    #[allow(dead_code)]
    window_id: WindowId,
    /// The peer whose tree holds the follow registry — this window's bound peer.
    #[allow(dead_code)]
    peer_id: String,
}

impl FeedWindow {
    pub fn new(window_id: WindowId, peer_id: &str, repaint: crate::content_site::resolver::RepaintCell) -> Self {
        Self {
            model: FeedModel::new(window_id, repaint),
            watch: WindowWatch::new(),
            window_id,
            peer_id: peer_id.to_string(),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Feed", // i18n-ignore — identity key; display via window.feed
            description: "Follow publishers and read what they posted", // i18n-ignore — dead_code
            scope: crate::window::WindowScope::Peer,
            create: |id, peer_id, pm| {
                let mut window = FeedWindow::new(id, peer_id, Default::default());
                // The follow registry is the one thing on this surface that
                // lives in the tree, so it is the one thing to watch. A write
                // marks the window dirty and the next frame re-reads the list —
                // never a cached copy (AP41).
                let prefix =
                    crate::app_paths::feed_follows_prefix(crate::app_paths::APP_ID, peer_id);
                pm.watch_prefix(&mut window.watch, peer_id, prefix);
                // **The gatherer registry is watched too, and forgetting it is
                // the defect that would look like a dead button**: a write marks
                // nothing dirty, the frame never re-reads, and the list a person
                // just added to renders unchanged. Two registries, two watches —
                // there is no "the feed prefix" that covers both.
                let gatherers =
                    crate::app_paths::feed_gatherers_prefix(crate::app_paths::APP_ID, peer_id);
                pm.watch_prefix(&mut window.watch, peer_id, gatherers);
                // ⭐ **The THIRD watch, and it is the composer's.** Your own
                // posts are read per render out of the entry prefix (AP41), so
                // without this a post marks nothing dirty, the frame never
                // re-reads, and the list you just added to renders unchanged —
                // the same dead-button defect the gatherer watch above exists to
                // avoid, one registry over. On the Worker arm it is doing more
                // than dirtying: the per-prefix mirror is filled only for
                // subscribed prefixes, so an unsubscribed entry prefix lists
                // EMPTY and the surface would say you have posted nothing.
                let entries = format!("/{peer_id}/{}", crate::feed::entry_prefix());
                pm.watch_prefix(&mut window.watch, peer_id, entries);
                Box::new(window)
            },
        }
    }
}

impl WindowView for FeedWindow {
    fn title(&self) -> String {
        crate::i18n::window_title("Feed") // i18n-ignore — lookup key
    }

    fn type_name(&self) -> &'static str {
        "Feed" // i18n-ignore — stable type identifier, not UI text
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    /// ⭐ **Open showing a named publisher — and following them is a separate
    /// act.**
    ///
    /// The address a deployment or a registry walk hands us names *whose feed*,
    /// and `FeedModel::select` is exactly that: the panel reads through
    /// [`crate::feed_route`] whether or not the subject is in the follow
    /// registry. So an aim shows you somebody without quietly adding them to a
    /// durable list you did not ask for — *naming a source is not asking to keep
    /// reading them*, which is the same line `add_gatherer` draws one button
    /// over.
    ///
    /// The subject is the target's peer; **the binding is untouched** — the
    /// follow and gatherer registries live in *our* tree and this window keeps
    /// reading them from the peer it was bound to.
    fn aim(&mut self, target: &crate::entity_ref::EntityRef, _peers: &Peers) -> crate::window::Aim {
        use crate::window::Aim;
        // §4.2 pins `app/feed/index` and what hangs under it. A deeper payload
        // names a *page* of the index and this surface has no page control; the
        // author is the whole of what it can honour, so a page is not a second
        // outcome, it is a term we do not read.
        if crate::open_target::payload(target, crate::open_target::FEED_SEGMENT).is_none() {
            return Aim::NotMine;
        }
        if target.peer().is_empty() {
            // An `Aim` detail is a D13 log field, never a label:
            // `WindowManager::spawn` prints it and no renderer reads it.
            return Aim::Unusable("names no publisher, and a feed is somebody's"); // i18n-ignore
        }
        self.model.select(target.peer());
        self.watch.mark_dirty();
        Aim::Aimed
    }

    fn handle_action(&mut self, action: &Action, peers: &Peers) {
        let Action::WindowEvent { window_id, event, value } = action else { return };
        if *window_id != self.window_id {
            return;
        }
        let me = self.peer_id.clone();
        match event.as_str() {
            // The typed peer id is a renderer-side DRAFT (`ctx.drafts` via
            // `components::text_input`), so it arrives packed in `value` on the
            // one-shot press rather than as a keystroke event.
            "feed_follow" => self.model.follow(peers, &me, value, now_ms_u64()),
            "feed_unfollow" => self.model.unfollow(peers, &me, value),
            "feed_select" => self.model.select(value),
            "feed_refresh" => self.model.refresh(),
            "feed_add_gatherer" => self.model.add_gatherer(peers, &me, value, now_ms_u64()),
            "feed_remove_gatherer" => self.model.remove_gatherer(peers, &me, value),
            // The composer. `value` is the draft for a post and the full hex
            // address for a removal — **the full one**, never `id_short`: the
            // shortened form is for a person to read and names no binding.
            "feed_post" => self.model.post(peers, &me, value, now_ms_u64()),
            "feed_remove_post" => self.model.remove_post(peers, &me, value),
            _ => return,
        }
        self.watch.mark_dirty();
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        peers: &Peers,
        ctx: &crate::dom::DomCtx,
    ) {
        // ⚠ **Mark dirty AND request a frame, composed — a bare repaint is not
        // enough.** A frame only rebuilds DIRTY windows, and a landed feed walk
        // writes nothing to the tree, so nothing else can dirty this one. Without
        // the compose the panel stays on *"Reading this feed…"* with every byte
        // already in hand; with only the mark, no frame is requested and it waits
        // for an unrelated one. `content_site::render_dom` carries the long
        // version of the same arrangement.
        let dirty = self.watch.flag();
        let rp = ctx.repaint.clone();
        self.model.set_repaint(std::rc::Rc::new(move || {
            dirty.mark();
            rp();
        }));
        let output = self.model.render_output(
            peers,
            &self.peer_id,
            crate::dom::programs::now_ms(),
        );
        crate::dom::feed::render(container, &output, ctx);
    }
}

/// Wall-clock ms for a follow record's `since`.
///
/// **`Date.now()`, not `performance.now()`** — `since` is a timestamp somebody
/// may one day see, and `now_ms()` is a monotonic clock measured from page load,
/// which would record every follow as having happened a few seconds after the
/// epoch. The two clocks answer different questions and this is the one place on
/// this surface that wants the other one.
#[cfg(target_arch = "wasm32")]
fn now_ms_u64() -> u64 {
    js_sys::Date::now() as u64
}

/// Natively there is no clock and nothing calls this; `0` is an honest *"we do
/// not know when"* rather than a fabricated instant.
#[cfg(not(target_arch = "wasm32"))]
fn now_ms_u64() -> u64 {
    0
}

#[cfg(test)]
mod aim_tests {
    use super::*;
    use crate::window::Aim;

    fn peer_id(seed: u8) -> String {
        let kp = entity_crypto::Keypair::from_seed([seed; 32]);
        entity_crypto::PeerId::from_public_key(&kp.public_key_bytes()).to_string()
    }

    fn window(peers: &Peers) -> FeedWindow {
        FeedWindow::new(1, peers.primary_peer_id(), Default::default())
    }

    /// A feed address opens the window **showing that publisher**, and the
    /// binding is untouched — the two peers stay two peers.
    #[test]
    fn a_feed_address_selects_its_publisher_and_leaves_the_binding_alone() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let author = peer_id(9);
        let mut w = window(&peers);
        assert_eq!(w.aim(&crate::open_target::feed(&author), &peers), Aim::Aimed);
        let out = w.model.render_output(&peers, &me, 0.0);
        assert_eq!(out.selected.as_deref(), Some(author.as_str()));
        assert_eq!(w.peer_id, me, "the store this window reads must not move");
    }

    /// ⭐ **Aiming shows somebody; it does not follow them.** A viewer opened at
    /// a publisher must not write a durable record nobody asked for — *naming a
    /// source is not asking to keep reading them*. The follow list stays empty
    /// and the panel still resolves, which is the property that makes the two
    /// separable at all.
    #[test]
    fn aiming_at_a_publisher_does_not_follow_them() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let author = peer_id(11);
        let mut w = window(&peers);
        w.aim(&crate::open_target::feed(&author), &peers);
        assert!(
            crate::feed_follows::list(&peers, &me).is_empty(),
            "an aim must not write a follow record"
        );
        assert_eq!(
            w.model.render_output(&peers, &me, 0.0).selected.as_deref(),
            Some(author.as_str()),
            "…and it must still be what the window is showing"
        );
    }

    /// Another convention's address is **`NotMine`, not a silent no-op**, and it
    /// selects nobody. Reachable only through a caller that routed badly, which
    /// is exactly why the outcome has to be nameable rather than inferred from
    /// the window looking unchanged.
    #[test]
    fn a_site_address_is_not_this_windows_and_selects_nobody() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let mut w = window(&peers);
        let site = crate::open_target::site(&peer_id(12), "demo", "about");
        assert_eq!(w.aim(&site, &peers), Aim::NotMine);
        assert_eq!(w.model.render_output(&peers, &me, 0.0).selected, None);
    }

    /// A feed address naming no publisher is **`Unusable`** — the window opens
    /// and shows nobody, which is what it would have done anyway. Kept apart
    /// from `NotMine` because one is *"wrong window"* and the other is *"right
    /// window, nothing to point it at"*.
    #[test]
    fn a_feed_address_with_no_publisher_is_unusable_rather_than_not_mine() {
        let peers = Peers::new_direct();
        let mut w = window(&peers);
        let anonymous = crate::entity_ref::EntityRef::live("", "/app/feed/index");
        assert!(matches!(w.aim(&anonymous, &peers), Aim::Unusable(_)));
    }
}

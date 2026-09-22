//! **Feed window** — follow a publisher by peer id and read what they posted.
//!
//! The first product surface over `APP-CONVENTION-FEED`. The pieces beneath it
//! were built in order and each has its own module doc: [`crate::feed`] is the
//! codec, [`crate::feed_publish`] the emitter, [`crate::feed_read`] the walk,
//! [`crate::feed_fetch`] the poll-shaped adapter over it, and
//! [`crate::feed_follows`] the durable follow registry.
//!
//! ## What this window does NOT do, stated rather than implied
//!
//! - **It holds nothing durable of anybody else's.** The fetched feed is
//!   in-memory and per-window, so there is no offline read and D24 does not
//!   engage. [`crate::feed_fetch`]'s module doc carries the three-way currency
//!   analysis for when that changes.
//! - **It cannot post.** No verb publishes a feed (`A-35` is armed on the first
//!   one that does), so this reads other people and never writes an entry.
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

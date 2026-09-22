//! **A synchronous render loop over an asynchronous read** — the poll-shaped
//! adapter a window needs in order to show somebody else's feed.
//!
//! [`crate::feed_read`] walks a feed with `await`. `render_dom` cannot await:
//! it is called from the frame loop, it must return markup this frame, and a
//! future that is not ready yet has nothing to return. So the two are joined the
//! way this repo already joins them for a remote content site — a cache of
//! `Loading | Ready | Failed`, a `spawn_local` that fills it, and a repaint that
//! brings the next frame back to read it.
//!
//! **That shape is `HttpPollResolver`'s, deliberately.** It is the third
//! expression of *"a foreign artifact fetched behind a synchronous surface"* in
//! this crate and it should not be a third *design* — the backoff-with-retry,
//! the loading state and the repaint-on-completion are all its, because a
//! surface that invented its own would drift from the one that has had four
//! incidents ground into it.
//!
//! ## What is durable here: nothing, and that is a decision
//!
//! **This does not write a foreign feed into our tree.** The cache below is
//! in-memory and per-surface, so closing the window forgets it and D24 does not
//! engage — there is no durable copy of someone else's bytes, so there is
//! nothing to keep current. That is a real limitation and it is the honest
//! starting point: a first reader re-fetches on open, and it is not offline.
//!
//! **When it is made durable, the analysis is already done and it is not one
//! answer for the whole feed.** A feed splits cleanly in three, and only the
//! first is what D24 is about:
//!
//! | what | keyed by | mutable? |
//! |---|---|---|
//! | the head, and each index page | a **fixed key** (§4.2) | **yes** — this is where currency is owed |
//! | an entry | **its own content hash** ([`crate::feed::entry_key`]) | **no** — it cannot go stale, because a different entry is a different key |
//! | a signature | the **target's** hash, not its own | **yes** — a publisher can re-sign an entry, and the key does not move when they do |
//!
//! The middle row is the interesting one: a content-addressed cache entry is
//! self-verifying and permanently current, which is the same property
//! `cache_policy_rule` already relies on to mark `content/{aa}/{bb}/{hash}`
//! immutable. The third row is the one a tidy implementation gets wrong — a
//! signature *looks* content-addressed because its key is a hash, and it is not
//! its own.
//!
//! ## `A-38` is what a real origin will make visible
//!
//! [`crate::feed_read::read_feed`] is terminal on a missing index because our
//! foreign-tree consumer resolves a **key** and has no type-filtered query, and
//! §4.3 rule 6 says that should be a slow path rather than no path. Over a
//! directory that is a note in a doc comment. Over an origin that 404s
//! `app/feed/index` it is a person being told an author has no feed when the
//! author has posted for a year.

#![allow(dead_code)] // no window calls this yet; the gates are native

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use entity_entity::Entity;

use crate::content_site::http_poll::BinSource;
use crate::content_site::signed_fetch::{PinnedPublisher, SignedFetchError, SignedSession};
use crate::feed_read::{FeedSource, ReadEntry};

/// How long a failed read is shown before the next poll refetches.
///
/// **`HttpPollResolver`'s number, on purpose.** Two foreign-fetch surfaces with
/// two backoffs is a difference a user experiences and nobody chose.
pub const RETRY_BACKOFF_MS: f64 = 4000.0;

/// How many entries one poll walks back through.
///
/// `FEED-R12` forbids assuming a page size, so this is a count of **entries**
/// and never of pages — a publisher using pages of 500 and one using pages of 3
/// both give a reader the newest `LIMIT`.
pub const LIMIT: usize = 50;

// ---------------------------------------------------------------------------
// The pure decision
// ---------------------------------------------------------------------------

/// What this surface holds for one author, between frames.
#[derive(Debug, Clone, PartialEq)]
pub enum FeedState {
    /// A `spawn_local` is in flight.
    Loading,
    /// The walk completed. Held for the session.
    Ready(Vec<ReadEntry>),
    /// The walk failed. Rendered until `retry_at_ms`, then refetched — so a
    /// transient origin fault is recoverable rather than permanent, which is
    /// finding #1 of the content-site report arriving one convention over.
    Failed { error: String, retry_at_ms: f64 },
}

/// What a frame should do about an author it is being asked to render.
///
/// **Pure, and separate from the pump, for the reason every decision in this
/// repo that matters is:** the pump is `cfg(target_arch = "wasm32")` — it needs
/// `spawn_local` and a real clock — so a decision living inside it is reachable
/// only through Selenium. Split out, every combination of *(state × clock)* is
/// gated by `make test` on both arms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedStep {
    /// Serve what is held; start nothing.
    Serve,
    /// A fetch is already in flight; wait for it. **Distinct from `Serve`** —
    /// conflating them is how a surface renders an empty list as *"this author
    /// has posted nothing"* while the request is still open.
    Wait,
    /// Nothing is held and nothing is in flight: start a walk.
    Start,
    /// A held failure whose backoff has elapsed: start a walk again. **Its own
    /// word rather than `Start`**, because *"we have never asked"* and *"we
    /// asked and it went wrong and we are trying again"* are different things
    /// to say to somebody looking at a spinner.
    Retry,
}

/// The whole of the poll decision.
///
/// `now` is passed in rather than read, because `now_ms()` answers **0.0**
/// natively (there is no `performance` outside a browser) — so a decision that
/// read its own clock would be untestable in exactly the arm `make test` runs.
/// That is the same reason `resolver::grace_elapsed` takes one.
pub fn feed_step(state: Option<&FeedState>, now: f64) -> FeedStep {
    match state {
        None => FeedStep::Start,
        Some(FeedState::Loading) => FeedStep::Wait,
        Some(FeedState::Ready(_)) => FeedStep::Serve,
        Some(FeedState::Failed { retry_at_ms, .. }) => {
            if now < *retry_at_ms {
                FeedStep::Serve // show the error; do not storm the origin
            } else {
                FeedStep::Retry
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The real source
// ---------------------------------------------------------------------------

/// A [`FeedSource`] over a **published origin**, through the same signed-root
/// client a content site is read with: the manifest, the root signature, the
/// trie walk and the two-hop pointer/blob fetch.
///
/// Generic over [`BinSource`] rather than hardcoding `FetchBinSource`, for the
/// reason the whole reader was split out in the first place — a type that can
/// only be constructed in a browser is a type no `make test` can evaluate. The
/// browser passes `FetchBinSource`; a gate passes a directory.
///
/// **The `Rc`s are the async trait doing its job.** A boxed future cannot borrow
/// `self`, so everything it needs is cloned in — see [`FeedSource`]'s own note.
pub struct OriginFeedSource<B: BinSource + 'static> {
    session: Rc<SignedSession>,
    bin: Rc<B>,
}

impl<B: BinSource + 'static> OriginFeedSource<B> {
    /// Pin the author by **peer id**, which is what makes this verifiable
    /// without asking anybody for a key: a canonical Ed25519 peer id embeds its
    /// own public key, so the origin cannot substitute a tree.
    ///
    /// ## `origin` is a parameter, and the first cut had it hardcoded to `""`
    ///
    /// That is not a detail. An empty origin makes every fetch **relative to the
    /// page**, so a browser following a publisher would have asked its own
    /// deployment for that publisher's tree — right for a peer hosted at the
    /// app's own origin, and silently wrong for every other one, producing a
    /// 404 that reads as *"this publisher has no feed"*.
    ///
    /// The caller supplies it from
    /// [`origins::get_origin`](crate::content_site::origins::get_origin), which
    /// is the accessor that resolves supersession — **not** a registry read of
    /// its own. AP54 is exactly this: seven surfaces went through the chokepoint
    /// and the one that did not kept serving a retired publisher.
    pub fn new(origin: &str, author: &str, bin: Rc<B>) -> Result<Self, String> {
        // `None` here is the same fact `Unattributed::KeyNotInPeerId` carries
        // one layer up: a peer id that does not embed its own key (Ed448, or the
        // legacy SHA-256 form) cannot pin a publisher, so there is nothing to
        // verify the tree against. Refusing is right — reading an unpinned tree
        // would be trusting the origin, which is the one thing the signed root
        // exists to avoid.
        let pin = PinnedPublisher::from_peer_id(origin, author).ok_or_else(|| {
            format!("{author} does not carry its own key, so its tree cannot be verified")
        })?;
        Ok(Self { session: Rc::new(SignedSession::new(pin)), bin })
    }
}

impl<B: BinSource + 'static> FeedSource for OriginFeedSource<B> {
    fn get(
        &self,
        relative_key: String,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Entity>, String>>>> {
        let session = Rc::clone(&self.session);
        let bin = Rc::clone(&self.bin);
        Box::pin(async move {
            match session.resolve(bin.as_ref(), &relative_key).await {
                Ok(e) => Ok(Some(e)),
                // **`Absent` is `Ok(None)`, and that is the whole contract.**
                // §4.3 rule 6 makes an entry the index names but does not serve
                // a SHORT view, and `FEED-R4` makes a missing signature an
                // ordinary fact — both of which need *"it is not there"* to be
                // distinguishable from *"we could not look"*. Folding them would
                // turn a withdrawn post into an outage, and an outage into a
                // withdrawn post.
                Err(SignedFetchError::Absent) => Ok(None),
                Err(e) => Err(format!("{e:?}")),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// The pump
// ---------------------------------------------------------------------------

/// One surface's view of the feeds it is showing.
///
/// In-memory and per-surface; see the module doc on why nothing here is
/// durable yet.
pub struct FeedPoller {
    cache: Rc<RefCell<HashMap<String, FeedState>>>,
    repaint: crate::content_site::resolver::RepaintCell,
}

impl FeedPoller {
    pub fn new(repaint: crate::content_site::resolver::RepaintCell) -> Self {
        Self { cache: Rc::new(RefCell::new(HashMap::new())), repaint }
    }

    /// What to render for `author` this frame, starting a walk if one is owed.
    ///
    /// The immutable borrow is dropped before the mutable re-borrow, which is
    /// `HttpPollResolver::resolve`'s shape and not an accident: this cache is
    /// read during a render pass, and a `RefCell` double-borrow inside the frame
    /// loop is the panic that shows up as a frozen window.
    pub fn poll(&self, author: &str, origin: &str, now: f64) -> Option<FeedState> {
        let step = {
            let cache = self.cache.borrow();
            feed_step(cache.get(author), now)
        };
        match step {
            FeedStep::Serve | FeedStep::Wait => {}
            FeedStep::Start | FeedStep::Retry => {
                self.cache.borrow_mut().insert(author.to_string(), FeedState::Loading);
                self.spawn_walk(author.to_string(), origin.to_string());
            }
        }
        self.cache.borrow().get(author).cloned()
    }

    /// Drop what is held for `author`, so the next poll walks again.
    ///
    /// **Not a "refresh" that fetches** — it forgets, and the ordinary poll does
    /// the rest. One path starts a walk, which is what stops two of them racing
    /// to fill one slot.
    pub fn forget(&self, author: &str) {
        self.cache.borrow_mut().remove(author);
    }

    #[cfg(target_arch = "wasm32")]
    fn spawn_walk(&self, author: String, origin: String) {
        use crate::content_site::http_poll::FetchBinSource;
        let cache = self.cache.clone();
        let repaint = self.repaint.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let state = match OriginFeedSource::new(&origin, &author, Rc::new(FetchBinSource)) {
                Ok(src) => match crate::feed_read::read_feed(&src, &author, LIMIT).await {
                    Ok(entries) => FeedState::Ready(entries),
                    Err(e) => FeedState::Failed {
                        error: e.to_string(),
                        retry_at_ms: crate::dom::programs::now_ms() + RETRY_BACKOFF_MS,
                    },
                },
                Err(e) => FeedState::Failed {
                    error: e,
                    retry_at_ms: crate::dom::programs::now_ms() + RETRY_BACKOFF_MS,
                },
            };
            cache.borrow_mut().insert(author, state);
            // `RepaintCell` is an optional callback, not an object with a
            // method — the same two lines every other pump in this crate uses.
            if let Some(rp) = repaint.borrow().clone() {
                rp();
            }
        });
    }

    /// Native builds have no `spawn_local` and no origin to walk. The slot stays
    /// `Loading` for ever, which is correct: a native `poll` is not a fetch that
    /// failed, it is one that was never made.
    #[cfg(not(target_arch = "wasm32"))]
    fn spawn_walk(&self, _author: String, _origin: String) {}

    /// Seed a slot directly. **Gates only** — it is how a native test drives the
    /// states the pump would otherwise have to reach through a browser.
    #[cfg(test)]
    pub(crate) fn seed(&self, author: &str, state: FeedState) {
        self.cache.borrow_mut().insert(author.to_string(), state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: f64 = 1_000_000.0;
    const ORIGIN: &str = "http://publisher.example";

    fn failed(retry_at_ms: f64) -> FeedState {
        FeedState::Failed { error: "the origin hung up".into(), retry_at_ms }
    }

    /// **Every combination of (state × clock) has a step, and the four stay
    /// apart.**
    ///
    /// The two collapses that matter are named in [`FeedStep`]: `Wait` folded
    /// into `Serve` renders an in-flight walk as *"this author has posted
    /// nothing"*, and `Retry` folded into `Start` loses the only difference
    /// between a first look and a recovery.
    #[test]
    fn every_state_and_clock_has_its_own_step() {
        assert_eq!(feed_step(None, NOW), FeedStep::Start, "never asked");
        assert_eq!(feed_step(Some(&FeedState::Loading), NOW), FeedStep::Wait, "in flight");
        assert_eq!(
            feed_step(Some(&FeedState::Ready(Vec::new())), NOW),
            FeedStep::Serve,
            "an EMPTY feed is a real answer and must not be re-walked for ever"
        );

        // Inside the backoff: show the error, do not storm the origin.
        assert_eq!(feed_step(Some(&failed(NOW + 1.0)), NOW), FeedStep::Serve);
        // The boundary belongs to the retry — `now < retry_at` is the wait, so
        // reaching the instant is reaching the end of it.
        assert_eq!(feed_step(Some(&failed(NOW)), NOW), FeedStep::Retry);
        assert_eq!(feed_step(Some(&failed(NOW - 1.0)), NOW), FeedStep::Retry);
    }

    /// ⚠ **A native clock is 0.0, and a held failure must still recover.**
    ///
    /// `now_ms()` has no `performance` to read outside a browser and answers
    /// `0.0` — so a failure recorded natively carries `retry_at_ms` of
    /// `0.0 + RETRY_BACKOFF_MS`, and every subsequent poll also passes `0.0`,
    /// which is *before* it. The step is `Serve` for ever.
    ///
    /// **That is correct rather than a defect, and it is worth pinning so
    /// nobody "fixes" it into one:** natively there is no pump, so `Retry` would
    /// start a walk that cannot run and the surface would flip between two lies
    /// instead of holding one honest error. `resolver.rs` reaches the same
    /// conclusion for its unreachable grace in the same words — *"Native
    /// (now_ms()==0) never trips the grace"*.
    #[test]
    fn a_clockless_arm_holds_its_error_rather_than_retrying_into_a_pump_that_is_not_there() {
        let recorded_natively = failed(0.0 + RETRY_BACKOFF_MS);
        assert_eq!(feed_step(Some(&recorded_natively), 0.0), FeedStep::Serve);
    }

    /// The poller starts exactly one walk, and a second frame does not start
    /// another. **The property a `Wait` that reads as `Serve` destroys** — a
    /// surface polled every frame would open a request per frame.
    #[test]
    fn polling_twice_starts_one_walk() {
        let poller = FeedPoller::new(Default::default());
        assert_eq!(poller.poll("QmAuthor", ORIGIN, NOW), Some(FeedState::Loading), "the first poll starts it");
        assert_eq!(
            poller.poll("QmAuthor", ORIGIN, NOW),
            Some(FeedState::Loading),
            "and the second finds it in flight rather than starting a second"
        );
    }

    /// `forget` puts a surface back to never-having-asked, and the next poll
    /// walks. This is what a *Refresh* control is: one path starts a walk.
    #[test]
    fn forgetting_an_author_makes_the_next_poll_walk_again() {
        let poller = FeedPoller::new(Default::default());
        poller.seed("QmAuthor", FeedState::Ready(Vec::new()));
        assert!(matches!(poller.poll("QmAuthor", ORIGIN, NOW), Some(FeedState::Ready(_))));
        poller.forget("QmAuthor");
        assert_eq!(poller.poll("QmAuthor", ORIGIN, NOW), Some(FeedState::Loading));
    }

    /// Two authors do not share a slot. Obvious, and the kind of thing a
    /// single-slot first cut gets wrong and nobody notices until a second feed
    /// is on screen.
    #[test]
    fn each_author_has_their_own_slot() {
        let poller = FeedPoller::new(Default::default());
        poller.seed("QmA", FeedState::Ready(Vec::new()));
        assert_eq!(poller.poll("QmB", ORIGIN, NOW), Some(FeedState::Loading));
        assert!(matches!(poller.poll("QmA", ORIGIN, NOW), Some(FeedState::Ready(_))));
    }

    // -- the real source, against a real published origin -------------------

    /// **THE GATE FOR [`OriginFeedSource`]: a feed read the way the browser will
    /// read one.**
    ///
    /// Nothing here is a double. A real projector publishes and signs a tree on
    /// disk; `OriginFeedSource` pins the author **by peer id alone**, and the
    /// walk goes through the manifest, the root signature, the trie and the
    /// two-hop pointer/blob fetch before a single entry is decoded. The only
    /// thing the browser swaps is which [`BinSource`] carries the bytes.
    #[test]
    fn a_published_feed_reads_back_through_the_real_signed_origin_source() {
        use crate::feed_publish::tests::{published_origin, Origin};
        let (dir, author) = published_origin(5);

        let src = OriginFeedSource::new("", &author, Rc::new(Origin(dir.path().into())))
            .expect("a canonical peer id pins its own tree");
        let read = crate::feed_read::block_on(crate::feed_read::read_feed(&src, &author, LIMIT))
            .expect("the feed reads over the signed origin");

        assert_eq!(read.len(), 5);
        assert!(
            read.iter().all(|r| r.attribution == crate::feed_read::Attribution::Signed),
            "FEED-R2 survives the trie walk — the signatures came back too"
        );
    }

    /// **THE ANTI-VACUITY HALF, and the first version of it did not measure what
    /// its own name claimed.**
    ///
    /// Asking the origin for a peer it does not host returns a **404** — a real
    /// property, and a much weaker one than *"the signature refused a
    /// substituted tree"*, which is what the doc comment said. Measured, not
    /// assumed: the error was
    /// `NoIndex { detail: "manifest: origin served no such entity (HTTP 404)" }`.
    /// ***If nothing you opened contradicted you, you did not run a check.***
    ///
    /// So this **performs the substitution**: the author's whole published
    /// subtree is copied to the stranger's peer segment, so every path a pinned
    /// reader asks for resolves and serves real, well-formed, correctly-hashed
    /// entities. The only thing wrong with them is **whose they are** — which is
    /// exactly what a hostile origin can do and what a content hash cannot
    /// notice. The pin is the whole defence, and this is the only test here that
    /// makes it do any work.
    ///
    /// The refusal, measured: `Verify("peer_id mismatch: expected <stranger>,
    /// got <author>")`. Asserted **by reason** below rather than by `is_err()`,
    /// because this test has already been green once for the wrong one.
    #[test]
    fn a_substituted_tree_is_refused_even_though_every_byte_in_it_is_valid() {
        use crate::feed_publish::tests::{other_author_id, published_origin, Origin};
        let (dir, author) = published_origin(3);
        let stranger = other_author_id();

        // The control: served under its real author, this tree is a good feed.
        let honest = OriginFeedSource::new("", &author, Rc::new(Origin(dir.path().into()))).unwrap();
        assert_eq!(
            crate::feed_read::block_on(crate::feed_read::read_feed(&honest, &author, LIMIT))
                .map(|r| r.len()),
            Ok(3),
            "the control: these exact bytes ARE a readable feed"
        );

        // The substitution — the same bytes, under somebody else's name.
        copy_tree(&dir.path().join(&author), &dir.path().join(&stranger));

        let substituted =
            OriginFeedSource::new("", &stranger, Rc::new(Origin(dir.path().into())))
                .expect("the stranger's peer id is canonical — pinning is not the check");
        let out = crate::feed_read::block_on(crate::feed_read::read_feed(
            &substituted,
            &stranger,
            LIMIT,
        ));
        // **Assert the REASON, not `is_err()`.** The first version of this test
        // was green on a 404, which is how it came to exist — so a regression
        // that moved the paths and reinstated the 404 would look identical to a
        // working pin. Measured: the refusal is
        // `Verify("peer_id mismatch: expected <stranger>, got <author>")`.
        let detail = format!("{out:?}");
        assert!(out.is_err(), "a substituted tree must not be served: {detail}");
        assert!(
            detail.contains("mismatch"),
            "and it must be REFUSED by the pin, not merely missing — a 404 here \
             would mean the substitution never happened: {detail}"
        );
        assert!(
            !detail.contains("404"),
            "a 404 means the copy did not land and this gate measured nothing: {detail}"
        );
    }

    fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let dest = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &dest);
            } else {
                std::fs::copy(entry.path(), dest).unwrap();
            }
        }
    }

    /// A peer id that does not embed its own key is refused **at construction**,
    /// and the message says so rather than reporting a missing feed.
    ///
    /// Same distinction `Unattributed::KeyNotInPeerId` draws one layer up:
    /// *"we cannot verify this"* is not *"this author has nothing"*.
    #[test]
    fn an_unpinnable_author_is_refused_with_a_reason_and_not_an_empty_feed() {
        use crate::feed_publish::tests::{published_origin, Origin};
        let (dir, _) = published_origin(1);
        let Err(err) = OriginFeedSource::new("", "not-a-peer-id", Rc::new(Origin(dir.path().into())))
        else {
            panic!("an unpinnable author has no verifiable tree and must be refused")
        };
        assert!(err.contains("does not carry its own key"), "and it says why: {err}");
    }
}

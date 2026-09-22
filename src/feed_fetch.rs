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
use crate::dispatch_handle::DispatchHandle;
use crate::feed_read::{read_feed, FeedSource, ReadEntry};
use crate::feed_route::{reduce, Leg, Resolution, Route};

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
///
/// **The outcome is a whole [`Resolution`] rather than three states**, because
/// the route is a ladder: *served by the live leg*, *both legs answered and they
/// have nothing*, *one answered empty and the other could not be checked* and
/// *nothing could be read at all* are four facts, and splitting them across
/// enum variants here would be re-collapsing what [`feed_route::reduce`] exists
/// to keep apart.
#[derive(Debug, Clone, PartialEq)]
pub enum FeedState {
    /// A `spawn_local` is in flight.
    Loading,
    /// The route resolved. Held for the session.
    Done {
        resolution: Resolution<ReadEntry>,
        /// When to walk again. **`Some` only for a total failure** — an author
        /// who answered and has nothing is not re-walked on a timer, and a
        /// transient fault is recoverable rather than permanent, which is
        /// finding #1 of the content-site report arriving one convention over.
        retry_at_ms: Option<f64>,
    },
}

impl FeedState {
    /// Record a resolution, arming the backoff only if nothing could be read.
    ///
    /// **One constructor, so the *"which outcomes retry"* rule has one
    /// expression.** A call site deciding it for itself is C15's drift with a
    /// user-visible symptom: a surface that re-walks a publisher who simply has
    /// no posts hammers their origin for ever.
    pub fn done(resolution: Resolution<ReadEntry>, now: f64) -> Self {
        let retry_at_ms = matches!(resolution, Resolution::Failed { .. })
            .then_some(now + RETRY_BACKOFF_MS);
        FeedState::Done { resolution, retry_at_ms }
    }
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
        Some(FeedState::Done { retry_at_ms: None, .. }) => FeedStep::Serve,
        Some(FeedState::Done { retry_at_ms: Some(at), .. }) => {
            if now < *at {
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

/// A [`MirrorSource`](crate::feed_mirror::MirrorSource) over a **gatherer's
/// published origin** — and it is deliberately NOT [`OriginFeedSource`] with a
/// wider key, because the two halves of a mirror stand on different evidence.
///
/// ## ⭐ Two legs, and the asymmetry is the trust argument
///
/// | what | under | verified by |
/// |---|---|---|
/// | the §6 mirror **record** | the gatherer | the gatherer's **signed root** — it is their own statement, bound in their own trie |
/// | every **carried entry** | its own author | the **two-hop content address**, plus `read_mirror`'s check against the pin the record names, plus the author's detached signature |
///
/// `RootProjector::record` skips a foreign peer — a root that named another
/// peer's keys would be asserting something it has no standing to assert — so
/// the carried bodies are *by construction* outside every signed root at that
/// origin. A source that tried to resolve them through the gatherer's
/// `SignedSession` would get `Absent` for every single one and report a healthy
/// mirror as empty; a source that resolved the **record** without the session
/// would be taking a stranger's word for what they gathered.
///
/// ⚠ **This is what a `(peer, key)` map in a test could never show.** The
/// gates for §6 used exactly that double, which answers both legs identically
/// and verifies neither — *a test double that supplies the thing you forgot to
/// ask for cannot notice that you forgot*, the same shape as
/// `OriginFeedSource`'s first cut taking an empty origin.
///
/// **Absence is `Ok(None)` on both legs**, for `OriginFeedSource`'s reason:
/// `read_mirror` needs *the gatherer has not gathered this* (`NoMirror`) and
/// *an entry this record names is not served* (a short view, §1.3) to be
/// distinguishable from *we could not look*.
pub struct OriginMirrorSource<B: BinSource + 'static> {
    origin: String,
    gatherer: String,
    session: Rc<SignedSession>,
    bin: Rc<B>,
}

impl<B: BinSource + 'static> OriginMirrorSource<B> {
    /// Pin `gatherer` at `origin`. Same refusal as [`OriginFeedSource::new`] and
    /// for the same reason — a peer id that does not embed its own key cannot
    /// pin anybody, and reading an unpinned tree is trusting the origin.
    pub fn new(origin: &str, gatherer: &str, bin: Rc<B>) -> Result<Self, String> {
        let pin = PinnedPublisher::from_peer_id(origin, gatherer).ok_or_else(|| {
            format!("{gatherer} does not carry its own key, so its tree cannot be verified")
        })?;
        Ok(Self {
            origin: origin.to_string(),
            gatherer: gatherer.to_string(),
            session: Rc::new(SignedSession::new(pin)),
            bin,
        })
    }
}

impl<B: BinSource + 'static> crate::feed_mirror::MirrorSource for OriginMirrorSource<B> {
    fn get(
        &self,
        peer: String,
        relative_key: String,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Entity>, String>>>> {
        let session = Rc::clone(&self.session);
        let bin = Rc::clone(&self.bin);
        let origin = self.origin.clone();
        let is_gatherer = peer == self.gatherer;
        Box::pin(async move {
            if is_gatherer {
                return match session.resolve(bin.as_ref(), &relative_key).await {
                    Ok(e) => Ok(Some(e)),
                    Err(SignedFetchError::Absent) => Ok(None),
                    Err(e) => Err(format!("{e:?}")),
                };
            }
            // The carried leg. Hop 1 names the bytes, hop 2 verifies them
            // against that name — which is all the evidence available here and
            // is **not** all the evidence the entry gets: `read_mirror`
            // re-checks the served hash against the pin the record declared
            // (§6.1 rule 2, *omit but never substitute*) and `finish_entry`
            // attributes it from the author's own detached signature.
            // **D24, answered rather than ratcheted past.** `foreign-cache-lint`
            // counts these two because a direct fetcher is how *"only if
            // absent"* gets expressed. It cannot be expressed here: this module
            // holds **nothing durable** (see its own doc — the cache is
            // in-memory and per-surface, which is why D24 does not engage for
            // the feed reader either), so there is no copy whose staleness could
            // be served. `ensure_current` is also the wrong door by construction
            // — it writes into `/{me}/{foreign}/…` in *our* tree, and what is
            // being read here is a stranger's segment at a stranger's origin.
            use crate::content_site::http_poll::{fetch_content, fetch_pointer, tree_bin_url};
            let url = tree_bin_url(&origin, &peer, &relative_key);
            let hash = match fetch_pointer(bin.as_ref(), &url).await {
                Ok(h) => h,
                Err(e) if e.is_terminal() => return Ok(None),
                Err(e) => return Err(format!("{e}")),
            };
            match fetch_content(bin.as_ref(), &origin, &hash).await {
                Ok(e) => Ok(Some(e)),
                Err(e) if e.is_terminal() => Ok(None),
                Err(e) => Err(format!("{e}")),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Walking the ladder
// ---------------------------------------------------------------------------

/// How one leg is read. **Two arms, because the set layer is where a mirror
/// differs from a feed and that difference is normative.**
///
/// `DX-R2` makes the **entry** layer one code path and `SYSTEM-DATA-EXCHANGE`
/// §1.2 says in as many words that the **set** layer is not: an author's own set
/// is an index head plus key-addressed pages, a gatherer's is one record naming
/// pins, *"and they are allowed to be"* different shapes. So the walk branches
/// exactly once, here, and both arms hand their rows to the same `finish_entry`
/// underneath — which is where the closure property is checkable and where it
/// holds.
pub enum LegReader {
    /// The author's own tree, live or published — walked with `read_feed`.
    Author(Box<dyn FeedSource>),
    /// A gatherer's tree — walked with `read_mirror`.
    ///
    /// The source is keyed by `(peer, key)` rather than scoped to one peer,
    /// because one gatherer's origin serves **several authors'** segments: that
    /// is what a mirror is.
    Mirror { gatherer: String, src: Box<dyn crate::feed_mirror::MirrorSource> },
}

/// Try each leg of a route in order and reduce what they said.
///
/// **`make_source` is the only thing that differs between a browser and a
/// gate**, which is what makes the sequencing — the half with the interesting
/// mistakes in it — reachable from `make test`. The browser hands back a
/// `FetchBinSource`-backed origin source and a `DispatchHandle`-backed live one;
/// a gate hands back a directory and a map.
///
/// A source that cannot be *built* is a failed leg, not a panic and not a
/// skipped one: *this author's peer id cannot pin a tree* is a reason the reader
/// deserves, and dropping the leg silently would make a two-leg route report as
/// a one-leg one.
pub async fn walk_route<F>(
    author: &str,
    route: &Route,
    mut make_source: F,
) -> Resolution<ReadEntry>
where
    F: FnMut(&Leg) -> Result<LegReader, String>,
{
    // §6.0's live subject: *this view is of that author's feed.* Built here
    // rather than passed in, because every mirror leg of this walk is a mirror
    // **of this author** and a caller supplying it is a caller that can get it
    // wrong.
    let subject = crate::feed::MirrorSubject::timeline(author);
    let mut results = Vec::with_capacity(route.legs.len());
    for leg in &route.legs {
        let result = match make_source(leg) {
            Err(why) => Err(why),
            Ok(LegReader::Author(src)) => {
                read_feed(src.as_ref(), author, LIMIT).await.map_err(|e| e.to_string())
            }
            Ok(LegReader::Mirror { gatherer, src }) => {
                crate::feed_mirror::read_mirror(src.as_ref(), &gatherer, &subject, LIMIT)
                    .await
                    .map_err(|e| e.to_string())
            }
        };
        // **A leg that carried posts ENDS the walk.** That is what makes this a
        // priority list rather than a fan-out: without it, a reader served by
        // the first leg would still fetch the second every time — a whole
        // signed-root walk of somebody's CDN for an answer already in hand.
        //
        // Note which condition stops it. **Serving stops it; answering does
        // not** — an empty leg falls through, for the reason `reduce`'s own doc
        // gives, and the two must not be collapsed into *"the leg answered"*.
        let served = matches!(&result, Ok(entries) if !entries.is_empty());
        results.push((leg.clone(), result));
        if served {
            break;
        }
    }
    reduce(results)
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

    /// Fill the late-bound repaint handle from the render path.
    ///
    /// ⚠ **A poller whose cell is empty walks, lands, and tells nobody.** The
    /// handle is not available at construction — it comes from `DomCtx` — so the
    /// cell starts `None` and the surface must fill it, exactly as
    /// `ContentSiteModel::set_repaint` does. Until 2026-09-12 the Feed window
    /// never did: the walk completed, every byte arrived, and the panel stayed on
    /// *"Reading this feed…"* until some unrelated write marked the window dirty.
    /// Found by the first gate that ever waited for a feed to LAND in a browser;
    /// every earlier gate had a click of its own coming.
    ///
    /// And what the handle must do is **mark dirty AND request a frame** — a
    /// bare repaint fires a frame in which this window is still clean, so its
    /// section is not rebuilt and the completed walk is never rendered. That is
    /// `content_site`'s own hard-won arrangement and its doc carries the long
    /// version.
    pub fn set_repaint(&self, repaint: crate::window::RepaintFn) {
        *self.repaint.borrow_mut() = Some(repaint);
    }

    /// What to render for `author` this frame, starting a walk if one is owed.
    ///
    /// The immutable borrow is dropped before the mutable re-borrow, which is
    /// `HttpPollResolver::resolve`'s shape and not an accident: this cache is
    /// read during a render pass, and a `RefCell` double-borrow inside the frame
    /// loop is the panic that shows up as a frozen window.
    /// `route` is built fresh by the caller each frame
    /// ([`feed_route::plan`]), never cached here: whether we are connected to an
    /// author changes under us, and a retained route is AP41's retention defect
    /// wearing a routing costume — a window that cached *"not connected"* at
    /// construction would keep reading a published tree after the author came
    /// online, for as long as it stayed open.
    /// `dispatch` is a **parameter and not a field**, deliberately (AP44). A
    /// handle stored on the poller and set by a `set_dispatch` call is a step
    /// the next author has to remember, and forgetting it makes the live leg
    /// fail silently on a route that asked for it. As an argument the compiler
    /// asks the question, and a caller passing `None` is making a statement.
    pub fn poll(
        &self,
        author: &str,
        route: &Route,
        dispatch: Option<&DispatchHandle>,
        now: f64,
    ) -> Option<FeedState> {
        // **Nothing to ask is not a walk that failed**, and it must not consume
        // a cache slot: the surface says something different for it, and
        // recording it would make the answer stick after a connection arrives.
        if route.is_unreachable() {
            return Some(FeedState::Done {
                resolution: Resolution::Unreachable,
                retry_at_ms: None,
            });
        }
        let step = {
            let cache = self.cache.borrow();
            feed_step(cache.get(author), now)
        };
        match step {
            FeedStep::Serve | FeedStep::Wait => {}
            FeedStep::Start | FeedStep::Retry => {
                self.cache.borrow_mut().insert(author.to_string(), FeedState::Loading);
                self.spawn_walk(author.to_string(), route.clone(), dispatch.cloned());
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

    /// Forget every walk this session holds.
    ///
    /// **For a change to the ROUTE rather than to an author.** A held
    /// [`FeedState`] is an answer to the route that was in force when it was
    /// walked; adding or removing a gatherer changes the route for *every*
    /// author at once, and `poll` would go on serving the old answers without
    /// re-walking — so somebody who added a gatherer because an author would not
    /// load would see the identical panel and conclude their change did nothing.
    pub fn forget_all(&self) {
        self.cache.borrow_mut().clear();
    }

    #[cfg(target_arch = "wasm32")]
    fn spawn_walk(&self, author: String, route: Route, dispatch: Option<DispatchHandle>) {
        use crate::content_site::http_poll::FetchBinSource;
        let cache = self.cache.clone();
        let repaint = self.repaint.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let subject = author.clone();
            let resolution = walk_route(&author, &route, |leg| match leg {
                Leg::Published(origin) => {
                    OriginFeedSource::new(origin, &subject, Rc::new(FetchBinSource))
                        .map(|s| LegReader::Author(Box::new(s)))
                }
                Leg::Live => dispatch
                    .clone()
                    .map(|d| {
                        LegReader::Author(Box::new(crate::feed_peer::PeerFeedSource::new(
                            d,
                            subject.clone(),
                        )))
                    })
                    // The route said live and the caller supplied no handle:
                    // an unrouted peer. **A reason, never a silently dropped
                    // leg** — see `walk_route`.
                    .ok_or_else(|| format!("no dispatch handle for {subject}")),
                // **Wired through [`OriginMirrorSource`], NOT through
                // `OriginFeedSource` with a wider key.** A gatherer's carried
                // bodies are deliberately outside every signed root at its origin
                // (`RootProjector::record` skips a foreign peer, which is §2.2's
                // whole reason for detached signatures), so a source resolving
                // them through the gatherer's `SignedSession` would get `Absent`
                // for every one and report a healthy mirror as empty. The record
                // goes through the session; the entries go by pin and hash. That
                // asymmetry is the trust argument and it is why this leg took its
                // own type rather than a `Box::new` on the line above.
                Leg::Mirror { gatherer, origin } => {
                    OriginMirrorSource::new(origin, gatherer, Rc::new(FetchBinSource)).map(|s| {
                        LegReader::Mirror { gatherer: gatherer.clone(), src: Box::new(s) }
                    })
                }
            })
            .await;
            // **Every resolution reports, including the ordinary one** — a
            // surface that logs only its failures cannot be told from one that
            // never ran (`window_hydration::report`'s lesson).
            tracing::info!("{}", crate::feed_route::describe(&author, &resolution));
            let state = FeedState::done(resolution, crate::dom::programs::now_ms());
            cache.borrow_mut().insert(author, state);
            // `RepaintCell` is an optional callback, not an object with a
            // method — the same two lines every other pump in this crate uses.
            if let Some(rp) = repaint.borrow().clone() {
                rp();
            }
        });
    }

    /// Native builds have no `spawn_local`. The slot stays `Loading` for ever,
    /// which is correct: a native `poll` is not a fetch that failed, it is one
    /// that was never made.
    ///
    /// **The walk itself is not wasm-only** — [`walk_route`] is plain async and
    /// is gated natively. What is missing here is an executor, not the logic.
    #[cfg(not(target_arch = "wasm32"))]
    fn spawn_walk(&self, _author: String, _route: Route, _dispatch: Option<DispatchHandle>) {}

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

    /// A one-leg published route — what every one of these gates had before the
    /// ladder existed, so they keep measuring the same property.
    fn published() -> Route {
        crate::feed_route::plan(false, Some(ORIGIN), &[], crate::feed_route::Preference::Unstated)
    }

    fn failed(retry_at_ms: f64) -> FeedState {
        FeedState::Done {
            resolution: Resolution::Failed {
                attempts: vec![crate::feed_route::Attempt {
                    leg: "published",
                    outcome: crate::feed_route::AttemptOutcome::Failed("the origin hung up".into()),
                }],
            },
            retry_at_ms: Some(retry_at_ms),
        }
    }

    fn served_nothing() -> FeedState {
        FeedState::Done {
            resolution: Resolution::NoPosts {
                attempts: vec![crate::feed_route::Attempt {
                    leg: "published",
                    outcome: crate::feed_route::AttemptOutcome::Empty,
                }],
            },
            retry_at_ms: None,
        }
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
            feed_step(Some(&served_nothing()), NOW),
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
        assert_eq!(poller.poll("QmAuthor", &published(), None, NOW), Some(FeedState::Loading), "the first poll starts it");
        assert_eq!(
            poller.poll("QmAuthor", &published(), None, NOW),
            Some(FeedState::Loading),
            "and the second finds it in flight rather than starting a second"
        );
    }

    /// `forget` puts a surface back to never-having-asked, and the next poll
    /// walks. This is what a *Refresh* control is: one path starts a walk.
    #[test]
    fn forgetting_an_author_makes_the_next_poll_walk_again() {
        let poller = FeedPoller::new(Default::default());
        poller.seed("QmAuthor", served_nothing());
        assert!(matches!(poller.poll("QmAuthor", &published(), None, NOW), Some(FeedState::Done { .. })));
        poller.forget("QmAuthor");
        assert_eq!(poller.poll("QmAuthor", &published(), None, NOW), Some(FeedState::Loading));
    }

    /// Two authors do not share a slot. Obvious, and the kind of thing a
    /// single-slot first cut gets wrong and nobody notices until a second feed
    /// is on screen.
    #[test]
    fn each_author_has_their_own_slot() {
        let poller = FeedPoller::new(Default::default());
        poller.seed("QmA", served_nothing());
        assert_eq!(poller.poll("QmB", &published(), None, NOW), Some(FeedState::Loading));
        assert!(matches!(poller.poll("QmA", &published(), None, NOW), Some(FeedState::Done { .. })));
    }

    // -- walking the ladder -------------------------------------------------
    //
    // These are the sequencing gates. The *ordering* is `feed_route::plan`'s and
    // the *verdict* is `reduce`'s, both pure and gated in their own module; what
    // is only reachable here is what happens BETWEEN the legs — which of them
    // gets consulted, in what order, and what stops the walk.

    use crate::feed_read::Tree;
    use crate::feed_route::Preference;

    /// A route with both legs, live first — what `plan` produces for an author
    /// who is connected and also published.
    fn both_legs() -> Route {
        crate::feed_route::plan(true, Some(ORIGIN), &[], Preference::Unstated)
    }

    /// Drive a two-leg walk over map-backed trees, recording which legs were
    /// actually consulted.
    fn walk(
        author: &str,
        route: &Route,
        live: Tree,
        published: Tree,
    ) -> (Resolution<ReadEntry>, Vec<&'static str>) {
        let consulted = std::cell::RefCell::new(Vec::new());
        let resolution = crate::feed_read::block_on(walk_route(author, route, |leg| {
            consulted.borrow_mut().push(leg.name());
            Ok(match leg {
                Leg::Live => LegReader::Author(Box::new(live.clone())),
                Leg::Published(_) => LegReader::Author(Box::new(published.clone())),
                Leg::Mirror { .. } => panic!("this harness has no mirror leg"),
            })
        }));
        (resolution, consulted.into_inner())
    }

    /// ⭐ **The ladder's reason for existing, end to end.**
    ///
    /// A publisher who serves from a CDN *because they cannot carry the load*
    /// has a live tree with no feed in it. The first leg answers honestly and
    /// carries nothing; stopping there reports *"this author has posted
    /// nothing"* to somebody one hop from their whole archive.
    #[test]
    fn a_live_author_with_nothing_in_their_tree_falls_through_to_what_they_published() {
        let (published, author, _) = crate::feed_publish::tests::published_tree(3);
        let (resolution, consulted) = walk(&author, &both_legs(), Tree::default(), published);
        match resolution {
            Resolution::Served { leg, entries } => {
                assert_eq!(leg.name(), "published");
                assert_eq!(entries.len(), 3);
            }
            other => panic!("the empty live leg swallowed the feed: {other:?}"),
        }
        assert_eq!(consulted, vec!["live", "published"], "and both were asked");
    }

    /// **Serving stops the walk.** Without this the reader fetches a whole
    /// signed-root tree off somebody's CDN for an answer it already has.
    ///
    /// Note what the assertion is: not *"the result came from live"* — which
    /// `reduce` guarantees on its own and which would pass with no
    /// short-circuit at all — but *"the second leg was never consulted"*.
    #[test]
    fn a_leg_that_serves_stops_the_ladder_and_the_next_one_is_never_asked() {
        let (live, author, _) = crate::feed_publish::tests::published_tree(2);
        let (other, _, _) = crate::feed_publish::tests::published_tree(5);
        let (resolution, consulted) = walk(&author, &both_legs(), live, other);
        assert!(matches!(resolution, Resolution::Served { leg: Leg::Live, .. }));
        assert_eq!(consulted, vec!["live"], "the published origin was fetched anyway");
    }

    /// A source that cannot be built is a **failed leg**, not a silently
    /// dropped one — an author whose peer id cannot pin a tree is a reason a
    /// reader deserves, and a dropped leg makes a two-leg route report as a
    /// one-leg one.
    #[test]
    fn a_leg_whose_source_cannot_be_built_is_a_failure_with_a_reason() {
        let route = both_legs();
        let resolution: Resolution<ReadEntry> =
            crate::feed_read::block_on(walk_route("QmAuthor", &route, |leg| match leg {
                Leg::Live => Err("no dispatch handle".to_string()),
                Leg::Published(_) => Err("that peer id carries no key".to_string()),
                Leg::Mirror { .. } => Err("this harness has no mirror leg".to_string()),
            }));
        match resolution {
            Resolution::Failed { attempts } => {
                assert_eq!(attempts.len(), 2, "both legs are in the report");
                let rendered = format!("{attempts:?}");
                assert!(rendered.contains("no dispatch handle"), "{rendered}");
                assert!(rendered.contains("carries no key"), "{rendered}");
            }
            other => panic!("{other:?}"),
        }
    }

    /// ⭐⭐ **An author nobody can reach, read through somebody who gathered
    /// them — and every entry still attributed to the AUTHOR.**
    ///
    /// This is the leg's reason for existing, and the assertions are in the order
    /// that makes them mean something:
    ///
    /// 1. the author's own legs are **consulted and fail** — so the mirror is
    ///    genuinely the leg that answered, not a leg that happened to be first;
    /// 2. the entries come back, **byte-identical**, through the same
    ///    `finish_entry` a direct read uses (`DX-R2`);
    /// 3. attribution names the **author**, not the gatherer (`DX-R13` / §6.1
    ///    rule 3) — the one assertion that separates a working mirror from a
    ///    plausible forgery, since the bytes check out either way.
    #[test]
    fn an_unreachable_author_is_still_readable_through_a_peer_who_gathered_them() {
        use crate::feed_mirror::{plan_mirror, MirrorSource};
        use std::collections::BTreeMap;

        let (published, author, _) = crate::feed_publish::tests::published_tree(3);
        let rows = crate::feed_read::block_on(crate::feed_read::read_feed(&published, &author, 100))
            .expect("the author's own feed reads");
        let gatherer = "2AGathererWhoRepublishesThem";
        let plan = plan_mirror(
            gatherer,
            &crate::feed::MirrorSubject::timeline(&author),
            &rows,
            &[],
            1_757_000_111,
        )
        .expect("the gather plans");

        /// A gatherer's whole origin: `(peer, key) -> entity`, since one mirror
        /// serves several authors' segments.
        #[derive(Default, Clone)]
        struct GathererOrigin(BTreeMap<(String, String), Entity>);
        impl MirrorSource for GathererOrigin {
            fn get(
                &self,
                peer: String,
                relative_key: String,
            ) -> Pin<Box<dyn Future<Output = Result<Option<Entity>, String>>>> {
                Box::pin(std::future::ready(Ok(self.0.get(&(peer, relative_key)).cloned())))
            }
        }

        let mut origin = GathererOrigin::default();
        for c in &plan.carried {
            origin.0.insert((c.peer.clone(), c.key.clone()), c.entity.clone());
        }
        origin.0.insert(
            (gatherer.to_string(), crate::feed::MirrorSubject::timeline(&author).key()),
            plan.record.to_entity().unwrap(),
        );

        // Not connected, no origin registered — and one gatherer we know of.
        let route = crate::feed_route::plan(
            false,
            None,
            &[crate::feed_route::Gatherer {
                peer_id: gatherer.to_string(),
                origin: "https://gatherer.example".into(),
            }],
            Preference::Unstated,
        );
        assert_eq!(route.legs.len(), 1, "the fixture's route is not the one under test");

        let consulted = std::cell::RefCell::new(Vec::new());
        let resolution = crate::feed_read::block_on(walk_route(&author, &route, |leg| {
            consulted.borrow_mut().push(leg.name());
            match leg {
                Leg::Mirror { gatherer, .. } => Ok(LegReader::Mirror {
                    gatherer: gatherer.clone(),
                    src: Box::new(origin.clone()),
                }),
                other => panic!("the route grew a leg: {other}"),
            }
        }));

        assert_eq!(consulted.into_inner(), vec!["mirror"]);
        match resolution {
            Resolution::Served { leg, entries } => {
                assert_eq!(leg.name(), "mirror", "the report named the wrong leg");
                assert_eq!(
                    matches!(&leg, Leg::Mirror { gatherer: g, .. } if g == gatherer),
                    true,
                    "the resolution must name WHICH gatherer served — it is the one \
                     place §6.1 rule 3 lets a surface say so"
                );
                assert_eq!(entries.len(), 3);
                for (before, after) in rows.iter().zip(entries.iter()) {
                    assert_eq!(after.hash, before.hash, "a hash moved through the mirror leg");
                    assert_eq!(
                        after.attribution,
                        crate::feed_read::Attribution::Signed,
                        "a mirrored entry lost its author's signature"
                    );
                    assert_eq!(
                        after.entry.author, author,
                        "an entry read through a mirror was attributed to the GATHERER"
                    );
                }
            }
            other => panic!("the mirror leg did not serve: {other:?}"),
        }
    }

    /// An author who is genuinely silent everywhere. **The one case where
    /// *"they have posted nothing"* is the true sentence** — and it is reached
    /// only after every leg answered.
    #[test]
    fn an_author_with_nothing_on_either_leg_is_no_posts_and_both_were_asked() {
        let (resolution, consulted) =
            walk("QmAuthor", &both_legs(), Tree::default(), Tree::default());
        // Neither tree has an index, so both legs report a read failure rather
        // than an empty feed — which is the honest answer here and is NOT
        // `NoPosts`: we could not read either one.
        assert!(matches!(resolution, Resolution::Failed { .. }), "{resolution:?}");
        assert_eq!(consulted, vec!["live", "published"]);
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

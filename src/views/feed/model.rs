//! Feed window model — **the reasoning; the catalog holds the wording.**
//!
//! Native and pure apart from the poller, which is why every decision this
//! surface makes is gated by `make test` rather than only through Selenium:
//! which key an attribution verdict renders under, what a press of Follow said,
//! and the difference between *"nobody selected"*, *"still loading"* and
//! *"this publisher has posted nothing"*.
//!
//! ## Session state, on purpose
//!
//! The **selection** (whose feed the panel is showing) and the **notice** (what
//! the last Follow press said) live in this struct and are not persisted. Who
//! you *follow* is durable and app-scoped ([`crate::feed_follows`]); which of
//! them you happen to be looking at is not a fact about the profile, and a
//! notice that survived a reload would be a stale answer to a question nobody
//! asked twice.
//!
//! **So this window never touches `window_state_path`**, which keeps it clear of
//! AP42's reused-slot hazard and out of the hydration census by construction
//! rather than by a row.
//!
//! ## The follow list is read per render (AP41)
//!
//! [`crate::feed_follows::list`] is a synchronous `tree_listing`, so it answers
//! from the per-prefix mirror. **Never cached here.** The window subscribes the
//! follows prefix, so a write marks it dirty and the next frame re-reads — the
//! `SettingsModel` shape, read-per-call and self-healing.

use crate::feed_follows::{self, FollowOutcome};
use crate::feed_gatherers::{self, AddOutcome};
use crate::feed_fetch::{FeedPoller, FeedState};
use crate::feed_read::{Attribution, Unattributed};
use crate::feed_route::{self, Resolution};
use crate::peers::Peers;
use crate::window::WindowId;

use super::output::{
    ComposeNotice, EntryRow, FeedOutput, FeedPanel, FollowRow, GathererRow, Notice, OwnPostRow, Via,
};

pub struct FeedModel {
    window_id: WindowId,
    poller: FeedPoller,
    selected: Option<String>,
    notice: Option<Notice>,
    /// What the last compose act was. **Session-only** — see
    /// [`super::output::FeedOutput::compose_notice`].
    compose_notice: Option<ComposeNotice>,
    /// Whether the *Manage* section is open. **Session-only, and deliberately
    /// not persisted**: which sections a person has expanded is a view state,
    /// not a fact about the profile, and writing it down would put a UI
    /// preference in the durable tree for `D25` to then have to answer
    /// ownership questions about. Same reasoning as the panel selection.
    ///
    /// Held in the MODEL rather than a `<details>` — `components::disclosure`
    /// re-renders closed on every repaint, and this section contains two text
    /// inputs somebody is typing into.
    manage_open: bool,
}

impl FeedModel {
    pub fn new(window_id: WindowId, repaint: crate::content_site::resolver::RepaintCell) -> Self {
        Self {
            window_id,
            poller: FeedPoller::new(repaint),
            selected: None,
            notice: None,
            compose_notice: None,
            manage_open: false,
        }
    }

    /// Hand the poller the repaint handle the render path owns — see
    /// [`crate::feed_fetch::FeedPoller::set_repaint`] for why an unfilled cell
    /// is a feed that loads forever.
    pub fn set_repaint(&self, repaint: crate::window::RepaintFn) {
        self.poller.set_repaint(repaint);
    }

    /// Expand or collapse *Manage* — the follow-by-id box, the follow list and
    /// the gatherers. Session-only; see the field.
    pub fn toggle_manage(&mut self) {
        self.manage_open = !self.manage_open;
    }

    /// Show this peer's feed. Selecting somebody **clears the notice** — it was
    /// about a different act, and a refusal left on screen beside a feed that
    /// loaded fine reads as being about the feed.
    pub fn select(&mut self, peer_id: &str) {
        self.selected = Some(peer_id.to_string());
        self.notice = None;
    }

    /// Follow a peer, and remember what to say about it.
    ///
    /// **A successful follow selects them**, because the reason somebody typed a
    /// peer id is to read that peer — making them press a second time to see
    /// anything is the surface asking them to repeat themselves.
    pub fn follow(&mut self, peers: &Peers, our_peer_id: &str, typed: &str, now: u64) {
        let subject = typed.trim();
        let outcome = feed_follows::follow(peers, our_peer_id, subject, now);
        self.notice = Some(notice_for(outcome));
        if matches!(outcome, FollowOutcome::Followed | FollowOutcome::AlreadyFollowing) {
            self.selected = Some(subject.to_string());
        }
    }

    /// Stop following, and stop showing them if they were on screen.
    ///
    /// **Also forgets the fetched feed.** Leaving it in the poller would keep a
    /// stranger's posts in memory after the person said they did not want them,
    /// and would serve them instantly if the same peer were re-followed — which
    /// looks like a cache and is really a failure to forget.
    pub fn unfollow(&mut self, peers: &Peers, our_peer_id: &str, subject: &str) {
        feed_follows::unfollow(peers, our_peer_id, subject);
        self.poller.forget(subject);
        if self.selected.as_deref() == Some(subject) {
            self.selected = None;
        }
        self.notice = None;
    }

    /// Read other authors through this peer from now on.
    ///
    /// ⭐ **Adding a gatherer forgets every walk this session has cached**, and
    /// that is the whole reason the method is not two lines. A route is planned
    /// per render from the live gatherer list, so a *new* gatherer changes the
    /// route for **every** author — but a held `FeedState` is an answer to the
    /// old route, and the poller serves it without re-walking. Somebody who adds
    /// a gatherer precisely because an author would not load would otherwise
    /// press the button, see the identical *"could not read"* panel, and have no
    /// way to know their change took effect. It does not select anybody: naming
    /// a source is not asking to read them.
    pub fn add_gatherer(&mut self, peers: &Peers, our_peer_id: &str, typed: &str, now: u64) {
        let outcome = feed_gatherers::add(peers, our_peer_id, typed.trim(), now);
        self.notice = Some(gatherer_notice(outcome));
        if matches!(outcome, AddOutcome::Added) {
            self.poller.forget_all();
        }
    }

    /// Stop reading other authors through this peer.
    ///
    /// **Also forgets every walk**, for the mirror image of the reason above: a
    /// panel still showing entries served *by* the gatherer somebody just
    /// removed is the surface disagreeing with the list beside it.
    pub fn remove_gatherer(&mut self, peers: &Peers, our_peer_id: &str, gatherer: &str) {
        feed_gatherers::remove(peers, our_peer_id, gatherer);
        self.poller.forget_all();
        self.notice = None;
    }

    /// **Post.** Mint an entry into the bound peer's own tree, sign it, and bind
    /// both.
    ///
    /// This is the whole publish on the live road — no origin, no root, no
    /// network. See [`crate::feed_compose`] for why that is the act and not a
    /// reduced version of one.
    ///
    /// ⚠ **The poller is NOT forgotten here, deliberately**, which is the
    /// opposite of [`Self::add_gatherer`]'s rule and worth stating because the
    /// two look alike. A gatherer changes the *route* for every author, so every
    /// held walk is an answer to a question that no longer applies. A post
    /// changes *our own tree*, which no held walk is an answer about — and
    /// dropping them would make posting silently re-fetch every author somebody
    /// was reading. Your own posts do not come from the poller at all; they are
    /// read from the tree each render.
    pub fn post(&mut self, peers: &Peers, our_peer_id: &str, typed: &str, now: u64) {
        let signer = crate::feed_compose::authoring_keypair(our_peer_id);
        self.post_signed_by(peers, our_peer_id, signer.as_ref(), typed, now);
    }

    /// [`Self::post`] with the authoring key supplied — **the testable core**,
    /// split for `persistence::publisher_keypair_in`'s reason and not a second
    /// entry point.
    ///
    /// The lookup reads process-global persistence, so a model that did it
    /// inline had exactly one reachable outcome in a native test
    /// (`NotOurPeer`), and the half that matters — mint, apply, notice — could
    /// only ever be exercised through a browser. A parameter is the difference
    /// between a wired surface and a gated one.
    pub fn post_signed_by(
        &mut self,
        peers: &Peers,
        our_peer_id: &str,
        signer: Option<&entity_crypto::IdentityKeypair>,
        typed: &str,
        now: u64,
    ) {
        let text = typed.trim();
        if text.is_empty() {
            self.compose_notice = Some(ComposeNotice::Empty);
            return;
        }
        let Some(signer) = signer else {
            self.compose_notice = Some(ComposeNotice::NotOurPeer);
            return;
        };
        let entry = crate::feed::FeedEntry::new(
            our_peer_id,
            now,
            // ⭐ **Markdown, which is what makes a feed entry and a site page
            // the same thing to an author.** SITE §3.1 gives a page
            // `format: "markdown"`; EMBED is the vocabulary FEED §2.3 imports
            // for `body`; so the two conventions already agree and this is the
            // line that honours it. The `fallback` is the raw source, which is
            // the right degradation: unrendered markdown is still readable
            // prose, which is EMBED §8's anti-graveyard rule getting the easy
            // case for free.
            crate::embed::EmbedNode::new(
                crate::feed_body::MARKDOWN_MEDIA_TYPE,
                crate::embed::EmbedData::new(
                    crate::embed::EmbedPayload::Inline(text.as_bytes().to_vec()),
                    text,
                ),
            ),
        );
        match crate::feed_compose::plan_add_entry(signer, &entry) {
            Ok(minted) => match peers.writer_handle_for(our_peer_id) {
                Some(writer) => {
                    crate::feed_compose::apply(&writer, our_peer_id, &minted.plan);
                    self.compose_notice = Some(ComposeNotice::Posted);
                }
                None => self.compose_notice = Some(ComposeNotice::Refused),
            },
            Err(why) => {
                tracing::warn!(error = %why, "feed compose: the post was refused");
                self.compose_notice = Some(ComposeNotice::Refused);
            }
        }
    }

    /// **Unpublish** one of your own posts — §7.3's unbinding, entry and
    /// signature both.
    ///
    /// ⛔ **The notice is `FEED-R21`'s sentence and there is no success variant
    /// beside it.** §7.5 makes presenting removal as deletion a MUST NOT and
    /// puts the honest wording at the moment of the action. The verb hands that
    /// sentence back ([`crate::feed_compose::RemovalMeaning`]) so a surface
    /// cannot take the plan without it.
    ///
    /// A malformed hash is refused rather than ignored: unbinding a key built
    /// from a hash we could not parse would remove nothing and report success.
    pub fn remove_post(&mut self, peers: &Peers, our_peer_id: &str, hash_hex: &str) {
        let Some(hash) = crate::entity_ref::hash_from_hex(hash_hex) else {
            self.compose_notice = Some(ComposeNotice::Refused);
            return;
        };
        let (plan, meaning) = crate::feed_compose::plan_remove_entry(our_peer_id, &hash);
        match peers.writer_handle_for(our_peer_id) {
            Some(writer) => {
                crate::feed_compose::apply(&writer, our_peer_id, &plan);
                let _ = meaning;
                self.compose_notice = Some(ComposeNotice::Removed);
            }
            None => self.compose_notice = Some(ComposeNotice::Refused),
        }
    }

    /// Drop what is held for the selected peer so the next render walks again.
    pub fn refresh(&mut self) {
        if let Some(sel) = &self.selected {
            self.poller.forget(sel);
        }
        self.notice = None;
    }

    /// **The clock is an argument, not a read.** `now_ms()` lives in `dom`,
    /// which is `cfg(wasm32)` — a model that reached for it would be a model no
    /// native test could call.
    /// ⭐ **This deployment's own publisher, if this profile has one.**
    ///
    /// Read per render and never retained (AP41): a warm boot settles the
    /// session config in phase 2, so a window opened during phase 1 that
    /// captured this would hold *"no home"* for the rest of the session. `None`
    /// is an honest answer here — no deployment, or a config prefix this arm has
    /// not mirrored yet — and it costs the fallback, never a wrong row.
    fn home_publisher(peers: &Peers, our_peer_id: &str) -> Option<String> {
        let cfg = crate::session_config::read_opt(peers, our_peer_id)?;
        let peer = cfg.home_site.peer_id;
        // The documented sentinel for *this profile's own peer*. It names a real
        // peer only by resolution, and marking a blank as home would put a row
        // with no peer id at the top of the browse list.
        (!peer.is_empty()).then_some(peer)
    }

    /// ⭐ **Whose feed the panel shows — DERIVED, not stored.**
    ///
    /// An explicit selection wins; with none, the deployment's own publisher
    /// does. *Arriving at a domain and opening this window should show you that
    /// domain's posts*, which is the whole difference between a reader and a
    /// text box — and deriving it rather than writing it into `self.selected` at
    /// open is what keeps it from fighting the person: unfollowing whoever was
    /// on screen clears the selection and falls back here, and a later boot with
    /// a different home follows the deployment instead of a captured answer.
    ///
    /// It is **session-only and not durable**, so this is a default view and not
    /// a deployment deciding something on a visitor's behalf — D25 does not
    /// engage. And it can land on a publisher with no feed, which renders
    /// `NoPosts`: an honest empty state is not the invented answer AP54 is about.
    fn effective_selection(&self, home: Option<&str>) -> Option<String> {
        self.selected.clone().or_else(|| home.map(str::to_string))
    }

    pub fn render_output(&self, peers: &Peers, our_peer_id: &str, now: f64) -> FeedOutput {
        let home = Self::home_publisher(peers, our_peer_id);
        let selected = self.effective_selection(home.as_deref());

        let follows: Vec<FollowRow> = feed_follows::list(peers, our_peer_id)
            .into_iter()
            .map(|f| FollowRow {
                selected: selected.as_deref() == Some(f.subject.as_str()),
                peer_id: f.subject,
            })
            .collect();

        // **The browse list.** Read per render from the same accessor the panel
        // routes through, so a row can never name a peer the panel would then
        // report as unreachable — and `list_origins` is the AP54 chokepoint, so
        // a retired publisher is resolved rather than offered.
        let followed: std::collections::BTreeSet<&str> =
            follows.iter().map(|f| f.peer_id.as_str()).collect();
        let mut known: Vec<crate::views::feed::output::KnownRow> =
            crate::content_site::origins::list_origins(peers, our_peer_id)
                .into_iter()
                .map(|(peer_id, _origin)| crate::views::feed::output::KnownRow {
                    followed: followed.contains(peer_id.as_str()),
                    selected: selected.as_deref() == Some(peer_id.as_str()),
                    home: home.as_deref() == Some(peer_id.as_str()),
                    own: peer_id == our_peer_id,
                    peer_id,
                })
                .collect();
        // **The home publisher first, then peer id.** A stable total order, so
        // the list does not reshuffle between frames — `own_posts`' rule below,
        // and here the first row is also the one a visitor arrived for.
        known.sort_by(|a, b| b.home.cmp(&a.home).then_with(|| a.peer_id.cmp(&b.peer_id)));

        // **The panel's subject, carrying what they are to this profile** — the
        // head offers a follow control and cannot decide which one without it.
        //
        // Built here rather than in the struct literal at the bottom for a
        // borrow reason worth stating: `followed` borrows `follows`, and that
        // literal moves `follows` before it reaches this field. The two facts
        // come from the same sources the browse rows above read, so a row and
        // the head can never disagree about the same peer.
        let selection = selected.as_ref().map(|peer_id| crate::views::feed::output::Selection {
            followed: followed.contains(peer_id.as_str()),
            own: peer_id == our_peer_id,
            home: home.as_deref() == Some(peer_id.as_str()),
            peer_id: peer_id.clone(),
        });

        let panel = match &selected {
            None => FeedPanel::NobodySelected,
            Some(author) => {
                // **Both inputs are read fresh every render, and neither is
                // cached on this model.** Whether we are connected changes under
                // us and so does an origin registration; a route captured once
                // is AP41's retention defect with a routing symptom — a window
                // that decided *"not connected"* when it opened would keep
                // reading a published tree after the author came online.
                //
                // `get_origin` is the accessor that resolves supersession, never
                // a registry read of our own: AP54 is a surface that skipped
                // this chokepoint and kept serving a retired publisher.
                let connected =
                    crate::peer_liveness::liveness_of(peers, author).is_connected();
                let origin =
                    crate::content_site::origins::get_origin(peers, our_peer_id, author);
                // **The mirror legs, from a list somebody typed.** This was
                // an empty slice with a note saying §6 gives a reader no way to
                // LEARN that a gatherer exists. That is still true and is why
                // this is a maintained list rather than anything inferred —
                // `feed_gatherers`' module doc carries the argument, and the
                // short version is that a viewer which cannot be told its
                // source invents one (AP54), and here the invention would be a
                // stranger's reading of a third party in the place of that
                // person's own posts.
                let mirrors = feed_gatherers::legs(peers, our_peer_id);
                let route = feed_route::plan(
                    connected,
                    origin.as_deref(),
                    &mirrors,
                    feed_route::Preference::Unstated,
                );
                // Bound to **our** peer — it is the handle we dispatch *from*;
                // the author is the target and travels separately.
                let dispatch = peers.dispatch_handle(our_peer_id);
                match self.poller.poll(author, &route, dispatch.as_ref(), now) {
                    None | Some(FeedState::Loading) => FeedPanel::Loading,
                    Some(FeedState::Done { resolution, .. }) => panel_for(&resolution),
                }
            }
        };

        // **Read per render, like the follows, and for the same AP41 reason.**
        // `routed` is recomputed here rather than carried on the durable row: an
        // origin registration changes under us, and a row that remembered
        // *"unrouted"* from when it was added would keep saying so after the
        // deployment learned where that gatherer is.
        let gatherers: Vec<GathererRow> = feed_gatherers::list(peers, our_peer_id)
            .into_iter()
            .map(|g| GathererRow {
                routed: crate::content_site::origins::get_origin(peers, our_peer_id, &g.peer_id)
                    .is_some(),
                peer_id: g.peer_id,
            })
            .collect();

        // **Your own posts, read per render and never retained** — AP41's rule,
        // and the same one the follow list above is written to. `read_owned_feed`
        // is a synchronous `tree_listing` + `get_entity`, so it answers from the
        // per-prefix mirror; the window subscribes the entry prefix, so a post
        // marks it dirty and the next frame re-reads.
        //
        // Newest first, which is §4.5's within-a-page order and the one a person
        // expects of their own timeline. Sorted here rather than trusted from the
        // listing: `read_owned_feed` collects into a hash-keyed `BTreeSet`, so
        // its order is by content hash — arbitrary, and stable enough to look
        // deliberate.
        let mut own_posts: Vec<OwnPostRow> = crate::feed_compose::own_posts(peers, our_peer_id)
            .into_iter()
            .map(|post| {
                let hex = post.hash.to_hex();
                OwnPostRow {
                    id_short: hex.chars().take(12).collect(),
                    hash_hex: hex,
                    text: post.entry.body.data.fallback.clone(),
                    // ⚠ **`decide`, not `decide_with`, and that is a stated
                    // bound rather than an oversight.** `feed_compose::own_posts`
                    // reads the local tree and resolves no blob closure, so a
                    // pointer body of ours would render as its fallback — which
                    // `BlobMiss::SourceCannot` says exactly. It is unreachable
                    // today because the composer mints inline only; the day it
                    // chunks, this line is the one that has to grow a resolver.
                    body: crate::feed_body::decide(&post.entry.body),
                    created_at: post.entry.created_at,
                }
            })
            .collect();
        own_posts.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                // Two posts in the same millisecond still need a total order, or
                // the list reshuffles between frames on nothing — `sort_key`'s
                // argument one surface up, where the cost is a rewritten
                // archive and here it is a list that will not sit still.
                .then_with(|| a.hash_hex.cmp(&b.hash_hex))
        });

        FeedOutput {
            window_id: self.window_id,
            follows,
            known,
            gatherers,
            // The EFFECTIVE selection, not what somebody clicked — a renderer
            // that highlighted `self.selected` would leave the home fallback
            // showing a feed with no row marked as its source.
            selected: selection,
            panel,
            notice: self.notice,
            own_posts,
            compose_notice: self.compose_notice,
            manage_open: self.manage_open,
            // Read per render, not captured: a profile can gain the key for a
            // peer between frames, and a composer that decided once would stay
            // switched off for the session.
            can_author: crate::feed_compose::authoring_keypair(our_peer_id).is_some(),
        }
    }
}

/// Which panel a resolved route renders as — **one arm per fact, and the four
/// facts are [`Resolution`]'s.**
///
/// The one that earns its keep is `Unreachable` staying apart from `NoPosts`:
/// *we had no way to ask* and *we asked and they have nothing* are a statement
/// about us and a statement about them, and rendering the first as the second
/// tells somebody a publisher has written nothing on the strength of a check we
/// never made.
fn panel_for(resolution: &Resolution<crate::feed_read::ReadEntry>) -> FeedPanel {
    match resolution {
        Resolution::Served { leg, entries } => FeedPanel::Entries {
            via: via_of(leg),
            rows: entries.iter().map(entry_row).collect(),
        },
        Resolution::NoPosts { .. } => FeedPanel::NoPosts,
        // **The detail is the legs' own words**, joined — a reader whose live
        // leg was refused and whose origin 504'd is looking at two different
        // problems, and one summary line that named only the last would send
        // them at the wrong one.
        Resolution::Failed { attempts } => FeedPanel::Failed {
            detail: attempts
                .iter()
                .filter_map(|a| match &a.outcome {
                    crate::feed_route::AttemptOutcome::Failed(d) => Some(format!("{}: {d}", a.leg)),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" · "),
        },
        Resolution::Unreachable => FeedPanel::NoRoute,
    }
}

/// The leg that served a feed, as the renderer needs it.
///
/// **Exhaustive over `Leg`**, so a fourth leg is a compile error here rather
/// than a silent fall-through — the old version matched `"live"` and sent
/// *everything else* to the published key, which would have rendered a mirror
/// as the author's own tree the day one arrived.
fn via_of(leg: &feed_route::Leg) -> Via {
    match leg {
        feed_route::Leg::Live => Via::Live,
        feed_route::Leg::Published(_) => Via::Published,
        feed_route::Leg::Mirror { gatherer, .. } => Via::Mirror { gatherer: gatherer.clone() },
    }
}

/// One entry as the renderer needs it.
fn entry_row(read: &crate::feed_read::ReadEntry) -> EntryRow {
    let hex = read.hash.to_hex();
    EntryRow {
        id_short: hex.chars().take(12).collect(),
        text: read.entry.body.data.fallback.clone(),
        // The blob the async read resolved, not a second lookup: a pointer body
        // is the only path to a post over EMBED §3's 16 KiB inline ceiling, and
        // the render pass this feeds is synchronous.
        body: crate::feed_body::decide_with(&read.entry.body, &read.body_blob),
        created_at: read.entry.created_at,
        attributed: read.attribution.may_name_the_author(),
        attribution_key: attribution_key(&read.attribution),
    }
}

/// **Which sentence a verdict gets, and every one of the seven is distinct.**
///
/// `FEED-R4` is a MUST about what a reader *presents*, so this is the rule
/// itself rather than presentation polish: *nobody signed this*, *we could not
/// check*, *somebody else signed it* and *the bytes do not verify* go to four
/// different people, and a surface that rendered any pair alike would lose the
/// only distinction that decides what to do about it.
pub fn attribution_key(a: &Attribution) -> &'static str {
    match a {
        Attribution::Signed => "feed.attr.signed",
        Attribution::Unattributed(u) => match u {
            Unattributed::NoSignature => "feed.attr.no_signature",
            Unattributed::KeyNotInPeerId { .. } => "feed.attr.cannot_check",
            Unattributed::SignatureUnreadable { .. } => "feed.attr.unreadable",
            Unattributed::WrongTarget { .. } => "feed.attr.wrong_target",
            Unattributed::SignerIsNotTheAuthor => "feed.attr.not_the_author",
            Unattributed::BadSignature => "feed.attr.bad_signature",
        },
    }
}

/// Which sentence a Follow press gets. Four outcomes, four sentences — and two
/// of them are not failures.
fn notice_for(outcome: FollowOutcome) -> Notice {
    Notice(match outcome {
        FollowOutcome::Followed => "feed.notice.followed",
        FollowOutcome::AlreadyFollowing => "feed.notice.already",
        FollowOutcome::ThatIsYou => "feed.notice.thats_you",
        FollowOutcome::NotAPeerId => "feed.notice.not_a_peer_id",
    })
}

/// Which sentence adding a gatherer gets. Four outcomes, four sentences — and
/// they are **their own keys, not the follow ones reused**: *"you already follow
/// them"* and *"you already read through them"* are different facts about
/// different lists, and a shared string would tell somebody the wrong one.
fn gatherer_notice(outcome: AddOutcome) -> Notice {
    Notice(match outcome {
        AddOutcome::Added => "feed.notice.gatherer_added",
        AddOutcome::AlreadyAGatherer => "feed.notice.gatherer_already",
        // **The follow key, reused.** Identical English sentence, so a second
        // key would be one sentence translated thirty times twice — the drift
        // `i18n-locale-check` caught on `btn.refresh`. The two keys above exist
        // because their sentences genuinely differ.
        AddOutcome::ThatIsYou => "feed.notice.thats_you",
        AddOutcome::NotAPeerId => "feed.notice.not_a_peer_id",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW_MS: u64 = 1_757_000_000_000;
    const CLOCK: f64 = 1_000_000.0;

    fn peer_id(seed: u8) -> String {
        let kp = entity_crypto::Keypair::from_seed([seed; 32]);
        entity_crypto::PeerId::from_public_key(&kp.public_key_bytes()).to_string()
    }

    /// A route that answered and carried nothing — *"we asked and they have no
    /// posts"*, which is the fact `NoPosts` renders and which must stay apart
    /// from the two that look like it.
    fn no_posts() -> FeedState {
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

    fn model() -> (FeedModel, Peers, String) {
        let peers = Peers::new_direct();
        let me = peers.system_peer_id().to_string();
        (FeedModel::new(1, Default::default()), peers, me)
    }

    /// Register where a publisher is hosted. **Every panel state below except
    /// `NoRoute` requires this**, which is the point of the state existing: the
    /// panel cannot ask anybody anything until the deployment knows where they
    /// are.
    fn route(peers: &Peers, me: &str, them: &str) {
        crate::content_site::origins::set_origin(peers, me, them, "http://publisher.example");
    }

    // -- the composer -----------------------------------------------------

    /// A profile authoring **as itself**, plus the key to sign with.
    ///
    /// ⚠ **The profile is constructed FROM the seed, and that is not a
    /// convenience.** A `Peers` generates its own primary, and a composer
    /// pointed at any other peer id has no writer for it (`writer_handle_for`
    /// answers `None`) and no SDK to read it back through — so a test that
    /// signed with a stand-in key would land on `Refused` and look like a
    /// product defect. The same fact `plan_add_entry` refuses on under
    /// `FEED-R1`, one layer down. One seed, two handles: the profile is built
    /// with it and the composer signs with it.
    fn authoring_profile(seed: u8) -> (Peers, entity_crypto::IdentityKeypair, String) {
        let kp = entity_crypto::IdentityKeypair::Ed25519(entity_crypto::Keypair::from_seed(
            [seed; 32],
        ));
        let peers = Peers::new_direct_with_keypair(entity_crypto::Keypair::from_seed([seed; 32]));
        let me = peers.primary_peer_id().to_string();
        assert_eq!(me, kp.peer_id().to_string(), "authoring as ourselves");
        (peers, kp, me)
    }

    /// ⭐ **A post lands in the tree and comes back on the surface.** The
    /// round trip the window performs: mint, apply, then `render_output` reading
    /// the entry prefix — which is the read the third `watch_prefix` exists to
    /// keep fresh.
    #[test]
    fn a_post_appears_in_your_own_posts() {
        let (peers, kp, me) = authoring_profile(9);
        let mut m = FeedModel::new(1, Default::default());

        assert!(
            m.render_output(&peers, &me, CLOCK).own_posts.is_empty(),
            "nothing posted yet"
        );

        m.post_signed_by(&peers, &me, Some(&kp), "hello world", NOW_MS);
        assert_eq!(m.compose_notice, Some(ComposeNotice::Posted));

        let out = m.render_output(&peers, &me, CLOCK);
        assert_eq!(out.own_posts.len(), 1);
        assert_eq!(out.own_posts[0].text, "hello world");
        assert_eq!(out.own_posts[0].created_at, NOW_MS);
        assert_eq!(
            out.own_posts[0].hash_hex.len(),
            66,
            "the FULL address, not the shortened id — a remove is built from it"
        );
        assert_eq!(out.own_posts[0].id_short.len(), 12);
    }

    /// ⛔ **And removing it takes it away — entry AND signature.** The signature
    /// half is asserted at the tree, because the surface cannot show it and §7.3
    /// is precisely about what the tree carries afterwards.
    #[test]
    fn removing_a_post_leaves_no_trace_of_it_in_the_tree() {
        let (peers, kp, me) = authoring_profile(10);
        let mut m = FeedModel::new(1, Default::default());
        m.post_signed_by(&peers, &me, Some(&kp), "up for a moment", NOW_MS);

        let out = m.render_output(&peers, &me, CLOCK);
        let hash_hex = out.own_posts[0].hash_hex.clone();
        let hash = crate::entity_ref::hash_from_hex(&hash_hex).unwrap();
        let sig_path = format!("/{me}/{}", crate::feed::signature_key(&me, &hash));
        assert!(peers.get_entity(&me, &sig_path).is_some(), "signed on the way in");

        m.remove_post(&peers, &me, &hash_hex);

        assert!(
            m.render_output(&peers, &me, CLOCK).own_posts.is_empty(),
            "gone from the surface"
        );
        assert!(
            peers.get_entity(&me, &sig_path).is_none(),
            "§7.3: the signature goes with it — a tree the entry was removed from \
             is byte-identical to one that never held it"
        );
    }

    /// ⛔ **`FEED-R21`: the removal notice is the unpublication sentence and it
    /// must not read as deletion.** Asserted on the rendered string, because the
    /// MUST NOT is about what a person is told, not about which enum variant was
    /// chosen.
    #[test]
    fn a_removal_never_tells_anybody_the_post_was_deleted() {
        let (peers, kp, me) = authoring_profile(11);
        let mut m = FeedModel::new(1, Default::default());
        m.post_signed_by(&peers, &me, Some(&kp), "x", NOW_MS);
        let hash_hex = m.render_output(&peers, &me, CLOCK).own_posts[0].hash_hex.clone();

        m.remove_post(&peers, &me, &hash_hex);

        let notice = m.compose_notice.expect("a removal says something");
        assert_eq!(notice, ComposeNotice::Removed);
        let rendered = crate::i18n::t(notice.key(), &[]).to_lowercase();
        assert!(!rendered.contains("delet"), "FEED-R21 is a MUST NOT: {rendered:?}");
        assert!(
            rendered.contains("still have it"),
            "§7.5's second clause is the load-bearing one: {rendered:?}"
        );
    }

    /// Three refusals, three different mistakes, and none of them is silence.
    #[test]
    fn every_compose_refusal_says_which_one_it_is() {
        let (peers, kp, me) = authoring_profile(12);
        let mut m = FeedModel::new(1, Default::default());

        m.post_signed_by(&peers, &me, Some(&kp), "   ", NOW_MS);
        assert_eq!(m.compose_notice, Some(ComposeNotice::Empty), "whitespace is empty");

        m.post_signed_by(&peers, &me, None, "real text", NOW_MS);
        assert_eq!(
            m.compose_notice,
            Some(ComposeNotice::NotOurPeer),
            "no key is not a typo and not a retry"
        );

        m.remove_post(&peers, &me, "not-a-hash");
        assert_eq!(
            m.compose_notice,
            Some(ComposeNotice::Refused),
            "an address we could not parse would unbind nothing and must not \
             report the unpublication sentence"
        );
    }

    /// **Every compose outcome gets its own key, and the count is asserted** —
    /// `doctor.rs`'s rule, so a sixth outcome cannot quietly reuse an existing
    /// sentence. `Removed` in particular must never share a key with `Posted`.
    #[test]
    fn every_compose_outcome_has_its_own_word() {
        let all = [
            ComposeNotice::Posted,
            ComposeNotice::Empty,
            ComposeNotice::NotOurPeer,
            ComposeNotice::Refused,
            ComposeNotice::Removed,
        ];
        assert_eq!(all.len(), 5, "a sixth outcome needs a row here and a sentence");
        let keys: std::collections::BTreeSet<&str> = all.iter().map(|o| o.key()).collect();
        assert_eq!(keys.len(), all.len(), "no two outcomes share a sentence");
        for o in all {
            let rendered = crate::i18n::t(o.key(), &[]);
            assert_ne!(rendered, o.key(), "{:?} has no catalog entry", o);
        }
    }

    /// Newest first, and a millisecond tie still has a total order — otherwise
    /// the list reshuffles between frames on nothing.
    #[test]
    fn your_own_posts_are_newest_first_and_the_order_is_total() {
        let (peers, kp, me) = authoring_profile(13);
        let mut m = FeedModel::new(1, Default::default());
        m.post_signed_by(&peers, &me, Some(&kp), "older", NOW_MS);
        m.post_signed_by(&peers, &me, Some(&kp), "newer", NOW_MS + 1000);
        // Two in the same millisecond — the tie-break case.
        m.post_signed_by(&peers, &me, Some(&kp), "tie a", NOW_MS + 2000);
        m.post_signed_by(&peers, &me, Some(&kp), "tie b", NOW_MS + 2000);

        let first = m.render_output(&peers, &me, CLOCK).own_posts;
        assert_eq!(first.len(), 4);
        assert_eq!(first[first.len() - 1].text, "older", "oldest last");
        assert_eq!(first[1].created_at, NOW_MS + 2000);

        let again = m.render_output(&peers, &me, CLOCK).own_posts;
        assert_eq!(first, again, "two renders of one tree give one order");
    }

    /// **Every attribution verdict gets its own key, and the count is
    /// asserted** — so a seventh `Unattributed` reason cannot quietly reuse an
    /// existing sentence, which is the failure `Attribution::may_name_the_author`
    /// is spelled positively to avoid one layer down.
    #[test]
    fn every_attribution_verdict_renders_under_its_own_key() {
        use entity_hash::Hash;
        let h = Hash::compute("test/note", b"x");
        let all = [
            Attribution::Signed,
            Attribution::Unattributed(Unattributed::NoSignature),
            Attribution::Unattributed(Unattributed::KeyNotInPeerId { peer_id: "x".into() }),
            Attribution::Unattributed(Unattributed::SignatureUnreadable { detail: "x".into() }),
            Attribution::Unattributed(Unattributed::WrongTarget { target: h, entry: h }),
            Attribution::Unattributed(Unattributed::SignerIsNotTheAuthor),
            Attribution::Unattributed(Unattributed::BadSignature),
        ];
        let keys: std::collections::BTreeSet<&str> =
            all.iter().map(attribution_key).collect();
        assert_eq!(keys.len(), 7, "seven verdicts, seven sentences: {keys:?}");
        assert!(keys.iter().all(|k| k.starts_with("feed.attr.")));
        // And every one of them resolves in the catalog — a key with no entry
        // renders as the key, which is worse than English.
        for key in &keys {
            assert!(
                crate::i18n::catalog_has("en", key),
                "{key} is not in the EN base"
            );
        }
    }

    /// The four Follow outcomes likewise, and their keys resolve too.
    #[test]
    fn every_follow_outcome_renders_under_its_own_key() {
        let all = [
            FollowOutcome::Followed,
            FollowOutcome::AlreadyFollowing,
            FollowOutcome::ThatIsYou,
            FollowOutcome::NotAPeerId,
        ];
        let keys: std::collections::BTreeSet<&str> =
            all.iter().map(|o| notice_for(*o).0).collect();
        assert_eq!(keys.len(), 4, "four outcomes, four sentences: {keys:?}");
        for key in &keys {
            assert!(crate::i18n::catalog_has("en", key), "{key} is not in the EN base");
        }
    }

    /// ⚠ **"Nobody selected", "no route", "still loading" and "posted nothing"
    /// are four different screens.**
    ///
    /// The two in the middle are the ones that matter. A walk in flight rendered
    /// as an empty feed tells somebody a publisher has written nothing, which
    /// may be false and which they have no way to question. And a publisher this
    /// deployment has no route to is not a publisher who is down — one of those
    /// is *"try again later"* and the other is *"nobody here knows where they
    /// are"*.
    #[test]
    fn an_unselected_panel_an_unrouted_one_a_loading_one_and_an_empty_one_are_four_answers() {
        let (mut m, peers, me) = model();
        let them = peer_id(41);

        assert_eq!(
            m.render_output(&peers, &me, CLOCK).panel,
            FeedPanel::NobodySelected,
            "nothing chosen"
        );

        m.select(&them);
        assert_eq!(
            m.render_output(&peers, &me, CLOCK).panel,
            FeedPanel::NoRoute,
            "selected, but this deployment does not know where they are hosted \
             — which is NOT a failed walk and NOT an empty feed"
        );

        route(&peers, &me, &them);
        assert_eq!(
            m.render_output(&peers, &me, CLOCK).panel,
            FeedPanel::Loading,
            "a walk was started and has not answered — NOT an empty feed"
        );

        // The walk completes with nothing in it: a real, different answer.
        m.poller.seed(&them, no_posts());
        assert_eq!(m.render_output(&peers, &me, CLOCK).panel, FeedPanel::NoPosts);

        m.poller.seed(
            &them,
            FeedState::Done {
                resolution: Resolution::Failed {
                    attempts: vec![crate::feed_route::Attempt {
                        leg: "published",
                        outcome: crate::feed_route::AttemptOutcome::Failed("no route".into()),
                    }],
                },
                retry_at_ms: Some(f64::MAX),
            },
        );
        assert_eq!(
            m.render_output(&peers, &me, CLOCK).panel,
            FeedPanel::Failed { detail: "published: no route".into() },
            "and a failure is a fourth — naming the leg, because a two-leg route \
             fails for two reasons and one summary line would hide one of them"
        );
    }

    /// Following somebody shows them, because that is why the peer id was
    /// typed. Following them **again** is not an error and does not un-select
    /// them.
    #[test]
    fn following_someone_selects_them_and_a_second_press_is_not_a_failure() {
        let (mut m, peers, me) = model();
        let them = peer_id(42);

        m.follow(&peers, &me, &them, NOW_MS);
        let out = m.render_output(&peers, &me, CLOCK);
        assert_eq!(out.selected_peer(), Some(them.as_str()));
        assert_eq!(out.notice, Some(Notice("feed.notice.followed")));
        assert_eq!(out.follows.len(), 1);
        assert!(out.follows[0].selected, "the row is marked as the one on screen");

        m.follow(&peers, &me, &them, NOW_MS + 1);
        let out = m.render_output(&peers, &me, CLOCK);
        assert_eq!(out.notice, Some(Notice("feed.notice.already")));
        assert_eq!(out.selected_peer(), Some(them.as_str()), "still showing them");
    }

    /// ⭐⭐ **The publisher you are reading carries what they are to you, so the
    /// panel can offer the control — and it FLIPS.**
    ///
    /// This is the gate for the journey ending one click short. Arriving here
    /// from the Registry Browser (`publication_probe` said they publish a feed →
    /// *Open in Feed* → `FeedWindow::aim`) **selects without following**, which
    /// is correct: reading somebody is not subscribing to them. What was missing
    /// is the second act being available where the first one landed, and a
    /// renderer cannot draw it from a bare `Option<String>`.
    ///
    /// Asserts the transition rather than an end state: a selection that
    /// reported `followed: true` unconditionally would pass any single-state
    /// check, and `Stranger → Followed → Stranger` is what a person does.
    #[test]
    fn the_publisher_on_screen_carries_whether_you_follow_them_and_it_flips() {
        use crate::views::feed::output::{relation, Relation};
        let (mut m, peers, me) = model();
        let them = peer_id(71);
        route(&peers, &me, &them);

        // Selected without following — the state `aim` leaves behind.
        m.select(&them);
        let sel = m.render_output(&peers, &me, CLOCK).selected.expect("on screen");
        assert_eq!(sel.peer_id, them);
        assert!(!sel.followed, "reading somebody is not following them");
        assert!(!sel.own);
        assert_eq!(relation(sel.own, sel.followed), Relation::Stranger, "offer Follow");

        m.follow(&peers, &me, &them, NOW_MS);
        let sel = m.render_output(&peers, &me, CLOCK).selected.expect("still on screen");
        assert!(sel.followed, "the head must now offer Unfollow, not Follow");
        assert_eq!(relation(sel.own, sel.followed), Relation::Followed);

        // And back. Unfollow clears the selection entirely (its own gate above),
        // so re-select to read the relation rather than asserting on `None`.
        m.unfollow(&peers, &me, &them);
        m.select(&them);
        let sel = m.render_output(&peers, &me, CLOCK).selected.expect("re-selected");
        assert!(!sel.followed, "unfollowing puts the control back to Follow");
    }

    /// **Your own peer gets no follow control**, on the panel head and in the
    /// browse row alike — `feed_follows::follow` refuses it
    /// (`a_refused_follow_selects_nobody_and_names_its_own_refusal` pins the
    /// refusal itself), so a button there could only ever produce a notice.
    #[test]
    fn reading_your_own_feed_offers_no_follow_control() {
        use crate::views::feed::output::{relation, Relation};
        let (mut m, peers, me) = model();

        m.select(&me);
        let sel = m.render_output(&peers, &me, CLOCK).selected.expect("on screen");
        assert!(sel.own);
        assert!(!sel.followed);
        assert_eq!(relation(sel.own, sel.followed), Relation::Own, "offer nothing");
    }

    /// A refused follow says which refusal it was and **selects nobody** — the
    /// panel must not start walking a peer id that was rejected.
    #[test]
    fn a_refused_follow_selects_nobody_and_names_its_own_refusal() {
        let (mut m, peers, me) = model();

        m.follow(&peers, &me, "not-a-peer-id", NOW_MS);
        let out = m.render_output(&peers, &me, CLOCK);
        assert_eq!(out.notice, Some(Notice("feed.notice.not_a_peer_id")));
        assert_eq!(out.selected_peer(), None);
        assert_eq!(out.panel, FeedPanel::NobodySelected);

        m.follow(&peers, &me, &me, NOW_MS);
        assert_eq!(
            m.render_output(&peers, &me, CLOCK).notice,
            Some(Notice("feed.notice.thats_you")),
            "your own peer id is a DIFFERENT mistake from a malformed one"
        );
    }

    /// Unfollowing takes them off the list, off the screen, and out of memory.
    ///
    /// The last one is the part a tidy version misses: a fetched feed left in
    /// the poller would be served instantly on a re-follow, which looks like a
    /// cache and is a failure to forget somebody the person said to forget.
    #[test]
    fn unfollowing_removes_them_from_the_list_the_panel_and_the_poller() {
        let (mut m, peers, me) = model();
        let them = peer_id(43);
        route(&peers, &me, &them);
        m.follow(&peers, &me, &them, NOW_MS);
        m.poller.seed(&them, no_posts());
        assert_eq!(m.render_output(&peers, &me, CLOCK).panel, FeedPanel::NoPosts);

        m.unfollow(&peers, &me, &them);
        let out = m.render_output(&peers, &me, CLOCK);
        assert_eq!(out.follows.len(), 0, "off the list");
        assert_eq!(out.selected_peer(), None, "off the screen");
        assert_eq!(out.panel, FeedPanel::NobodySelected);

        // Re-follow: the panel must go back to Loading, not straight to the
        // answer it held before.
        m.follow(&peers, &me, &them, NOW_MS);
        assert_eq!(
            m.render_output(&peers, &me, CLOCK).panel,
            FeedPanel::Loading,
            "their old posts were forgotten, so this is a fresh walk"
        );
    }

    // -----------------------------------------------------------------------
    // The third source leg — `APP-CONVENTION-FEED` §6's gatherers.
    // -----------------------------------------------------------------------

    /// ⭐⭐ **THE FEATURE IN ONE ASSERTION: naming a gatherer makes an otherwise
    /// unreachable author readable.**
    ///
    /// `NoRoute` is *"there are no legs"* — not connected, no origin for this
    /// author, nowhere to ask. Adding a **routed** gatherer puts a third leg in
    /// the route, so the same author becomes a walk in flight. That is the whole
    /// of what §6.2's *"one leg of an ordered source set"* buys a person, and
    /// the two panels are the before and after.
    ///
    /// `feed_route::plan` has taken a gatherer list since it shipped and been
    /// handed an **empty slice** by the product the entire time; this is the
    /// gate that the slice is no longer empty.
    #[test]
    fn naming_a_gatherer_gives_an_unreachable_author_a_leg() {
        let (mut m, peers, me) = model();
        let them = peer_id(61);
        let gatherer = peer_id(62);

        m.select(&them);
        assert_eq!(
            m.render_output(&peers, &me, CLOCK).panel,
            FeedPanel::NoRoute,
            "the starting state must be *no legs at all*, or this gate measures nothing"
        );

        // A gatherer we cannot reach either adds nothing — an honest absence,
        // not a leg pointing at a URL we would have had to invent.
        m.add_gatherer(&peers, &me, &gatherer, NOW_MS);
        assert_eq!(
            m.render_output(&peers, &me, CLOCK).panel,
            FeedPanel::NoRoute,
            "an unrouted gatherer manufactured a leg"
        );

        // …and once this deployment knows where the gatherer is hosted, the
        // author is readable through them.
        route(&peers, &me, &gatherer);
        assert_eq!(
            m.render_output(&peers, &me, CLOCK).panel,
            FeedPanel::Loading,
            "a routed gatherer did not put a leg in the route — the mirror leg is \
             still the empty slice it was before"
        );
    }

    /// ⛔ **`FEED-R13` / §6.1 rule 3: the gatherer is named on the *via* line and
    /// NOWHERE ELSE.**
    ///
    /// Attribution follows each entry's own detached signature, always. This
    /// asserts both halves: the via line *does* name which gatherer served (a
    /// reader told their view may be partial needs to know whose reading it is),
    /// and **no entry row carries the gatherer's id at all** — `EntryRow` has no
    /// field it could travel in, so the rule is a property of the types rather
    /// than one a renderer has to remember.
    #[test]
    fn a_mirror_names_the_gatherer_on_the_via_line_and_never_on_an_entry() {
        use crate::embed::{EmbedData, EmbedNode, EmbedPayload};
        use crate::feed::FeedEntry;
        use crate::feed_read::{Obtained, ReadEntry};

        let (mut m, peers, me) = model();
        let author = peer_id(63);
        let gatherer = peer_id(64);
        m.select(&author);
        route(&peers, &me, &author);

        let body = EmbedNode::new(
            "text/plain",
            EmbedData::new(EmbedPayload::Inline(b"a post".to_vec()), "a post"),
        );
        let entry = FeedEntry::new(&author, NOW_MS, body);
        let entity = entry.to_entity().unwrap();
        let hash = entity.content_hash;
        m.poller.seed(
            &author,
            FeedState::Done {
                resolution: Resolution::Served {
                    leg: feed_route::Leg::Mirror {
                        gatherer: gatherer.clone(),
                        origin: "http://g.example".into(),
                    },
                    entries: vec![ReadEntry {
                        hash,
                        entry,
                        attribution: Attribution::Signed,
                        obtained: Obtained { entity, signature: None },
                        body_blob: crate::feed_body::BodyBlob::Inline,
                    }],
                },
                retry_at_ms: None,
            },
        );

        let out = m.render_output(&peers, &me, CLOCK);
        let FeedPanel::Entries { via, rows } = &out.panel else {
            panic!("expected entries, got {:?}", out.panel)
        };
        assert_eq!(via.key(), "feed.via.mirror");
        assert_eq!(
            via.source_peer(),
            Some(gatherer.as_str()),
            "the via line must name WHICH gatherer — a partial view nobody can \
             attribute is one nobody can judge"
        );
        assert!(crate::i18n::catalog_has("en", via.key()));

        assert_eq!(rows.len(), 1);
        assert!(rows[0].attributed, "attribution follows the entry's own signature");
        // The structural half: nothing the renderer could put in a byline.
        let row = format!("{:?}", rows[0]);
        assert!(
            !row.contains(&gatherer),
            "an entry row carries the gatherer's peer id — §6.1 rule 3 makes \
             naming the gatherer as the author non-conformant: {row}"
        );

        // …and the author's own legs carry no source peer, which is what makes
        // the mirror arm's asymmetry a statement rather than an accident.
        assert_eq!(super::via_of(&feed_route::Leg::Live).source_peer(), None);
        assert_eq!(
            super::via_of(&feed_route::Leg::Published("o".into())).source_peer(),
            None
        );
    }

    /// ⛔ **Changing the gatherer list forgets every held walk.**
    ///
    /// A route is planned per render from the live list, so adding a gatherer
    /// changes the route for **every** author at once — but a held `FeedState`
    /// is an answer to the *old* route and `poll` serves it without re-walking.
    /// Somebody who adds a gatherer precisely because an author would not load
    /// would otherwise press the button, see the identical panel, and conclude
    /// their change did nothing.
    #[test]
    fn changing_the_gatherer_list_forgets_every_held_walk() {
        let (mut m, peers, me) = model();
        let them = peer_id(65);
        let gatherer = peer_id(66);
        route(&peers, &me, &them);
        m.select(&them);
        m.poller.seed(&them, no_posts());
        assert_eq!(m.render_output(&peers, &me, CLOCK).panel, FeedPanel::NoPosts);

        m.add_gatherer(&peers, &me, &gatherer, NOW_MS);
        assert_eq!(
            m.render_output(&peers, &me, CLOCK).panel,
            FeedPanel::Loading,
            "the old answer survived a route change — the new leg will never be tried"
        );

        // …and removing one, for the mirror image: a panel still showing entries
        // served BY the gatherer somebody just removed is the surface
        // disagreeing with the list beside it.
        m.poller.seed(&them, no_posts());
        m.remove_gatherer(&peers, &me, &gatherer);
        assert_eq!(m.render_output(&peers, &me, CLOCK).panel, FeedPanel::Loading);
    }

    /// A gatherer with no route **stays on screen and says so**. Dropping the
    /// row as well as the leg would leave somebody looking at a list they just
    /// added a peer to, with no way to tell whether it worked.
    #[test]
    fn an_unrouted_gatherer_is_shown_and_marked_rather_than_hidden() {
        let (mut m, peers, me) = model();
        let g = peer_id(67);
        m.add_gatherer(&peers, &me, &g, NOW_MS);

        let out = m.render_output(&peers, &me, CLOCK);
        assert_eq!(out.gatherers.len(), 1, "the row vanished with the leg");
        assert_eq!(out.gatherers[0].peer_id, g);
        assert!(!out.gatherers[0].routed);

        route(&peers, &me, &g);
        assert!(
            m.render_output(&peers, &me, CLOCK).gatherers[0].routed,
            "`routed` is recomputed per render, not remembered from when the row \
             was added — an origin registration changes under us"
        );
    }

    /// The gatherer outcomes get their own sentences where the sentence differs,
    /// and **share one where it does not**.
    ///
    /// `ThatIsYou` reuses the follow key deliberately: the English is identical,
    /// so a second key would be one sentence translated thirty times twice —
    /// the drift `i18n-locale-check` caught when a fresh `feed.refresh` rendered
    /// "Refresh" differently from `btn.refresh` in four locales.
    #[test]
    fn every_gatherer_outcome_renders_under_a_key_that_resolves() {
        let all = [
            AddOutcome::Added,
            AddOutcome::AlreadyAGatherer,
            AddOutcome::ThatIsYou,
            AddOutcome::NotAPeerId,
        ];
        let keys: Vec<&str> = all.iter().map(|o| gatherer_notice(*o).0).collect();
        assert_eq!(
            keys.iter().collect::<std::collections::BTreeSet<_>>().len(),
            4,
            "four outcomes, four sentences: {keys:?}"
        );
        for key in &keys {
            assert!(crate::i18n::catalog_has("en", key), "{key} is not in the EN base");
        }
        assert_eq!(
            gatherer_notice(AddOutcome::ThatIsYou).0,
            notice_for(FollowOutcome::ThatIsYou).0,
            "one English sentence, one key"
        );
        assert_ne!(
            gatherer_notice(AddOutcome::AlreadyAGatherer).0,
            notice_for(FollowOutcome::AlreadyFollowing).0,
            "…and two different facts get two keys: *you already follow them* and \
             *you already read through them* are about different lists"
        );
    }

    /// A selection clears a notice, because the notice was about a different
    /// act. A refusal left beside a feed that loaded fine reads as being about
    /// the feed.
    #[test]
    fn selecting_someone_clears_a_notice_from_a_previous_act() {
        let (mut m, peers, me) = model();
        m.follow(&peers, &me, "not-a-peer-id", NOW_MS);
        assert!(m.render_output(&peers, &me, CLOCK).notice.is_some());
        m.select(&peer_id(44));
        assert_eq!(m.render_output(&peers, &me, CLOCK).notice, None);
    }

    /// An entry row carries the body's authored `fallback` and the entry's own
    /// hash as its id — EMBED §3 makes the fallback mandatory and non-empty so
    /// that a reader with no renderer for the payload still has a sentence, and
    /// §2.2.1 makes the hash the entry's identity.
    #[test]
    fn an_entry_row_shows_the_authored_fallback_and_the_entrys_own_hash() {
        use crate::embed::{EmbedData, EmbedNode, EmbedPayload};
        use crate::feed::FeedEntry;
        use crate::feed_read::{Obtained, ReadEntry};

        let body = EmbedNode::new(
            "text/plain",
            EmbedData::new(EmbedPayload::Inline(b"hello".to_vec()), "hello"),
        );
        let entry = FeedEntry::new("QmAuthor", NOW_MS, body);
        let entity = entry.to_entity().unwrap();
        let hash = entity.content_hash;
        let row = entry_row(&ReadEntry {
            hash,
            entry,
            attribution: Attribution::Unattributed(Unattributed::NoSignature),
            obtained: Obtained { entity, signature: None },
            body_blob: crate::feed_body::BodyBlob::Inline,
        });

        assert_eq!(row.text, "hello");
        assert_eq!(row.created_at, NOW_MS);
        assert_eq!(row.id_short.len(), 12);
        assert!(hash.to_hex().starts_with(&row.id_short));
        assert!(!row.attributed, "FEED-R4: no signature means nobody is named");
        assert_eq!(row.attribution_key, "feed.attr.no_signature");
    }

    // -- the browse list --------------------------------------------------

    /// Declare `them` as this deployment's home publisher.
    fn home(peers: &Peers, me: &str, them: &str) {
        crate::session_config::set_home_site(peers, me, them, "demo");
    }

    /// **Every publisher this profile can reach is offered, and each row says
    /// what that peer is to this profile.**
    ///
    /// The set is the *routed* set on purpose — a row with no origin can only
    /// produce `NoRoute`, so offering it would be AP54's shape pointed forward.
    #[test]
    fn the_browse_list_is_every_publisher_this_profile_can_reach() {
        let (model, peers, me) = model();
        let (alice, bob) = (peer_id(11), peer_id(12));
        route(&peers, &me, &alice);
        route(&peers, &me, &bob);
        feed_follows::follow(&peers, &me, &bob, NOW_MS);

        let out = model.render_output(&peers, &me, CLOCK);
        let ids: Vec<&str> = out.known.iter().map(|r| r.peer_id.as_str()).collect();
        assert!(ids.contains(&alice.as_str()) && ids.contains(&bob.as_str()), "{ids:?}");

        let row = |p: &str| out.known.iter().find(|r| r.peer_id == p).unwrap().clone();
        assert!(row(&bob).followed, "bob is in the follow registry");
        assert!(!row(&alice).followed, "alice is reachable and not followed");
        assert!(!row(&alice).own && !row(&alice).home);
    }

    /// ⛔ **A publisher we have no route to is NOT in the browse list**, even
    /// when we follow them — the list is what can be opened. They keep their
    /// row under *Following*, so nothing is lost, and the panel still tells them
    /// `NoRoute` if they pick it.
    #[test]
    fn a_publisher_with_no_route_is_followed_but_not_offered_to_browse() {
        let (model, peers, me) = model();
        let stranger = peer_id(13);
        feed_follows::follow(&peers, &me, &stranger, NOW_MS);

        let out = model.render_output(&peers, &me, CLOCK);
        assert!(out.known.is_empty(), "no origin, nothing to open: {:?}", out.known);
        assert_eq!(out.follows.len(), 1, "and the follow is not lost");
    }

    /// ⭐⭐ **THE HEADLINE: arriving at a deployment and opening this window
    /// shows that deployment's posts, with nobody choosing anything.**
    ///
    /// This is the whole difference between a reader and a text box. It is
    /// derived per render rather than written into `self.selected` at open —
    /// see [`FeedModel::effective_selection`] — so it cannot fight a person who
    /// then picks somebody else, and a later boot under a different home
    /// follows the deployment rather than a captured answer.
    #[test]
    fn arriving_at_a_deployment_reads_its_own_publisher_without_anybody_choosing() {
        let (model, peers, me) = model();
        let publisher = peer_id(21);
        route(&peers, &me, &publisher);
        home(&peers, &me, &publisher);

        let out = model.render_output(&peers, &me, CLOCK);
        assert_eq!(out.selected_peer(), Some(publisher.as_str()));
        assert!(
            !matches!(out.panel, FeedPanel::NobodySelected),
            "a visitor who chose nothing is reading, not looking at an empty pane"
        );
        let row = out.known.iter().find(|r| r.peer_id == publisher).unwrap();
        assert!(row.home, "and the row says why it is the one on screen");
        assert!(row.selected, "the highlight follows the EFFECTIVE selection");
        assert!(!row.followed, "⛔ reading is not following — nothing durable was written");
        assert!(out.follows.is_empty(), "the registry is untouched");
    }

    /// **An explicit choice outranks the fallback**, and the home row stops
    /// being highlighted — otherwise two rows would claim to be what is on
    /// screen.
    #[test]
    fn an_explicit_choice_outranks_the_home_fallback() {
        let (mut model, peers, me) = model();
        let (publisher, other) = (peer_id(21), peer_id(22));
        route(&peers, &me, &publisher);
        route(&peers, &me, &other);
        home(&peers, &me, &publisher);

        model.select(&other);
        let out = model.render_output(&peers, &me, CLOCK);
        assert_eq!(out.selected_peer(), Some(other.as_str()));
        let row = |p: &str| out.known.iter().find(|r| r.peer_id == p).unwrap().clone();
        assert!(row(&other).selected);
        assert!(!row(&publisher).selected, "the home row is no longer the one being read");
        assert!(row(&publisher).home, "but it is still the home publisher");
    }

    /// **The home publisher sorts first**, because it is the row a visitor
    /// arrived for — and the rest by peer id, so the list cannot reshuffle
    /// between frames on nothing.
    #[test]
    fn the_home_publisher_sorts_first_and_the_rest_are_a_total_order() {
        let (model, peers, me) = model();
        let mut others: Vec<String> = (30u8..33).map(peer_id).collect();
        let publisher = peer_id(40);
        for p in others.iter().chain(std::iter::once(&publisher)) {
            route(&peers, &me, p);
        }
        home(&peers, &me, &publisher);
        others.sort();

        let out = model.render_output(&peers, &me, CLOCK);
        let ids: Vec<String> = out.known.iter().map(|r| r.peer_id.clone()).collect();
        assert_eq!(ids[0], publisher, "the home publisher leads: {ids:?}");
        assert_eq!(&ids[1..], &others[..], "and the rest are sorted: {ids:?}");
    }

    /// **With no deployment there is no fallback, and that is an honest
    /// `NobodySelected`** rather than a guess at which of several publishers
    /// somebody meant — AP54's invention, which this surface must not make.
    #[test]
    fn with_no_home_declared_nothing_is_selected_on_this_profiles_behalf() {
        let (model, peers, me) = model();
        route(&peers, &me, &peer_id(51));
        route(&peers, &me, &peer_id(52));

        let out = model.render_output(&peers, &me, CLOCK);
        assert_eq!(out.known.len(), 2, "both are offered");
        assert!(out.selected_peer().is_none(), "and neither is chosen for them");
        assert!(matches!(out.panel, FeedPanel::NobodySelected));
    }
}

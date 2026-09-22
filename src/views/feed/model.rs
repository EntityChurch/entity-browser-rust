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

use super::output::{EntryRow, FeedOutput, FeedPanel, FollowRow, GathererRow, Notice, Via};

pub struct FeedModel {
    window_id: WindowId,
    poller: FeedPoller,
    selected: Option<String>,
    notice: Option<Notice>,
}

impl FeedModel {
    pub fn new(window_id: WindowId, repaint: crate::content_site::resolver::RepaintCell) -> Self {
        Self { window_id, poller: FeedPoller::new(repaint), selected: None, notice: None }
    }

    /// Hand the poller the repaint handle the render path owns — see
    /// [`crate::feed_fetch::FeedPoller::set_repaint`] for why an unfilled cell
    /// is a feed that loads forever.
    pub fn set_repaint(&self, repaint: crate::window::RepaintFn) {
        self.poller.set_repaint(repaint);
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
    pub fn render_output(&self, peers: &Peers, our_peer_id: &str, now: f64) -> FeedOutput {
        let follows: Vec<FollowRow> = feed_follows::list(peers, our_peer_id)
            .into_iter()
            .map(|f| FollowRow {
                selected: self.selected.as_deref() == Some(f.subject.as_str()),
                peer_id: f.subject,
            })
            .collect();

        let panel = match &self.selected {
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

        FeedOutput {
            window_id: self.window_id,
            follows,
            gatherers,
            selected: self.selected.clone(),
            panel,
            notice: self.notice,
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
        assert_eq!(out.selected.as_deref(), Some(them.as_str()));
        assert_eq!(out.notice, Some(Notice("feed.notice.followed")));
        assert_eq!(out.follows.len(), 1);
        assert!(out.follows[0].selected, "the row is marked as the one on screen");

        m.follow(&peers, &me, &them, NOW_MS + 1);
        let out = m.render_output(&peers, &me, CLOCK);
        assert_eq!(out.notice, Some(Notice("feed.notice.already")));
        assert_eq!(out.selected.as_deref(), Some(them.as_str()), "still showing them");
    }

    /// A refused follow says which refusal it was and **selects nobody** — the
    /// panel must not start walking a peer id that was rejected.
    #[test]
    fn a_refused_follow_selects_nobody_and_names_its_own_refusal() {
        let (mut m, peers, me) = model();

        m.follow(&peers, &me, "not-a-peer-id", NOW_MS);
        let out = m.render_output(&peers, &me, CLOCK);
        assert_eq!(out.notice, Some(Notice("feed.notice.not_a_peer_id")));
        assert_eq!(out.selected, None);
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
        assert_eq!(out.selected, None, "off the screen");
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
        });

        assert_eq!(row.text, "hello");
        assert_eq!(row.created_at, NOW_MS);
        assert_eq!(row.id_short.len(), 12);
        assert!(hash.to_hex().starts_with(&row.id_short));
        assert!(!row.attributed, "FEED-R4: no signature means nobody is named");
        assert_eq!(row.attribution_key, "feed.attr.no_signature");
    }
}

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
use crate::feed_fetch::{FeedPoller, FeedState};
use crate::feed_read::{Attribution, Unattributed};
use crate::feed_route::{self, Resolution};
use crate::peers::Peers;
use crate::window::WindowId;

use super::output::{EntryRow, FeedOutput, FeedPanel, FollowRow, Notice};

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
                let route = feed_route::plan(
                    connected,
                    origin.as_deref(),
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

        FeedOutput {
            window_id: self.window_id,
            follows,
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
        Resolution::Served { via, entries } => {
            FeedPanel::Entries { via: via_key(via), rows: entries.iter().map(entry_row).collect() }
        }
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

/// Catalog key for the leg that served a feed.
///
/// **A `match` rather than `format!("feed.via.{via}")`**, so a new leg is a
/// compile-time decision about what to call it rather than a key that silently
/// resolves to nothing in thirty locales.
fn via_key(via: &str) -> &'static str {
    match via {
        "live" => "feed.via.live",
        _ => "feed.via.published",
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

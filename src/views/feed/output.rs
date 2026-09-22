//! Renderer-neutral output for the Feed window.
//!
//! **Every string here is a catalog KEY, never a sentence.** The reasoning about
//! which key applies is the model's — that is `doctor.rs`'s rule, earned when a
//! verdict surface shipped English-only: a translator needs to know that
//! *"we could not check this"* must not read as *"this is not theirs"*, and a
//! JSON catalog has nowhere to say it, while a renderer has no business
//! deciding it.

#![allow(dead_code)]

use crate::window::WindowId;

#[derive(Debug, Clone, PartialEq)]
pub struct FeedOutput {
    pub window_id: WindowId,
    /// Who this profile follows, sorted and stable.
    pub follows: Vec<FollowRow>,
    /// ⭐ **Every publisher this profile can reach** — the browse list, so that
    /// arriving at a deployment and opening this window does not require
    /// pasting a 45-character peer id. See [`KnownRow`].
    pub known: Vec<KnownRow>,
    /// Who this profile reads other authors **through** — `APP-CONVENTION-FEED`
    /// §6's gatherers. A separate list from [`Self::follows`] because it is a
    /// different relationship, not a flag on the same one.
    pub gatherers: Vec<GathererRow>,
    /// Whose feed the panel is showing, if any — **and what they are to this
    /// profile**, because the panel offers a control that depends on it.
    ///
    /// See [`Selection`]. Read the peer id through [`FeedOutput::selected_peer`]
    /// where the relation is not wanted.
    pub selected: Option<Selection>,
    pub panel: FeedPanel,
    /// The result of the last Follow press, if there was one this session.
    pub notice: Option<Notice>,
    /// **Your own posts**, newest first, read out of your own tree.
    ///
    /// Separate from [`Self::panel`], which is whoever you selected — including,
    /// possibly, yourself. The two would render identically and mean different
    /// things: one is a read over a road, the other is what is in your tree
    /// right now.
    pub own_posts: Vec<OwnPostRow>,
    /// The result of the last compose act, if there was one this session.
    ///
    /// Session-only, like [`Self::notice`] — what you last did is not a fact
    /// about the profile, and a removal's sentence surviving a reload would be
    /// an answer to a question nobody asked twice.
    pub compose_notice: Option<ComposeNotice>,
    /// Whether the *Manage* section is expanded. Session-only; see the model.
    pub manage_open: bool,
    /// Whether this profile holds the authoring key for the bound peer.
    ///
    /// Drives whether the composer is offered at all. **Spelled positively** so
    /// a future reason to disable it cannot silently start rendering as
    /// available — `AppServerView::is_serving`'s bug, one surface over.
    pub can_author: bool,
}

impl FeedOutput {
    /// Whose feed is on screen, without the relation.
    ///
    /// Exists because most callers — and every gate written before the panel
    /// had a control — want only the peer id, and `selected.as_ref().map(…)` at
    /// each of them would read as if the relation were incidental. It is not:
    /// it decides which button the panel draws.
    pub fn selected_peer(&self) -> Option<&str> {
        self.selected.as_ref().map(|s| s.peer_id.as_str())
    }
}

/// Whose feed the panel is showing, and what they are to this profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub peer_id: String,
    /// Already in the durable follow registry.
    pub followed: bool,
    /// This profile's own peer.
    pub own: bool,
    /// This deployment's own publisher — the row a visitor arrived for, and the
    /// panel's fallback when nobody has chosen.
    pub home: bool,
}

impl Selection {
    /// ⭐ **What to call this publisher, given that we have no name for them.**
    ///
    /// ⛔ **There is no display name in this system and we may not invent one.**
    /// `peers::peer_metadata` resolves through `sdk_for`, so it answers only for
    /// peers that live in this process — a foreign publisher has no label
    /// anywhere. The registry's by-name index is enumerable but yields *names*,
    /// and reversing it to `peer → name` costs one resolve per name, which is
    /// the same O(list) round trip `KnownRow` refuses for probing. And the real
    /// answer — a profile — is `APP-CONVENTION-FEED` §12's `F-6`, whose **shape
    /// is ruled and whose path arch is deliberately holding**, so minting one
    /// here would make us the baseline for a decision that is not ours.
    ///
    /// So the heading says the one true thing we do know, and the id is
    /// rendered in full beside it rather than shortened. **Never truncated:**
    /// half a peer id cannot be copied and still cannot be read, which is
    /// strictly worse than the whole one.
    ///
    /// Three outcomes; `own` outranks `home` because a deployment publishing
    /// under this profile's own peer is both, and *this is you* is the more
    /// specific statement.
    /// ⛔ **The generic arm reuses the browse table's column key rather than
    /// minting a `feed.panel.publisher`.** "Publisher" already has a key, and
    /// one English word gets one key — `i18n-locale-check` reds on a second
    /// (Hungarian rendered the pair `Kiadó` / `Közzétevő`), which is C15 inside
    /// the catalog with a translator on the far end. The key's name says `col`
    /// because the column asked for the word first; it is one noun either way.
    pub fn heading_key(&self) -> &'static str {
        if self.own {
            "feed.panel.you"
        } else if self.home {
            "feed.panel.home"
        } else {
            "feed.known.col.publisher"
        }
    }
}

/// ⭐ **Which follow control a surface may offer for one publisher — three
/// outcomes, not a `bool`.**
///
/// *Follow them* and *stop following them* are the two a `bool` can express.
/// The third is the one that matters: **your own peer has no control at all**,
/// because `feed_follows::follow` refuses it with its own word
/// (`FollowOutcome::ThatIsYou`) — so a Follow button on your own row is a
/// control whose only possible outcome is a refusal. The browse list offered
/// exactly that until this decision existed, which is what a two-state answer
/// to a three-state question costs.
///
/// One expression, two call sites (the panel head and the browse row), so the
/// two cannot disagree about the same peer — which they could while the
/// renderer decided it inline in each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// Not followed, and not us — offer **Follow**.
    Stranger,
    /// In the follow registry — offer **Unfollow**.
    Followed,
    /// Us. Offer nothing.
    Own,
}

/// [`Relation`] from the two facts a row already carries.
///
/// `own` outranks `followed` deliberately: the registry verb refuses to file
/// your own peer, so the pair cannot both be true through any supported path —
/// and if one ever arrives (a hand-written entity, an older build), the arm
/// that offers no control is the safe one.
pub fn relation(own: bool, followed: bool) -> Relation {
    if own {
        Relation::Own
    } else if followed {
        Relation::Followed
    } else {
        Relation::Stranger
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FollowRow {
    pub peer_id: String,
    pub selected: bool,
}

/// One publisher **this profile already knows how to reach** — the browse list.
///
/// ⭐ **The set is the ROUTED set, not everything we have ever heard of**, and
/// that is the design rather than a shortcut. A row with no origin can only
/// produce `FeedPanel::NoRoute`, so offering it is AP54's shape pointed
/// forward: *a viewer must not offer what it cannot open.* What populates it
/// costs nothing new — `origins::list_origins` is the same supersession-resolving
/// accessor the panel already reads, and a deployment's own publisher is in it
/// because `adopt_deployment_origin` put it there at boot.
///
/// **Rows are not probed.** `publication_probe` could pre-annotate each with
/// *does this peer publish a feed*, and it would cost one signed-root walk per
/// row — O(list) round trips to decorate a list. Selecting a row reads it, and
/// [`FeedPanel`]'s four states already answer the same question honestly at the
/// moment somebody asks it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownRow {
    pub peer_id: String,
    /// Already in the durable follow registry.
    pub followed: bool,
    /// The panel is showing them right now — including by the home fallback,
    /// which is why this is computed from the *effective* selection and not
    /// from what somebody clicked.
    pub selected: bool,
    /// ⭐ **This deployment's own publisher.** The row a visitor arriving at a
    /// domain is looking for, and the one the panel falls back to when nobody
    /// has chosen — see `FeedModel::effective_selection`.
    pub home: bool,
    /// Us. Marked rather than filtered: *your own posts* have their own section
    /// and read out of the tree, while this row reads them back over a road, so
    /// the two are different facts about the same peer and hiding one of them
    /// makes a publisher's own view of themselves unreachable.
    pub own: bool,
}

/// One gatherer this profile reads through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GathererRow {
    pub peer_id: String,
    /// Whether this deployment knows where their tree is served.
    ///
    /// ⛔ **Surfaced rather than silently filtered.** A gatherer with no
    /// registered origin contributes **no leg** — there is no URL to build, and
    /// inventing one relative to the page is `OriginFeedSource`'s empty-origin
    /// defect, whose 404 read as *"this publisher has no feed"*. But dropping
    /// the row from the screen as well would leave somebody looking at a list
    /// they added a peer to, wondering why nothing changed. The row stays and
    /// says so.
    pub routed: bool,
}

/// What the entry panel is showing.
///
/// **`Loading` and `NoPosts` are separate variants and that is the whole
/// point** — an in-flight walk rendered as *"this publisher has not posted
/// anything"* is a lie about somebody's work, and it is the collapse
/// `FeedStep::Wait` exists one layer down to prevent.
#[derive(Debug, Clone, PartialEq)]
pub enum FeedPanel {
    /// Nobody is selected. Distinct from following nobody.
    NobodySelected,
    /// **No origin is registered for this publisher, so there is nowhere to
    /// ask.** Its own state rather than a `Failed`: a walk that failed was
    /// attempted and this one cannot be, and the two send a person to different
    /// places — one is *"try again / the publisher is down"*, the other is
    /// *"this deployment does not know where they are hosted"*. `get_origin`
    /// returning `None` is the whole condition.
    NoRoute,
    /// A walk is in flight.
    Loading,
    /// Every leg of the route answered and the publisher has no entries.
    ///
    /// **Distinct from `NoRoute`**, which is the same blank panel for the
    /// opposite reason: there we never asked.
    NoPosts,
    /// Entries, and **which leg served them**.
    ///
    /// `via` is shown rather than kept for the log because the legs are not
    /// interchangeable to a reader: a live read is as fresh as the author is, a
    /// published one is as fresh as their last publish, and a mirror is somebody
    /// else's reading which §6.1 rule 2 allows to be short. Somebody deciding
    /// whether they are seeing everything needs to know which they got.
    Entries { via: Via, rows: Vec<EntryRow> },
    /// The walk failed. Carries the reason verbatim — it is a diagnostic from
    /// the transport, not a sentence we authored, so it is shown beside a
    /// localized heading rather than in place of one.
    Failed { detail: String },
}

/// Which leg served the entries on screen.
///
/// ⛔ **The mirror arm carries a peer id and the other two do not, and that
/// asymmetry is `FEED-R13` / §6.1 rule 3 made structural.** Attribution follows
/// each entry's own detached signature, always; a surface naming the gatherer as
/// the author is non-conformant. **The *via* line is the single place a
/// gatherer's id may legitimately appear**, so it is the only place this type
/// admits one — and [`EntryRow`] has no field it could travel in, which is what
/// makes "never a byline" a property of the types rather than a rule a renderer
/// has to remember.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Via {
    /// A live connection to the author.
    Live,
    /// The author's own published tree.
    Published,
    /// A gatherer's mirror — **whose** is the whole point; see the type doc.
    Mirror { gatherer: String },
}

impl Via {
    /// The catalog key for this leg.
    ///
    /// **A `match`, never `format!("feed.via.{…}")`** — a fourth leg must be a
    /// compile-time decision about what to call it, not a key that silently
    /// resolves to nothing in thirty locales.
    pub fn key(&self) -> &'static str {
        match self {
            Via::Live => "feed.via.live",
            Via::Published => "feed.via.published",
            Via::Mirror { .. } => "feed.via.mirror",
        }
    }

    /// The gatherer's peer id, when there is one. `None` for the author's own
    /// legs, which is what the renderer branches on rather than on the variant.
    pub fn source_peer(&self) -> Option<&str> {
        match self {
            Via::Mirror { gatherer } => Some(gatherer.as_str()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntryRow {
    /// The entry's own content hash, shortened for display. It is the entry's
    /// identity (§2.2.1 makes every reference to one a pin), so it is the only
    /// honest thing to show as an id.
    pub id_short: String,
    /// The body's authored `fallback` — EMBED §3 makes it mandatory and
    /// non-empty precisely so that a reader with no renderer for the payload
    /// still has a sentence.
    ///
    /// ⚠ **This is the DEGRADATION, not the body.** Draw [`Self::body`]; this
    /// field is what that resolves to on the ladder's last rung, and it is kept
    /// separate so a renderer cannot reach for it by accident — which is
    /// precisely what both call sites did until 2026-09-16.
    pub text: String,
    /// ⭐ **What a person actually reads** — EMBED §6's ladder, decided by
    /// [`crate::feed_body::decide`].
    pub body: crate::feed_body::BodyRender,
    pub created_at: u64,
    /// Whether a renderer may name the author. Mirrors
    /// [`Attribution::may_name_the_author`](crate::feed_read::Attribution::may_name_the_author)
    /// and is **spelled positively** for its reason.
    pub attributed: bool,
    /// Catalog key for the attribution verdict — one of `feed.attr.*`.
    pub attribution_key: &'static str,
}

/// Catalog key for the outcome of a Follow press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Notice(pub &'static str);

/// One of **your own** posts, read back out of your own tree.
///
/// ⭐ **A separate type from [`EntryRow`], and the difference is the whole
/// reason.** An `EntryRow` is somebody else's post arriving over a road, so it
/// carries an attribution verdict — *may a renderer name the author* — which is
/// a question about evidence. Your own post in your own tree does not raise that
/// question, and giving it an `attributed: true` field would be a claim
/// manufactured by the renderer rather than checked by
/// [`crate::feed_read::attribute`]. What it carries instead is what a *removal*
/// needs: the full hash, because §7.3's unbinding is by address and a shortened
/// one does not name a binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnPostRow {
    /// The full content hash, hex. **Not shortened** — it is what `Remove`
    /// sends back.
    pub hash_hex: String,
    /// Shortened, for display beside the text.
    pub id_short: String,
    /// The authored `fallback` — see [`EntryRow::text`]; draw [`Self::body`].
    pub text: String,
    /// What a person reads. Your own post goes through the **same** ladder a
    /// stranger's does — one code path, which is `SYSTEM-DATA-EXCHANGE` §1.1's
    /// rule applied where it is cheapest to get wrong.
    pub body: crate::feed_body::BodyRender,
    pub created_at: u64,
}

/// What the composer can say after an act. Each is a catalog key.
///
/// ⛔ **`Removed` is not a success message and must not be rendered as one.**
/// `FEED-R21` is a MUST NOT — *a conformant application MUST NOT present
/// removal as deletion* — and §7.5 puts the sentence *"at the moment of the
/// action rather than in a help page"*. So the removal outcome carries
/// [`crate::feed_compose::RemovalMeaning::COPY_KEY`] and nothing else: there is
/// no cheerful variant for it to be confused with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeNotice {
    /// The post is in your tree and a connected peer can read it.
    Posted,
    /// Nothing was typed.
    Empty,
    /// This profile does not hold the authoring key for the bound peer, so it
    /// cannot sign. Its own outcome rather than a generic failure: it is not
    /// something the person did wrong and not something retrying fixes.
    NotOurPeer,
    /// The mint or the encode refused. Ours.
    Refused,
    /// §7.5's sentence. **Unpublication, never deletion.**
    Removed,
}

impl ComposeNotice {
    /// The catalog key. `Removed`'s is the composer's own constant rather than a
    /// `feed.*` sibling, so the wording and the rule it discharges live
    /// together.
    pub fn key(self) -> &'static str {
        match self {
            ComposeNotice::Posted => "feed.compose.posted",
            ComposeNotice::Empty => "feed.compose.empty",
            ComposeNotice::NotOurPeer => "feed.compose.not_our_peer",
            ComposeNotice::Refused => "feed.compose.refused",
            ComposeNotice::Removed => crate::feed_compose::RemovalMeaning::COPY_KEY,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⭐ **Three outcomes, and the count is asserted** so a fourth relation
    /// cannot quietly reuse one of these arms. The pair a two-state answer
    /// merges is `Stranger` and `Own` — both *"not followed"*, and only one of
    /// them may be offered a Follow button.
    #[test]
    fn a_publisher_is_a_stranger_a_follow_or_us_and_never_two_of_them() {
        let all = [
            relation(false, false),
            relation(false, true),
            relation(true, false),
        ];
        assert_eq!(all, [Relation::Stranger, Relation::Followed, Relation::Own]);

        let distinct: std::collections::BTreeSet<_> =
            all.iter().map(|r| format!("{r:?}")).collect();
        assert_eq!(distinct.len(), 3, "each relation needs its own answer");
    }

    /// **The arm that offers nothing wins a contradiction.** `feed_follows::follow`
    /// refuses our own peer (`FollowOutcome::ThatIsYou`), so the pair cannot both
    /// be true through any supported path — but a hand-written entity or an older
    /// build could produce one, and *offer no control* is the safe answer to a
    /// state that should not exist.
    #[test]
    fn our_own_peer_is_never_offered_a_follow_control_even_if_a_row_claims_we_follow_it() {
        assert_eq!(relation(true, true), Relation::Own);
    }

    /// ⭐ **Three headings, each its own key, and `own` outranks `home`.**
    ///
    /// A deployment publishing under this profile's own peer is *both*, and
    /// *this is you* is the more specific statement — the one a person can act
    /// on. The count is asserted so a fourth case cannot quietly reuse one of
    /// these, which is how a heading starts lying about a publisher.
    #[test]
    fn a_publisher_with_no_name_is_still_called_something_and_you_outrank_the_deployment() {
        let sel = |own, home| Selection {
            peer_id: "2KSOMEBODY".to_string(),
            followed: false,
            own,
            home,
        };
        // The generic arm reuses the table's column key — see `heading_key`.
        assert_eq!(sel(false, false).heading_key(), "feed.known.col.publisher");
        assert_eq!(sel(false, true).heading_key(), "feed.panel.home");
        assert_eq!(sel(true, false).heading_key(), "feed.panel.you");
        assert_eq!(
            sel(true, true).heading_key(),
            "feed.panel.you",
            "a deployment publishing as us is still us"
        );

        let distinct: std::collections::BTreeSet<_> =
            [sel(false, false), sel(false, true), sel(true, false)]
                .iter()
                .map(|s| s.heading_key())
                .collect();
        assert_eq!(distinct.len(), 3, "each heading needs its own key");
    }

    /// ⛔ **The heading is never the peer id, and never a piece of one.**
    ///
    /// Both directions are the defect this replaced: the id *was* the section
    /// title, and the obvious repair — shortening it — is worse, because half an
    /// identifier can neither be read nor copied. The id is rendered in full
    /// beside the heading instead. This asserts the property rather than the
    /// wording, so a re-worded heading does not red it.
    #[test]
    fn no_heading_key_carries_any_part_of_the_publishers_identifier() {
        let peer = "2KABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ABCDEFG";
        for (own, home) in [(false, false), (false, true), (true, false)] {
            let key = Selection {
                peer_id: peer.to_string(),
                followed: false,
                own,
                home,
            }
            .heading_key();
            assert!(key.starts_with("feed."), "a catalog key, never a value");
            assert!(
                !peer.contains(key) && !key.contains(&peer[..8]),
                "the heading must not carry the identifier or a prefix of it"
            );
        }
    }

    /// `selected_peer` is the whole of what a caller that does not care about
    /// the relation should see — and `None` stays `None` rather than becoming a
    /// `Selection` with an empty peer id.
    #[test]
    fn nobody_selected_reads_as_nobody_rather_than_an_empty_publisher() {
        let mut out = FeedOutput {
            window_id: 1,
            follows: Vec::new(),
            known: Vec::new(),
            gatherers: Vec::new(),
            selected: None,
            panel: FeedPanel::NobodySelected,
            notice: None,
            own_posts: Vec::new(),
            compose_notice: None,
            manage_open: false,
            can_author: false,
        };
        assert_eq!(out.selected_peer(), None);

        out.selected = Some(Selection {
            peer_id: "2KSOMEBODY".to_string(),
            followed: true,
            own: false,
            home: false,
        });
        assert_eq!(out.selected_peer(), Some("2KSOMEBODY"));
    }
}

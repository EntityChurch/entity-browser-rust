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
    /// Who this profile reads other authors **through** — `APP-CONVENTION-FEED`
    /// §6's gatherers. A separate list from [`Self::follows`] because it is a
    /// different relationship, not a flag on the same one.
    pub gatherers: Vec<GathererRow>,
    /// Whose feed the panel is showing, if any.
    pub selected: Option<String>,
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
    /// Whether this profile holds the authoring key for the bound peer.
    ///
    /// Drives whether the composer is offered at all. **Spelled positively** so
    /// a future reason to disable it cannot silently start rendering as
    /// available — `AppServerView::is_serving`'s bug, one surface over.
    pub can_author: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FollowRow {
    pub peer_id: String,
    pub selected: bool,
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
    pub text: String,
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
    pub text: String,
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

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
    /// Which pane is on screen. Session-only; see the model.
    pub tab: FeedTab,
    /// Whether this profile may author as the bound peer, and if not, why.
    ///
    /// Drives whether the composer is offered at all — and **carries the reason
    /// rather than a bool**, because the two refusals are different facts with
    /// different remedies (see [`crate::feed_compose::AuthorKey`]). It was a
    /// `bool` until 2026-09-17, which is how a second tab was told its own
    /// profile did not hold its own key.
    ///
    /// The predicate that drives the control is spelled **positively**
    /// ([`Self::can_author`]) so a future reason to disable the composer cannot
    /// silently start rendering as available — `AppServerView::is_serving`'s
    /// bug, one surface over.
    pub author_key: crate::feed_compose::AuthorKey,
    /// Whether this session has acknowledged the caveat. Session-only; see the
    /// model's field for why it is not carried across tabs.
    pub caveat_dismissed: bool,
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

    /// May the composer be offered — the one positive predicate, so no renderer
    /// re-derives it by negating a refusal.
    ///
    /// **Two arms say yes.** A session identity authors: *persistence is not
    /// permission*, and the thing that is true about it goes in
    /// [`Self::compose_caveat`] beside the control rather than in place of
    /// it.
    pub fn can_author(&self) -> bool {
        self.author_key.may_author()
    }

    /// What to say beside the composer, or `None` when there is nothing to say.
    ///
    /// ⭐⭐ **COPY ONLY — it does not decide anything, and that is the point.**
    /// The first cut of this split had the renderer branch on *"is there a
    /// refusal key"*, so [`Self::can_author`] had **no consumer in the render
    /// path** and the surface re-derived the decision from the presence of a
    /// sentence. Two expressions of one rule, C15's shape, and it was measured
    /// rather than reasoned: neutering `AuthorKey::may_author` back to the
    /// shipped refusal left the browser gate **green**, because nothing the
    /// browser draws was reading it. Where the note goes is
    /// [`Self::can_author`]'s answer; what it says is this one's.
    pub fn compose_note(&self) -> Option<ComposeNote> {
        match self.author_key {
            crate::feed_compose::AuthorKey::Held => None,
            crate::feed_compose::AuthorKey::NotOurs => Some(ComposeNote {
                key: "feed.compose.not_our_peer",
                // i18n-ignore — a wire token read by gates, never rendered
                reason: "not-ours",
            }),
            crate::feed_compose::AuthorKey::SessionOnly => Some(ComposeNote {
                key: "feed.compose.session_identity",
                // i18n-ignore — ditto
                reason: "session-identity",
            }),
        }
    }

    /// What to draw **above a working composer**, or `None` when there is
    /// nothing left to say.
    ///
    /// A caveat is a standing fact about this session, so it is drawn on every
    /// render until the reader says they have read it — which is what makes a
    /// dismiss control right here and wrong on a refusal. The split is the
    /// same one [`Self::can_author`] and [`Self::compose_note`] already carry:
    /// *there is something to tell you* is not *and therefore you may not*,
    /// and now *and you have not read it yet* is a third fact rather than a
    /// third meaning loaded onto one of the first two.
    ///
    /// ⛔ **The refusal is deliberately not routed through here.** It explains a
    /// control that is absent, so dismissing it would leave a dead box with
    /// nothing beside it — the state `a_withheld_composer_always_says_why`
    /// exists to make unreachable.
    pub fn compose_caveat(&self) -> Option<ComposeNote> {
        if !self.can_author() || self.caveat_dismissed {
            return None;
        }
        self.compose_note()
    }
}

/// A sentence to draw beside the composer, and a stable word for it.
///
/// The `reason` is what a gate reads off the DOM — **never the sentence**, which
/// is translated in thirty locales and is red the day somebody improves the
/// wording (this window's own anchor lesson, paid for once already). It travels
/// with the key rather than in a second accessor so the two cannot drift into
/// describing different situations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComposeNote {
    /// Catalog key for the sentence a person reads.
    pub key: &'static str,
    /// Stable token for a gate. Not rendered.
    pub reason: &'static str,
}

/// ⭐⭐ **Which of the window's three panes is on screen — three ALTERNATIVES,
/// not three sections.**
///
/// Reading somebody's feed, writing your own and administering the list are
/// three different acts on three different trees, and this window stacked them:
/// the reading surface first, then a collapsible *Your feed*, then a
/// collapsible *Manage sources*. With a real archive on screen — 34 posts, the
/// number the published-feed rig serves — the other two headers sat a screen and
/// a half below the fold, so *"post something of my own"* meant scrolling past
/// everything somebody else had written. The collapse was not the problem; the
/// **order** was, and no ordering of a stack fixes it, because whichever pane is
/// second is under the first one's content.
///
/// ⛔ **The tab is not the selection.** Which publisher you are reading lives in
/// [`FeedOutput::selected`]; this is only which pane draws. They are separate
/// because [`FeedTab::Read`] has two pages of its own — the list of publishers
/// and one publisher's posts — and *that* page is decided by whether anybody is
/// selected, which is the Knowledge Base's list/reader shape and the Site
/// Browser's navigation. A person gets out of an archive by going **back**, not
/// by scrolling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedTab {
    /// Publishers you can reach, and one publisher's posts.
    Read,
    /// **Your own** posts — the composer and what is in your tree.
    Yours,
    /// Who you follow, who you read through, and the peer-id box.
    Sources,
}

impl FeedTab {
    /// The stable wire value — what a press dispatches and what `data-tab`
    /// carries. **Never the label**, which is translated into thirty locales.
    pub fn value(self) -> &'static str {
        match self {
            FeedTab::Read => "read",
            FeedTab::Yours => "yours",
            FeedTab::Sources => "sources",
        }
    }

    /// The catalog key for the tab's caption.
    ///
    /// ⭐ **All three reuse keys the catalog already has**, which is the
    /// `btn.refresh` lesson applied before the fact: one English word gets one
    /// key, and a fresh `feed.tab.read` beside `feed.read` is how one noun ends
    /// up translated two ways with a translator on the far end.
    pub fn label_key(self) -> &'static str {
        match self {
            FeedTab::Read => "feed.read",
            FeedTab::Yours => "feed.compose.heading",
            FeedTab::Sources => "feed.manage",
        }
    }

    /// Read a tab back from a press. **Unknown falls to `Read`** — the reading
    /// surface is this window's subject, so an event we do not recognise lands
    /// on the pane a person opened the window for rather than on a form.
    pub fn from_value(v: &str) -> Self {
        match v {
            "yours" => FeedTab::Yours,
            "sources" => FeedTab::Sources,
            _ => FeedTab::Read,
        }
    }

    /// Every pane, in the order they are offered. Reading first: it is what the
    /// window is for.
    pub const ALL: [FeedTab; 3] = [FeedTab::Read, FeedTab::Yours, FeedTab::Sources];
}

/// Whose feed the panel is showing, and what they are to this profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub peer_id: String,
    /// Already in the durable follow registry.
    pub followed: bool,
    /// This profile's own peer.
    pub own: bool,
    /// What to call them — see [`PeerLabel`]. The id is still rendered in full
    /// beneath it; a word is a handle, not an identity.
    pub label: PeerLabel,
    /// Where this deployment says they are hosted, verbatim. `None` when no
    /// origin is registered, `Some("")` when it is **same-origin**, which is a
    /// recorded value and not an absence.
    pub origin: Option<String>,
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
    /// ⛔ **There were three outcomes and the middle one is retired** —
    /// *"This site's publisher"*, for the deployment's own peer. That is a
    /// routing fact about how this profile can reach them and it said nothing
    /// about the feed on screen, while reading as a status the publisher holds;
    /// see [`crate::views::feed::model::FeedModel::effective_selection`] for the
    /// whole of why no publisher is privileged on this surface any more.
    ///
    /// ⛔ **The generic arm reuses the browse table's column key rather than
    /// minting a `feed.panel.publisher`.** "Publisher" already has a key, and
    /// one English word gets one key — `i18n-locale-check` reds on a second
    /// (Hungarian rendered the pair `Kiadó` / `Közzétevő`), which is C15 inside
    /// the catalog with a translator on the far end. The key's name says `col`
    /// because the column asked for the word first; it is one noun either way.
    pub fn heading_key(&self) -> &'static str {
        if self.own {
            "feed.panel.you"
        } else {
            "feed.known.col.publisher"
        }
    }
}

/// ⭐⭐ **WHAT TO CALL A PUBLISHER WHEN THE ONLY THING WE HAVE IS A KEY — four
/// facts, each from a different place, and none of them a claim about anyone.**
///
/// # The gap this closes
///
/// Every row on this surface rendered a 45-character peer id and nothing else,
/// including the row a person reached by resolving `billslab.com` through a
/// registry thirty seconds earlier. The name was in hand at the moment of the
/// act and thrown away, so the surface downstream had a key and no way back to
/// the word.
///
/// # ⛔ There is still no naming authority, and this does not invent one
///
/// [`Petname`](Self::Petname) and [`Via`](Self::Via) are both
/// `APP-CONVENTION-FEED` §2.4's own fields on the reader's **private** follow
/// record: the label is *"local, chosen by the reader, and never
/// authoritative"* and `via` is *"the identifier as typed or scanned, for
/// provenance display only."* Neither is transmitted, neither is a statement
/// about the publisher, and neither can be wrong in a way that matters — a
/// petname is yours, and a `via` records **what you did**, which does not go
/// stale the way a resolved binding does (AP30: this is not a remote assertion
/// written down as current truth).
///
/// [`Origin`](Self::Origin) is not a name at all — it is where this deployment
/// says the publisher is hosted, a routing fact already in the registry — and it
/// is a separate arm precisely so a surface can render it as one. The honest
/// sentence for it is *hosted at X*, never *called X*.
///
/// # Precedence, and why
///
/// Petname → `via` → origin → nothing. **The reader's own choice outranks
/// everything** (D25's axis: a value the user set is not a value we may
/// override); what they typed to get here outranks where the bytes happen to
/// live, because it is more specific to *this publisher* than a host that may
/// serve several.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerLabel {
    /// You named them. Yours, editable, and never sent anywhere.
    Petname(String),
    /// The identifier you got here by — a registry name you resolved. §2.4's
    /// `via`, provenance display only.
    Via(String),
    /// Where this deployment says they are hosted, host and port only. A
    /// routing fact; render it as one.
    Origin(String),
    /// Nothing but the id. **Its own arm rather than an empty string**, so a
    /// renderer cannot draw a blank heading and a gate can tell *"we have no
    /// word for them"* from *"their word is empty"*.
    Unnamed,
}

impl PeerLabel {
    /// The word to draw, or `None` when there is none.
    ///
    /// [`Origin`](Self::Origin) is deliberately **not** returned here: it is not
    /// a name, and a caller reaching for *"what do I call them"* must not get a
    /// hostname back under that question. The renderer asks for it by name.
    pub fn name(&self) -> Option<&str> {
        match self {
            PeerLabel::Petname(s) | PeerLabel::Via(s) => Some(s.as_str()),
            PeerLabel::Origin(_) | PeerLabel::Unnamed => None,
        }
    }

    /// Where this came from. **A stable token for a gate, never rendered** —
    /// the lesson this window paid for once already, when three gates anchored
    /// on a translated sentence and went red the day it was reworded.
    pub fn source(&self) -> &'static str {
        match self {
            // i18n-ignore — wire tokens read by gates, never rendered
            PeerLabel::Petname(_) => "petname",
            PeerLabel::Via(_) => "via",
            PeerLabel::Origin(_) => "origin",
            PeerLabel::Unnamed => "none",
        }
    }
}

/// Resolve the label for one publisher from the three places a word can come
/// from.
///
/// **Pure and takes strings**, so every combination is gated by `make test`
/// rather than through a browser — and so no caller has to build a `Follow`
/// entity to ask the question.
///
/// Blank is absence at every level: a petname of `"   "` is not a name, and
/// falling through to the next source is what a person who cleared their alias
/// meant. An **empty origin is same-origin**, which is a legitimate recorded
/// value (the 2026-09-17 production defect is about exactly that) and is not a
/// host — so it contributes no label rather than an empty one.
pub fn peer_label(
    petname: Option<&str>,
    via: Option<&str>,
    origin: Option<&str>,
) -> PeerLabel {
    fn word(s: Option<&str>) -> Option<&str> {
        s.map(str::trim).filter(|s| !s.is_empty())
    }
    if let Some(p) = word(petname) {
        return PeerLabel::Petname(p.to_string());
    }
    if let Some(v) = word(via) {
        return PeerLabel::Via(v.to_string());
    }
    if let Some(host) = word(origin).and_then(origin_host) {
        return PeerLabel::Origin(host);
    }
    PeerLabel::Unnamed
}

/// The host (and port) out of an origin URL — `https://billslab.com/x` →
/// `billslab.com`.
///
/// Hand-written rather than through a URL parser because this crate's origins
/// are `{scheme}://{host}[:{port}]` by construction (`deploy_origin` builds
/// them) and a dependency for a `split_once` would be the larger cost. Anything
/// that does not look like that answers `None` — **a string we could not read is
/// not a host**, and showing a mangled one beside a publisher's name is worse
/// than showing nothing.
fn origin_host(origin: &str) -> Option<String> {
    let rest = origin.split_once("://").map(|(_, r)| r).unwrap_or(origin);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    (!host.is_empty()).then(|| host.to_string())
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
    /// What to call them — see [`PeerLabel`].
    pub label: PeerLabel,
    /// The petname exactly as stored, for the alias box to open on. **Not
    /// [`Self::label`]'s word**, which may have fallen through to a `via` or a
    /// host — an edit box pre-filled with something the person did not type is
    /// an edit box that turns a resolved name into a petname on the first Save.
    pub petname: Option<String>,
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
    /// The panel is showing them right now.
    pub selected: bool,
    /// Us. Marked rather than filtered: *your own posts* have their own section
    /// and read out of the tree, while this row reads them back over a road, so
    /// the two are different facts about the same peer and hiding one of them
    /// makes a publisher's own view of themselves unreachable.
    pub own: bool,
    /// What to call them — see [`PeerLabel`]. On this list the
    /// [`PeerLabel::Origin`] arm is the one that usually answers: these are the
    /// peers this deployment routes to, so where they are hosted is the one
    /// thing it knows about all of them.
    pub label: PeerLabel,
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

    /// ⭐ **Two headings, each its own key, and there is no third.**
    ///
    /// The retired third was *"This site's publisher"*. The count is asserted so
    /// a new case cannot quietly reuse one of these, which is how a heading
    /// starts lying about a publisher — and it is **2**, deliberately, so that
    /// re-introducing a privileged publisher has to change this line and read
    /// `heading_key`'s doc on the way past.
    #[test]
    fn a_publisher_with_no_name_is_still_called_something_and_it_is_never_a_status() {
        let sel = |own| Selection {
            peer_id: "2KSOMEBODY".to_string(),
            followed: false,
            own,
            label: PeerLabel::Unnamed,
            origin: None,
        };
        // The generic arm reuses the table's column key — see `heading_key`.
        assert_eq!(sel(false).heading_key(), "feed.known.col.publisher");
        assert_eq!(sel(true).heading_key(), "feed.panel.you");

        let distinct: std::collections::BTreeSet<_> =
            [sel(false), sel(true)].iter().map(|s| s.heading_key()).collect();
        assert_eq!(distinct.len(), 2, "each heading needs its own key");
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
        for own in [false, true] {
            let key = Selection {
                peer_id: peer.to_string(),
                followed: false,
                own,
                label: PeerLabel::Unnamed,
                origin: None,
            }
            .heading_key();
            assert!(key.starts_with("feed."), "a catalog key, never a value");
            assert!(
                !peer.contains(key) && !key.contains(&peer[..8]),
                "the heading must not carry the identifier or a prefix of it"
            );
        }
    }

    /// ⭐ **Three panes, three wire values, three captions — and the count is
    /// asserted**, so a fourth cannot quietly reuse another's value (which would
    /// make two tabs one tab) or another's caption (which would make one of them
    /// unfindable).
    ///
    /// It also pins that every caption **resolves in the catalog**: all three
    /// reuse a key that already existed, and a key with no entry renders as the
    /// key itself — which is worse than English and invisible to a gate that
    /// only checked the mapping.
    #[test]
    fn every_pane_has_its_own_wire_value_and_its_own_caption() {
        assert_eq!(FeedTab::ALL.len(), 3, "a fourth pane needs a row in every match here");
        let values: std::collections::BTreeSet<&str> =
            FeedTab::ALL.iter().map(|t| t.value()).collect();
        assert_eq!(values.len(), 3, "two panes share a wire value: {values:?}");
        let keys: std::collections::BTreeSet<&str> =
            FeedTab::ALL.iter().map(|t| t.label_key()).collect();
        assert_eq!(keys.len(), 3, "two panes share a caption: {keys:?}");
        for key in &keys {
            assert!(crate::i18n::catalog_has("en", key), "{key} is not in the EN base");
        }
    }

    /// A press round-trips, and **an unrecognised one lands on the reading
    /// surface** rather than on a form somebody did not ask for. Reachable from
    /// an older build's persisted event or a hand-dispatched action.
    #[test]
    fn an_unknown_pane_falls_back_to_reading_and_never_to_a_form() {
        for tab in FeedTab::ALL {
            assert_eq!(FeedTab::from_value(tab.value()), tab, "{tab:?} does not round-trip");
        }
        assert_eq!(FeedTab::from_value(""), FeedTab::Read);
        assert_eq!(FeedTab::from_value("compose"), FeedTab::Read);
    }

    /// ⭐⭐ **THE THREE ARMS, AND *WITHHELD* IS ONE OF THEM RATHER THAN TWO.**
    ///
    /// This test used to be called *"each reason the composer is withheld says a
    /// different thing"* and asserted, of the session identity, that *"a refusal
    /// must not offer the composer"*. It was green, it read as a decision, and
    /// the decision was wrong — AP45, in the one place a wrong decision is
    /// hardest to see, because a passing test under a confident name is what
    /// stops anybody re-reading the rule.
    ///
    /// Each arm is pinned on all three axes — may it author, which note is
    /// drawn, which token — and the two sentences are asserted to differ **in
    /// the catalog**, not merely the keys to differ: a second key resolving to
    /// the same English is the `btn.refresh` drift this window already paid for.
    /// It also pins that both resolve at all, since a key with no entry renders
    /// as the key itself.
    #[test]
    fn a_stranger_is_refused_a_temporary_identity_is_warned_and_our_own_is_neither() {
        use crate::feed_compose::AuthorKey;
        let out = |k: AuthorKey| FeedOutput {
            window_id: 1,
            follows: Vec::new(),
            known: Vec::new(),
            gatherers: Vec::new(),
            selected: None,
            panel: FeedPanel::NobodySelected,
            notice: None,
            own_posts: Vec::new(),
            compose_notice: None,
            tab: FeedTab::Yours,
            author_key: k,
            caveat_dismissed: false,
        };

        // Durable: authors, says nothing.
        let held = out(AuthorKey::Held);
        assert!(held.can_author());
        assert_eq!(held.compose_note(), None);

        // Temporary: authors, AND says what is true about it. Two separate
        // rows, because they are the two facts a renderer combines — a surface
        // that merely moved the sentence without offering the box satisfies one
        // of them, and that surface is what shipped.
        let session = out(AuthorKey::SessionOnly);
        assert!(
            session.can_author(),
            "persistence is not permission — a temporary identity's tree is \
             still its own, and writing an entry into it IS the publish"
        );
        assert_eq!(
            session.compose_note().map(|n| n.key),
            Some("feed.compose.session_identity")
        );
        assert_eq!(session.compose_note().map(|n| n.reason), Some("session-identity"));

        // A stranger's peer: the one refusal, and it is about whose peer it is.
        let theirs = out(AuthorKey::NotOurs);
        assert!(!theirs.can_author());
        assert_eq!(theirs.compose_note().map(|n| n.key), Some("feed.compose.not_our_peer"));
        assert_eq!(theirs.compose_note().map(|n| n.reason), Some("not-ours"));

        // Two notes, two sentences, two tokens — none of them shared. The
        // sentences are compared in the **catalog**: a second key resolving to
        // the same English is the `btn.refresh` drift this window already paid
        // for, and it would put the whole point of the split — telling a person
        // which situation they are in — back where it was.
        let notes = [theirs.compose_note().unwrap(), session.compose_note().unwrap()];
        let mut sentences = std::collections::BTreeSet::new();
        for n in notes {
            assert!(crate::i18n::catalog_has("en", n.key), "{} is not in the EN base", n.key);
            sentences.insert(crate::i18n::t(n.key, &[]));
        }
        assert_eq!(sentences.len(), 2, "two notes, one sentence: {sentences:?}");
        let tokens: std::collections::BTreeSet<_> =
            notes.iter().map(|n| n.reason).collect();
        assert_eq!(tokens.len(), 2, "two notes, one token: {tokens:?}");
    }

    /// ⛔ **A withheld composer always says why — the invariant the renderer's
    /// unreachable arm leans on.**
    ///
    /// `render_composer` draws the refusal note from `compose_note()` and would
    /// have nothing to put in it if an arm ever refused silently: a dead box
    /// with no sentence beside it, which is exactly the state the note exists to
    /// replace. Enumerated over every arm, so a fourth one cannot be added
    /// without answering it.
    ///
    /// The converse is deliberately **not** asserted — `SessionOnly` authors
    /// *and* carries a note, and a test demanding "a note implies a refusal"
    /// would re-pin the defect this change removed.
    #[test]
    fn a_withheld_composer_always_says_why() {
        use crate::feed_compose::AuthorKey;
        for k in [AuthorKey::Held, AuthorKey::SessionOnly, AuthorKey::NotOurs] {
            let out = FeedOutput {
                window_id: 1,
                follows: Vec::new(),
                known: Vec::new(),
                gatherers: Vec::new(),
                selected: None,
                panel: FeedPanel::NobodySelected,
                notice: None,
                own_posts: Vec::new(),
                compose_notice: None,
                tab: FeedTab::Yours,
                author_key: k.clone(),
                caveat_dismissed: false,
            };
            if !out.can_author() {
                assert!(
                    out.compose_note().is_some(),
                    "{k:?} withholds the composer and says nothing beside it"
                );
            }
        }
    }

    /// ⭐ **A caveat can be read and put away; a refusal cannot.**
    ///
    /// The dismissal exists because the caveat is a standing fact rather than
    /// news — it is owed once, before the reader types, and after that it is the
    /// surface repeating itself over a box somebody is trying to use.
    ///
    /// ⛔ **The second half is the one that matters and is asserted over every
    /// arm.** Dismissal must not be able to reach a refusal: that sentence
    /// explains a control that is *not there*, so hiding it leaves a dead box
    /// with nothing beside it — precisely the state
    /// `a_withheld_composer_always_says_why` exists to make unreachable. A
    /// dismissal wired to `compose_note()` instead of `compose_caveat()` reds
    /// here rather than shipping a silent refusal.
    #[test]
    fn dismissing_reaches_the_caveat_and_never_the_refusal() {
        use crate::feed_compose::AuthorKey;
        let out = |k: AuthorKey, dismissed: bool| FeedOutput {
            window_id: 1,
            follows: Vec::new(),
            known: Vec::new(),
            gatherers: Vec::new(),
            selected: None,
            panel: FeedPanel::NobodySelected,
            notice: None,
            own_posts: Vec::new(),
            compose_notice: None,
            tab: FeedTab::Yours,
            author_key: k,
            caveat_dismissed: dismissed,
        };

        // The caveat: drawn, then not.
        assert_eq!(
            out(AuthorKey::SessionOnly, false).compose_caveat().map(|n| n.reason),
            Some("session-identity"),
            "an unacknowledged session identity is told about"
        );
        assert_eq!(
            out(AuthorKey::SessionOnly, true).compose_caveat(),
            None,
            "and once read, not told again for the life of this session"
        );
        // …and dismissing it does not withdraw the permission it is about.
        assert!(
            out(AuthorKey::SessionOnly, true).can_author(),
            "dismissing a caveat is reading it, not giving up the composer"
        );

        // Every arm: a composer that is NOT offered still says why, dismissed or
        // not. `compose_note` is what the refusal branch draws and it is not
        // routed through the dismissal.
        for k in [AuthorKey::Held, AuthorKey::SessionOnly, AuthorKey::NotOurs] {
            let o = out(k.clone(), true);
            if !o.can_author() {
                assert!(
                    o.compose_note().is_some(),
                    "{k:?} refuses the composer and a dismissal silenced the reason"
                );
                assert_eq!(
                    o.compose_caveat(),
                    None,
                    "{k:?} refuses, so there is no caveat above a box that is not there"
                );
            }
        }

        // Nothing to say, nothing to dismiss.
        assert_eq!(out(AuthorKey::Held, false).compose_caveat(), None);
    }

    /// ⛔ **The temporary-identity sentence must name a temporary identity, must
    /// not blame the reader's keys, and must not read as a refusal.**
    ///
    /// The refusal it was split out of says *"this profile does not hold its
    /// key"*, which is false here and is what sent somebody looking for a key
    /// problem. The third clause is the one this change adds: for a day the
    /// sentence said a post here *"would not be saved and would not be yours"* —
    /// the second half plainly false, and the whole of it phrased as a reason
    /// the control was gone rather than a fact about a control that is there.
    /// Asserted on the EN base only — a translator is told the same thing in the
    /// catalog comment, and holding thirty locales to an English substring is a
    /// check that measures nothing.
    #[test]
    fn the_temporary_identity_sentence_warns_and_never_refuses() {
        let s = crate::i18n::t("feed.compose.session_identity", &[]);
        assert!(!s.contains("key"), "it is not a key problem: {s}");
        assert!(
            s.contains("temporary identity"),
            "it has to name what IS true: {s}"
        );
        assert!(
            !s.contains("would not be yours"),
            "it IS theirs — this session is that peer and signs as it: {s}"
        );
    }

    /// ⭐ **The precedence, every step of it, and the count.**
    ///
    /// Four sources, four arms, and each one must be reachable — a fallthrough
    /// that skipped a level would be invisible except to whoever had set the
    /// thing that got ignored.
    #[test]
    fn your_own_name_outranks_the_one_you_resolved_and_both_outrank_a_hostname() {
        assert_eq!(
            peer_label(Some("Bill"), Some("billslab.com"), Some("https://bl.example")),
            PeerLabel::Petname("Bill".into()),
            "the reader's own choice wins — D25's axis"
        );
        assert_eq!(
            peer_label(None, Some("billslab.com"), Some("https://bl.example")),
            PeerLabel::Via("billslab.com".into()),
            "what you typed to get here is more specific than where the bytes live"
        );
        assert_eq!(
            peer_label(None, None, Some("https://bl.example")),
            PeerLabel::Origin("bl.example".into())
        );
        assert_eq!(peer_label(None, None, None), PeerLabel::Unnamed);

        let all = [
            peer_label(Some("Bill"), None, None),
            peer_label(None, Some("billslab.com"), None),
            peer_label(None, None, Some("https://bl.example")),
            peer_label(None, None, None),
        ];
        let sources: std::collections::BTreeSet<&str> =
            all.iter().map(|l| l.source()).collect();
        assert_eq!(sources.len(), 4, "each source needs its own token: {sources:?}");
    }

    /// ⛔ **A hostname is not a name, and `name()` must not hand one back.**
    ///
    /// The whole reason `Origin` is a separate arm is that the honest sentence
    /// for it is *hosted at X*, never *called X*. A caller asking *"what do I
    /// call them"* that got a host back would render a routing fact as a
    /// publisher's name — a claim nobody made.
    #[test]
    fn a_hostname_is_not_returned_as_a_name() {
        assert_eq!(peer_label(Some("Bill"), None, None).name(), Some("Bill"));
        assert_eq!(peer_label(None, Some("b.com"), None).name(), Some("b.com"));
        assert_eq!(peer_label(None, None, Some("https://b.example")).name(), None);
        assert_eq!(peer_label(None, None, None).name(), None);
    }

    /// **Blank is absence at every level**, which is what makes clearing an
    /// alias work: a petname of `"  "` falls through to the next source rather
    /// than drawing an empty heading.
    #[test]
    fn a_blank_name_falls_through_instead_of_rendering_as_one() {
        assert_eq!(
            peer_label(Some("   "), Some("billslab.com"), None),
            PeerLabel::Via("billslab.com".into())
        );
        assert_eq!(peer_label(Some(""), Some("  "), None), PeerLabel::Unnamed);
    }

    /// ⭐ **An EMPTY origin is same-origin — a recorded value, not an absence —
    /// and it is still not a host.**
    ///
    /// This is the shape every single-domain deployment publishes (`--bind` with
    /// `@/`), and reading `""` as absence is the 2026-09-17 production defect
    /// one module over. Here the right answer happens to be the same as
    /// absence's — there is no hostname in it — but it must arrive by *"that is
    /// not a host"* rather than by *"there is no origin"*, because an empty
    /// string reaching a formatter renders as `Hosted at `.
    #[test]
    fn same_origin_contributes_no_hostname_rather_than_an_empty_one() {
        assert_eq!(peer_label(None, None, Some("")), PeerLabel::Unnamed);
        assert_eq!(
            peer_label(None, Some("billslab.com"), Some("")),
            PeerLabel::Via("billslab.com".into()),
            "…and it does not shadow a name we do have"
        );
    }

    /// The host, and nothing but the host. Port kept — on a review estate it is
    /// the only thing telling six publishers apart.
    #[test]
    fn a_hostname_is_the_authority_with_the_scheme_and_path_removed() {
        for (origin, want) in [
            ("https://billslab.com", "billslab.com"),
            ("https://billslab.com/", "billslab.com"),
            ("http://localhost:8531", "localhost:8531"),
            ("https://example.org/prefix/x?y#z", "example.org"),
            // No scheme at all: the whole string is the authority.
            ("billslab.com", "billslab.com"),
        ] {
            assert_eq!(
                peer_label(None, None, Some(origin)),
                PeerLabel::Origin(want.into()),
                "{origin}"
            );
        }
        // Nothing that could be a host → no label, rather than a mangled one.
        assert_eq!(peer_label(None, None, Some("https://")), PeerLabel::Unnamed);
        assert_eq!(peer_label(None, None, Some("/relative")), PeerLabel::Unnamed);
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
            tab: FeedTab::Read,
            author_key: crate::feed_compose::AuthorKey::NotOurs,
            caveat_dismissed: false,
        };
        assert_eq!(out.selected_peer(), None);

        out.selected = Some(Selection {
            peer_id: "2KSOMEBODY".to_string(),
            followed: true,
            own: false,
            label: PeerLabel::Unnamed,
            origin: None,
        });
        assert_eq!(out.selected_peer(), Some("2KSOMEBODY"));
    }
}

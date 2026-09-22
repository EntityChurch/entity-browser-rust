//! `feed_route` — **which ways to read one author's feed, in what order.**
//!
//! Not *live or published*. A **priority list**, and the list is mostly decided
//! by what exists: a peer you are connected to who has never published is
//! readable one way, a publisher asleep behind a CDN the other, and most of the
//! time exactly one leg is available and there is no decision to make at all.
//!
//! ## Why this is a ladder and not a choice
//!
//! The author's two arms are not two implementations of one capability — they are
//! two *deployments*, and a publisher picks between them for reasons a reader
//! cannot see. The operator's framing, which is the design:
//!
//! > *You may always only publish one or the other if you know you're providing
//! > data or a service that either requires live, or that would crash you if you
//! > tried to do it live because you can't support the load — so you'd publish
//! > to a CDN.*
//!
//! So the reader's job is not to pick the "right" transport, it is to **try what
//! is there** and to say which one answered.
//!
//! ## The third leg is a different KIND of leg — [`Leg::Mirror`]
//!
//! A gatherer's republication (`APP-CONVENTION-FEED` §6) is not a third way to
//! reach the author; it is **somebody else's reading of them**, and §6.2 places it
//! as *"one leg of an ordered source set… not a replacement for the author."* Two
//! consequences worth having in one place:
//!
//! - **It goes last** and the reason is authority, not latency. A mirror can only
//!   ever be *short* — §6.1 rule 2 gives it no way to claim otherwise — so
//!   preferring it over a reachable author trades a complete answer for a partial
//!   one. The cost argument that makes mirrors worth having (one root check
//!   instead of 500) is about a **batch no-op check across many authors**, a
//!   different operation from reading one author's feed.
//! - **What it carries is not who wrote it.** `DX-R13` / §6.1 rule 3: attribution
//!   follows each entry's own detached signature, always, and a surface naming the
//!   gatherer as the author is non-conformant. The leg name reaches the log and
//!   the *"via"* line; it never reaches a byline.
//!
//! ## ⚠ A publisher cannot state a preference today, and that is measured
//!
//! The obvious next question — *can the peer publish which they would rather be
//! read over?* — has a measured answer: **no mechanism exists.**
//! `APP-CONVENTION-FEED`'s only use of the word *live* is §2.2's
//! **live-reference** atom, which is about whether a *reference inside an entry*
//! is pinned to a hash or resolved fresh — a different axis entirely, and one
//! that says nothing about how the feed itself should be reached. §4.2's
//! `index-head` carries `latest`, `page_size`, `updated_at`, `oldest`; there is
//! no field for this, and `/entity-deployment.json` is the **deployer's**
//! statement about a domain rather than a **publisher's** about their own feed.
//!
//! [`Preference`] therefore has one variant. It exists as a **named parameter
//! rather than an absence**, so the day a ruling lands the ordering function
//! grows an arm instead of the callers growing a branch. Routed to arch as
//! `A-43`; nothing is invented here.
//!
//! ## The default order, and the argument against it
//!
//! With nothing declared, **live is tried first**: a connection proves the
//! author is up *now*, and is the fresher of the two by construction.
//!
//! **The honest cost, stated rather than buried:** the live leg is today the
//! weaker read. A feed index is a *publish* artifact — `plan_index` builds the
//! head and pages into the out-dir and nothing writes them into the tree — so a
//! live read falls through to §4.3 rule 6's prefix enumeration, which
//! **reconstructs** the order from each entry's `created_at` where the published
//! leg **reads** the order the author wrote. §4.5 makes that order authored, so
//! the two can legitimately differ. Preferring live is therefore preferring
//! freshness over fidelity until the index lives in the tree, which is the fix
//! that removes the trade rather than balancing it.

use std::fmt;

/// One way to reach an author's feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Leg {
    /// Over a live connection to the author themselves.
    Live,
    /// Over their published tree, served at this origin.
    Published(String),
    /// Over a **gatherer's** mirror of this author — `APP-CONVENTION-FEED` §6.
    ///
    /// ⭐ **The third leg is not a third transport.** Live and published are two
    /// deployments of *the author*; this is somebody else's republication of what
    /// they gathered, and §6.2 is explicit that it is *"one leg of an ordered
    /// source set, alongside the author's live peer and the author's own published
    /// origin"* — **not a replacement for the author**, which is why it is last in
    /// [`plan`]'s default order.
    ///
    /// What it buys is cost. A reader following 500 authors directly pays a signed
    /// root check per author; one following a peer who mirrors them pays **one**,
    /// because the witness is that gatherer's root and one root covers every
    /// mirror under it. That is the 250× argument, and it is arithmetic over a
    /// measured no-op rather than a measurement — nobody has built a 500-follow
    /// reader on either seat.
    Mirror {
        /// Whose republication this is. **Never rendered as the author of
        /// anything** (§6.1 rule 3 / `DX-R13`) — it names who to ask, and
        /// attribution comes off each entry's own signature.
        gatherer: String,
        /// Where the gatherer's tree is served.
        origin: String,
    },
}

impl Leg {
    /// The stable word for this leg — a log field and an i18n key suffix.
    ///
    /// **Not `Display`**, which would have to render the origin: a name that
    /// varies by deployment cannot key a translated string. The same argument
    /// applies to the gatherer's peer id.
    pub fn name(&self) -> &'static str {
        match self {
            Leg::Live => "live",
            Leg::Published(_) => "published",
            Leg::Mirror { .. } => "mirror",
        }
    }
}

impl fmt::Display for Leg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Leg::Live => write!(f, "live"),
            Leg::Published(origin) => write!(f, "published at {origin}"),
            Leg::Mirror { gatherer, origin } => {
                write!(f, "mirrored by {gatherer} at {origin}")
            }
        }
    }
}

/// What the publisher said about how they would rather be read.
///
/// One variant, deliberately — see the module doc. **Do not add speculative
/// arms**: a variant nothing can construct is a branch no gate can reach, and
/// the ruling that creates the second one will also say what it is called.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Preference {
    /// No mechanism exists for a publisher to say. Today this is every author.
    #[default]
    Unstated,
}

/// The legs to try, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub legs: Vec<Leg>,
}

impl Route {
    /// There is no way to reach this author at all: not connected, and no origin
    /// registered for them.
    ///
    /// **Its own predicate rather than `legs.is_empty()` at each call site** —
    /// the surface renders a different sentence for it, and a caller that
    /// open-codes the emptiness check is one refactor away from rendering
    /// *"they have posted nothing"* for somebody we never asked.
    pub fn is_unreachable(&self) -> bool {
        self.legs.is_empty()
    }
}

/// A peer who republishes this author, and where their tree is served.
///
/// ⚠ **Where this list comes from is an open question, and the honest answer
/// today is nowhere.** §6 gives a reader no way to learn that a gatherer exists:
/// §6.0.1 makes a mirror's address *derivable* once you know **whose** mirror you
/// want, and nothing names the gatherers. So the product passes an empty slice —
/// the mechanism is built and gated, and the discovery half is routed rather than
/// invented here, because whichever shape publishes first becomes the baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gatherer {
    pub peer_id: String,
    pub origin: String,
}

/// Build the reading order for one author.
///
/// `connected` comes from the kernel read-model
/// ([`crate::peer_liveness::liveness_of`]), never from an app-tier belief —
/// AP12/D8 refuses a fourth parallel liveness store and this is not going to be
/// the fifth. `origin` comes from
/// [`origins::get_origin`](crate::content_site::origins::get_origin), the
/// accessor that resolves supersession, so a retired publisher's origin cannot
/// enter a route (AP54). `mirrors` is [`Gatherer`] — read its note before
/// wondering why every caller in the product passes `&[]`.
pub fn plan(
    connected: bool,
    origin: Option<&str>,
    mirrors: &[Gatherer],
    pref: Preference,
) -> Route {
    let mut legs = Vec::new();
    match pref {
        Preference::Unstated => {
            if connected {
                legs.push(Leg::Live);
            }
            if let Some(o) = origin {
                legs.push(Leg::Published(o.to_string()));
            }
            // ⭐ **Mirrors go LAST, and the reason is authority rather than
            // cost.** §6.2: a mirror *"is not a replacement for the author"* — it
            // can only ever be short, and a gatherer's view of what somebody
            // posted is evidence about the gatherer's reading, not about the
            // author's writing. The cost argument (one root check instead of 500)
            // is real and is about a **batch no-op check across many authors**,
            // which is a different operation from reading one author's feed; it
            // does not license preferring a stranger's copy over the author when
            // the author is right there.
            for g in mirrors {
                legs.push(Leg::Mirror {
                    gatherer: g.peer_id.clone(),
                    origin: g.origin.clone(),
                });
            }
        }
    }
    Route { legs }
}

// ---------------------------------------------------------------------------
// Reducing what the legs said
// ---------------------------------------------------------------------------

/// What one leg came back with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// The leg answered and carried posts.
    Entries(usize),
    /// The leg answered and the author has nothing there. **A real answer**, and
    /// the reason the ladder does not stop here — see [`reduce`].
    Empty,
    /// The leg could not be read.
    Failed(String),
}

/// One leg's result, for the log and for the detail line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub leg: &'static str,
    pub outcome: AttemptOutcome,
}

/// How a whole route resolved. **Four facts, and they go to four different
/// sentences** — the rule this repo has paid for repeatedly (AP40): a person
/// told *"this publisher has not posted anything"* about a publisher we could
/// not reach has been given a claim about somebody else's behaviour in place of
/// a report about ours.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolution<T> {
    /// Posts, and **the leg that supplied them, whole**.
    ///
    /// ⭐ **It carries the `Leg` rather than its `name()` because a mirror leg
    /// has a fact the name cannot hold: WHOSE reading this is.** §6.1 rule 3
    /// makes the gatherer's peer id something a surface must never put in a
    /// byline — and the *via* line is the one place it may legitimately appear,
    /// so a resolution that could say *"mirrored"* and not *"by whom"* would
    /// leave a reader unable to judge a view they have been told is partial by
    /// construction. `name()` is still the i18n key suffix; this is the value it
    /// projects from.
    Served { leg: Leg, entries: Vec<T> },
    /// At least one leg answered, and no leg had anything. The attempts are
    /// carried because *"we asked both and they have nothing"* and *"we asked
    /// one, they have nothing, and the other we could not check"* are different
    /// statements and the second must not be able to wear the first's words.
    NoPosts { attempts: Vec<Attempt> },
    /// Every leg failed. Nothing is known about whether this author has posted.
    Failed { attempts: Vec<Attempt> },
    /// There were no legs: no connection and no known origin.
    Unreachable,
}

/// Reduce the legs' results to one outcome.
///
/// ## The stopping rule, and the case it exists for
///
/// **First non-empty wins. An EMPTY answer does not stop the ladder.**
///
/// That looks wrong for about ten seconds — a leg answered, so why ask another?
/// Because *an empty answer from one leg is not evidence about a different one*,
/// and the ladder exists precisely because a publisher may serve their feed at
/// only one of the two. A live peer whose tree carries no index and no entries
/// and a published tree carrying a year of posts is not a hypothetical: it is
/// what a publisher who *"can't support the load, so publishes to a CDN"* looks
/// like from the reader's side, and stopping at the first empty leg would report
/// *"this author has posted nothing"* to somebody one hop from a year of posts.
///
/// That is AP54's family — the product had the fact and told the wrong surface —
/// and it is cheap to get wrong here because the wrong version is the simpler
/// one.
///
/// **A FAILED leg does not stop it either**, for the ordinary reason: that is
/// what a fallback is.
pub fn reduce<T>(results: Vec<(Leg, Result<Vec<T>, String>)>) -> Resolution<T> {
    if results.is_empty() {
        return Resolution::Unreachable;
    }
    let mut attempts = Vec::with_capacity(results.len());
    let mut served: Option<(Leg, Vec<T>)> = None;
    let mut any_answered = false;

    for (leg, result) in results {
        let name = leg.name();
        match result {
            Ok(entries) if entries.is_empty() => {
                any_answered = true;
                attempts.push(Attempt { leg: name, outcome: AttemptOutcome::Empty });
            }
            Ok(entries) => {
                any_answered = true;
                attempts.push(Attempt { leg: name, outcome: AttemptOutcome::Entries(entries.len()) });
                if served.is_none() {
                    served = Some((leg, entries));
                }
            }
            Err(detail) => {
                attempts.push(Attempt { leg: name, outcome: AttemptOutcome::Failed(detail) });
            }
        }
    }

    match served {
        Some((leg, entries)) => Resolution::Served { leg, entries },
        None if any_answered => Resolution::NoPosts { attempts },
        None => Resolution::Failed { attempts },
    }
}

/// One line per leg, for the D13 channel.
///
/// **Every resolution reports, including the ordinary one.** A surface that
/// logs only when something went wrong cannot be told from one that never ran —
/// the lesson `window_hydration::report` was built on, where the entire healthy
/// path was silent and a window said how it resolved only when it adopted.
pub fn describe<T>(author: &str, resolution: &Resolution<T>) -> String {
    match resolution {
        Resolution::Served { leg, entries } => {
            // `Leg`'s `Display`, not `name()` — the log is where *"mirrored by
            // G at O"* belongs, and it was previously the bare word "mirror".
            format!("feed-route: {author} served {} entries via {leg}", entries.len())
        }
        Resolution::NoPosts { attempts } => {
            format!("feed-route: {author} has no posts — {}", render_attempts(attempts))
        }
        Resolution::Failed { attempts } => {
            format!("feed-route: {author} unreadable — {}", render_attempts(attempts))
        }
        Resolution::Unreachable => {
            format!("feed-route: {author} has no route — not connected and no origin registered")
        }
    }
}

fn render_attempts(attempts: &[Attempt]) -> String {
    attempts
        .iter()
        .map(|a| match &a.outcome {
            AttemptOutcome::Entries(n) => format!("{}={n}", a.leg),
            AttemptOutcome::Empty => format!("{}=empty", a.leg),
            AttemptOutcome::Failed(d) => format!("{}=failed({d})", a.leg),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- plan -------------------------------------------------------------

    #[test]
    fn a_connected_author_with_no_origin_is_read_live() {
        assert_eq!(plan(true, None, &[], Preference::Unstated).legs, vec![Leg::Live]);
    }

    #[test]
    fn an_author_we_are_not_connected_to_is_read_from_their_origin() {
        assert_eq!(
            plan(false, Some("https://e.example"), &[], Preference::Unstated).legs,
            vec![Leg::Published("https://e.example".into())]
        );
    }

    /// The operator's default: a connection proves they are up now.
    #[test]
    fn with_both_available_live_is_tried_first_and_published_is_the_fallback() {
        assert_eq!(
            plan(true, Some("https://e.example"), &[], Preference::Unstated).legs,
            vec![Leg::Live, Leg::Published("https://e.example".into())]
        );
    }

    /// ⭐ **Every one of the author's own legs outranks every mirror** (§6.2 —
    /// *"not a replacement for the author"*).
    ///
    /// The tempting order is the other one: a mirror is the **cheap** leg, and a
    /// reader following 500 people would rather pay one root check than 500. That
    /// argument is about a batch no-op check across many authors; applied to
    /// reading *one* author it trades a complete answer for one that can only ever
    /// be short, from a peer with no authority over the subject.
    ///
    /// Asserted as an **order over the whole list**, not as *"mirror is not
    /// first"* — with three legs the weaker assertion passes an implementation
    /// that puts a mirror between the two author legs.
    #[test]
    fn a_gatherers_mirror_is_tried_after_every_one_of_the_authors_own_legs() {
        let mirrors = vec![
            Gatherer { peer_id: "2FirstGatherer".into(), origin: "https://g1.example".into() },
            Gatherer { peer_id: "2SecondGatherer".into(), origin: "https://g2.example".into() },
        ];
        let legs = plan(true, Some("https://e.example"), &mirrors, Preference::Unstated).legs;
        assert_eq!(
            legs,
            vec![
                Leg::Live,
                Leg::Published("https://e.example".into()),
                Leg::Mirror {
                    gatherer: "2FirstGatherer".into(),
                    origin: "https://g1.example".into()
                },
                Leg::Mirror {
                    gatherer: "2SecondGatherer".into(),
                    origin: "https://g2.example".into()
                },
            ],
            "a mirror was tried before one of the author's own legs"
        );
        // And a mirror alone is a route: an author we cannot reach at all is still
        // readable through somebody who gathered them, which is most of why §6
        // exists.
        let only = plan(false, None, &mirrors[..1], Preference::Unstated);
        assert!(!only.is_unreachable(), "a gathered author was reported unreachable");
        assert_eq!(only.legs.len(), 1);
    }

    /// **Not `legs.is_empty()` at the call site** — the surface that renders
    /// this says something no other outcome says.
    #[test]
    fn no_connection_and_no_origin_is_unreachable_rather_than_an_empty_feed() {
        let route = plan(false, None, &[], Preference::Unstated);
        assert!(route.is_unreachable());
        assert!(!plan(true, None, &[], Preference::Unstated).is_unreachable());
    }

    // -- reduce -----------------------------------------------------------

    fn ok(n: usize) -> Result<Vec<u8>, String> {
        Ok((0..n as u8).collect())
    }
    fn err(d: &str) -> Result<Vec<u8>, String> {
        Err(d.to_string())
    }

    #[test]
    fn the_first_leg_that_carries_posts_serves_them_and_names_itself() {
        let r = reduce(vec![(Leg::Live, ok(3)), (Leg::Published("o".into()), ok(9))]);
        match r {
            Resolution::Served { leg, entries } => {
                assert_eq!(leg.name(), "live");
                assert_eq!(entries.len(), 3, "the second leg must not overwrite the first");
            }
            other => panic!("{other:?}"),
        }
    }

    /// ⭐ **The whole reason this is a ladder.** A live peer with nothing in
    /// their tree and a published tree full of posts is what a publisher who
    /// serves from a CDN looks like from here. Stopping at the empty leg reports
    /// *"they have posted nothing"* one hop from a year of posts.
    #[test]
    fn an_empty_answer_does_not_stop_the_ladder() {
        let r = reduce(vec![(Leg::Live, ok(0)), (Leg::Published("o".into()), ok(4))]);
        match r {
            Resolution::Served { leg, entries } => {
                assert_eq!(leg.name(), "published");
                assert_eq!(entries.len(), 4);
            }
            other => panic!("an empty first leg swallowed the feed: {other:?}"),
        }
    }

    #[test]
    fn a_failed_leg_falls_through_to_the_next_one() {
        let r = reduce(vec![(Leg::Live, err("no route to peer")), (Leg::Published("o".into()), ok(2))]);
        assert!(matches!(r, Resolution::Served { leg: Leg::Published(_), .. }));
    }

    /// *"We asked and they have nothing"* is a claim about the author.
    #[test]
    fn every_leg_answering_empty_is_no_posts() {
        let r = reduce(vec![(Leg::Live, ok(0)), (Leg::Published("o".into()), ok(0))]);
        match r {
            Resolution::NoPosts { attempts } => {
                assert_eq!(attempts.len(), 2);
                assert!(attempts.iter().all(|a| a.outcome == AttemptOutcome::Empty));
            }
            other => panic!("{other:?}"),
        }
    }

    /// *"We could not look"* is a claim about **us**, and must never be able to
    /// render as the sentence above.
    #[test]
    fn every_leg_failing_is_not_the_same_fact_as_no_posts() {
        let r = reduce(vec![(Leg::Live, err("403")), (Leg::Published("o".into()), err("504"))]);
        match r {
            Resolution::Failed { attempts } => {
                assert_eq!(attempts.len(), 2);
                assert!(matches!(attempts[0].outcome, AttemptOutcome::Failed(_)));
            }
            other => panic!("a total failure rendered as a fact about the author: {other:?}"),
        }
    }

    /// The mixed case, and the reason `NoPosts` carries its attempts: one leg
    /// answered honestly and the other was never established, so the detail has
    /// to survive into the report even though the verdict is the same word.
    #[test]
    fn one_leg_empty_and_one_failed_is_no_posts_that_still_carries_the_failure() {
        let r = reduce(vec![(Leg::Live, ok(0)), (Leg::Published("o".into()), err("504"))]);
        match r {
            Resolution::NoPosts { attempts } => {
                assert_eq!(attempts[0].outcome, AttemptOutcome::Empty);
                assert!(
                    matches!(&attempts[1].outcome, AttemptOutcome::Failed(d) if d == "504"),
                    "the unchecked leg was dropped from the report"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn no_legs_is_unreachable_and_never_a_failure() {
        let r: Resolution<u8> = reduce(vec![]);
        assert_eq!(r, Resolution::Unreachable);
    }

    // -- the report -------------------------------------------------------

    /// Every outcome speaks, including the ordinary one.
    #[test]
    fn all_four_resolutions_report_and_no_two_say_the_same_thing() {
        let lines = vec![
            describe("A", &Resolution::Served { leg: Leg::Live, entries: vec![1u8] }),
            describe(
                "A",
                &Resolution::<u8>::NoPosts {
                    attempts: vec![Attempt { leg: "live", outcome: AttemptOutcome::Empty }],
                },
            ),
            describe(
                "A",
                &Resolution::<u8>::Failed {
                    attempts: vec![Attempt {
                        leg: "published",
                        outcome: AttemptOutcome::Failed("504".into()),
                    }],
                },
            ),
            describe("A", &Resolution::<u8>::Unreachable),
        ];
        let distinct: std::collections::BTreeSet<&str> =
            lines.iter().map(|s| s.as_str()).collect();
        assert_eq!(distinct.len(), 4, "two resolutions render alike: {lines:#?}");
    }

    #[test]
    fn the_failing_legs_reason_reaches_the_report() {
        let line = describe(
            "A",
            &Resolution::<u8>::Failed {
                attempts: vec![Attempt {
                    leg: "live",
                    outcome: AttemptOutcome::Failed("not granted".into()),
                }],
            },
        );
        assert!(line.contains("not granted"), "{line}");
    }
}

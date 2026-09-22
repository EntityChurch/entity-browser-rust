//! **What does this peer publish?** — asked of the publisher's own signed root,
//! because nobody else can answer it honestly.
//!
//! # The gap this fills, stated from the specs rather than from the UI
//!
//! `EXTENSION-REGISTRY` §3's binding answers *who a name is and how to reach
//! them* — `name`, `target_peer_id`, `transports`. §3b adds a second record for
//! *what shared **infrastructure** a deployment offers* (reflector, signaling,
//! relays), and its own preamble says why it had to exist: *"all three
//! implementations built §3b.2's pool-selection rule and had no protocol way to
//! learn a pool to select from."*
//!
//! **There is no third record saying what a peer PUBLISHES.** So a reader who
//! resolves a name holds a publisher and an origin and cannot tell whether they
//! carry sites, a feed, both or neither — which is why
//! `views/registry_browser` shipped guessing `sites`, and why
//! `APP-CONVENTION-FEED` §9.5 records *"following an idea … needs a topic
//! identifier and a discovery walk this document does not specify; the
//! aggregation half is solved by §6, the navigation half is open."*
//!
//! # Why the answer is a PROBE and not a field
//!
//! A registry field would be **a third party's claim about somebody else's
//! tree** — AP30's shape exactly: a durable record of a remote assertion, with
//! no path back to the assertion, going stale the moment a publisher adds or
//! drops an axis. It would also be unfalsifiable at the reader: nothing about a
//! signed binding makes its `metadata` true.
//!
//! Walking the publisher's own signed root gives something strictly better, and
//! the difference is the whole design:
//!
//! ⭐ **A verified NEGATIVE.** [`Publishes::No`] is not *"we looked and found
//! nothing"*; it is *"the root this publisher signed does not bind it."*
//! `SignedFetchError::Absent` is that fact — its own doc says *"the key is
//! genuinely not in the signed tree … a fabricated binding cannot appear here,
//! which is the point"* — and `EXTENSION-TREE` §3.8 R1 is what keeps it honest
//! on the walk arm, turning an origin that withholds an interior node into a
//! visible `incomplete_walk` rather than a correct, complete, shorter answer.
//! That is the same asymmetry `EXTENSION-REGISTRY` §6a.3a leans on for browsing
//! names, applied one tier up to browsing *conventions*.
//!
//! A host-served `.list` could never give it, and neither could a registry
//! field. **You can only get a trustworthy "no" from the party who would have
//! had to say "yes".**
//!
//! # What it does not answer
//!
//! *Which peers exist* — that is `EXTENSION-DISCOVERY`'s question and a
//! different mechanism (§1: REGISTRY is *lookup*, DISCOVERY is *find*). This
//! module answers the third question neither of them does, **about a peer you
//! already hold**, and it scales with the number of peers you ask. Asking every
//! peer in a registry is O(registry) round trips and is exactly why an
//! aggregated answer — a gatherer's mirror (`APP-CONVENTION-FEED` §6), or a
//! record type that does not exist yet — is the open design question rather
//! than something to invent here.

use crate::content_site::signed_fetch::{EnumerationBudget, SignedFetchError, SignedSession};
use crate::content_site::http_poll::BinSource;
use crate::open_target::{EntryPoint, Viewer};

/// A probe walks a **prefix**, so it is bounded on both axes a large or hostile
/// origin can grow. Tighter than [`DEFAULT_ENUMERATION_BUDGET`] on purpose:
/// this asks *"do you publish sites"*, not *"give me your sites"*, and a probe
/// that downloads a publisher's whole key set to answer a yes/no is a probe
/// nobody can afford to run against a list of peers.
///
/// **Not tuned to a measurement, and it is a ceiling rather than an
/// expectation** — a walk that hits it answers [`Publishes::Partial`], which is
/// a real answer, not a failure.
pub const PROBE_BUDGET: EnumerationBudget = EnumerationBudget { max_nodes: 32, max_keys: 64 };

/// Why a probe learned nothing. **Five causes, three destinations** — which is
/// the reason this is not one string: an unreachable origin sends a person to
/// wait, a withheld blob sends them to distrust the host, an unproven chain
/// sends them to the publisher, and the last two send them to their own reader
/// for two different reasons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unknown {
    /// The origin could not be reached, or served nothing at a URL we need.
    /// Retryable, and the commonest answer for a publisher who is simply down.
    Unreachable(String),
    /// The origin did not produce a blob **its own signed root declares**
    /// (`tree/incomplete-walk`). The host chose; this is not the publisher's
    /// defect and it is not a transient fault.
    Withheld(String),
    /// The chain does not hold — bad signature, wrong key, or a body that does
    /// not hash to its address. **The publisher's defect.**
    Unproven(String),
    /// We refused a root that proved itself, on our own policy — today the
    /// anti-rollback floor. *Their laptop is not an attack*
    /// (`SignedFetchError::Declined`); this sends a reader to their own floor,
    /// never to the publisher.
    OurFloor(String),
    /// Our own pump did not converge (`SignedFetchError::Budget` — *"a
    /// structural bug or a pathological tree"*).
    ///
    /// **Kept apart from [`OurFloor`](Self::OurFloor) although both are ours.**
    /// A floor is a policy a reader can reason about and change; this is a
    /// defect to report. Merging them would file a bug as a setting.
    Exhausted,
}

impl Unknown {
    /// Whose problem this is. Read by the surface so a sentence can name a
    /// party instead of saying *"could not check"* four different ways.
    pub fn party(&self) -> &'static str {
        match self {
            Unknown::Unreachable(_) => "origin", // i18n-ignore — a log/diagnostic key
            Unknown::Withheld(_) => "origin",    // i18n-ignore
            Unknown::Unproven(_) => "publisher", // i18n-ignore
            Unknown::OurFloor(_) => "reader",    // i18n-ignore
            Unknown::Exhausted => "reader",      // i18n-ignore
        }
    }

    /// The detail, for a log line or a diagnostic row — never a label.
    pub fn detail(&self) -> &str {
        match self {
            Unknown::Unreachable(d)
            | Unknown::Withheld(d)
            | Unknown::Unproven(d)
            | Unknown::OurFloor(d) => d,
            // No detail to carry: the variant IS the whole fact.
            Unknown::Exhausted => "the walk did not converge", // i18n-ignore — a log detail
        }
    }
}

/// What one convention's probe established. **Four outcomes, and collapsing any
/// two of them produces a sentence that is wrong about somebody.**
///
/// In particular *"they publish none"* and *"we could not tell"* are the pair a
/// tidy version merges, and the cost is the one this repo keeps paying: a
/// reader is told a publisher has nothing when the truth is that nobody asked
/// them successfully.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Publishes {
    /// The signed root binds this convention's entry point. `units` is how many
    /// addressable things were found where that is cheap to know — `None` for a
    /// keyed probe, which establishes presence and counts nothing.
    Yes { units: Option<usize> },
    /// ⭐ **A verified negative** — the root this publisher signed does not bind
    /// it. See the module doc: this is the outcome a registry field could never
    /// supply.
    No,
    /// The walk ran out of budget with nothing found yet. **Not a `No`**: there
    /// are more keys than we looked at, so the question is genuinely open, and
    /// rendering it as *"nothing here"* is the bounded-enumeration defect
    /// `Enumeration::complete` exists to prevent.
    Partial { nodes_walked: usize },
    /// We did not learn. Carries whose problem it is.
    Unknown(Unknown),
}

impl Publishes {
    /// Is there something here to open? Only a `Yes` licenses an *Open* button
    /// — a surface offering one for the other three sends a reader into a
    /// window that can only render an empty rail.
    pub fn is_offerable(&self) -> bool {
        matches!(self, Publishes::Yes { .. })
    }

    /// A stable word per outcome, for the D13 log line and for a gate to assert
    /// on. **Five distinct words including the `Unknown` split**, so a sixth
    /// outcome cannot quietly reuse one.
    pub fn word(&self) -> &'static str {
        match self {
            Publishes::Yes { .. } => "publishes", // i18n-ignore — log field
            Publishes::No => "publishes-none",    // i18n-ignore
            Publishes::Partial { .. } => "partial", // i18n-ignore
            Publishes::Unknown(u) => match u {
                Unknown::Unreachable(_) => "unreachable", // i18n-ignore
                Unknown::Withheld(_) => "withheld",       // i18n-ignore
                Unknown::Unproven(_) => "unproven",       // i18n-ignore
                Unknown::OurFloor(_) => "declined",       // i18n-ignore
                Unknown::Exhausted => "exhausted",        // i18n-ignore
            },
        }
    }
}

/// **The pure half.** Map a keyed resolve's outcome onto [`Publishes`].
///
/// Split out and taking the error by value so every arm is gated by `make test`
/// on both arms rather than only through a browser — the same reason
/// `session_config::decide_home` and `feed_route::plan` are pure.
pub fn classify_key(outcome: Result<(), SignedFetchError>) -> Publishes {
    match outcome {
        Ok(()) => Publishes::Yes { units: None },
        Err(e) => classify_err(e),
    }
}

/// **The pure half of the walk arm.** `keys` is what the bounded enumeration
/// recovered under the prefix; `complete` is whether it finished.
///
/// `units` counts **site manifests**, not keys: a site is many entities and a
/// reader asked *"how many sites"*. A prefix whose keys do not parse as
/// manifests still answers `Yes` with no count — the convention pins no layout,
/// so finding something we cannot name is not the same as finding nothing.
pub fn classify_prefix(
    outcome: Result<(Vec<String>, bool, usize), SignedFetchError>,
    peer_id: &str,
) -> Publishes {
    let (keys, complete, nodes_walked) = match outcome {
        Ok(v) => v,
        Err(e) => return classify_err(e),
    };
    if keys.is_empty() {
        return if complete { Publishes::No } else { Publishes::Partial { nodes_walked } };
    }
    let manifests = keys
        .iter()
        .filter(|k| {
            crate::content_site::paths::parse_manifest_path(&format!("/{peer_id}/{k}")).is_some()
        })
        .count();
    Publishes::Yes { units: (complete && manifests > 0).then_some(manifests) }
}

/// The one place a `SignedFetchError` becomes an answer about publication.
///
/// **`Absent` is the only arm that is a fact about the publisher's intent**;
/// every other variant is a fact about the attempt, which is why they land on
/// `Unknown` with the party named rather than on `No`.
fn classify_err(e: SignedFetchError) -> Publishes {
    match e {
        SignedFetchError::Absent => Publishes::No,
        SignedFetchError::Transport(d) => Publishes::Unknown(Unknown::Unreachable(d)),
        SignedFetchError::IncompleteWalk(d) => Publishes::Unknown(Unknown::Withheld(d)),
        SignedFetchError::Verify(d) => Publishes::Unknown(Unknown::Unproven(d)),
        SignedFetchError::Declined(d) => Publishes::Unknown(Unknown::OurFloor(d)),
        SignedFetchError::Budget => Publishes::Unknown(Unknown::Exhausted),
    }
}

/// One convention's answer, ready to render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The `window_registry` identity key of the viewer that would open it —
    /// the same string `open_target::route` returns, so an *Open* built from a
    /// finding cannot name a window the table does not.
    pub window_type: &'static str,
    /// What was asked for, so a log line and a diagnostic row can say it.
    pub entry: EntryPoint,
    pub outcome: Publishes,
}

/// Probe **one** convention against a publisher's signed root.
pub async fn probe_one<S: BinSource + ?Sized>(
    src: &S,
    session: &SignedSession,
    peer_id: &str,
    viewer: &Viewer,
) -> Finding {
    let outcome = match viewer.entry {
        EntryPoint::Key(_) => {
            classify_key(session.resolve(src, viewer.entry.relative()).await.map(|_| ()))
        }
        EntryPoint::Prefix(_) => {
            let walked = session
                .enumerate_bounded(src, viewer.entry.relative(), PROBE_BUDGET)
                .await
                .map(|e| (e.keys, e.complete, e.nodes_walked));
            classify_prefix(walked, peer_id)
        }
    };
    Finding { window_type: viewer.window_type, entry: viewer.entry, outcome }
}

/// Probe **every** registered convention.
///
/// Sequential rather than joined, deliberately: the legs share one
/// `SignedSession`, whose whole job is to hold one `seq` floor and one content
/// cache per publisher, and two concurrent walks against it would race the
/// manifest slot each `resolve` refreshes. The probe is a handful of round
/// trips; the floor is the thing worth protecting.
///
/// **Every row is returned, including the refusals.** A surface that filtered
/// to the `Yes` rows could not distinguish a publisher who carries only sites
/// from one whose feed we could not check — which is the collapse the whole
/// module exists to avoid.
pub async fn probe_all<S: BinSource + ?Sized>(
    src: &S,
    session: &SignedSession,
    peer_id: &str,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for v in crate::open_target::viewers() {
        let f = probe_one(src, session, peer_id, v).await;
        tracing::info!(
            publisher = %peer_id,
            viewer = %f.window_type,
            entry = %v.entry.relative(),
            outcome = %f.outcome.word(),
            "publication probe"
        );
        out.push(f);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_site::signed_fetch::DEFAULT_ENUMERATION_BUDGET;

    /// Every outcome a reader can be shown, in one place.
    fn every_outcome() -> Vec<Publishes> {
        vec![
            Publishes::Yes { units: None },
            Publishes::No,
            Publishes::Partial { nodes_walked: 1 },
            Publishes::Unknown(Unknown::Unreachable("x".into())),
            Publishes::Unknown(Unknown::Withheld("x".into())),
            Publishes::Unknown(Unknown::Unproven("x".into())),
            Publishes::Unknown(Unknown::OurFloor("x".into())),
            Publishes::Unknown(Unknown::Exhausted),
        ]
    }

    /// Eight outcomes reach a reader and **every one has its own word**, so a
    /// ninth cannot quietly reuse one and so a surface rendering two of them
    /// alike reds here rather than in front of a person.
    ///
    /// The count is asserted for the reason the Doctor's roster learned the hard
    /// way: a census whose own claim about itself is not checked is the last
    /// thing anybody re-reads.
    #[test]
    fn every_probe_outcome_has_its_own_word() {
        let all = every_outcome();
        let words: std::collections::BTreeSet<_> = all.iter().map(|p| p.word()).collect();
        assert_eq!(words.len(), all.len(), "two probe outcomes render as one word: {words:?}");
        assert_eq!(all.len(), 8, "an outcome was added or removed without revisiting this census");
    }

    /// ⭐ **`Absent` is the ONLY error that is a fact about the publisher.**
    /// Everything else is a fact about the attempt, and reading any of them as
    /// *"they publish none"* tells a reader a publisher has nothing when the
    /// truth is that our question never landed.
    #[test]
    fn only_a_verified_absence_is_a_no() {
        assert_eq!(classify_key(Err(SignedFetchError::Absent)), Publishes::No);
        for e in [
            SignedFetchError::Transport("down".into()),
            SignedFetchError::IncompleteWalk("cut".into()),
            SignedFetchError::Verify("bad sig".into()),
            SignedFetchError::Declined("seq rollback".into()),
            SignedFetchError::Budget,
        ] {
            let got = classify_key(Err(e.clone()));
            assert!(
                matches!(got, Publishes::Unknown(_)),
                "{e:?} established nothing about what they publish, but graded as {}",
                got.word()
            );
            assert_ne!(got, Publishes::No);
        }
    }

    /// The five `Unknown` causes name **three different parties**, because that
    /// is where the reader is sent. A single "could not check" string would send
    /// all five to the same wrong place.
    #[test]
    fn a_refusal_names_whose_problem_it_is() {
        let cases = [
            (SignedFetchError::Transport("x".into()), "origin"),
            (SignedFetchError::IncompleteWalk("x".into()), "origin"),
            (SignedFetchError::Verify("x".into()), "publisher"),
            (SignedFetchError::Declined("x".into()), "reader"),
        ];
        for (e, party) in cases {
            let Publishes::Unknown(u) = classify_key(Err(e.clone())) else {
                panic!("{e:?} should establish nothing");
            };
            assert_eq!(u.party(), party, "{e:?} points a reader at the wrong party");
            assert_eq!(u.detail(), "x", "the detail must survive for the log line");
        }
        // `Budget` carries no detail from the fetcher, so it supplies its own —
        // an empty detail would render as a refusal that will not say why.
        let Publishes::Unknown(u) = classify_key(Err(SignedFetchError::Budget)) else {
            panic!("a non-converging pump establishes nothing");
        };
        assert_eq!(u.party(), "reader");
        assert!(!u.detail().is_empty(), "every refusal owes a sentence");
    }

    /// **An empty walk that FINISHED is a no; one that ran out of budget is
    /// not.** This is `Enumeration::complete`'s own rule — *"the caller cannot
    /// tell 'this registry has 40 names' from 'this registry has 40,000 and I
    /// stopped'"* — arriving at the one surface that would otherwise print
    /// *"this publisher has no sites"* about a publisher with thousands.
    #[test]
    fn a_truncated_walk_is_not_an_absence() {
        assert_eq!(classify_prefix(Ok((vec![], true, 3)), "PEER"), Publishes::No);
        assert_eq!(
            classify_prefix(Ok((vec![], false, 32)), "PEER"),
            Publishes::Partial { nodes_walked: 32 },
        );
    }

    /// Units count **sites**, not keys — a reader asked how many sites, and one
    /// site is a manifest plus its pages plus its assets.
    #[test]
    fn units_count_manifests_and_not_keys() {
        let keys = vec![
            "sites/demo/manifest".to_string(),
            "sites/demo/pages/about".to_string(),
            "sites/demo/assets/logo.png".to_string(),
            "sites/notes/manifest".to_string(),
        ];
        assert_eq!(
            classify_prefix(Ok((keys, true, 5)), "PEER"),
            Publishes::Yes { units: Some(2) },
            "four keys under two site manifests is two sites"
        );
    }

    /// **Something we cannot name is still something.** SITE pins no layout, so
    /// a publisher placing a site we cannot parse is a `Yes` with no count —
    /// never a `No`, which would be us reporting our own parser's limit as
    /// their emptiness.
    #[test]
    fn a_prefix_we_cannot_parse_is_still_published() {
        let keys = vec!["sites/odd/something-else".to_string()];
        assert_eq!(classify_prefix(Ok((keys, true, 2)), "PEER"), Publishes::Yes { units: None });
    }

    /// A truncated walk that **found** something is a `Yes` with no count: the
    /// presence is established, the number is not.
    #[test]
    fn a_truncated_walk_that_found_something_reports_no_count() {
        let keys = vec!["sites/demo/manifest".to_string()];
        assert_eq!(classify_prefix(Ok((keys, false, 32)), "PEER"), Publishes::Yes { units: None });
    }

    /// Only a `Yes` licenses an Open. The other seven put a reader in a window
    /// that can render nothing, which is the `("Site Browser", peer)` guess
    /// this whole surface was built to retire.
    #[test]
    fn nothing_but_a_yes_offers_to_open() {
        let mut offered = 0;
        for p in every_outcome() {
            if matches!(p, Publishes::Yes { .. }) {
                assert!(p.is_offerable());
                offered += 1;
            } else {
                assert!(!p.is_offerable(), "{} must not offer an Open", p.word());
            }
        }
        assert_eq!(offered, 1, "exactly one outcome may offer to open something");
    }

    /// The probe's budget is **tighter than the browse budget**, and this says
    /// so rather than leaving it to a reader to compare two constants in two
    /// files. A probe that inherited the browse budget would download a
    /// publisher's key set to answer a yes/no.
    #[test]
    fn the_probe_budget_is_tighter_than_a_browse() {
        assert!(PROBE_BUDGET.max_nodes < DEFAULT_ENUMERATION_BUDGET.max_nodes);
        assert!(PROBE_BUDGET.max_keys < DEFAULT_ENUMERATION_BUDGET.max_keys);
    }
}

/// **The probe against a real published tree.**
///
/// Native-only because it publishes one: `RootProjector` and `publish_fixture`
/// are `cfg(not(wasm32))` — a publisher writes a directory and the browser has
/// none.
///
/// ⚠ **These are the gates that matter, and the pure ones above cannot stand in
/// for them.** `classify_*` takes an outcome; only a walk over a signed root
/// produces one. The specific thing a map-backed double would leave unmeasured
/// is the whole claim: that [`Publishes::No`] comes from **`Absent` against a
/// verified root** rather than from a fetch that happened to miss.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod published {
    use super::*;
    use crate::content_site::signed_fetch::PinnedPublisher;
    use crate::feed_gather::DirOrigin;
    use crate::feed_read::block_on;
    use std::path::Path;

    fn session_for(author: &str) -> SignedSession {
        SignedSession::new(
            PinnedPublisher::from_peer_id("", author).expect("a canonical peer-id pins itself"),
        )
    }

    fn probe(dir: &Path, author: &str) -> Vec<Finding> {
        let session = session_for(author);
        block_on(probe_all(&DirOrigin(dir.to_path_buf()), &session, author))
    }

    fn outcome<'a>(f: &'a [Finding], window_type: &str) -> &'a Publishes {
        &f.iter().find(|f| f.window_type == window_type).expect("every viewer is probed").outcome
    }

    /// ⭐⭐ **The claim, end to end: a publisher who published a feed and no
    /// sites reads as exactly that — and the negative is VERIFIED.**
    ///
    /// The `No` is what a registry field could never supply and a host-served
    /// `.list` could never be trusted for. It comes from the author's own
    /// signed root not binding `sites/`, which is the only party whose silence
    /// means anything.
    ///
    /// **Both halves are asserted deliberately.** A gate checking only the
    /// `Yes` passes for a probe that answers `Yes` to everything, and a gate
    /// checking only the `No` passes for one that answers `No` to everything.
    #[test]
    fn a_feed_only_publisher_reads_as_a_feed_and_a_verified_absence_of_sites() {
        let (dir, author) = crate::feed_publish::tests::published_dir(3);
        let found = probe(dir.path(), &author);

        assert_eq!(found.len(), crate::open_target::viewers().len(), "every viewer owes an answer");
        assert_eq!(
            outcome(&found, crate::open_target::FEED),
            &Publishes::Yes { units: None },
            "§4.2's head is bound in this publisher's signed root"
        );
        assert_eq!(
            outcome(&found, crate::open_target::SITE_BROWSER),
            &Publishes::No,
            "this root binds no `sites/` key, and that is a fact about the publisher — \
             not a fetch that missed"
        );
    }

    /// The mirror image, through **one** projector, which is how
    /// `publish_axes` really does it: a publisher carrying both axes reads as
    /// both, and the site arm counts its sites.
    ///
    /// A `units` count is only available on the walk arm — the keyed probe
    /// establishes presence and counts nothing — and that asymmetry is the
    /// conventions', not ours.
    #[test]
    fn a_publisher_carrying_both_axes_reads_as_both_and_counts_its_sites() {
        use crate::content_site::format::{NavItem, SiteManifest, SitePage};
        use crate::content_site::publish_fixture::emit_owned_sites;
        use crate::content_site::read::OwnedSite;
        use crate::content_site::signed_root::RootProjector;

        let dir = tempfile::tempdir().unwrap();
        let mut root = RootProjector::new(crate::feed_publish::tests::identity()).unwrap();
        let author = root.peer_id().to_string();

        crate::feed_publish::publish_feed(
            dir.path(),
            &mut root,
            &crate::feed_publish::tests::entries_for(&author, 2),
            &[],
            10,
            crate::feed_publish::tests::NOW,
        )
        .expect("the feed half publishes");

        let sites: Vec<OwnedSite> = ["demo", "notes"]
            .iter()
            .map(|id| OwnedSite {
                peer_id: author.clone(),
                site_id: (*id).to_string(),
                manifest: SiteManifest::new(*id, "Demo", "index", vec![NavItem::new("Home", "/index")]),
                pages: vec![("index".into(), SitePage::markdown("Home", "# Hello"))],
                assets: Vec::new(),
                content: Vec::new(),
            })
            .collect();
        emit_owned_sites(dir.path(), &sites, "", Some(&mut root)).expect("the site half publishes");
        root.finish(dir.path()).expect("one root over both axes");

        let found = probe(dir.path(), &author);
        assert_eq!(
            outcome(&found, crate::open_target::FEED),
            &Publishes::Yes { units: None },
            "a keyed entry point establishes presence and counts nothing"
        );
        assert_eq!(
            outcome(&found, crate::open_target::SITE_BROWSER),
            &Publishes::Yes { units: Some(2) },
            "two site manifests under the walked prefix is two sites"
        );
    }

    /// ⭐ **An origin that cannot be reached is not a publisher with nothing** —
    /// the collapse this module exists to prevent, measured rather than
    /// reasoned: point the probe at an empty directory and **every** answer must
    /// be an `Unknown`, never a `No`.
    ///
    /// Note which party it names: the manifest itself is missing, so this is
    /// `Unreachable` and points at the origin. A publisher who genuinely
    /// publishes nothing is a *different* rig (they would have to have signed a
    /// root), and that case is the one above.
    #[test]
    fn an_unreachable_origin_is_never_read_as_publishing_nothing() {
        let (_real, author) = crate::feed_publish::tests::published_dir(1);
        let empty = tempfile::tempdir().unwrap();
        for f in probe(empty.path(), &author) {
            assert!(
                matches!(f.outcome, Publishes::Unknown(Unknown::Unreachable(_))),
                "{} graded an unreachable origin as `{}`",
                f.window_type,
                f.outcome.word()
            );
            assert!(!f.outcome.is_offerable());
        }
    }
}

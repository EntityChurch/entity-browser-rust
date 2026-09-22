//! **Corridor ① — a feed `entity-workbench-go` published, walked by our
//! reader**, with no shared code on the path.
//!
//! The first run in this ecosystem pointing a real publisher at a real reader.
//! `J-4` compared two **encoders** over authored input, which is a different
//! claim: per `[ADR-0012]`, two producers agreeing is not evidence that a
//! consumer is right.
//!
//! ## The fixture is a real Go emission, vendored, and it is CUT TWICE
//!
//! `tests/fixtures/crossimpl-go-feed/` is the byte-for-byte output of
//! workbench-go's `publish/cmd/crossimpl-feed` at workbench-go `779616d`.
//! Deterministic — a pinned seed, a pinned publish instant, and a pinned
//! per-entry `created_at` (entry *n* is epoch + *n* minutes), which matters
//! more here than for the site fixture: an entry's `created_at` is inside its
//! hashed bytes, so an unpinned clock moves all 34 entry hashes **and** all 34
//! signature paths. Re-cut:
//!
//! ```text
//! cd ../entity-workbench-go && go run ./publish/cmd/crossimpl-feed -out /tmp/corridor
//! ```
//!
//! | cut | declared prefix | bindings | entities | entry signatures committed |
//! |---|---|---|---|---|
//! | `peer-root/` | `/` | 438 | 459 | **34 / 34** |
//! | `feed-only/` | `app/feed/` | 37 | 42 | **0 / 34** |
//!
//! ⚠ **Two rules to check, not a recommended shape and a deprecated one.** The
//! pair is `FEED-14` from arch's `A-36` proposal §4.5. `peer-root` is the
//! `A-36` fix measured, and measuring it is what produced their `A-38` — the
//! same widening that commits the signatures also commits that peer's device
//! declarations and local folder paths. **A single-prefix fixture passes
//! against both rules and measures neither**, which is the same observation we
//! made about single-*page* feed fixtures one axis over.
//!
//! **34 entries against a 32-entry page size** is the `B-13` shape, so the
//! oldest entry lives on a page the head does not name and `FEED-R12` is
//! falsifiable — it had been unfalsifiable on both seats simultaneously, because
//! every feed fixture either seat had ever cut was one page.
//!
//! ## ⭐ What building this found, and neither defect was visible from inside
//!
//! Both are in [`crate::feed_gather::PublishedTree`], the source `publish
//! --gather` uses, and **both blocked the walk outright** — this is the first
//! time anything in this tree read a feed somebody else published.
//!
//! 1. **The layout was derived, not read.** We pinned
//!    `{peer}/system/peer/published-root`; they serve `{out}/manifest`. Hop 0,
//!    for every conformant publisher whose layout is not ours.
//! 2. **§4.2's pinned address was sent verbatim to a prefix-relative resolve.**
//!    The narrow cut commits `index`, we asked for `app/feed/index`. This is the
//!    reader trap `entity-workbench-go` filed and arch took into
//!    `GUIDE-APPLICATION-DEVELOPMENT`, arriving on our side within the week.
//!
//! **No gate here could have seen either**, and the reason is one sentence:
//! every `--gather` fixture in this tree is a directory *our own publisher
//! wrote*, so the layout the gatherer assumed and the layout the fixture used
//! were the same expression, and our emitter has never declared a narrow
//! prefix. *A test population you generated cannot contain the shape you are
//! missing* — where here the missing shape is **another implementation**.
//!
//! ## ⭐ `B-14` — report which path answered, not the answer
//!
//! Their ask, and the finding that earned it: reading their own fixture back,
//! their reader returned **all 34 entries, correctly attributed, with no error —
//! and every index lookup had missed.** §4.3 rule 6's enumeration fallback did
//! exactly what the convention requires and hid a completely broken primary
//! path. *A conformance fallback built to survive a hostile publisher will
//! equally survive your own broken primary.* No assertion phrased over the
//! **answer** can fail.
//!
//! See [`the_index_is_what_answered_and_no_fallback_could_have_rescued_it`] for
//! our number and why it is structural rather than lucky.
#![cfg(all(test, not(target_arch = "wasm32")))]

use std::path::PathBuf;

use crate::feed_gather::PublishedTree;
use crate::feed_read::{block_on, read_feed_from, FeedSource, FeedWindow, Resumed, Unattributed};

/// The fixture's publisher, pinned by their `-seed`. **Its own seed, distinct
/// from their site fixture's**, so the two are two peers: a reader consuming
/// both against one peer-id could not tell a namespace rule from a coincidence,
/// and `FEED-R1` is exactly a namespace rule.
const PEER_ID: &str = "2KAoCfAP6ZZyLmS9wYz4rUmpehd4JMeek32NLN58R3ehpi";

/// Authored entry count, deliberately larger than their pinned 32-entry page
/// size.
const ENTRIES: usize = 34;

fn cut(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/crossimpl-go-feed")
        .join(name)
}

fn open(name: &str) -> PublishedTree {
    PublishedTree::open(cut(name), PEER_ID)
        .expect("their fixture peer-id is Ed25519 canonical, so it carries its own key")
}

/// Read one cut through the shipped consumer — [`PublishedTree`] is the source
/// `publish --gather` uses, not a double built for this file. That is the whole
/// point: a double would have supplied the layout we forgot to ask for.
fn walk(name: &str) -> FeedWindow {
    block_on(read_feed_from(&open(name), PEER_ID, 100, None))
        .unwrap_or_else(|e| panic!("{name}: their published feed did not read back: {e:?}"))
}

/// Each entry's authored text, which their emitter numbers `0..33` in publish
/// order. Read off the `fallback`, which `EMBED` §3 makes mandatory and
/// non-empty, so it is the one field guaranteed to be there whatever arm the
/// payload took.
fn bodies(w: &FeedWindow) -> Vec<String> {
    w.entries.iter().map(|e| e.entry.body.data.fallback.clone()).collect()
}

fn attributed(w: &FeedWindow) -> usize {
    w.entries.iter().filter(|e| e.attribution.may_name_the_author()).count()
}

// ---------------------------------------------------------------------------
// `FEED-14`, positive arm
// ---------------------------------------------------------------------------

/// **The corridor.** Their publisher, our consumer: the manifest decodes, the
/// root verifies against the key their peer-id carries, the trie walks, §4.2's
/// index leads to both pages, every entry's two-hop body arrives, and
/// `FEED-R4`'s verdict lands on each one separately.
#[test]
fn a_feed_the_other_seat_published_reads_back_whole_and_attributed() {
    let w = walk("peer-root");

    assert_eq!(w.entries.len(), ENTRIES, "every authored entry came back");
    assert_eq!(
        attributed(&w),
        ENTRIES,
        "`FEED-14` positive arm: the signatures are inside the committed set, so \
         every entry names its author. Unattributed: {:?}",
        w.entries
            .iter()
            .filter(|e| !e.attribution.may_name_the_author())
            .map(|e| (e.hash.to_hex(), e.attribution.clone()))
            .collect::<Vec<_>>()
    );
}

/// **`FEED-R12` — the walk crosses a page boundary**, which is what 34-against-32
/// was chosen for. The oldest entry is on a page the head's `current` does not
/// name, so a reader that stopped at the named page returns 2 and passes every
/// count-free assertion.
///
/// Ordering is asserted at **both ends and in full**: §4.5 makes the order
/// authored rather than derived, so a reader that sorted by `created_at` would
/// agree with this fixture by luck and disagree with a backdated post.
#[test]
fn the_walk_reaches_both_ends_of_an_archive_that_does_not_fit_on_one_page() {
    let got = bodies(&walk("peer-root"));

    let expected: Vec<String> = (0..ENTRIES)
        .rev()
        .map(|n| format!("corridor entry {n} — authored by the Go arm for the joint read"))
        .collect();
    assert_eq!(got, expected, "newest-first, whole, across both index pages");
}

// ---------------------------------------------------------------------------
// `FEED-14`, negative arm
// ---------------------------------------------------------------------------

/// **`FEED-14`'s negative arm, and the reason the fixture is cut twice.** The
/// same 34 entries published over `app/feed/`, which **excludes**
/// `system/signature/`. A static reader with no second channel must reach every
/// entry and be able to name nobody for any of them.
///
/// ⚠ **The refusal is `NoSignature` specifically**, not merely *not attributed*.
/// `FEED-R4`'s named case is *the author never signed*; a reader that reported
/// these as `BadSignature` or `SignerIsNotTheAuthor` would be making a statement
/// about the author to cover a **publisher's** scoping choice — which is the
/// `A-36` argument for the rule in the first place: with obligation 3
/// unspecified, *absent* has two causes and a conformant reader publishes a
/// falsehood manufactured by a third party.
#[test]
fn a_feed_published_over_a_prefix_that_excludes_the_signatures_can_name_nobody() {
    let w = walk("feed-only");

    assert_eq!(
        w.entries.len(),
        ENTRIES,
        "the entries ARE in this cut — only their signatures are outside it"
    );
    assert_eq!(bodies(&w), bodies(&walk("peer-root")), "same feed, same order");
    assert_eq!(attributed(&w), 0, "`FEED-14` negative arm");

    for e in &w.entries {
        assert!(
            matches!(
                e.attribution,
                crate::feed_read::Attribution::Unattributed(Unattributed::NoSignature)
            ),
            "{}: a signature the PUBLISHER did not commit must read as unsigned, \
             never as a fault of the author's — got {:?}",
            e.hash.to_hex(),
            e.attribution
        );
    }
}

// ---------------------------------------------------------------------------
// `B-14`
// ---------------------------------------------------------------------------

/// **`B-14`: which path produced the answer.**
///
/// Their number was `0` by index and `34` by enumeration, with every
/// caller-visible field correct. Ours is **34 by index and 0 by enumeration on
/// both cuts** — and the `0` is *structural*, which is a stronger statement than
/// *"it did not happen"*:
///
/// - [`Resumed`] is the per-walk witness. Anything other than
///   [`Resumed::Unpositioned`] means §4.2's index answered, because that variant
///   is the only thing `read_feed_from` returns when it takes §4.3 rule 6's
///   branch — there are no pages on that path, so there is no `{page, applied}`
///   to report.
/// - [`FeedSource::list`] **defaults to `Ok(None)`** — *this source cannot
///   enumerate* — and [`PublishedTree`] does not override it. So on the
///   published leg the fallback is not merely unused, it is unavailable: an
///   index miss is a loud [`crate::feed_read::FeedReadError::NoIndex`] and can
///   never be a quiet success.
///
/// ⭐ **That is not a virtue we designed and it is worth saying so.** It is the
/// consequence of `SignedSession::enumerate` not being wired to this trait yet,
/// and `feed_read`'s own doc records it as a gap. It does mean this seat cannot
/// have had their defect **on this leg** — and `feed_peer`, the live one, *does*
/// implement `list`, so it can. The assertion is written here rather than in a
/// comment so the day somebody wires enumeration in, the claim is re-measured
/// rather than inherited.
#[test]
fn the_index_is_what_answered_and_no_fallback_could_have_rescued_it() {
    for name in ["peer-root", "feed-only"] {
        let w = walk(name);
        assert_ne!(
            w.resumed,
            Resumed::Unpositioned,
            "{name}: §4.2's index answered, not §4.3 rule 6's enumeration"
        );
        assert_eq!(
            w.entries.len(),
            ENTRIES,
            "{name}: {ENTRIES} via the index, 0 via enumeration"
        );

        let src = open(name);
        assert_eq!(
            block_on(src.list(crate::feed::entry_prefix().to_string())),
            Ok(None),
            "{name}: the published leg cannot enumerate at all, so a broken index \
             cannot be masked by a fallback here"
        );
    }
}

// ---------------------------------------------------------------------------
// `FEED-12` — our half of the two-gatherer run
// ---------------------------------------------------------------------------
//
// §6.0.1's coordinate is `[derive-to-meet]`: two gatherers of one author must
// land on one key **with nothing failing loudly if they do not** — each simply
// publishes into a slot the other never looks in, and every assertion either
// seat can make alone stays green. That is why the vector exists and why the
// comparand has to be a value, written down, that the other seat can check
// without running our code.
//
// ⭐ **What is ours alone and what is not.** The key below and the readback are
// a one-seat run and are done. `FEED-12` proper needs the second gatherer, each
// carrying a different `via` hint, each finding the other's mirror by computing
// this key — and `entity-workbench-go` has to emit their half before anyone can
// assert the interesting direction. The protocol is in
// `ROUTING-2026-09-17-a-entity-workbench-go-…`.

/// **The `FEED-12` comparand: where a mirror of the corridor author lives.**
///
/// Derived independently of the code under test, by the method
/// [`crate::feed::path_coordinate`]'s own pinned vector uses — `hashlib` over
/// `EXTENSION-REVISION` §3.1's framing, `"00" || sha256(ecf_for_hash(
/// "system/tree/path", to_ecf(text("/{author}/app/feed/index"))))` — and
/// cross-checked by reproducing that vector's literal with the same script
/// before computing this one. **It is also what a real `publish --gather` of
/// their fixture emitted**, which is the third derivation and the only one that
/// goes through the projector.
///
/// ⚠ **A wire value.** If it moves, a published mirror moved with it, and the
/// question is which seat's derivation changed — never *"update the literal"*.
///
/// ⛔ **The COORDINATE alone, with the prefix taken from production — and that
/// is not tidiness.** Writing the whole key as a literal is the fifth instance
/// of `spec vocab` reading a tree path as a type tag (the first four: a trailing
/// `app/share/records/`, `app/feed/index`, a parametric family, and a doc
/// comment *about* the hazard). It reds `vocab-lint` with
/// `implemented-undeclared`, naming a type tag no spec declares and we do not
/// emit — the expensive false direction, accusing a conformant seat of inventing
/// vocabulary. **The repair is the one `feed::index_head_key` already took:
/// derive the key, never spell it**, which also means this pins the value that
/// is genuinely the comparand and leaves §6.0.1's prefix to the module that owns
/// it.
const MIRROR_COORDINATE: &str =
    "004245e92d8954b390b78dab80277cfb1220185f74d35935413235f0196ea2be51";

/// Our gatherer, pinned so the emission is reproducible. The CLI spelling that
/// produces the same bytes:
///
/// ```text
/// make site OUT=<dir> NO_SITES=1 \
///   GATHER='2KAoCfAP6ZZyLmS9wYz4rUmpehd4JMeek32NLN58R3ehpi@tests/fixtures/crossimpl-go-feed/peer-root' \
///   IDENTITY_SEED=00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff
/// ```
///
/// Run, not written from memory (AP37). Two cuts into different directories are
/// byte-identical in the mirror head, both pages and all 68 carried entities;
/// **only `system/peer/published-root` differs**, because it carries
/// `published_at` off a wall clock with no seam — the same `K-3` fact that makes
/// the trie root and not that head the comparand for `G-PIN-4`.
const GATHERER: &str = "2K7sRmfmtwghK8rqjkoXTM2d3EkCPNyX5XveTqZhQfs6v6";

/// Everything one gather produced, served as an origin does: any peer's segment
/// at one place. The gatherer's own record and pages under **its** id, every
/// carried body under **its author's**.
#[derive(Default)]
struct Served(std::collections::BTreeMap<(String, String), entity_entity::Entity>);

impl crate::feed_mirror::MirrorSource for Served {
    fn get(
        &self,
        peer: String,
        relative_key: String,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<entity_entity::Entity>, String>>>,
    > {
        Box::pin(std::future::ready(Ok(self.0.get(&(peer, relative_key)).cloned())))
    }
}

/// Gather the corridor author through the **shipped** gatherer — the one
/// `publish --gather` calls — and serve the result.
fn gathered() -> (crate::feed_mirror::MirrorPlan, Served) {
    let plan = block_on(crate::feed_gather::gather_timeline(
        &cut("peer-root"),
        PEER_ID,
        GATHERER,
        crate::feed_gather::DEFAULT_GATHER_LIMIT,
    ))
    .expect("their published tree gathers");

    let subject = crate::feed::MirrorSubject::from_reference(&plan.record.subject);
    let mut served = Served::default();
    for c in &plan.carried {
        served.0.insert((c.peer.clone(), c.key.clone()), c.entity.clone());
    }
    for page in &plan.pages {
        served
            .0
            .insert((GATHERER.to_string(), subject.page_key(page.page)), page.to_entity().unwrap());
    }
    served
        .0
        .insert((GATHERER.to_string(), subject.key()), plan.record.to_entity().unwrap());
    (plan, served)
}

/// **`FEED-12`, our half: the key.**
///
/// A mirror of *their* author, by *our* gatherer, is bound at the address the
/// other seat derives from the author's peer id and nothing else — no hop to the
/// author, no index, no prior contact. `entity-workbench-go` computes the same
/// string from `revision.PrefixHash("/" + peer + "/app/feed/index")`.
///
/// ⚠ **Asserted through the SUBJECT the gather produced**, never through a
/// `MirrorSubject::timeline(PEER_ID)` built here: the question is where the
/// publisher actually put it, and re-deriving the subject in the test would be
/// the test agreeing with itself about an address production disagrees with.
#[test]
fn a_mirror_of_their_author_is_bound_at_the_key_the_other_seat_derives() {
    let (plan, _) = gathered();
    let subject = crate::feed::MirrorSubject::from_reference(&plan.record.subject);

    assert_eq!(
        subject.coordinate(),
        MIRROR_COORDINATE,
        "`FEED-12`'s comparand moved — this is a WIRE event"
    );
    assert_eq!(
        subject.key(),
        format!("{}{MIRROR_COORDINATE}", crate::feed::mirror_prefix()),
        "…and §6.0.1's prefix is what the coordinate hangs off"
    );
    assert_eq!(
        subject,
        crate::feed::MirrorSubject::Timeline {
            peer: PEER_ID.to_string(),
            path: "/app/feed/index".to_string()
        },
        "`FEED-R26`: the subject of a gather is the author's LIVE index path, \
         never a pin to a head that moves when they post"
    );
}

/// **The whole hop over bytes we did not author:** their 34 entries through our
/// gatherer, paged, served, and back through `read_mirror` — the same
/// `finish_entry` a direct feed read uses — with every one still attributed to
/// **them** and every hash unmoved.
///
/// The hashes are asserted **against the direct read of the same fixture**, not
/// against a literal: what is being measured is that the hop changed nothing, so
/// the comparand is the same bytes read the other way.
///
/// ⛔⭐⭐ **What this does NOT measure, and the first draft of this comment
/// claimed it did: §6.1's byte-preservation `MUST`.** The obvious falsifier —
/// make `plan_mirror` carry `row.entry.to_entity()` instead of the obtained
/// bytes — was run against this gate and came back **GREEN**, and reds only
/// [`crate::feed_mirror::tests::a_gatherer_that_re_encodes_publishes_a_feed_nobody_wrote`],
/// whose entry carries a field the reader does not model.
///
/// ⇒ ***another implementation's bytes are a different population, not
/// automatically a divergent one.*** Our decoder round-trips every entry in
/// their fixture losslessly because both seats model the same fields — which is
/// the corridor working, and is exactly why it cannot falsify the rule that
/// exists for the case where they do not. §6.1 names the hazard as *"a round
/// trip through bytes your own encoder produced proves nothing"* and the
/// property that repairs it is **an unknown field**, not a foreign author. A
/// cross-implementation instance therefore needs one authored by *them*: a
/// one-flag arm at cut time, the same shape as the two-cut ask `FEED-14` made.
/// Routed; until it exists, the byte-preservation `MUST` is measured here by a
/// synthetic entry and by nothing cross-implementation.
#[test]
fn their_entries_survive_our_gather_and_still_name_them() {
    let direct = walk("peer-root");
    let (plan, served) = gathered();
    let subject = crate::feed::MirrorSubject::from_reference(&plan.record.subject);

    let read = block_on(crate::feed_mirror::read_mirror(&served, GATHERER, &subject, 100))
        .expect("our own mirror of their feed reads back");

    assert_eq!(read.len(), ENTRIES, "an entry was lost crossing the gather");
    assert_eq!(
        read.iter().filter(|e| e.attribution.may_name_the_author()).count(),
        ENTRIES,
        "`FEED-R2` through a republication: every entry still names its author. \
         Unattributed: {:?}",
        read.iter()
            .filter(|e| !e.attribution.may_name_the_author())
            .map(|e| (e.hash.to_hex(), e.attribution.clone()))
            .collect::<Vec<_>>()
    );
    for row in &read {
        assert_eq!(row.entry.author, PEER_ID, "§6.1 rule 3: the gatherer is not the author");
    }

    let mut before: Vec<String> = direct.entries.iter().map(|e| e.hash.to_hex()).collect();
    let mut after: Vec<String> = read.iter().map(|e| e.hash.to_hex()).collect();
    before.sort();
    after.sort();
    assert_eq!(before, after, "a hash moved across the republication");
}

/// **`FEED-13`'s anti-vacuity arm, over a real foreign corpus.**
///
/// Their 34 entries against our default 32-entry page size is two pages, so the
/// paging gates in `feed_mirror` — which run on fixtures we authored — have a
/// cross-implementation instance: a reader asking for one page's worth stops
/// after one page, and the view genuinely spans more than one.
#[test]
fn a_gathered_view_of_their_feed_spans_more_than_one_page() {
    let (plan, served) = gathered();
    let subject = crate::feed::MirrorSubject::from_reference(&plan.record.subject);

    assert!(
        plan.pages.len() > 1,
        "34 entries at a {}-entry page size is not one page — a single-page view \
         passes against a flat list and measures nothing",
        crate::feed_publish::DEFAULT_PAGE_SIZE
    );
    assert_eq!(plan.record.current as usize, plan.pages.len() - 1, "the head names the top page");
    assert_eq!(plan.record.oldest, 0, "nothing has been dropped");

    let short = block_on(crate::feed_mirror::read_mirror(&served, GATHERER, &subject, 2))
        .expect("the walk reads");
    assert_eq!(short.len(), 2, "a reader asking for two got two");
}


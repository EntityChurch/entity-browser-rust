//! **What enters a publish's projection** — the one list, and the obligations a
//! convention takes on by joining it.
//!
//! `AGENTS.md` has recorded the defect this replaces for weeks: *"what ENTERS
//! the projection is a hardcoded enumeration of two L5 conventions, not a policy
//! over the tree"* — `emit_owned_sites(sites)` followed by
//! `for set in app_sets { emit_app_set }`, spelled out inside `run_projection`,
//! with a third convention costing an edit in five places. It also recorded when
//! to fix it: ***structurally WITH the third axis, not speculatively before***,
//! because a registration table with one implementor and two special cases is a
//! table nobody can trust. The feed is that third axis.
//!
//! ## What a row buys, and what it does not
//!
//! A row is the **`.bin` projection** — read a subgraph out of the peer's tree,
//! record it into the one [`RootProjector`] whose `finish` signs the publish.
//! That is the half all three conventions share, and it is the half a fourth
//! gets for free: implement [`PublishAxis`], add a line to [`axes`], and the
//! compiler will not let you skip an obligation.
//!
//! **It is deliberately not everything a publish does**, and the residue is
//! named here rather than left for the next reader to discover:
//!
//! | still per-convention | why |
//! |---|---|
//! | the legacy-web `.html` export | **only sites have one.** A projection to HTML is a site concern by construction — `static_export` renders pages and nav, and a feed's analogue would be a different artifact answering a different question |
//! | `--bare-root` | one site at a domain root; there is no "bare-root feed" |
//! | `--plan`'s per-unit add/remove naming | the value of the plan is naming *which site* disappears, and a uniform "N units" term would be strictly worse reporting than what sites and apps have. Every axis owes a term (see [`PublishAxis::incoming`]); only the naming is bespoke |
//! | the `http_poll` URL builders | consumer-side, and the feed's already exist ([`crate::feed_fetch`]) |
//!
//! ## The clean is wholesale, so [`PublishAxis::tree_prefixes`] is load-bearing
//!
//! `run_projection` removes `{base}/{peer}/` in one `remove_dir_all`. Anything
//! under a peer's prefix is therefore **in scope by construction** — a publish
//! that forgets to re-emit an axis does not leave it alone, it deletes it. That
//! is what made the apps blind spot expensive (*"plan a content fix, forget
//! `--ingest-apps`, publish"* reported *"nothing would be removed"* and then
//! deleted every app bundle on the domain), and it is why a row states its
//! prefixes: they are what the accounting is *about*.
//!
//! ## ⭐ The declaration is MEASURED, and it was a false statement until it was
//!
//! `tree_prefix` was singular, `FeedAxis` answered `app/feed/`, and
//! [`crate::feed_publish::publish_feed`] also writes `system/signature/{hex}` —
//! `FEED-R2`'s per-entry detached signature, at the kernel's invariant pointer.
//! So the one method whose whole job is to state what a publish touches was
//! **wrong about the axis that had most recently joined the table**, and the
//! only thing standing between that and a live defect was that nothing read it:
//! the clean is `clean.push(base.join(peer_id))`, the whole peer subtree, by a
//! mechanism that never consults the declaration.
//!
//! ⇒ ***a value that is only printed is not a guard.*** Second instance in this
//! tree after `--prune`'s `assets_named_by`, which was described in its own doc
//! as *"the whole safety argument"* and wired to nothing — and a second instance
//! of a shape in one tree is when it earns a check rather than a note.
//! [`every_key_an_axis_records_falls_under_a_prefix_it_declared`] is the check:
//! it runs each axis's [`project`](PublishAxis::project) against a real
//! projector and compares the keys it **recorded** with the prefixes it
//! **declared**, so the two can no longer drift in silence.
//!
//! **What the false declaration was one commit away from costing**, stated
//! because it is the reason this is not hygiene: the day anyone scopes the clean
//! to the declared prefixes — which is what the declaration is *for* —
//! `app/feed/` is cleaned and rebuilt while `system/signature/` is left behind,
//! unbound by the new root, and **every entry in a statically published feed
//! becomes unattributable.** `entity-workbench-go` reached exactly that
//! conclusion reading our source; it was wrong about today's code (our root
//! commits over the peer subtree, so an own-feed signature is in it — measured,
//! `a_published_feed_reaches` reads 34 of 34 attributed) and right about the
//! system the declaration described.
//!
//! **This is the emitter half of arch's `A-36` ruling** (*the publisher commits
//! to the evidence, or the subject is not attributable*): the ruling is that a
//! publish MUST commit over a scope containing both the entries and their
//! signatures, which our peer-root projection already does. Nothing on the
//! emitter moved. What moved is that the table now **says** so, and the reader
//! half is `FEED-14` —
//! `crate::feed_publish::a_feed_published_over_a_scope_that_excludes_the_signatures_is_read_as_unattributed`.

// Native-only: every emitter below writes a directory, which is what
// `RootProjector`, `publish_fixture` and `feed_publish` are all gated on.
#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;

use crate::content_site::read::OwnedSite;
use crate::content_site::signed_root::RootProjector;
use crate::content_site::{paths, publish_fixture};
use crate::feed_tree::OwnedFeed;

/// One L5 convention's `.bin` half of a publish.
///
/// Implement it, add a line to [`axes`], and the four obligations below are
/// enforced by the compiler rather than by whoever remembers to grep
/// `run_projection`.
pub trait PublishAxis {
    /// What an operator sees this called in a report or a refusal.
    fn name(&self) -> &'static str;

    /// Every **peer-relative** tree prefix this axis writes under — what the
    /// wholesale clean removes, and therefore what a publish carrying none of
    /// this axis silently deletes. See the module doc.
    ///
    /// ⚠ **Plural, because an axis does not only write its own convention's
    /// namespace.** The feed's entries and index live under `app/feed/`, and
    /// `FEED-R2`'s per-entry signature lives at `system/signature/{hex}` — the
    /// *kernel's* invariant pointer (V7 §3.5), which the convention borrows
    /// rather than owns. A singular answer forced that second prefix to go
    /// unstated, which it did for two days.
    ///
    /// **Under the publisher's own peer, always.** Foreign segments are
    /// [`carried_peers`](Self::carried_peers)' subject and are deliberately not
    /// expressible here: the clean is rooted at `{base}/{peer}/` and cannot
    /// reach them, so listing one would describe a removal that cannot happen.
    fn tree_prefixes(&self) -> Vec<&'static str>;

    /// How many units this publish carries: sites, app bundles, posts. Zero
    /// means the axis contributes nothing — which is a real answer and not an
    /// error, since most publishers use one convention.
    fn incoming(&self) -> usize;

    /// **Peers other than the publisher whose segments this axis writes into.**
    ///
    /// Three of the four rows answer with an empty list, and until the mirror
    /// they all did — a publish writes under its own peer, which is exactly what
    /// made [`tree_prefix`](Self::tree_prefix) a *complete* statement of what
    /// the wholesale clean removes. `APP-CONVENTION-FEED` §6's gatherer is the
    /// first axis whose bytes are not its publisher's: a carried entry is bound
    /// at its own **author's** address, which is precisely what lets a consumer
    /// reach it with the reader it already has.
    ///
    /// `REFERENCE-PUBLISHING-PIPELINE` §0.2a recorded that as the reason a
    /// mirror *"is not a row"* — `tree_prefix`, whose whole job is to name what
    /// the clean would remove, cannot describe this axis truthfully. **This
    /// method is what makes that false**: a row states the foreign half instead
    /// of `tree_prefix` lying about it. Two consequences it exists to carry:
    ///
    /// 1. **The wholesale clean does not reach them.** `{base}/{author}/` is not
    ///    `{base}/{peer}/`, so carried pointers survive a republish that drops
    ///    the author — the safe direction (D24), and **not free**, because the
    ///    blobs those pointers name live in the SHARED `content/` store, which
    ///    the clean *does* remove. `run_projection` suppresses that half when a
    ///    foreign tree is present, and the gate is
    ///    `a_republish_that_does_not_re_gather_leaves_no_dangling_pointer`.
    /// 2. **`--verify`'s sweep is rooted at `{base}/{peer}/`**, so it never
    ///    visits them either. That one is answered by structure rather than by
    ///    scope — a mirror record *declares* what it carries, so `run_verify`
    ///    follows the declaration instead of widening the sweep.
    ///
    /// ⚠ **Returned for reporting and for suppression, never to authorize a
    /// delete.** Enumerating to decide what *not* to remove is safe in the worst
    /// case (orphans accumulate); enumerating to decide what to remove is how a
    /// publish destroys a co-hosted publisher's tree, which this repo has
    /// already done once (AP52/AP53).
    ///
    /// **Deliberately not defaulted.** A `fn carried_peers(&self) -> Vec<String>
    /// { Vec::new() }` on the trait would be right for every axis that exists
    /// and silently wrong for the next one that is not — AP44's shape exactly,
    /// where the rule decays on the first implementor who did not have the whole
    /// set in their head. Three rows below answer it in one line; a fifth
    /// convention gets `error[E0046]` instead of an empty list it never chose.
    fn carried_peers(&self) -> Vec<String>;

    /// Record every entity into `root`. Returns a report line when there is
    /// something to say.
    ///
    /// **`out_dir` is the un-prefixed output root**, because that is what two of
    /// the three emitters take; an implementor whose emitter wants the prefixed
    /// base joins it itself ([`paths::prefixed_root`]). Getting that backwards
    /// publishes a whole axis *outside* the hosting scope it belongs to, where
    /// no consumer resolving `{origin}/{prefix}/…` will ever look — gated by
    /// `a_prefixed_publish_puts_every_axis_inside_the_prefix`.
    fn project(
        &self,
        out_dir: &Path,
        peer_id: &str,
        prefix: &str,
        root: &mut RootProjector,
    ) -> Result<Option<String>, String>;
}

/// **The list.** A fourth convention is a line here and an `impl` above.
///
/// Order is report order, not a dependency: the root is a trie over every
/// binding and does not care which arrived first.
pub fn axes<'a>(
    peer_id: &'a str,
    sites: &'a [OwnedSite],
    app_sets: &'a crate::apps::ingest::IngestedSets,
    feed: Option<&'a OwnedFeed>,
    mirrors: &'a [crate::feed_mirror::MirrorPlan],
) -> Vec<Box<dyn PublishAxis + 'a>> {
    vec![
        Box::new(SiteAxis { sites }),
        Box::new(AppsAxis { peer_id, sets: app_sets }),
        Box::new(FeedAxis { feed }),
        Box::new(MirrorAxis { mirrors }),
    ]
}

// ---------------------------------------------------------------------------
// sites — `APP-CONVENTION-SEMANTIC-CONTENT-SITE`
// ---------------------------------------------------------------------------

pub struct SiteAxis<'a> {
    pub sites: &'a [OwnedSite],
}

impl PublishAxis for SiteAxis<'_> {
    fn name(&self) -> &'static str {
        "sites"
    }
    fn tree_prefixes(&self) -> Vec<&'static str> {
        vec!["sites/"]
    }
    fn incoming(&self) -> usize {
        self.sites.len()
    }
    /// A site publish writes under its publisher and nowhere else.
    fn carried_peers(&self) -> Vec<String> {
        Vec::new()
    }
    fn project(
        &self,
        out_dir: &Path,
        _peer_id: &str,
        prefix: &str,
        root: &mut RootProjector,
    ) -> Result<Option<String>, String> {
        publish_fixture::emit_owned_sites(out_dir, self.sites, prefix, Some(root))
            .map_err(|e| e.to_string())?;
        // Sites are reported by `report_projection`, which names each one and
        // its page count — richer than a line here could be, and it has to run
        // on the `--html-only` path too where this axis never executes.
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// apps — the app-set subgraph
// ---------------------------------------------------------------------------

pub struct AppsAxis<'a> {
    pub peer_id: &'a str,
    pub sets: &'a crate::apps::ingest::IngestedSets,
}

impl PublishAxis for AppsAxis<'_> {
    fn name(&self) -> &'static str {
        "apps"
    }
    fn tree_prefixes(&self) -> Vec<&'static str> {
        vec!["apps/"]
    }
    fn incoming(&self) -> usize {
        self.sets.values().map(|i| i.catalog.entries.len()).sum()
    }
    /// An app set is the publisher's own; bundles live at `{peer}/apps/…`.
    fn carried_peers(&self) -> Vec<String> {
        Vec::new()
    }
    fn project(
        &self,
        out_dir: &Path,
        peer_id: &str,
        prefix: &str,
        root: &mut RootProjector,
    ) -> Result<Option<String>, String> {
        let mut lines = Vec::new();
        for (set, ing) in self.sets {
            if ing.catalog.entries.is_empty() {
                continue;
            }
            let n = publish_fixture::emit_app_set(
                out_dir,
                peer_id,
                set,
                &ing.catalog,
                &ing.bundles,
                &ing.assets,
                prefix,
                Some(root),
            )
            .map_err(|e| format!("app-set '{set}': {e}"))?;
            let mut line = format!("apps[{set}]: {n} bundle(s) → {peer_id}/apps/{set}/");
            if !ing.assets.is_empty() {
                let files: usize = ing.assets.iter().map(|b| b.index.entries.len()).sum();
                let bytes: u64 = ing.assets.iter().map(|b| b.index.total_bytes()).sum();
                line.push_str(&format!(
                    " + {} asset bundle(s), {files} file(s), {:.1} MiB",
                    ing.assets.len(),
                    bytes as f64 / 1048576.0
                ));
            }
            lines.push(line);
        }
        Ok((!lines.is_empty()).then(|| lines.join("\n  ")))
    }
}

// ---------------------------------------------------------------------------
// feed — `APP-CONVENTION-FEED`
// ---------------------------------------------------------------------------

pub struct FeedAxis<'a> {
    pub feed: Option<&'a OwnedFeed>,
}

impl PublishAxis for FeedAxis<'_> {
    fn name(&self) -> &'static str {
        "feed"
    }
    fn tree_prefixes(&self) -> Vec<&'static str> {
        vec![
            // §4.2's pinned index and, beneath it, the pages. Our entry keys
            // (`app/feed/entries/`) sit alongside; both are under `app/feed/`.
            "app/feed/",
            // ⭐ **`FEED-R2`, and the prefix this axis does not own.** One
            // detached `system/signature` per entry, at V7 §3.5's invariant
            // pointer — the kernel's namespace, which is why it is easy to
            // forget it is written here. It is also the whole of arch's `A-36`:
            // a publish scope that excludes this prefix produces a feed whose
            // every entry reads as unattributed, and a reader cannot tell that
            // from an author who never signed.
            "system/signature/",
        ]
    }
    fn incoming(&self) -> usize {
        self.feed.map_or(0, |f| f.entries.len())
    }
    /// A peer has ONE feed and it is theirs — `FEED-R1` refuses an entry
    /// claiming any other author, on the emitting side as well as the
    /// reading one, so this axis cannot write into a stranger's namespace.
    fn carried_peers(&self) -> Vec<String> {
        Vec::new()
    }
    fn project(
        &self,
        out_dir: &Path,
        _peer_id: &str,
        prefix: &str,
        root: &mut RootProjector,
    ) -> Result<Option<String>, String> {
        let Some(feed) = self.feed else { return Ok(None) };
        // **The prefixed base, because `publish_feed` writes `{dir}/{peer}/…`
        // directly** — unlike the two emitters above it takes no `prefix`. A
        // feed written to the un-prefixed root would sit outside the hosting
        // scope its own signed root is served from.
        let base = paths::prefixed_root(out_dir, prefix);
        let report = crate::feed_publish::publish_feed(
            &base,
            root,
            &feed.entries,
            &feed.content,
            crate::feed_publish::DEFAULT_PAGE_SIZE,
            head_clock(feed),
        )?;
        Ok(Some(format!(
            "feed: {} post(s) over {} page(s) → {}/app/feed/",
            report.entry_count, report.page_count, feed.peer_id
        )))
    }
}

// ---------------------------------------------------------------------------
// mirrors — `APP-CONVENTION-FEED` §6
// ---------------------------------------------------------------------------

/// **The fourth axis — and the one that pays for [`PublishAxis::carried_peers`].**
///
/// `REFERENCE-PUBLISHING-PIPELINE` §0.2a left three questions open before a
/// mirror could be a row. Answered here, and the answers are in the code rather
/// than only in the prose:
///
/// **1. A fourth axis, not a second subgraph of [`FeedAxis`].** The deciding
/// argument is the *plan's* per-unit term, not tidiness: `run_plan` exists to
/// name **which** thing a republish would delete, and a publisher who carries
/// posts without re-gathering would otherwise be told *"0 post(s) REMOVED"* on
/// the same run that deletes every mirror they hold. A post and a gathered view
/// are different units and [`incoming`](PublishAxis::incoming) is a count of
/// units.
///
/// **2. The prefix NESTS, and it is asserted rather than implied.**
/// `app/feed/mirrors/` is inside `app/feed/`, so the census is not a partition
/// and would be misread as one — `the_only_nesting_between_two_axes_is_the_one_
/// feed_6_puts_there` pins exactly which pair overlaps, so a fifth convention
/// that collides with an existing prefix by accident fails instead of quietly
/// sharing accounting. Both statements are true at once because the clean is
/// wholesale over `{base}/{peer}/`: every prefix here is nested inside *that*.
///
/// **3. `--verify` is answered by structure, not by scope** — see `run_verify`'s
/// mirror arm. §0.2a said *"this one is the wrong root and no extra arm fixes
/// it"*; that is true of an arm operating on the swept file set and false of one
/// that follows a **declaration**, which is the same move that closed the
/// `app/site-asset` and `app/feed/entry` instances (*a heuristic scan filters on
/// presence, so it cannot see absence — structure can*).
pub struct MirrorAxis<'a> {
    pub mirrors: &'a [crate::feed_mirror::MirrorPlan],
}

impl PublishAxis for MirrorAxis<'_> {
    fn name(&self) -> &'static str {
        "mirrors"
    }
    fn tree_prefixes(&self) -> Vec<&'static str> {
        // §6.0.1's derived key, under the GATHERER. The carried bodies are not
        // here and cannot be — see `carried_peers`. No `system/signature/` row:
        // a gathered entry's signature is its **author's**, so it is carried
        // under the author's segment and never minted under ours.
        vec![crate::feed::mirror_prefix()]
    }
    fn incoming(&self) -> usize {
        self.mirrors.len()
    }
    /// ⭐ **The only non-empty answer in the table.** Each gathered author's
    /// entries and detached signatures are bound under *their* peer segment,
    /// which is what makes them reachable by the reader a consumer already has
    /// (`feed_mirror::Carried`).
    fn carried_peers(&self) -> Vec<String> {
        let mut peers: Vec<String> =
            self.mirrors.iter().flat_map(|m| m.carried_peers()).collect();
        peers.sort();
        peers.dedup();
        peers
    }
    fn project(
        &self,
        out_dir: &Path,
        _peer_id: &str,
        prefix: &str,
        root: &mut RootProjector,
    ) -> Result<Option<String>, String> {
        if self.mirrors.is_empty() {
            return Ok(None);
        }
        // The prefixed base, for `FeedAxis`'s reason: `publish_mirror` writes
        // `{dir}/{peer}/…` directly and takes no prefix of its own.
        let base = paths::prefixed_root(out_dir, prefix);
        let mut entries = 0usize;
        for plan in self.mirrors {
            crate::feed_mirror::publish_mirror(&base, root, plan)?;
            entries += plan.entry_count();
        }
        let authors = self.carried_peers().len();
        Ok(Some(format!(
            "mirrors: {} gathered view(s), {entries} entry(ies) from {authors} author(s) \
             → {}",
            self.mirrors.len(),
            crate::feed::mirror_prefix(),
        )))
    }
}

/// The instant the feed's §4.2 head is stamped with.
///
/// ⚠ **`SystemTime::now()` is the obvious answer and it would make every publish
/// of this tree produce a different signed root.** `plan_index` gives the head
/// the *publish* instant deliberately — it is the one small mutable pointer a
/// reader polls, and a page's stamp is a witness of its own entries so that an
/// untouched page keeps its bytes (`A-41`). But **who supplies that instant is
/// the caller's decision**, and a wall clock is what turns a projection into
/// something that is not a function of the tree it read: the head lands in the
/// trie, so the site root moves on every run, and `G-PIN-4`'s whole comparand —
/// *one fixture, two publishers, identical site root* — becomes unreachable for
/// any tree carrying a feed. It is exactly the defect
/// `system/peer/published-root`'s `published_at` already has, and which
/// `EXTENSION-TREE` §3.2's determinism rule 3 states as *"no timestamp — a
/// snapshot is pure structural data."*
///
/// So the publisher hands it the feed's own **high-water mark**: the newest
/// authored `created_at`. It still moves whenever the feed changes, which is
/// everything a poller needs, and it never moves when it has not.
fn head_clock(feed: &OwnedFeed) -> u64 {
    feed.entries.iter().map(|e| e.created_at).max().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// **A publish's axes are enumerated in one place, and every one of them
    /// states the tree prefixes the wholesale clean removes.** The census is the
    /// point: the clean deletes `{base}/{peer}/` entire, so an axis nobody
    /// listed is not an axis that is left alone — it is one that is silently
    /// deleted the first time somebody publishes without it.
    ///
    /// ⚠ **This census asserts what the table SAYS. It cannot tell you whether
    /// the table is true** — that is
    /// [`every_key_an_axis_records_falls_under_a_prefix_it_declared`], and for
    /// two days this test was green over a `FeedAxis` row that was wrong.
    #[test]
    fn every_axis_names_the_tree_prefixes_the_clean_would_remove() {
        let sets = crate::apps::ingest::IngestedSets::new();
        let all = axes("QmPeer", &[], &sets, None, &[]);
        let rows: Vec<(&str, Vec<&str>)> =
            all.iter().map(|a| (a.name(), a.tree_prefixes())).collect();
        assert_eq!(
            rows,
            vec![
                ("sites", vec!["sites/"]),
                ("apps", vec!["apps/"]),
                ("feed", vec!["app/feed/", "system/signature/"]),
                ("mirrors", vec!["app/feed/mirrors/"]),
            ],
            "a fifth L5 convention is a row in `axes` — if this fails because you added \
             one, add it here and check `run_plan` gives it a term"
        );
        // Nothing to publish is not an error on any axis; most publishers use
        // one convention.
        assert!(all.iter().all(|a| a.incoming() == 0));
    }

    /// ⭐⭐ **THE DECLARATION, MEASURED — every key an axis records falls under a
    /// prefix that axis declared.**
    ///
    /// Each axis is handed something real to publish, projected into a temp dir
    /// through a real [`RootProjector`], and the keys it **recorded** are
    /// compared against the prefixes it **declared**. That is the difference
    /// between a table and a guard: the census above is satisfied by whatever
    /// the rows happen to say, and this one is satisfied only by their being
    /// true.
    ///
    /// **It found a live false statement the first time it was run** —
    /// `FeedAxis` declared `app/feed/` and records one `system/signature/{hex}`
    /// per entry (`FEED-R2`). See the module doc for what that was one commit
    /// away from costing.
    ///
    /// **Anti-vacuity is the load-bearing half**, because an axis handed nothing
    /// records nothing and passes trivially: every axis must contribute at least
    /// one key, and the set of axes exercised is asserted, so a fifth row cannot
    /// join the table without a fixture. *A census you have not falsified reports
    /// what you hoped.*
    ///
    /// **Foreign keys are out of scope by construction, not by filtering:**
    /// [`RootProjector::record`] early-returns for a peer that is not the
    /// projector's own, so a mirror's carried entries never reach `bindings`.
    /// That is [`PublishAxis::carried_peers`]' subject and
    /// [`only_the_mirror_axis_writes_under_a_peer_that_is_not_the_publisher`]
    /// is where it is measured.
    #[test]
    fn every_key_an_axis_records_falls_under_a_prefix_it_declared() {
        let dir = tempfile::tempdir().unwrap();
        let mut projector = RootProjector::new(fixture_identity()).unwrap();
        let peer_id = projector.peer_id().to_string();

        let sites = vec![fixture_site(&peer_id)];
        let sets = fixture_app_sets();
        let feed = fixture_feed(&peer_id);
        let mirrors = fixture_mirrors(&peer_id);

        let mut exercised: Vec<&str> = Vec::new();
        let mut recorded_by_axis: Vec<(&str, Vec<String>)> = Vec::new();
        for axis in axes(&peer_id, &sites, &sets, Some(&feed), &mirrors) {
            let before: BTreeSet<String> =
                projector.bindings_for_measurement().into_keys().collect();
            axis.project(dir.path(), &peer_id, "", &mut projector)
                .unwrap_or_else(|e| panic!("axis '{}' projects: {e}", axis.name()));
            let after: BTreeSet<String> =
                projector.bindings_for_measurement().into_keys().collect();

            let new_keys: Vec<String> = after.difference(&before).cloned().collect();
            assert!(
                !new_keys.is_empty(),
                "axis '{}' recorded nothing — its fixture does not exercise it, so this \
                 gate cannot see whether its declaration is true",
                axis.name()
            );

            let declared = axis.tree_prefixes();
            let undeclared: Vec<&String> = new_keys
                .iter()
                .filter(|k| !declared.iter().any(|p| k.starts_with(p)))
                .collect();
            assert!(
                undeclared.is_empty(),
                "axis '{}' declares {declared:?} and recorded {undeclared:?}. A key outside \
                 every declared prefix is a publish this table cannot account for — add the \
                 prefix to `tree_prefixes`, and check whether a clean scoped to the \
                 declaration would strand it.",
                axis.name()
            );

            exercised.push(axis.name());
            recorded_by_axis.push((axis.name(), new_keys));
        }

        assert_eq!(
            exercised,
            vec!["sites", "apps", "feed", "mirrors"],
            "a fifth axis joined the table without a fixture here, so its declaration is \
             unmeasured"
        );

        // ⭐ The row this gate exists for, named rather than left implicit: the
        // feed records under BOTH of its declared prefixes, so dropping either
        // one reds this test rather than going quietly unstated.
        let feed_keys = &recorded_by_axis
            .iter()
            .find(|(n, _)| *n == "feed")
            .expect("the feed axis ran")
            .1;
        assert!(
            feed_keys.iter().any(|k| k.starts_with("app/feed/")),
            "the feed recorded no entry or index key: {feed_keys:?}"
        );
        assert!(
            feed_keys.iter().any(|k| k.starts_with("system/signature/")),
            "the feed recorded no `FEED-R2` signature, so this gate's own subject is \
             absent from its fixture: {feed_keys:?}"
        );
    }

    // -- fixtures for the measured census -----------------------------------

    fn fixture_identity() -> entity_crypto::Keypair {
        entity_crypto::Keypair::from_seed([23u8; 32])
    }

    fn fixture_site(peer_id: &str) -> OwnedSite {
        use crate::content_site::format::{SiteManifest, SitePage};
        OwnedSite {
            peer_id: peer_id.to_string(),
            site_id: "demo".into(),
            manifest: SiteManifest::new("demo", "Demo", "index", vec![]),
            pages: vec![("index".to_string(), SitePage::markdown("Index", "hello"))],
            assets: vec![],
            content: Default::default(),
        }
    }

    fn fixture_app_sets() -> crate::apps::ingest::IngestedSets {
        use crate::apps::format::{AppBundle, AppCatalog, AppEntry};
        use crate::apps::ingest::IngestedApps;
        let mut sets = crate::apps::ingest::IngestedSets::new();
        sets.insert(
            "apps".to_string(),
            IngestedApps {
                catalog: AppCatalog {
                    entries: vec![AppEntry {
                        id: "demo".into(),
                        name: "Demo".into(),
                        ..Default::default()
                    }],
                },
                bundles: vec![("demo".to_string(), AppBundle::new("<html></html>"))],
                assets: vec![],
            },
        );
        sets
    }

    fn fixture_feed(peer_id: &str) -> OwnedFeed {
        use crate::embed::{EmbedData, EmbedNode, EmbedPayload};
        use crate::feed::FeedEntry;
        let body = EmbedNode::new(
            "text/plain",
            EmbedData::new(EmbedPayload::Inline(b"a post".to_vec()), "a post"),
        );
        OwnedFeed {
            peer_id: peer_id.to_string(),
            entries: vec![FeedEntry::new(peer_id, 1_000, body)],
            content: Vec::new(),
        }
    }

    fn fixture_mirrors(gatherer: &str) -> Vec<crate::feed_mirror::MirrorPlan> {
        use crate::feed::{FeedMirror, MirrorSubject};
        use crate::feed_mirror::{Carried, MirrorPlan};
        let author = "2AuthorPeerIdForThisCensus";
        vec![MirrorPlan {
            record: FeedMirror::new(
                MirrorSubject::timeline(author).reference(),
                0,
                7,
                gatherer,
            ),
            pages: Vec::new(),
            carried: vec![Carried {
                peer: author.to_string(),
                key: "app/feed/entries/ff".into(),
                entity: entity_entity::Entity::new(
                    "app/feed/entry",
                    entity_ecf::to_ecf(&entity_ecf::Value::Map(Vec::new())),
                )
                .unwrap(),
            }],
            content: Vec::new(),
        }]
    }

    /// ⛔ **The census reads like a partition and is not one — so say which pair
    /// overlaps, by name.**
    ///
    /// `app/feed/mirrors/` nests inside `app/feed/` because §6 puts it there,
    /// and that is a fact about the convention rather than a collision. Every
    /// *other* pair is disjoint, and a fifth row that lands inside an existing
    /// prefix by accident would share another axis's accounting silently — the
    /// two would each report a term, and one clean would satisfy both.
    ///
    /// Asserted as an exhaustive set rather than "the mirror nests": a check
    /// spelled that way is satisfied by a table in which everything nests.
    #[test]
    fn the_only_nesting_between_two_axes_is_the_one_feed_6_puts_there() {
        let sets = crate::apps::ingest::IngestedSets::new();
        let all = axes("QmPeer", &[], &sets, None, &[]);

        let mut nested: Vec<(&str, &str)> = Vec::new();
        for a in &all {
            for b in &all {
                if a.name() == b.name() {
                    continue;
                }
                // Plural on both sides: a pair collides if ANY of one axis's
                // prefixes sits inside ANY of the other's. Checking only the
                // first would have stopped seeing the feed's second prefix the
                // day it was added.
                if a.tree_prefixes()
                    .iter()
                    .any(|pa| b.tree_prefixes().iter().any(|pb| pa.starts_with(pb)))
                {
                    nested.push((a.name(), b.name()));
                }
            }
        }
        assert_eq!(
            nested,
            vec![("mirrors", "feed")],
            "an axis prefix moved inside another one. If that is deliberate, it belongs \
             in this list with the reason; if it is not, the two rows now report one \
             subgraph twice."
        );
    }

    /// ⭐ **Exactly one axis writes outside its publisher's namespace, and the
    /// table says which.**
    ///
    /// Until §6's gatherer this was true of all of them and nothing recorded it,
    /// which is why `tree_prefix` read as a complete statement of what a publish
    /// touches. The count is asserted so a fifth convention that carries foreign
    /// bytes cannot join the table without the clean and the verify being asked
    /// about it.
    #[test]
    fn only_the_mirror_axis_writes_under_a_peer_that_is_not_the_publisher() {
        let sets = crate::apps::ingest::IngestedSets::new();
        let empty = axes("QmPeer", &[], &sets, None, &[]);
        assert!(
            empty.iter().all(|a| a.carried_peers().is_empty()),
            "an axis carrying nothing still named a foreign peer"
        );

        // One mirror of one author, which is the only shape that can answer
        // non-empty — and a plan with no carried rows would make this vacuous.
        let author = "2AuthorPeerIdForThisCensus";
        let mirrors = fixture_mirrors("QmPeer");
        let all = axes("QmPeer", &[], &sets, None, &mirrors);
        let carrying: Vec<(&str, Vec<String>)> = all
            .iter()
            .map(|a| (a.name(), a.carried_peers()))
            .filter(|(_, peers)| !peers.is_empty())
            .collect();
        assert_eq!(carrying, vec![("mirrors", vec![author.to_string()])]);
    }

    /// **The head's clock is the feed's own high-water mark, not the wall
    /// clock** — so two publishes of one unchanged tree produce one root. See
    /// [`head_clock`].
    #[test]
    fn the_feeds_head_is_stamped_from_its_entries_and_not_from_the_wall_clock() {
        use crate::embed::{EmbedData, EmbedNode, EmbedPayload};
        use crate::feed::FeedEntry;

        let node = |s: &str| {
            EmbedNode::new(
                "text/plain",
                EmbedData::new(EmbedPayload::Inline(s.as_bytes().to_vec()), s),
            )
        };
        let feed = OwnedFeed {
            peer_id: "QmPeer".into(),
            entries: vec![
                FeedEntry::new("QmPeer", 1_000, node("first")),
                // Out of order on purpose: `max`, not `last`. §4.5 makes the
                // order authored, so a backdated post at the end must not drag
                // the high-water mark backwards.
                FeedEntry::new("QmPeer", 9_000, node("newest")),
                FeedEntry::new("QmPeer", 2_000, node("backdated")),
            ],
            content: Vec::new(),
        };
        assert_eq!(head_clock(&feed), 9_000);

        let empty =
            OwnedFeed { peer_id: "QmPeer".into(), entries: Vec::new(), content: Vec::new() };
        assert_eq!(head_clock(&empty), 0, "and it never reaches for a clock it does not have");
    }
}

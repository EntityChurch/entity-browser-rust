//! **Peer supersession — "the peer you have was replaced by this one."**
//!
//! When a domain re-keys, every durable thing that named the retired publisher
//! is stale at once: the session config's `home_site`, the nav state of every
//! window AND the overlay, cached site preferences, a share link somebody
//! bookmarked. The first fix for this chased those down one surface at a time
//! and got two of them, which is not a fix — it leaves the same bug reachable
//! by a different door, and the door it leaves open is whichever surface the
//! author did not happen to be looking at. (`surface = site` healed;
//! `surface = window`, which is how `entitychurchfoundation.org` deploys, did
//! not.)
//!
//! **So this does not chase surfaces. It records the fact.** A supersession is
//! a fact about a *peer*, not about a window: one entity, written once at
//! adoption, consulted where a stored peer-id is turned back into something we
//! act on.
//!
//! ## Exactly what is covered — read this before trusting the paragraph above
//!
//! [`resolve`] has **one** caller: `ContentSiteState::from_entity`. That covers
//! **persisted navigation state on every surface** — the overlay, every Site
//! Browser window, and any surface added later that decodes the same state —
//! which is the point, and it is what the window-surface gate proves.
//!
//! It does **not** cover other durable references to a retired peer:
//! the site-origin registry (`site_origin_path(.., target_peer_id)`), cached-site
//! preferences (`app_paths.rs` `foreign_peer_id`), the site cache
//! (`site_peer_id`), or a share link a user saved. Mostly those are inert once
//! nothing points at them — but "mostly" is not "covered", and an earlier
//! version of this comment claimed *"every surface … and stored references no
//! enumeration could have found"*, which was false. Extending the resolve to
//! those is a deliberate act with its own gate, not something to assume from
//! this paragraph. (`AUDIT-REKEY-RECONCILE-2026-08-27` F9.)
//!
//! ## Why a record and not a sweep
//!
//! A sweep was the obvious shape and it is the wrong one, for a reason worth
//! keeping: **it cannot be made correct on both arms.** Rewriting stored state
//! requires enumerating it, and window nav state lives two levels down
//! (`workspace/windows/{id}/state`). The recursive enumeration is the *sync*
//! `tree_listing` (→ `cache_list` → `starts_with`), whose Worker-arm mirror is
//! not reliably seeded at boot; the async one returns **immediate children
//! only**, and directory entries are structurally unrepresentable across the
//! worker boundary, so they are dropped before a caller could recurse. Either
//! way a sweep silently misses entries on one arm — a repair that reports
//! success while leaving some of the damage, which is the exact class
//! (AP26) this whole thread is about.
//!
//! A record has no such failure mode: the only enumeration is of this registry
//! itself, which is **flat and one level**, the shape `tree_listing_async`
//! reads correctly (and the shape `site-origins` already proves in production).
//!
//! ## Chains
//!
//! A domain may re-key more than once. Records compose — `A→B` then `B→C` — and
//! [`resolve`] follows the chain, so a browser that slept through two re-keys
//! lands on the current peer rather than the previous corpse. The walk is
//! bounded and cycle-guarded: a malformed pair must degrade to "no answer",
//! never to a hang.
//!
//! ## A record is not permanent — and that is load-bearing (audit F2)
//!
//! The first version of this module wrote records durably and had **no delete
//! path, no listing and no expiry**. That is a permanent client brick with a
//! trivial trigger: a transient bad `/entity-deployment.json` — a misconfigured
//! publish, a bad templating run — is adopted by every browser that boots in
//! that window and *survives the origin being fixed*, because nothing ever
//! re-reads it. The usual reassurance ("whoever can write the deployment doc can
//! write the WASM bundle anyway") is true and irrelevant: a bad bundle is
//! repaired by fixing the bundle, a bad supersession is repaired by nothing.
//! Recovery was clear-site-data, which is destructive and, with no export, costs
//! the user everything else too — **brick-matrix cell #6, at E5**.
//!
//! So every boot that obtains a live deployment document re-checks the records
//! against it ([`revalidate`]) and drops the ones the domain contradicts. The
//! exit becomes **E1: a reload repairs it**, which is the design target.
//!
//! Two invariants make that safe, and both are the difference between a repair
//! and a new bug:
//!
//! 1. **Absence of evidence is never evidence.** No document this boot — offline,
//!    the D23 deadline expired, or a doc that declines to name a home peer —
//!    means *no change*. A truncated document must not be able to wipe a valid
//!    repair. This is the same rule the adoption path already states ("says
//!    nothing" ≠ "says something different"), and D23's deadline makes the
//!    `None` case **more** frequent, not less, so it carries more weight now
//!    than it did before the boot fetch was bounded.
//! 2. **Revalidation runs AFTER adoption, in the same boot.** A legitimate
//!    second re-key (`A→B` already recorded, the domain now publishes `C`)
//!    arrives as a document that agrees with *neither* record until the adoption
//!    path has written `B→C`. Run the check first and it deletes `A→B` — the
//!    record a window still holding `A` depends on. Ordering is the contract,
//!    the same shape as the anti-rollback floor's "mark successful before you
//!    advance it".

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use entity_entity::Entity;

use crate::app_paths::{self, APP_ID};
use crate::peers::Peers;

/// Entity type for one supersession record (frontend app state).
const SUPERSESSION_TYPE: &str = "app/state/peer_supersession";

/// Longest chain [`resolve`] will follow. Re-keys are rare events; a chain
/// longer than this is corruption or a cycle, and the safe answer to both is
/// "stop and return what you have" rather than to keep walking.
const MAX_CHAIN: usize = 8;

thread_local! {
    /// `retired_peer_id → replacement_peer_id`, loaded once at boot.
    static ACTIVE: RefCell<BTreeMap<String, String>> = const {
        RefCell::new(BTreeMap::new())
    };
}

/// Resolve a possibly-retired peer-id to the peer that replaced it.
///
/// Returns the input unchanged when nothing superseded it — which is the
/// overwhelmingly common case and must stay allocation-cheap and total. Never
/// fails: an unknown peer is not an error, it is simply a peer we have no
/// supersession record for.
pub fn resolve(peer_id: &str) -> String {
    ACTIVE.with(|c| resolve_in(&c.borrow(), peer_id))
}

/// [`resolve`] against an arbitrary map rather than the live one.
///
/// Split out so the revalidation predicate can be exercised as a pure function
/// — it has to reason about *hypothetical* maps (what is left after a drop),
/// which the thread-local cannot express, and it is the part with the sharp
/// edges.
fn resolve_in(map: &BTreeMap<String, String>, peer_id: &str) -> String {
    let mut seen: Vec<&str> = Vec::new();
    let mut cur: &str = peer_id;
    for _ in 0..MAX_CHAIN {
        // A self-record (`A→A`) and a cycle (`A→B→A`) are the same hazard;
        // both terminate here rather than looping.
        if seen.contains(&cur) {
            break;
        }
        seen.push(cur);
        match map.get(cur) {
            Some(next) if next != cur => cur = next.as_str(),
            _ => break,
        }
    }
    cur.to_string()
}

/// The records `publisher` — the home peer the live deployment document names —
/// contradicts. Pure; the caller decides what to do with them.
///
/// Every record in this registry was authored by *this origin's*
/// `/entity-deployment.json` naming its home peer (the adoption path is the only
/// writer, and it compares against `home_site.peer_id`), so the live document is
/// authoritative over all of them. Two rules, and **the order matters**:
///
/// - **`publisher` is not retired.** A record keyed on the peer the domain says
///   is currently publishing asserts that peer is dead, and the domain has just
///   said otherwise. This is the rule that catches the F2 case, and it is also
///   what makes a deployer's *rollback* to an earlier publisher work.
/// - **Every surviving chain ends at `publisher`.** A chain terminating anywhere
///   else leads to a peer this domain has abandoned — an orphan, or corruption
///   (a cycle terminates at its own entry point, so it fails this too).
///
/// The second rule is evaluated on the map **with the first rule's drops already
/// removed**, because they interact: after a bad doc is corrected the map
/// briefly holds `A→Z` *and* `Z→A`, and judging `Z→A` against the uncorrected
/// map would condemn the one record that is right.
///
/// An empty `publisher` is not an answer and drops nothing — see the module note
/// on absence of evidence.
pub fn stale_against(map: &BTreeMap<String, String>, publisher: &str) -> Vec<String> {
    stale_against_declared(map, publisher, &BTreeMap::new())
}

/// [`stale_against`] generalized to a domain that hosts **several** publishers
/// and may declare succession for any of them — `DESIGN-RESILIENCE…` §1.1f
/// item 2.
///
/// # What "current" means, and the mistake to not repeat
///
/// **Currency is what the document AFFIRMS, never what it merely routes to.**
///
/// ```text
/// current = { home } ∪ values(declared) − keys(declared)
/// ```
///
/// The tempting definition is *every peer in `origins`*, and it is wrong in the
/// most common case there is. After a re-key the document names the **new** peer
/// as home and deliberately keeps the retired one's `origins` entry, so old URLs
/// keep resolving while visitors roll over (`HomeClaim::Takes` preserves sibling
/// origins). Reading that entry as *current* contradicts the record the re-key
/// adoption just wrote and deletes it on the same boot — measured, five e2e
/// gates, all of them re-key gates
/// (`the_retired_peer_still_being_routable_does_not_contradict_the_record`).
/// **An `origins` entry says *reachable here*, which is exactly what a retired
/// peer stays.**
///
/// So a peer is current iff the document positively names it: as the home
/// publisher, or as the *replacement* in a succession it declares. Peers it
/// declares **retired** are subtracted, which is what lets a chain's intermediate
/// hops (`A→B→C` declared as two pairs) not be mistaken for live publishers.
///
/// The two rules are the single-publisher ones with `publisher` widened to
/// `current`, and **the order still matters** for the same reason:
///
/// - **A record keyed on a current peer is dropped.** The domain has just said
///   that peer is publishing; the record asserts it is dead. This is the F2 case
///   and it is also what makes a deployer's *rollback* to an earlier publisher
///   work.
/// - **Every surviving chain must end at a current peer.** Anywhere else is an
///   orphan or corruption (a cycle terminates at its own entry point, so it
///   fails this too). Evaluated on the map with the first rule's drops already
///   removed, because they interact.
///
/// This is also what keeps F2's escape working for a *declared* succession the
/// deployer withdraws: with the declaration gone the replacement stops being
/// affirmed, the chain orphans, and rule 2 drops it on the next boot.
///
/// An empty `current` drops nothing — the document named no live publisher, and
/// absence of evidence is never evidence (module note, invariant 1).
pub fn stale_against_declared(
    map: &BTreeMap<String, String>,
    home: &str,
    declared: &BTreeMap<String, String>,
) -> Vec<String> {
    if map.is_empty() {
        return Vec::new();
    }
    let retired_by_declaration: BTreeSet<&str> =
        declared.keys().map(String::as_str).collect();
    let current: BTreeSet<&str> = std::iter::once(home)
        .chain(declared.values().map(String::as_str))
        .filter(|p| !p.is_empty() && !retired_by_declaration.contains(*p))
        .collect();
    if current.is_empty() {
        return Vec::new();
    }

    let mut stale: Vec<String> = Vec::new();
    let mut kept: BTreeMap<String, String> = BTreeMap::new();
    for (retired, replacement) in map {
        if current.contains(retired.as_str()) {
            stale.push(retired.clone());
        } else {
            kept.insert(retired.clone(), replacement.clone());
        }
    }
    for retired in kept.keys() {
        if !current.contains(resolve_in(&kept, retired).as_str()) {
            stale.push(retired.clone());
        }
    }
    stale.sort();
    stale.dedup();
    stale
}

/// True when `peer_id` has been superseded — i.e. [`resolve`] would move it.
/// Exposed so a surface can *say* "this publisher was replaced" rather than
/// silently swapping it, which is the difference between a repair the user can
/// understand and one that is merely quiet.
pub fn is_retired(peer_id: &str) -> bool {
    resolve(peer_id) != peer_id
}

/// Install a supersession in the live map. Idempotent.
///
/// A peer never supersedes itself; an empty operand is not a supersession.
/// Both are rejected here rather than at the call sites, so a malformed
/// deployment document cannot install a self-loop that [`resolve`] then has to
/// defend against.
pub fn record(retired: &str, replacement: &str) {
    if retired.is_empty() || replacement.is_empty() || retired == replacement {
        return;
    }
    ACTIVE.with(|c| {
        c.borrow_mut().insert(retired.to_string(), replacement.to_string());
    });
}

/// Replace the whole live map (boot load, and the reset every test needs).
pub fn set_all(map: BTreeMap<String, String>) {
    ACTIVE.with(|c| *c.borrow_mut() = map);
}

/// The live map, for diagnostics and for Doctor.
pub fn snapshot() -> BTreeMap<String, String> {
    ACTIVE.with(|c| c.borrow().clone())
}

/// Tree path of the record naming `retired`'s replacement.
pub fn supersession_path(our_peer_id: &str, retired: &str) -> String {
    app_paths::peer_supersession_path(APP_ID, our_peer_id, retired)
}

/// Build the durable record. Stores only the replacement: *when* it happened is
/// the generation marker's job (deployment design P2), and duplicating it here
/// would create a second, quietly-diverging answer to the same question.
pub fn supersession_entity(replacement: &str) -> Entity {
    let data = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
        "superseded_by" => entity_ecf::text(replacement)
    });
    Entity::new(SUPERSESSION_TYPE, data).unwrap()
}

/// Decode a record, or `None` if it is not one / carries no replacement.
pub fn decode(entity: &Entity) -> Option<String> {
    let value: ciborium::Value = ciborium::from_reader(entity.data.as_slice()).ok()?;
    let who = value.as_map()?.iter().find_map(|(k, v)| match k.as_text() {
        Some("superseded_by") => v.as_text().map(str::to_string),
        _ => None,
    })?;
    if who.is_empty() {
        return None;
    }
    Some(who)
}

/// Load every durable record into the live map. Called once at boot, **before**
/// any surface hydrates its persisted navigation state.
///
/// Flat, one-level prefix, so the immediate-children semantics of
/// `tree_listing_async` are exactly right here (see the module note on why a
/// deeper sweep is not). A read failure leaves the map empty, which degrades to
/// today's behaviour rather than to a wrong answer.
pub async fn load(peers: &Peers, our_peer_id: &str) -> usize {
    let prefix = app_paths::peer_supersessions_prefix(APP_ID, our_peer_id);
    let entries = match peers.tree_listing_async(our_peer_id, &prefix).await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, "peer-supersession: registry listing failed — none loaded");
            return 0;
        }
    };
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for entry in entries {
        let Some(rest) = entry.path.strip_prefix(&prefix) else {
            continue;
        };
        let retired = rest.trim_start_matches('/');
        if retired.is_empty() || retired.contains('/') {
            continue;
        }
        let got = peers.get_entity_async(our_peer_id, &entry.path).await.ok().flatten();
        if let Some(replacement) = got.as_ref().and_then(decode) {
            if replacement != retired {
                map.insert(retired.to_string(), replacement);
            }
        }
    }
    let n = map.len();
    if n > 0 {
        tracing::info!(records = n, "peer-supersession: loaded retired-publisher records");
    }
    set_all(map);
    n
}

/// Persist a supersession and install it live, in that order.
///
/// Durability is the point: the adoption that discovers a re-key happens **once**
/// (the next boot sees a config that already agrees with the domain, so there is
/// no divergence left to detect). If the record were not written down, a window
/// still holding the retired peer would never be resolved on any later boot —
/// the repair would work exactly once, for whatever happened to be open.
pub async fn persist(peers: &Peers, our_peer_id: &str, retired: &str, replacement: &str) {
    record(retired, replacement);
    if retired.is_empty() || replacement.is_empty() || retired == replacement {
        return;
    }
    let path = supersession_path(our_peer_id, retired);
    match peers
        .put_and_wait(our_peer_id, &path, supersession_entity(replacement), 5_000)
        .await
    {
        Ok(()) => tracing::info!(
            retired = %retired,
            replacement = %replacement,
            "peer-supersession: recorded a retired publisher"
        ),
        Err(e) => tracing::error!(
            error = %e,
            retired = %retired,
            "peer-supersession: could not persist the record — the live map still has it, \
             but it will be lost on reload"
        ),
    }
}

/// Drop one record — from the live map *and* from durable storage.
///
/// The paired remove for [`persist`] (D9), and the thing whose absence was
/// finding F2. It is **awaited**, like its twin, because a fire-and-forget
/// removal that loses its race leaves the record on disk and resurrects it on
/// the next boot — which is the bug, not a fix for it. `Ok(false)` from the
/// store (nothing was there) is success: the caller's goal is the record's
/// absence, not the act of deleting.
///
/// The live map is updated first and synchronously, so [`resolve`] is correct
/// for the rest of this boot even if the durable removal fails; the failure is
/// logged loudly rather than swallowed, because a record that keeps coming back
/// is exactly the symptom nobody would otherwise be able to name.
pub async fn forget(peers: &Peers, our_peer_id: &str, retired: &str) -> bool {
    if retired.is_empty() {
        return false;
    }
    ACTIVE.with(|c| {
        c.borrow_mut().remove(retired);
    });
    let path = supersession_path(our_peer_id, retired);
    match peers.remove_and_wait(our_peer_id, &path, 5_000).await {
        Ok(_) => true,
        Err(e) => {
            tracing::error!(
                error = %e,
                retired = %retired,
                "peer-supersession: the record was dropped from the live map but NOT from \
                 durable storage — it will come back on the next boot"
            );
            false
        }
    }
}

/// Persist every succession the live deployment document **declares**, for any
/// hosted peer — `DESIGN-RESILIENCE…` §1.1f item 1's consumer half. Returns the
/// records this boot newly wrote.
///
/// This is the multi-peer half of *detect*. The home peer's re-key is inferred
/// from a single slot changing and keeps its own path in `boot_phase2`; every
/// other peer needs the deployer to say so, because a key leaving the `origins`
/// map as another arrives is ambiguous between a re-key and a tenant swap, and
/// guessing writes a supersession against a peer that is alive.
///
/// **Idempotent by comparison, not by write.** A document declaring a succession
/// re-declares it on every boot for as long as the deployer leaves it there, and
/// re-persisting an identical record each time would be a durable write per boot
/// for no change. Records already live and agreeing are skipped; a record whose
/// replacement has *moved* is re-persisted, because the document outranks what we
/// hold.
///
/// **Runs before [`revalidate`]**, like the home-peer adoption it sits beside and
/// for the same reason (module note, invariant 2): a succession the document
/// declares this boot has to be in the map before anything judges the map
/// against the document, or the check condemns the record it just asked for.
pub async fn adopt_declared(
    peers: &Peers,
    our_peer_id: &str,
    declared: &BTreeMap<String, String>,
) -> usize {
    let to_write = declared_to_adopt(&snapshot(), declared);
    for (retired, replacement) in &to_write {
        tracing::info!(
            retired = %retired,
            replacement = %replacement,
            "peer-supersession: the deployment DECLARES a succession — adopting it"
        );
        persist(peers, our_peer_id, retired, replacement).await;
    }
    to_write.len()
}

/// Which declared successions [`adopt_declared`] would write, given what is
/// already held. Pure, so every combination is gated by `make test` on both
/// arms rather than only through a boot — the `decide_home` / `ladder_step`
/// shape, and for the same reason: the only caller is an `async fn` reached
/// from `boot_phase2`.
fn declared_to_adopt(
    held: &BTreeMap<String, String>,
    declared: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    declared
        .iter()
        // `record` rejects these too; filtering here keeps the log and the
        // return count honest about what was actually adopted.
        .filter(|(retired, replacement)| {
            !retired.is_empty() && !replacement.is_empty() && retired != replacement
        })
        // Already held and agreeing. A document re-declares its succession on
        // every boot for as long as the deployer leaves it there, so without
        // this the mechanism is a durable write per boot forever.
        .filter(|(retired, replacement)| {
            held.get(retired.as_str()) != Some(replacement) // the document outranks what we hold
        })
        .map(|(r, p)| (r.clone(), p.clone()))
        .collect()
}

/// Re-check every record against the live deployment document and drop the ones
/// it contradicts. Returns the records dropped.
///
/// **Call this after the adoption path has run, and only with a document
/// actually obtained this boot** — both conditions are in the module note, and
/// getting either wrong turns the repair into a deletion of good records.
///
/// `home` is the home peer the live document names and `declared` is its
/// `superseded` map — together, every peer the document **affirms** as current.
/// A document that affirmed nobody declined to say anything, which is not a
/// disagreement: nothing is dropped. See [`stale_against_declared`] for why an
/// `origins` entry is deliberately *not* an affirmation.
pub async fn revalidate(
    peers: &Peers,
    our_peer_id: &str,
    home: &str,
    declared: &BTreeMap<String, String>,
) -> Vec<String> {
    let stale = stale_against_declared(&snapshot(), home, declared);
    if stale.is_empty() {
        return Vec::new();
    }
    for retired in &stale {
        tracing::warn!(
            retired = %retired,
            home = %home,
            "peer-supersession: DROPPING a record the domain contradicts — the deployment \
             document says this publisher is current, so the record was wrong or is spent"
        );
        forget(peers, our_peer_id, retired).await;
    }
    tracing::info!(
        dropped = stale.len(),
        home = %home,
        declared = declared.len(),
        "peer-supersession: revalidated against the live deployment document"
    );
    stale
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every test shares one thread-local, so each starts from empty.
    fn reset() {
        set_all(BTreeMap::new());
    }

    #[test]
    fn an_unknown_peer_resolves_to_itself() {
        reset();
        assert_eq!(resolve("2KAnything"), "2KAnything");
        assert!(!is_retired("2KAnything"));
        // The empty string is not special-cased anywhere; it must still be total.
        assert_eq!(resolve(""), "");
    }

    #[test]
    fn a_recorded_peer_resolves_to_its_replacement() {
        reset();
        record("2KOld", "2KNew");
        assert_eq!(resolve("2KOld"), "2KNew");
        assert!(is_retired("2KOld"));
        // The replacement itself is NOT retired — the arrow points one way.
        assert!(!is_retired("2KNew"));
    }

    /// Two re-keys in a row. A browser that missed both must land on the
    /// current peer, not the intermediate corpse.
    #[test]
    fn chains_resolve_to_the_end() {
        reset();
        record("2KA", "2KB");
        record("2KB", "2KC");
        assert_eq!(resolve("2KA"), "2KC");
        assert_eq!(resolve("2KB"), "2KC");
    }

    /// A cycle is corruption, and the only acceptable behaviour is to stop.
    /// This test exists because the alternative is a hang at boot.
    #[test]
    fn a_cycle_terminates_instead_of_looping() {
        reset();
        record("2KA", "2KB");
        record("2KB", "2KA");
        let out = resolve("2KA");
        assert!(out == "2KA" || out == "2KB", "cycle must terminate, got {out:?}");
    }

    /// A self-supersession is silently dropped rather than stored — otherwise
    /// every `resolve` pays a cycle-guard for a record that means nothing.
    #[test]
    fn a_self_supersession_is_not_recorded() {
        reset();
        record("2KA", "2KA");
        record("", "2KB");
        record("2KC", "");
        assert!(snapshot().is_empty());
        assert_eq!(resolve("2KA"), "2KA");
    }

    #[test]
    fn the_record_round_trips_through_its_codec() {
        let e = supersession_entity("2KNew");
        assert_eq!(decode(&e).as_deref(), Some("2KNew"));
        // An empty replacement is not a supersession — it decodes to None so a
        // truncated write cannot install a peer-id of "".
        assert!(decode(&supersession_entity("")).is_none());
    }

    /// Build a map the way boot would have, for the revalidation tests.
    fn map_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    /// The ordinary case, and the one that must never fire: the domain still
    /// publishes what the records point at, so nothing is stale.
    #[test]
    fn a_record_the_domain_agrees_with_is_kept() {
        let m = map_of(&[("2KOld", "2KNew")]);
        assert!(stale_against(&m, "2KNew").is_empty());
    }

    /// Two re-keys deep. Both records lead to the current publisher, so both
    /// stand — a browser that slept through both still resolves correctly.
    #[test]
    fn a_chain_that_ends_at_the_publisher_is_kept() {
        let m = map_of(&[("2KA", "2KB"), ("2KB", "2KC")]);
        assert!(stale_against(&m, "2KC").is_empty());
    }

    /// **The F2 case.** A transient bad document said `2KBad`, so `A→Bad` was
    /// adopted; the origin is fixed and the doc says `A` again, so the adoption
    /// path has just written `Bad→A`. The false record must go and the true one
    /// must stay — and getting this backwards is what a one-pass predicate does.
    #[test]
    fn a_corrected_document_drops_the_false_record_and_keeps_the_true_one() {
        let m = map_of(&[("2KA", "2KBad"), ("2KBad", "2KA")]);
        let stale = stale_against(&m, "2KA");
        assert_eq!(stale, vec!["2KA".to_string()], "only the false record is stale");

        // And the surviving map must actually repair a window still on the bad
        // peer — the point of keeping it.
        let mut kept = m.clone();
        for k in &stale {
            kept.remove(k);
        }
        assert_eq!(resolve_in(&kept, "2KBad"), "2KA");
        assert_eq!(resolve_in(&kept, "2KA"), "2KA");
    }

    /// A deployer rolls the domain back to an earlier publisher. The record
    /// saying that publisher is retired is now false by the domain's own word.
    #[test]
    fn a_rollback_to_an_earlier_publisher_retires_the_record_against_it() {
        let m = map_of(&[("2KA", "2KB")]);
        assert_eq!(stale_against(&m, "2KA"), vec!["2KA".to_string()]);
    }

    /// An orphan — a chain leading to a peer the domain has abandoned. Inert
    /// today, but it is exactly the shape a stale record has, and keeping it
    /// means `resolve` can still move a peer onto a dead one.
    #[test]
    fn a_chain_ending_anywhere_else_is_stale() {
        let m = map_of(&[("2KA", "2KGone")]);
        assert_eq!(stale_against(&m, "2KCurrent"), vec!["2KA".to_string()]);
    }

    /// Corruption. A cycle terminates at its own entry point, so it can never
    /// end at the publisher and every record in it is dropped.
    #[test]
    fn a_cycle_is_entirely_stale() {
        let m = map_of(&[("2KA", "2KB"), ("2KB", "2KA")]);
        let mut stale = stale_against(&m, "2KC");
        stale.sort();
        assert_eq!(stale, vec!["2KA".to_string(), "2KB".to_string()]);
    }

    /// **Absence of evidence.** A document that declines to name a home peer
    /// must drop nothing — otherwise a truncated or half-written
    /// `/entity-deployment.json` wipes a valid repair, which is a worse failure
    /// than the one being fixed. D23's deadline makes this case more common,
    /// not less: a slow origin now yields no document rather than hanging.
    #[test]
    fn an_empty_publisher_drops_nothing() {
        let m = map_of(&[("2KA", "2KB"), ("2KB", "2KC")]);
        assert!(stale_against(&m, "").is_empty());
        assert!(stale_against(&BTreeMap::new(), "2KC").is_empty());
    }

    /// **The single-publisher assumption, made visible.**
    ///
    /// `stale_against` takes ONE publisher — the home peer — and rule 2 condemns
    /// every chain that ends anywhere else. A domain that hosts several peers
    /// (`DeploymentConfig::origins` is a map, and `boot_load` logs
    /// `hosted_peer_origins` as the multi-tenant signal) therefore has exactly one
    /// peer whose supersession records can survive a boot.
    ///
    /// This is not a live defect today, because the adoption path is the only
    /// writer and it only ever compares `home_site.peer_id` — so a record keyed on
    /// a non-home peer is never created in the first place. It is pinned here
    /// because it is the trap waiting for whoever fixes that: recording a
    /// non-home re-key WITHOUT widening this predicate produces a record that is
    /// written on one boot and deleted on the next, forever.
    #[test]
    fn a_record_for_a_peer_that_is_not_the_home_publisher_is_dropped() {
        // A document that names only its home peer says nothing about anyone
        // else, so a record about another peer orphans and is dropped. This was
        // the whole of the old behaviour and it is still correct *for a
        // single-publisher document*; what changed is that the document can now
        // say more (see the `hosted` tests below), not that this case moved.
        let m = map_of(&[("2KOther", "2KOtherNew")]);
        assert_eq!(
            stale_against(&m, "2KHome"),
            vec!["2KOther".to_string()],
            "a document naming one publisher cannot vouch for a chain ending elsewhere"
        );
    }

    /// **The re-key document's own shape, and the regression that widening the
    /// predicate introduced on 2026-09-07 before this test existed.**
    ///
    /// After a re-key the document names the NEW peer as home and — deliberately,
    /// since `HomeClaim::Takes` preserves sibling `origins` entries so old URLs
    /// keep resolving — **still lists the retired one**. A predicate that treated
    /// every `origins` key as *current* therefore contradicted the record the
    /// inferred home adoption had just written, and dropped it on the same boot.
    /// Five e2e gates red, all of them re-key gates.
    ///
    /// The lesson generalized into the predicate: **currency is what the document
    /// AFFIRMS — its home peer and the replacements it declares — never merely
    /// what it routes to.** An `origins` entry says *reachable here*, which is
    /// exactly what a retired peer stays.
    #[test]
    fn the_retired_peer_still_being_routable_does_not_contradict_the_record() {
        let held = map_of(&[("2KOld", "2KNew")]);
        assert!(
            stale_against_declared(&held, "2KNew", &BTreeMap::new()).is_empty(),
            "the document names 2KNew as home and still routes to 2KOld — that is the \
             ordinary re-key document, not a contradiction"
        );
    }

    /// **The §1.1f defect, as a test.** A domain hosts two publishers; the
    /// non-home one re-keys and the document declares it. The record is correct
    /// and the document agrees with it — and the single-publisher predicate
    /// condemned it anyway, so it was written on one boot and deleted on the
    /// next, forever.
    #[test]
    fn a_declared_succession_for_a_non_home_peer_survives_revalidation() {
        let held = map_of(&[("2KOther", "2KOtherNew")]);
        let declared = map_of(&[("2KOther", "2KOtherNew")]);
        assert!(
            stale_against_declared(&held, "2KHome", &declared).is_empty(),
            "a succession the document itself declares must survive the document"
        );
    }

    /// The other direction, so the widening cannot be read as blanket tolerance:
    /// with nothing declared, a record claiming the **home** peer is dead is
    /// still contradicted and still dropped. F2's rule, widened rather than
    /// weakened — and the escape a deployer uses to repair a mistaken record.
    #[test]
    fn a_record_against_the_affirmed_home_is_still_dropped() {
        let held = map_of(&[("2KA", "2KB")]);
        assert_eq!(
            stale_against_declared(&held, "2KA", &BTreeMap::new()),
            vec!["2KA".to_string()],
            "the domain says 2KA is publishing; a record calling it retired is wrong"
        );
    }

    /// **F2's escape for a DECLARED succession.** A deployer who declares one by
    /// mistake repairs it by withdrawing the declaration: the replacement stops
    /// being affirmed, the chain orphans, and rule 2 drops it on the next boot.
    /// E1 — a reload repairs it — which is what makes the whole mechanism safe
    /// to ship.
    #[test]
    fn withdrawing_a_declaration_drops_the_record_it_created() {
        let held = map_of(&[("2KA", "2KB")]);
        assert!(
            stale_against_declared(&held, "2KHome", &map_of(&[("2KA", "2KB")])).is_empty(),
            "while declared, it stands"
        );
        assert_eq!(
            stale_against_declared(&held, "2KHome", &BTreeMap::new()),
            vec!["2KA".to_string()],
            "withdrawn, it must not survive — or a bad declaration is permanent (F2)"
        );
    }

    /// A chain may end at a peer the document declares as a **replacement**, not
    /// only at the home peer. The narrower rule is what made this
    /// single-publisher. Note the intermediate hop is subtracted from `current`
    /// by being a declared *key*, so it is not itself mistaken for a live
    /// publisher.
    #[test]
    fn a_chain_ending_at_a_declared_replacement_survives() {
        let held = map_of(&[("2KA", "2KB"), ("2KB", "2KC")]);
        let declared = map_of(&[("2KA", "2KB"), ("2KB", "2KC")]);
        assert!(
            stale_against_declared(&held, "2KHome", &declared).is_empty(),
            "2KC is the declared replacement, so a chain terminating there is not an orphan"
        );
    }

    /// **A document may affirm through its declaration alone.** Not every
    /// document names a home — `a_deployment_that_moves_its_registry_pin`'s
    /// declares only a pin, deliberately — so a succession must stand on a
    /// document that says nothing about a home peer. Under the old
    /// single-publisher predicate an empty `publisher` short-circuited to
    /// "drop nothing", which was right by accident; here it is the rule.
    #[test]
    fn a_succession_stands_on_a_document_that_names_no_home() {
        let held = map_of(&[("2KA", "2KB")]);
        assert!(
            stale_against_declared(&held, "", &map_of(&[("2KA", "2KB")])).is_empty(),
            "the declaration alone affirms 2KB, so the chain is not an orphan"
        );
    }

    /// Absence of evidence, on the widened predicate. A document affirming
    /// nobody drops nothing rather than everything — the safe direction, and the
    /// case D23's deadline makes more common, not less.
    #[test]
    fn a_document_that_affirms_nobody_drops_nothing() {
        let held = map_of(&[("2KA", "2KB")]);
        assert!(stale_against_declared(&held, "", &BTreeMap::new()).is_empty());
        // A document whose only affirmation is a peer it also retires affirms
        // nobody — a self-cancelling declaration must not become a mass delete.
        assert!(stale_against_declared(&held, "", &map_of(&[("2KB", "2KB")])).is_empty());
    }

    #[test]
    fn a_declared_succession_is_adopted_once_and_not_rewritten_every_boot() {
        let declared = map_of(&[("2KA", "2KB")]);
        assert_eq!(
            declared_to_adopt(&BTreeMap::new(), &declared),
            vec![("2KA".to_string(), "2KB".to_string())],
            "a succession we do not hold is adopted"
        );
        assert!(
            declared_to_adopt(&map_of(&[("2KA", "2KB")]), &declared).is_empty(),
            "re-declaring what we already hold must not be a durable write per boot"
        );
        assert_eq!(
            declared_to_adopt(&map_of(&[("2KA", "2KStale")]), &declared),
            vec![("2KA".to_string(), "2KB".to_string())],
            "the document outranks a replacement we hold that has since moved"
        );
    }

    /// The same shape one step on, and **the pair to
    /// `a_chain_ending_at_any_hosted_peer_survives`**: an internally consistent
    /// chain ending somewhere the *document did not name* is still an orphan.
    /// The distinction the multi-publisher fix crossed is between "the document
    /// named only its home" (this) and "the document named the chain's end"
    /// (that) — not between home and non-home peers.
    #[test]
    fn a_non_home_chain_is_dropped_however_well_formed_it_is() {
        let m = map_of(&[("2KA", "2KB"), ("2KB", "2KOtherNew")]);
        let mut stale = stale_against(&m, "2KHome");
        stale.sort();
        assert_eq!(stale, vec!["2KA".to_string(), "2KB".to_string()]);
    }

    /// A chain longer than the bound must still terminate and return a peer,
    /// never panic or spin.
    #[test]
    fn an_overlong_chain_is_bounded() {
        reset();
        for i in 0..(MAX_CHAIN + 4) {
            record(&format!("2K{i}"), &format!("2K{}", i + 1));
        }
        let out = resolve("2K0");
        assert!(out.starts_with("2K"), "bounded walk still returns a peer, got {out:?}");
    }
}

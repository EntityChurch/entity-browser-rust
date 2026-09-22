//! What this session tried to bring current from a remote source, and how it
//! went — the recording seam Entity Doctor's checks 2 and 3 read.
//!
//! # Why this exists
//!
//! Incident B (the half-loaded launcher,
//! `DESIGN-RESILIENCE-RECONCILIATION-AND-ENTITY-DOCTOR.md` §2): two app
//! catalogs fetch independently, one exhausts its bounded retry ladder on a
//! weak signal, **nothing is written, the other set renders, and the grid looks
//! complete.** The only report was `tracing::warn!` — *"reopen the window to
//! retry"* — in a console nobody has on a phone.
//!
//! The design names the shared disease with incident A: *a belief learned once
//! from a remote source, never re-checked, internally consistent, looking
//! healthy, and reported to nobody* (§2.1). **A failed refresh is a fact about
//! the belief, not a no-op** (§3). This module is where that fact lands so a
//! surface can state it.
//!
//! # Deliberately modest — this is not telemetry
//!
//! Same scope discipline as [`crate::diagnostics`]: **session-scoped and
//! in-memory**, a bounded map keyed by `(peer, what)` with the latest outcome
//! winning. No new persisted path family, so no writer/reader-at-boot/GC story
//! to owe (D9), and no durable record of a transient fact.
//!
//! **The cost of that choice is stated rather than hidden, because it is
//! exactly the kind of gap that reads as a clean bill of health:** a reload
//! empties the ledger, so *"nothing has gone wrong"* and *"nothing has been
//! attempted yet"* are different facts and [`Snapshot::attempted`] keeps them
//! apart. A surface that renders an empty ledger as "healthy" would be the
//! `warn!` bug again with better typography.

use std::cell::RefCell;
use std::collections::BTreeMap;

/// How an attempt to bring one artifact current turned out.
///
/// The split between [`Withheld`](RefreshOutcome::Withheld) and
/// [`Unreachable`](RefreshOutcome::Unreachable) is load-bearing and is not
/// ours to invent — it is [`PollError::NotFound`](crate::content_site::http_poll::PollError)'s
/// own distinction: *the origin answered, and the answer was "that is not
/// here"* versus anything that did not get an answer. **Collapsing them is how
/// "withheld" and "unreachable" arrive as the same value**, and Doctor's check 2
/// is built entirely on telling them apart: an origin that answers `404` for
/// everything under one peer is evidence about that peer, and a dropped
/// connection is evidence about the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// The local copy is the current bytes — fetched, or already current.
    /// Recorded on success too, deliberately: without it an empty ledger and a
    /// healthy one are indistinguishable.
    Current,
    /// The origin **answered**, and said the artifact is not there.
    Withheld,
    /// Nothing was heard, or what came back could not be used.
    Unreachable(String),
}

impl RefreshOutcome {
    /// Whether this outcome means the surface reading that artifact is showing
    /// everything it should be.
    pub fn is_current(&self) -> bool {
        matches!(self, Self::Current)
    }
}

/// One artifact this session tried to keep current.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshRecord {
    /// The peer whose origin was asked.
    pub peer_id: String,
    /// What was being refreshed, in terms a person can read — this reaches the
    /// user (S6), so it is a set name, not an artifact path.
    pub what: String,
    pub outcome: RefreshOutcome,
}

/// The ledger as a reader sees it.
///
/// `attempted` is the anti-vacuity field: `records` being empty of failures
/// means *nothing failed*, which is only good news if something was tried.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub records: Vec<RefreshRecord>,
    /// Total attempts recorded this session, including ones overwritten by a
    /// later attempt at the same key.
    pub attempted: usize,
    /// Set when the cap was hit and an attempt could not be recorded. A reader
    /// that reports "all clear" off a truncated ledger is reporting the cap.
    pub truncated: bool,
}

/// A ledger that stays small on its own. Keyed by `(peer, what)` so a retry
/// replaces its predecessor — which is also the correct semantics: a set that
/// failed and then succeeded is current, and the ledger should say so.
const MAX_KEYS: usize = 256;

thread_local! {
    static LEDGER: RefCell<BTreeMap<(String, String), RefreshOutcome>> =
        const { RefCell::new(BTreeMap::new()) };
    static ATTEMPTED: RefCell<usize> = const { RefCell::new(0) };
    static TRUNCATED: RefCell<bool> = const { RefCell::new(false) };
}

/// Record the outcome of one refresh attempt. Cheap, infallible, and safe to
/// call from a spawned task — the call sites are fetch completions, not the
/// render loop.
pub fn record(peer_id: &str, what: &str, outcome: RefreshOutcome) {
    ATTEMPTED.with(|a| *a.borrow_mut() += 1);
    let key = (peer_id.to_string(), what.to_string());
    LEDGER.with(|l| {
        let mut l = l.borrow_mut();
        // An existing key is always updatable — the cap bounds distinct
        // artifacts, and refusing an UPDATE at the cap would freeze a stale
        // failure in place and report a repaired set as broken.
        if l.len() >= MAX_KEYS && !l.contains_key(&key) {
            TRUNCATED.with(|t| *t.borrow_mut() = true);
            return;
        }
        l.insert(key, outcome);
    });
}

/// Read the ledger. Sorted by `(peer, what)` because it comes from a
/// `BTreeMap` — a report whose row order changes between renders reads as
/// churn.
pub fn snapshot() -> Snapshot {
    Snapshot {
        records: LEDGER.with(|l| {
            l.borrow()
                .iter()
                .map(|((peer_id, what), outcome)| RefreshRecord {
                    peer_id: peer_id.clone(),
                    what: what.clone(),
                    outcome: outcome.clone(),
                })
                .collect()
        }),
        attempted: ATTEMPTED.with(|a| *a.borrow()),
        truncated: TRUNCATED.with(|t| *t.borrow()),
    }
}

/// Drop everything — tests only. Production has no clear path on purpose: the
/// ledger's whole value is that it outlives the window that produced the
/// failure, and a reload already empties it.
#[cfg(test)]
pub fn reset_for_test() {
    LEDGER.with(|l| l.borrow_mut().clear());
    ATTEMPTED.with(|a| *a.borrow_mut() = 0);
    TRUNCATED.with(|t| *t.borrow_mut() = false);
    RETRY_GENERATION.with(|g| g.set(0));
}

// ── The retry signal — how a remedy reaches the mechanism ────────────────────
//
// Incident B's repair is *try again*, and today the only way to get it is to
// close and re-open the window: the "have I asked this session" guard is a set
// owned by the window instance, so a new instance retries and a live one never
// does. A user staring at a launcher that is missing half its contents has no
// way to ask for that from the surface that told them.
//
// A **generation counter**, not a queue or a callback: the surface that reports
// the problem does not know which windows exist, and should not. It bumps a
// number; any window holding a fetch guard compares it against what it last
// saw and drops the guard. That keeps the remedy a one-line, allocation-free
// signal with no registry of listeners to keep in step (AP44 — no call site has
// to be remembered), and it is correct whether zero, one or several launchers
// are open.
//
// **The honest bound, which the remedy's own copy states:** a window that is
// not open cannot fetch. Re-opening one already retries — that is today's
// workaround — so this exists for the case that matters, where the user is
// looking at the incomplete grid right now.

thread_local! {
    static RETRY_GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Ask every surface holding a once-per-window fetch guard to drop it. Returns
/// the new generation, so a caller can report what it did rather than assert it.
pub fn request_retry() -> u64 {
    RETRY_GENERATION.with(|g| {
        g.set(g.get() + 1);
        g.get()
    })
}

/// The current retry generation. A holder stores what it last acted on and
/// re-arms when this moves.
pub fn retry_generation() -> u64 {
    RETRY_GENERATION.with(|g| g.get())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_later_attempt_at_the_same_key_replaces_its_predecessor() {
        reset_for_test();
        record("peerA", "games", RefreshOutcome::Unreachable("timeout".into()));
        record("peerA", "games", RefreshOutcome::Current);
        let s = snapshot();
        assert_eq!(s.records.len(), 1, "the retry must replace, not accumulate");
        assert_eq!(s.records[0].outcome, RefreshOutcome::Current);
        // Both attempts are still counted: the map holds the current state, the
        // counter holds the history, and a reader needs both to tell "nothing
        // failed" from "nothing was tried".
        assert_eq!(s.attempted, 2);
    }

    #[test]
    fn success_is_recorded_so_an_empty_ledger_is_distinguishable_from_a_healthy_one() {
        reset_for_test();
        assert_eq!(snapshot().attempted, 0, "a fresh session has attempted nothing");
        record("peerA", "apps", RefreshOutcome::Current);
        let s = snapshot();
        assert_eq!(s.attempted, 1);
        assert!(s.records.iter().all(|r| r.outcome.is_current()));
    }

    #[test]
    fn a_retry_request_moves_the_generation_so_a_holder_can_notice() {
        reset_for_test();
        let before = retry_generation();
        let after = request_retry();
        assert_eq!(after, before + 1);
        assert_eq!(retry_generation(), after);
        // Two requests are two distinct generations: a user pressing Retry
        // twice must re-arm twice, not be de-duplicated into one.
        assert_eq!(request_retry(), after + 1);
    }

    /// The cap must not be able to freeze a stale failure. If an update at an
    /// existing key were refused once the map is full, a set that failed and
    /// then recovered would be reported broken for the rest of the session —
    /// the diagnostic itself becoming the stale belief it exists to catch.
    #[test]
    fn the_cap_bounds_new_keys_but_never_blocks_an_update() {
        reset_for_test();
        for i in 0..MAX_KEYS {
            record("peerA", &format!("set{i}"), RefreshOutcome::Withheld);
        }
        assert!(!snapshot().truncated, "the cap was not reached yet");

        record("peerA", "one-too-many", RefreshOutcome::Withheld);
        let s = snapshot();
        assert!(s.truncated, "a new key past the cap must set the truncation flag");
        assert_eq!(s.records.len(), MAX_KEYS);

        // …and an existing key still updates.
        record("peerA", "set0", RefreshOutcome::Current);
        let s = snapshot();
        assert_eq!(
            s.records.iter().find(|r| r.what == "set0").map(|r| &r.outcome),
            Some(&RefreshOutcome::Current),
            "an update to an existing key must land even at the cap"
        );
    }
}

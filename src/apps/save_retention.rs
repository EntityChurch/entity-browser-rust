//! Bounded save-state retention — the app-owned content-store cleanup policy.
//!
//! Save-state at `/{peer}/app/entity-browser/apps/{set}/state/{id}` is written
//! on (debounced) updates while a game/app runs. The content store is
//! **append-only**: each distinct save value adds a blob that a later overwrite
//! never reclaims. Left
//! alone, a long session leaves hundreds of orphaned blobs even though only the
//! latest is ever read.
//!
//! This is **the documented pattern** for an app maintaining its own slice of
//! the store: keep a small ring of the last `cap` content hashes we've written;
//! when a new save pushes one out of the window, that evicted hash is eligible
//! for reclaim via the binding-safe `WriterHandle::content_remove`
//! (`entity_sdk::content_remove_if_unbound`). Because save-state is single
//! -version and app-private, "no live path binds it" is sufficient proof it's
//! safe to drop — the general refs/version reachability case stays the kernel's
//! job (GUIDE-GC §2/§3).
//!
//! The ring is in-memory (session-scoped). Seed it from the live save's hash at
//! window open ([`SaveRing::seeded`]) so the first reclaim also drops the prior
//! session's superseded blob; the residual is at most `cap-1` small blobs per
//! app per reload, which kernel GC eventually sweeps.

use std::collections::VecDeque;

use entity_hash::Hash;

/// How many recent save versions to retain before reclaiming older blobs.
/// A small margin (not a restorable buffer — only the latest is path-bound):
/// it avoids reclaim-then-immediately-rewrite thrash if a game re-reaches a
/// just-evicted state.
pub const DEFAULT_RETAIN: usize = 5;

/// In-memory ring of the most-recently-written save-state content hashes for
/// one app, newest at the front, capped at `cap`.
#[derive(Debug)]
pub struct SaveRing {
    hashes: VecDeque<Hash>,
    cap: usize,
}

impl SaveRing {
    /// Empty ring retaining `cap` versions (clamped to ≥1).
    pub fn new(cap: usize) -> Self {
        Self {
            hashes: VecDeque::new(),
            cap: cap.max(1),
        }
    }

    /// Ring seeded with the current live save hash (read at window open), so
    /// the first [`record`](Self::record) that overflows can reclaim the prior
    /// session's now-superseded blob rather than leaking it.
    pub fn seeded(cap: usize, live: Option<Hash>) -> Self {
        let mut ring = Self::new(cap);
        if let Some(h) = live {
            ring.hashes.push_front(h);
        }
        ring
    }

    /// Record a newly-written save hash. Returns the hash that fell out of the
    /// retention window and is now eligible for reclaim, or `None` when:
    /// - `new` duplicates the current head (identical state → store deduped,
    ///   nothing written), or
    /// - the window didn't overflow, or
    /// - the evicted hash is still retained elsewhere in the window (a value
    ///   re-reached within the window — must NOT be reclaimed).
    pub fn record(&mut self, new: Hash) -> Option<Hash> {
        // Identical to the latest write — the content store deduped it; don't
        // grow the ring or reclaim anything.
        if self.hashes.front() == Some(&new) {
            return None;
        }
        self.hashes.push_front(new);
        if self.hashes.len() <= self.cap {
            return None;
        }
        let evicted = self.hashes.pop_back()?;
        // A value still present elsewhere in the window is a live retention
        // target — reclaiming its blob now would drop a version we still mean
        // to keep.
        if self.hashes.contains(&evicted) {
            None
        } else {
            Some(evicted)
        }
    }

    /// Whether `new` would be a no-op write (same as the latest recorded
    /// hash) — lets the caller skip a redundant `put`.
    pub fn is_head(&self, new: &Hash) -> bool {
        self.hashes.front() == Some(new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(seed: &str) -> Hash {
        Hash::compute("t", seed.as_bytes())
    }

    #[test]
    fn no_reclaim_until_window_overflows() {
        let mut r = SaveRing::new(3);
        assert_eq!(r.record(h("1")), None);
        assert_eq!(r.record(h("2")), None);
        assert_eq!(r.record(h("3")), None);
        // 4th push evicts the oldest (h1).
        assert_eq!(r.record(h("4")), Some(h("1")));
        assert_eq!(r.record(h("5")), Some(h("2")));
    }

    #[test]
    fn head_duplicate_is_a_noop() {
        let mut r = SaveRing::new(3);
        assert_eq!(r.record(h("a")), None);
        assert!(r.is_head(&h("a")));
        // Re-recording the same head neither grows the ring nor reclaims.
        assert_eq!(r.record(h("a")), None);
        // And a genuinely new value after that still doesn't overflow cap=3.
        assert_eq!(r.record(h("b")), None);
        assert_eq!(r.record(h("c")), None);
        assert_eq!(r.record(h("d")), Some(h("a")));
    }

    #[test]
    fn evicted_hash_still_in_window_is_not_reclaimed() {
        // cap 2; record a, b, a → window [a, b, a], overflow evicts back `a`,
        // but `a` is still at the front → must NOT reclaim it.
        let mut r = SaveRing::new(2);
        assert_eq!(r.record(h("a")), None);
        assert_eq!(r.record(h("b")), None);
        assert_eq!(r.record(h("a")), None, "evicted `a` still retained at front");
    }

    #[test]
    fn seeded_ring_reclaims_the_prior_live_blob() {
        // Seeded with the prior session's live hash; with cap 2, the third
        // distinct write pushes the seed out → reclaim it.
        let mut r = SaveRing::seeded(2, Some(h("live")));
        assert_eq!(r.record(h("n1")), None);
        assert_eq!(r.record(h("n2")), Some(h("live")));
    }
}

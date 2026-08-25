//! **Long-lived [`SignedSession`]s, keyed by publisher.**
//!
//! A `SignedSession` is not a convenience wrapper — it is where the **`seq`
//! floor** lives, and the floor is only meaningful for as long as the session
//! is. Build a client per fetch and a hostile origin can roll a site back *page
//! by page* (`seq 1` for one page, `seq 0` for the next) while every signature
//! and every hash still verifies, because both trees really were published by
//! that key. Nothing else in the chain catches it (see `signed_fetch`'s docs).
//!
//! So sessions have to outlive a single call, and on the browser arm there is no
//! natural owner: the shell verb, the site window and a future name-bar are three
//! callers that must share one floor per publisher, or the floor is per-surface
//! and the rollback comes back through whichever surface is newest.
//!
//! This is that owner. One process-wide map, `peer_id → session`, so every
//! consumer of the same publisher shares a floor and a content cache. Same shape
//! as `reach_keeper::global()`, and for the same reason: the thing being tracked
//! is a property of the *peer*, not of the window that happened to ask.
//!
//! **Rc + thread_local, deliberately.** The browser arm is single-threaded and a
//! `SignedSession` is `!Send`; making this `Arc<Mutex<…>>` would buy nothing and
//! would not compile against the `!Send` futures it hands out.
//!
//! **The origin is not part of the key, and that is the security half.** A
//! publisher is pinned by peer-id; the origin is only where we fetch it from.
//! Keying on `(peer_id, origin)` would hand a hostile mirror a fresh floor by
//! serving the same tree from a second URL — the rollback we just closed,
//! wearing a hostname.
//!
//! **Known limit, deliberately not fixed here: the first origin wins.** A
//! publisher that genuinely moves needs the session re-pointed, and
//! `SignedSession` bakes the origin into its `PinnedPublisher` (used by
//! `manifest_url`, the signature fetch and `content_url`). Threading a mutable
//! origin through it is a refactor of the trust chain, and a mirror move is not
//! a case any shipped surface exercises yet — so it is recorded rather than
//! guessed at. `reset()` is the only escape and it lowers the floor to zero,
//! which is why it is test-facing.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::signed_fetch::{PinnedPublisher, SignedSession};

thread_local! {
    static SESSIONS: RefCell<HashMap<String, Rc<SignedSession>>> = RefCell::new(HashMap::new());
}

/// The session for `peer_id`, served from `origin`.
///
/// Returns `None` when `peer_id` is not a canonical-form peer-id that carries
/// its own public key — the SHA-256 legacy form genuinely needs an out-of-band
/// key and there is nowhere here to get one.
///
/// A second call with a *different* origin returns the **existing** session,
/// origin and all. That is the load-bearing half — the floor must survive
/// someone serving the same tree from a second URL — and its cost is the
/// first-origin-wins limit in the module docs.
pub fn session_for(peer_id: &str, origin: &str) -> Option<Rc<SignedSession>> {
    with_session(peer_id, || PinnedPublisher::from_peer_id(origin, peer_id))
}

/// The session for `peer_id` at the layout **it advertised** — the path a
/// binding-resolved publisher takes.
///
/// Same first-wins rule, and it now carries a second consequence worth naming:
/// a publisher first met at a *typed origin* (convention layout) keeps that
/// layout even if a later binding advertises a real one. The floor is what the
/// cache exists to protect, so the session is not rebuilt — but it means the
/// order in which a publisher is first met decides whether we consume it by
/// convention or by advertisement. Recorded, not designed around; the escape is
/// [`reset`], which lowers the floor.
pub fn session_for_layout(
    peer_id: &str,
    layout: super::publish_layout::PublishLayout,
) -> Option<Rc<SignedSession>> {
    with_session(peer_id, || PinnedPublisher::with_layout(peer_id, layout))
}

fn with_session(
    peer_id: &str,
    build: impl FnOnce() -> Option<PinnedPublisher>,
) -> Option<Rc<SignedSession>> {
    SESSIONS.with(|s| {
        let mut map = s.borrow_mut();
        if let Some(existing) = map.get(peer_id) {
            // First origin wins — see the module docs. The floor matters more
            // than the mirror, and silently minting a second session is the bug.
            return Some(Rc::clone(existing));
        }
        let session = Rc::new(SignedSession::new(build()?));
        map.insert(peer_id.to_string(), Rc::clone(&session));
        Some(session)
    })
}

/// Drop every session. Only for tests and an explicit "forget what I've seen" —
/// note that forgetting a session **lowers the rollback floor to zero**, which is
/// why there is no user-facing affordance for it.
pub fn reset() {
    SESSIONS.with(|s| s.borrow_mut().clear());
}

/// How many publishers this tab currently holds a floor for.
pub fn len() -> usize {
    SESSIONS.with(|s| s.borrow().len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(seed: u8) -> String {
        entity_crypto::Keypair::from_seed([seed; 32]).peer_id().as_str().to_string()
    }

    /// Two callers asking for the same publisher get the **same** session, which
    /// is the whole point: a shared `seq` floor. Two different publishers do not.
    #[test]
    fn one_session_per_publisher_shared_across_callers() {
        reset();
        let a = pid(0x11);
        let b = pid(0x12);
        let s1 = session_for(&a, "https://a.example").expect("canonical peer-id pins");
        let s2 = session_for(&a, "https://a.example").expect("same publisher");
        assert!(Rc::ptr_eq(&s1, &s2), "the same publisher must share one session");

        let s3 = session_for(&b, "https://b.example").expect("other publisher");
        assert!(!Rc::ptr_eq(&s1, &s3));
        assert_eq!(len(), 2);
    }

    /// **The rollback hole this module exists to close.** Serving the same tree
    /// from a second origin must NOT mint a fresh session — that would hand back
    /// a `seq` floor of zero, which is exactly the per-call-client rollback with
    /// a hostname in front of it.
    ///
    /// Also pins the cost of that choice: the returned session still points at
    /// the FIRST origin (the module's known limit), so this test is what fails
    /// if someone later adds mirror-following without adding a floor to carry.
    #[test]
    fn a_second_origin_for_the_same_publisher_does_not_mint_a_second_floor() {
        reset();
        let a = pid(0x13);
        let first = session_for(&a, "https://a.example").unwrap();
        let second = session_for(&a, "https://mirror.example").unwrap();
        assert!(Rc::ptr_eq(&first, &second), "a mirror must not mint a new floor");
        assert_eq!(len(), 1, "a mirror is the same publisher, not a second one");
        assert_eq!(second.pin().origin, "https://a.example", "first origin wins — known limit");
    }

    /// A string that is not a canonical-form peer-id carries no public key, so
    /// there is nothing to pin with. Refused rather than guessed at — and it must
    /// leave no entry behind, or a failed pin would still occupy the map.
    ///
    /// (The legacy SHA-256 form would be the other case, but `peer_id_with_hash_type`
    /// now refuses to construct one for Ed25519 — *"SHA-256-form is legacy-decode-only …
    /// canonical form is HASH_TYPE_IDENTITY (Amendment 3)"* — so it cannot be built here.
    /// `from_peer_id` still returns `None` for one arriving off the wire.)
    #[test]
    fn a_peer_id_that_carries_no_key_cannot_be_pinned() {
        reset();
        assert!(session_for("not-a-peer-id", "https://x.example").is_none());
        assert!(session_for("", "https://x.example").is_none());
        assert_eq!(len(), 0, "a refused pin must not occupy the map");
    }
}

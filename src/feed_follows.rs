//! **Who this profile follows** — the durable half of the Feed surface.
//!
//! `APP-CONVENTION-FEED` §2.4's `app/feed/follow` is *"the reader's private
//! data"*: nothing publishes it, no cross-impl consumer reads it, and the tree
//! path is therefore ours. [`crate::feed::Follow`] is the codec; this is the
//! registry.
//!
//! ## It is a flat registry, deliberately the same one as `site-origins`
//!
//! One level, keyed by the followed peer id, one entity per row. That is
//! `origins`' shape and `peer_supersessions`' shape, and copying it is not
//! laziness — it means the same `tree_listing` immediate-children read is
//! correct here, the same `seed_write` arm-routing applies, and a reader of this
//! module already knows how it behaves.
//!
//! ## It is APP-scoped, not window-scoped
//!
//! See [`crate::app_paths::feed_follows_prefix`]. Two Feed windows must agree
//! about who you follow, and closing one must not unfollow anybody — so this is
//! a property of the profile, and it never touches `window_state_path`. That
//! also keeps it clear of AP42's reused-slot hazard, which is about state whose
//! address is a slot number.
//!
//! ## ⚠ Read per call; do not cache the result in a model (AP41)
//!
//! [`list`] is a **synchronous** `tree_listing`, which answers from the
//! per-prefix cache mirror — empty on the Worker arm until a subscription fills
//! it, and racing the store on Direct. That is fine for a render, which happens
//! again next frame, and it is a defect the moment a constructor keeps the
//! answer. The `SettingsModel` shape: **read per call and self-heal.** The
//! window subscribes [`crate::app_paths::feed_follows_prefix`] so a write marks
//! it dirty and the next frame re-reads.

#![allow(dead_code)] // the window lands with this module; native gates drive it

use crate::app_paths::{self, APP_ID};
use crate::feed::Follow;
use crate::peers::Peers;

/// Every peer this profile follows, **sorted by peer id** so a render is stable
/// frame to frame.
///
/// Ordering is ours and carries no meaning — §4.5's ordering contract is about
/// entries *within* a publisher's page, and it says in as many words that a
/// reader merging several publishers *"has no cross-publisher order in the
/// data"*. A stable arbitrary order is the honest presentation choice; a
/// most-recent-first order would look like a claim.
pub fn list(peers: &Peers, our_peer_id: &str) -> Vec<Follow> {
    let prefix = app_paths::feed_follows_prefix(APP_ID, our_peer_id);
    let mut out: std::collections::BTreeMap<String, Follow> = std::collections::BTreeMap::new();
    for entry in peers.tree_listing(our_peer_id, &prefix) {
        let Some(rest) = entry.path.strip_prefix(&prefix) else { continue };
        let subject = rest.trim_start_matches('/');
        // One level — `{prefix}{subject}`. Skip empties and anything deeper.
        if subject.is_empty() || subject.contains('/') {
            continue;
        }
        let Some(entity) = peers.get_entity(our_peer_id, &entry.path) else { continue };
        // **A row that does not decode is skipped, not fatal.** This registry
        // shares a path prefix with nothing today, but the tree is a universal
        // namespace and `Follow::from_entity` opens by checking the entity type
        // for exactly that reason. One unreadable row must not cost a person
        // every other publisher they follow.
        if let Ok(follow) = Follow::from_entity(&entity) {
            out.insert(subject.to_string(), follow);
        }
    }
    out.into_values().collect()
}

/// Whether this profile follows `subject`.
pub fn is_following(peers: &Peers, our_peer_id: &str, subject: &str) -> bool {
    peers
        .get_entity(our_peer_id, &app_paths::feed_follow_path(APP_ID, our_peer_id, subject))
        .is_some()
}

/// Start following `subject`.
///
/// **Idempotent by content, which is what makes it safe to call from a button.**
/// The store is content-addressed, so re-following someone with the same `since`
/// writes identical bytes and fires no subscription (AP43) — and re-following
/// with a *new* `since` would move the record for no reason, so [`follow`]
/// refuses to overwrite an existing row. Unfollow-then-follow is how you reset
/// it, which is a deliberate act rather than a side effect of clicking twice.
pub fn follow(peers: &Peers, our_peer_id: &str, subject: &str, since: u64) -> FollowOutcome {
    if subject.trim().is_empty() {
        return FollowOutcome::NotAPeerId;
    }
    // The peer id has to carry its own key or nothing can ever be verified
    // against it — `OriginFeedSource::new` refuses such an author outright, so
    // accepting one here would file a follow that can only ever fail. Refused at
    // the point somebody typed it, where the message can still be about what
    // they typed.
    if entity_crypto::PeerId::from(subject.to_string()).derive_public_key().is_none() {
        return FollowOutcome::NotAPeerId;
    }
    if subject == our_peer_id {
        return FollowOutcome::ThatIsYou;
    }
    if is_following(peers, our_peer_id, subject) {
        return FollowOutcome::AlreadyFollowing;
    }
    let entity = match Follow::new(subject, since).to_entity() {
        Ok(e) => e,
        Err(_) => return FollowOutcome::NotAPeerId,
    };
    let path = app_paths::feed_follow_path(APP_ID, our_peer_id, subject);
    peers.seed_write(our_peer_id, path, entity);
    FollowOutcome::Followed
}

/// Stop following `subject`. Removing what is not there is not an error — the
/// end state is the same and a button that reports a failure for reaching the
/// state you asked for is a button that teaches people to distrust it.
pub fn unfollow(peers: &Peers, our_peer_id: &str, subject: &str) {
    let path = app_paths::feed_follow_path(APP_ID, our_peer_id, subject);
    peers.seed_remove(our_peer_id, path);
}

/// What happened when somebody pressed Follow.
///
/// **Four outcomes rather than a `bool`,** because they are four different
/// sentences and three of them are not failures: *you already follow them* is
/// not an error, *that is your own peer id* is a different mistake from *that is
/// not a peer id at all*, and only the last one means "check what you pasted".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowOutcome {
    Followed,
    AlreadyFollowing,
    /// The subject is this profile's own peer. Following yourself would work —
    /// the tree is a universal namespace — and it is almost always a paste
    /// error, so it gets its own word rather than silently succeeding.
    ThatIsYou,
    /// Empty, or a peer id that does not carry its own key. **The same refusal
    /// `OriginFeedSource::new` makes**, moved to where a person can still see
    /// what they typed.
    NotAPeerId,
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_757_000_000_000;

    /// A real peer id, from a real keypair — `derive_public_key` must succeed on
    /// it, so a hand-typed `"QmWhatever"` would make every gate below vacuous by
    /// landing on `NotAPeerId`.
    fn peer_id(seed: u8) -> String {
        let kp = entity_crypto::Keypair::from_seed([seed; 32]);
        entity_crypto::PeerId::from_public_key(&kp.public_key_bytes()).to_string()
    }

    fn peers() -> (Peers, String) {
        let p = Peers::new_direct();
        let me = p.system_peer_id().to_string();
        (p, me)
    }

    #[test]
    fn following_a_peer_puts_them_in_the_list_and_unfollowing_takes_them_out() {
        let (p, me) = peers();
        let them = peer_id(11);
        assert_eq!(list(&p, &me).len(), 0, "a fresh profile follows nobody");

        assert_eq!(follow(&p, &me, &them, NOW), FollowOutcome::Followed);
        let rows = list(&p, &me);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].subject, them);
        assert_eq!(rows[0].since, NOW);
        assert!(is_following(&p, &me, &them));

        unfollow(&p, &me, &them);
        assert_eq!(list(&p, &me).len(), 0);
        assert!(!is_following(&p, &me, &them));
    }

    /// **Each refusal has its own word**, and two of the four are not failures.
    /// A button that reported one sentence for all of them would tell somebody
    /// who already follows a publisher to check their clipboard.
    #[test]
    fn every_way_a_follow_can_be_refused_has_its_own_outcome() {
        let (p, me) = peers();
        let them = peer_id(12);

        assert_eq!(follow(&p, &me, "", NOW), FollowOutcome::NotAPeerId, "empty");
        assert_eq!(
            follow(&p, &me, "not-a-peer-id", NOW),
            FollowOutcome::NotAPeerId,
            "a peer id that carries no key can never be verified against"
        );
        assert_eq!(follow(&p, &me, &me, NOW), FollowOutcome::ThatIsYou);

        assert_eq!(follow(&p, &me, &them, NOW), FollowOutcome::Followed);
        assert_eq!(
            follow(&p, &me, &them, NOW + 5000),
            FollowOutcome::AlreadyFollowing,
            "and a second press does not move `since` — re-following is not an edit"
        );
        assert_eq!(list(&p, &me)[0].since, NOW, "the original record is intact");
    }

    /// Unfollowing somebody you do not follow reaches the state you asked for,
    /// so it is not an error.
    #[test]
    fn unfollowing_a_stranger_is_not_an_error() {
        let (p, me) = peers();
        unfollow(&p, &me, &peer_id(13));
        assert_eq!(list(&p, &me).len(), 0);
    }

    /// The list is sorted and stable, so a render does not reshuffle between
    /// frames. Ordering carries no meaning — §4.5 gives a merged view no
    /// cross-publisher order, so an arbitrary-but-stable one is the honest
    /// choice and a recency order would look like a claim.
    #[test]
    fn the_list_is_stable_between_reads() {
        let (p, me) = peers();
        let ids: Vec<String> = (20..25).map(peer_id).collect();
        for id in &ids {
            assert_eq!(follow(&p, &me, id, NOW), FollowOutcome::Followed);
        }
        let first: Vec<String> = list(&p, &me).into_iter().map(|f| f.subject).collect();
        let second: Vec<String> = list(&p, &me).into_iter().map(|f| f.subject).collect();
        assert_eq!(first, second);
        assert_eq!(first.len(), 5);
        let mut sorted = first.clone();
        sorted.sort();
        assert_eq!(first, sorted, "sorted by peer id");
    }

    /// **A row that does not decode costs that row and nothing else.** The tree
    /// is a universal namespace, so a prefix is not a guarantee about what lives
    /// under it — the same first question `Follow::from_entity` asks by checking
    /// the entity type, and the reason AP42's decoders open with that check.
    #[test]
    fn an_undecodable_row_is_skipped_and_the_rest_of_the_list_survives() {
        let (p, me) = peers();
        let good = peer_id(30);
        assert_eq!(follow(&p, &me, &good, NOW), FollowOutcome::Followed);

        // Something else entirely, parked at a follow-shaped key.
        let junk = entity_entity::Entity::new(
            "app/state/some_other_window",
            entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![])),
        )
        .unwrap();
        p.seed_write(
            &me,
            app_paths::feed_follow_path(APP_ID, &me, "IMPOSTOR"),
            junk,
        );

        let rows = list(&p, &me);
        assert_eq!(rows.len(), 1, "the junk row is skipped, not fatal");
        assert_eq!(rows[0].subject, good);
    }
}

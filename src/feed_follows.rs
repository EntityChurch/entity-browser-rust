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

/// [`follow`], **recording the identifier you got here by** — §2.4's `via`,
/// *"the identifier as typed or scanned, for provenance display only."*
///
/// ⭐ **This is the only moment the name exists.** A registry resolve
/// establishes `billslab.com → 2KBj…`, the binding expires, and nothing durable
/// anywhere holds the pair — so a Feed window opened from that resolve had a
/// 45-character key and no way back to the word the person typed. The follow
/// record is the one durable thing this act writes, and the convention already
/// put a slot for exactly this in it.
///
/// ⛔ **It is provenance, never a claim about the publisher.** §2.4: the record
/// is *the reader's private data*, nothing publishes it, and `via` is display
/// only. That is what keeps this clear of AP30 — we are not writing down a
/// remote assertion and treating it as current truth, we are writing down **what
/// we did**, which does not go stale because it already happened. A registry
/// that later binds that name elsewhere does not make *"I got here by typing
/// billslab.com"* false.
///
/// ⭐ **Backfilled onto an existing row, and only when absent.** Somebody who
/// followed by pasting an id and *then* resolved the name would otherwise never
/// get one — the `AlreadyFollowing` arm returns before any write. Absent-only,
/// because the first route you recorded is the one that happened; a later one
/// overwriting it would make the field mean *"most recent route"*, which is a
/// different fact and one nothing asked for.
///
/// The outcome is still [`FollowOutcome`]'s four: **a backfill is not a follow**
/// and must not be able to wear `Followed`'s word. What happened to the follow
/// is what the notice is about.
pub fn follow_via(
    peers: &Peers,
    our_peer_id: &str,
    subject: &str,
    since: u64,
    via: Option<&str>,
) -> FollowOutcome {
    let outcome = follow(peers, our_peer_id, subject, since);
    let Some(via) = via.map(str::trim).filter(|v| !v.is_empty()) else {
        return outcome;
    };
    match outcome {
        // Just written, with no `via` — put it on.
        FollowOutcome::Followed => {
            amend(peers, our_peer_id, subject, |f| {
                f.via = Some(via.to_string());
            });
        }
        // Already there. Absent-only; see the doc.
        FollowOutcome::AlreadyFollowing => {
            amend(peers, our_peer_id, subject, |f| {
                if f.via.is_none() {
                    f.via = Some(via.to_string());
                }
            });
        }
        // Nothing was written, so there is nothing to amend.
        FollowOutcome::ThatIsYou | FollowOutcome::NotAPeerId => {}
    }
    outcome
}

/// **Name somebody.** §2.4's `label` — *"a petname: local, chosen by the reader,
/// and never authoritative"*.
///
/// This is the whole answer to *"I cannot read a public key"* that needs no
/// naming authority: the name lives in the reader's own tree and is transmitted
/// as a claim about nobody. An empty string **clears** it, which is a different
/// act from setting one and gets a different word.
///
/// ⛔ **It hangs off the follow record, so naming somebody means following
/// them** — and that is the convention's shape rather than a limitation we
/// invented. A petname for a publisher you do not follow would need a second
/// registry keyed by peer id, holding a fact nothing else reads, for a row that
/// is not in your list. If that is ever wanted, it is a design question and not
/// a field to slip in here.
pub fn set_label(
    peers: &Peers,
    our_peer_id: &str,
    subject: &str,
    label: &str,
) -> LabelOutcome {
    let label = label.trim();
    if !is_following(peers, our_peer_id, subject) {
        return LabelOutcome::NotFollowing;
    }
    let cleared = label.is_empty();
    amend(peers, our_peer_id, subject, |f| {
        f.label = if cleared { None } else { Some(label.to_string()) };
    });
    if cleared {
        LabelOutcome::Cleared
    } else {
        LabelOutcome::Named
    }
}

/// Read one follow row, change it, write it back.
///
/// ⚠ **Read-modify-write, and the read is what makes it safe.** Rebuilding a
/// `Follow` from the arguments to hand would silently drop whichever of `since`
/// / `via` / `label` the caller did not think about — which is the shape of
/// every *"we re-encoded rather than carrying"* defect in this tree, at the one
/// size where it is invisible. A row that does not decode is left alone: an
/// amendment is not a repair, and overwriting something we could not read is how
/// a bad decode becomes a lost record.
///
/// Idempotent by content, like [`follow`]: writing back an unchanged `Follow`
/// produces identical bytes, fires no subscription (AP43), and costs nothing.
fn amend(peers: &Peers, our_peer_id: &str, subject: &str, change: impl FnOnce(&mut Follow)) {
    let path = app_paths::feed_follow_path(APP_ID, our_peer_id, subject);
    let Some(entity) = peers.get_entity(our_peer_id, &path) else { return };
    let Ok(mut follow) = Follow::from_entity(&entity) else {
        tracing::warn!(
            subject = %subject,
            "feed follow: the row did not decode; leaving it alone rather than overwriting it"
        );
        return;
    };
    change(&mut follow);
    match follow.to_entity() {
        Ok(e) => peers.seed_write(our_peer_id, path, e),
        Err(why) => tracing::warn!(error = %why, "feed follow: the amended row would not encode"),
    }
}

/// What happened when somebody set or cleared a petname.
///
/// **Three, because they are three sentences.** *Naming* and *un-naming* are
/// opposite acts and a surface that reported them alike would tell somebody
/// their alias was saved when they had just deleted it; and *you do not follow
/// them* is neither, because §2.4 hangs the petname off the follow record — see
/// [`set_label`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelOutcome {
    Named,
    Cleared,
    /// There is no follow record to hang a name on.
    NotFollowing,
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

    /// ⭐ **The name you resolved survives the act that used it.**
    ///
    /// A registry resolve is the only moment `billslab.com → 2KBj…` exists: the
    /// binding expires, nothing durable holds the pair, and until this landed a
    /// Feed window opened from that resolve had the key and nothing else. The
    /// follow record is the one durable thing the press writes.
    #[test]
    fn following_by_a_resolved_name_records_the_name_you_got_there_by() {
        let (p, me) = peers();
        let them = peer_id(40);

        assert_eq!(
            follow_via(&p, &me, &them, NOW, Some("billslab.com")),
            FollowOutcome::Followed
        );
        let row = &list(&p, &me)[0];
        assert_eq!(row.via.as_deref(), Some("billslab.com"));
        assert_eq!(row.label, None, "a petname is the reader's to choose, not ours");
    }

    /// ⭐ **Backfilled when absent, and never overwritten** — the two halves of
    /// `via`'s rule, asserted separately because they fail in opposite
    /// directions.
    ///
    /// Someone who pasted a peer id and *then* resolved its name would get
    /// nothing without the backfill: `follow`'s `AlreadyFollowing` arm returns
    /// before any write. And a second, different route must not rewrite the
    /// first, or the field stops meaning *how I got here* and starts meaning
    /// *the last time I arrived*, which nothing asked for.
    #[test]
    fn a_missing_route_is_filled_in_and_a_recorded_one_is_never_rewritten() {
        let (p, me) = peers();
        let them = peer_id(41);

        // Followed by pasting an id: no route recorded.
        assert_eq!(follow(&p, &me, &them, NOW), FollowOutcome::Followed);
        assert_eq!(list(&p, &me)[0].via, None);

        // …then resolved through a registry. The follow did not change; what we
        // know about it did.
        assert_eq!(
            follow_via(&p, &me, &them, NOW + 1, Some("billslab.com")),
            FollowOutcome::AlreadyFollowing,
            "a backfill is not a follow and must not wear its word"
        );
        let row = &list(&p, &me)[0];
        assert_eq!(row.via.as_deref(), Some("billslab.com"));
        assert_eq!(row.since, NOW, "and it is not an edit of anything else");

        // A later, different route leaves it alone.
        assert_eq!(
            follow_via(&p, &me, &them, NOW + 2, Some("somewhere.else")),
            FollowOutcome::AlreadyFollowing
        );
        assert_eq!(list(&p, &me)[0].via.as_deref(), Some("billslab.com"));
    }

    /// A refused follow writes nothing, so there is nothing for a `via` to land
    /// on — asserted because the amend runs after the follow and a version that
    /// did not check the outcome would create a row out of a refusal.
    #[test]
    fn a_refused_follow_records_no_route_and_no_row() {
        let (p, me) = peers();
        assert_eq!(
            follow_via(&p, &me, "not-a-peer-id", NOW, Some("billslab.com")),
            FollowOutcome::NotAPeerId
        );
        assert_eq!(
            follow_via(&p, &me, &me.clone(), NOW, Some("me.example")),
            FollowOutcome::ThatIsYou
        );
        assert!(list(&p, &me).is_empty(), "a refusal creates no follow row");
    }

    /// ⭐ **The petname, and the three outcomes are three sentences.**
    ///
    /// Naming and un-naming are opposite acts; reporting them alike would tell
    /// somebody their alias was saved at the moment they deleted it.
    #[test]
    fn naming_somebody_and_un_naming_them_are_different_acts_with_different_words() {
        let (p, me) = peers();
        let them = peer_id(42);

        assert_eq!(
            set_label(&p, &me, &them, "Bill"),
            LabelOutcome::NotFollowing,
            "§2.4 hangs the petname off the follow record, so there is nowhere to put it"
        );

        follow(&p, &me, &them, NOW);
        assert_eq!(set_label(&p, &me, &them, "  Bill  "), LabelOutcome::Named);
        assert_eq!(list(&p, &me)[0].label.as_deref(), Some("Bill"), "trimmed");

        assert_eq!(set_label(&p, &me, &them, ""), LabelOutcome::Cleared);
        assert_eq!(list(&p, &me)[0].label, None);
        assert_eq!(set_label(&p, &me, &them, "   "), LabelOutcome::Cleared, "blank clears");
    }

    /// ⭐⭐ **An amendment carries every field it did not come to change.**
    ///
    /// This is the one that would fail silently: rebuilding a `Follow` from the
    /// arguments at hand is the obvious implementation and it drops `since` and
    /// whichever of `label`/`via` this call is not about. It is the
    /// re-encode-rather-than-carry shape at the smallest size it comes in, and
    /// nothing downstream would report it — the row still decodes, still lists,
    /// and has quietly lost when you followed somebody.
    #[test]
    fn amending_one_field_keeps_the_others() {
        let (p, me) = peers();
        let them = peer_id(43);

        follow_via(&p, &me, &them, NOW, Some("billslab.com"));
        set_label(&p, &me, &them, "Bill");

        let row = &list(&p, &me)[0];
        assert_eq!(row.subject, them);
        assert_eq!(row.since, NOW, "the follow instant survived the naming");
        assert_eq!(row.via.as_deref(), Some("billslab.com"), "…and so did the route");
        assert_eq!(row.label.as_deref(), Some("Bill"));

        // …and clearing the name keeps the rest, which is the same property in
        // the direction where a naive implementation looks most correct.
        set_label(&p, &me, &them, "");
        let row = &list(&p, &me)[0];
        assert_eq!(row.since, NOW);
        assert_eq!(row.via.as_deref(), Some("billslab.com"));
    }

    /// A row that does not decode is **left alone**, not overwritten. An
    /// amendment is not a repair, and rewriting something we could not read is
    /// how an unreadable record becomes a lost one.
    #[test]
    fn an_unreadable_row_is_not_repaired_by_being_overwritten() {
        let (p, me) = peers();
        let junk = entity_entity::Entity::new(
            "app/state/some_other_window",
            entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![])),
        )
        .unwrap();
        let path = app_paths::feed_follow_path(APP_ID, &me, "IMPOSTOR");
        p.seed_write(&me, path.clone(), junk.clone());

        // `is_following` is presence, so the label verb gets past its guard and
        // reaches the decode — which is exactly the path being measured.
        assert_eq!(set_label(&p, &me, "IMPOSTOR", "Bill"), LabelOutcome::Named);
        let held = p.get_entity(&me, &path).expect("the row is still there");
        assert_eq!(held.data, junk.data, "and it is byte-identical to what was there");
    }
}

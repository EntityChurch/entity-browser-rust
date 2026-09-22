//! **Who this profile reads other people THROUGH** — the third source leg's
//! durable half.
//!
//! [`crate::feed_follows`] is *who you read*; this is *who you are willing to be
//! told about them by*. `APP-CONVENTION-FEED` §6.2 places a gatherer's mirror as
//! *"one leg of an ordered source set… not a replacement for the author"*, and
//! [`crate::feed_route::plan`] already takes the list — it has been passed an
//! **empty slice** since the day it shipped, because nothing put anything in it.
//! This is what puts something in it.
//!
//! ## ⛔ Somebody TYPES a gatherer. Nothing infers one, and that is the design.
//!
//! §6 gives a reader **no way to learn that a gatherer exists** — measured, and
//! routed: there is no advertisement, no registry binding, no field in
//! `/entity-deployment.json`, and the convention's own §6.0.1 is a *derivation*
//! (you can compute where a mirror would be **if** you already knew whose) rather
//! than a discovery. So the honest surface is a list a person maintains.
//!
//! **The alternative is the failure this repo already has a name for.** A viewer
//! that cannot be told its source invents one, and the invention is always *"the
//! first thing I already hold"* — `views/games::app_source` picking the first
//! foreign entry in the origins registry, which is AP54. A mirror makes that
//! worse than usual, because the wrong guess is not a stale publisher but **a
//! stranger's reading of somebody else**, presented in the place their own posts
//! would be. `DESIGN-2026-09-12-BROWSING-WITHOUT-PRIVILEGING-A-CONVENTION` §4
//! says it in as many words: *discovery is separable and must not be smuggled in*.
//!
//! ## Shape: `feed_follows`', deliberately
//!
//! A flat one-level registry under [`crate::app_paths::feed_gatherers_prefix`],
//! APP-scoped rather than window-scoped (two Feed windows must agree; closing one
//! must not drop a source), read per call and never cached in a model (AP41).
//! Copying that module's shape means the same `tree_listing` read, the same
//! `seed_write` arm-routing and the same behaviour a reader of either already
//! knows.
//!
//! ## Why a local type and not `app/feed/follow` with a flag
//!
//! A follow record is §2.4's declared type and the SHARE correction is one
//! convention over: **emit the declared fields and nothing else**, because an
//! encoder that always adds a key of its own can never be byte-stable
//! cross-impl. A gatherer entry is not a follow — it answers a different
//! question — so it gets our own `app/state/*` type rather than a field
//! smuggled into somebody else's schema.

#![allow(dead_code)] // the window wires this; the gates below drive it natively

use entity_ecf::{text, to_ecf, uinteger, Value};
use entity_entity::Entity;

use crate::app_paths::{self, APP_ID};
use crate::peers::Peers;

/// Our own local type. **Not a convention tag** — `vocab-lint` would be right to
/// call an `app/feed/*` spelling here undeclared vocabulary, and it would be
/// right for a second reason: §6 declares no type for this because it declares no
/// discovery at all.
pub const GATHERER_TYPE: &str = "app/state/feed-gatherer";

/// One peer this profile will read other authors through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GathererRow {
    pub peer_id: String,
    /// When it was added. Local bookkeeping; nothing on the wire reads it.
    pub since: u64,
}

impl GathererRow {
    pub fn to_entity(&self) -> Result<Entity, String> {
        let value = Value::Map(vec![
            (text("peer_id"), text(self.peer_id.clone())),
            (text("since"), uinteger(self.since)),
        ]);
        // `to_ecf`, never `ciborium::into_writer` — canonical map ordering is
        // what the peer will reproduce, and an entity carrying a hash over
        // non-canonical bytes fails the L1 put admission ladder silently.
        Entity::new(GATHERER_TYPE, to_ecf(&value)).map_err(|e| format!("{e:?}"))
    }

    /// **Opens by checking the entity type** (AP42). The tree is a universal
    /// namespace, so a prefix is not a promise about what lives under it.
    pub fn from_entity(e: &Entity) -> Result<Self, String> {
        if e.entity_type != GATHERER_TYPE {
            return Err(format!("not a gatherer record: {}", e.entity_type));
        }
        let value: Value =
            ciborium::from_reader(e.data.as_slice()).map_err(|err| format!("{err}"))?;
        let Value::Map(map) = value else { return Err("not a map".into()) };
        let get = |k: &str| map.iter().find(|(key, _)| key.as_text() == Some(k)).map(|(_, v)| v);
        let peer_id = get("peer_id")
            .and_then(|v| v.as_text())
            .ok_or_else(|| "no peer_id".to_string())?
            .to_string();
        let since = get("since").and_then(|v| v.as_integer()).and_then(|i| u64::try_from(i).ok());
        Ok(GathererRow { peer_id, since: since.unwrap_or(0) })
    }
}

/// Every gatherer this profile reads through, sorted by peer id.
///
/// ⚠ **Read per call.** `tree_listing` is synchronous and answers from the
/// per-prefix cache mirror; retaining the result in a model is AP41.
pub fn list(peers: &Peers, our_peer_id: &str) -> Vec<GathererRow> {
    let prefix = app_paths::feed_gatherers_prefix(APP_ID, our_peer_id);
    let mut out: std::collections::BTreeMap<String, GathererRow> = Default::default();
    for entry in peers.tree_listing(our_peer_id, &prefix) {
        let Some(rest) = entry.path.strip_prefix(&prefix) else { continue };
        let id = rest.trim_start_matches('/');
        if id.is_empty() || id.contains('/') {
            continue;
        }
        let Some(entity) = peers.get_entity(our_peer_id, &entry.path) else { continue };
        // One unreadable row costs that row, never the rest of the list.
        if let Ok(row) = GathererRow::from_entity(&entity) {
            out.insert(id.to_string(), row);
        }
    }
    out.into_values().collect()
}

/// What happened when somebody added a gatherer.
///
/// **Four outcomes, and only one of them is "check what you pasted."** Same
/// shape and same reason as [`crate::feed_follows::FollowOutcome`]: *you already
/// read through them* is not a failure, and *that is your own peer id* is a
/// different mistake from *that is not a peer id*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddOutcome {
    Added,
    AlreadyAGatherer,
    /// Your own peer. **Refused rather than allowed**, and not only because it is
    /// usually a paste error: `feed_route::plan` would put a leg in the route for
    /// reading yourself through yourself, and `walk_route` would spend a fetch on
    /// it. A profile's own mirrors, when it has any, are not a *source leg*.
    ThatIsYou,
    /// Empty, or a peer id that carries no key. **The same refusal
    /// `OriginMirrorSource::new` makes** — a gatherer whose tree cannot be pinned
    /// can never be verified, so accepting one here files a source that can only
    /// ever fail, at a distance from the person who typed it.
    NotAPeerId,
}

/// Read other authors through `gatherer` from now on.
pub fn add(peers: &Peers, our_peer_id: &str, gatherer: &str, since: u64) -> AddOutcome {
    let gatherer = gatherer.trim();
    if gatherer.is_empty()
        || entity_crypto::PeerId::from(gatherer.to_string()).derive_public_key().is_none()
    {
        return AddOutcome::NotAPeerId;
    }
    if gatherer == our_peer_id {
        return AddOutcome::ThatIsYou;
    }
    if is_gatherer(peers, our_peer_id, gatherer) {
        return AddOutcome::AlreadyAGatherer;
    }
    let entity = match (GathererRow { peer_id: gatherer.to_string(), since }).to_entity() {
        Ok(e) => e,
        Err(_) => return AddOutcome::NotAPeerId,
    };
    peers.seed_write(
        our_peer_id,
        app_paths::feed_gatherer_path(APP_ID, our_peer_id, gatherer),
        entity,
    );
    AddOutcome::Added
}

/// Whether this profile reads through `gatherer`.
pub fn is_gatherer(peers: &Peers, our_peer_id: &str, gatherer: &str) -> bool {
    peers
        .get_entity(our_peer_id, &app_paths::feed_gatherer_path(APP_ID, our_peer_id, gatherer))
        .is_some()
}

/// Stop reading through `gatherer`. Removing what is not there is not an error.
pub fn remove(peers: &Peers, our_peer_id: &str, gatherer: &str) {
    peers.seed_remove(
        our_peer_id,
        app_paths::feed_gatherer_path(APP_ID, our_peer_id, gatherer),
    );
}

/// The gatherer legs for a route — **each resolved through
/// [`origins::get_origin`](crate::content_site::origins::get_origin)**, the
/// accessor that applies supersession.
///
/// ⭐ **A gatherer we have no route to contributes NO LEG, and that is a
/// decision rather than a filter.** `feed_route::Gatherer` carries an origin
/// because a mirror is fetched over HTTP from the gatherer's own tree; with no
/// origin there is no URL to build, and manufacturing one relative to the page
/// would ask *our* deployment for a stranger's mirrors — `OriginFeedSource`'s
/// empty-origin defect, which produced a 404 that read as *"this publisher has
/// no feed"*. A dropped leg is honest; an invented one is a wrong answer.
pub fn legs(
    peers: &Peers,
    our_peer_id: &str,
) -> Vec<crate::feed_route::Gatherer> {
    list(peers, our_peer_id)
        .into_iter()
        .filter_map(|row| {
            let origin =
                crate::content_site::origins::get_origin(peers, our_peer_id, &row.peer_id)?;
            Some(crate::feed_route::Gatherer { peer_id: row.peer_id, origin })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_757_000_000_000;

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
    fn adding_a_gatherer_puts_them_in_the_list_and_removing_takes_them_out() {
        let (p, me) = peers();
        let g = peer_id(51);
        assert_eq!(list(&p, &me).len(), 0, "a fresh profile reads through nobody");

        assert_eq!(add(&p, &me, &g, NOW), AddOutcome::Added);
        let rows = list(&p, &me);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].peer_id, g);
        assert_eq!(rows[0].since, NOW);
        assert!(is_gatherer(&p, &me, &g));

        remove(&p, &me, &g);
        assert_eq!(list(&p, &me).len(), 0);
        assert!(!is_gatherer(&p, &me, &g));
    }

    /// Each refusal has its own word, and two of the four are not failures.
    #[test]
    fn every_way_adding_a_gatherer_can_be_refused_has_its_own_outcome() {
        let (p, me) = peers();
        let g = peer_id(52);

        assert_eq!(add(&p, &me, "", NOW), AddOutcome::NotAPeerId);
        assert_eq!(
            add(&p, &me, "not-a-peer-id", NOW),
            AddOutcome::NotAPeerId,
            "a gatherer whose tree cannot be pinned can never be verified"
        );
        assert_eq!(add(&p, &me, &me, NOW), AddOutcome::ThatIsYou);
        assert_eq!(add(&p, &me, &g, NOW), AddOutcome::Added);
        assert_eq!(add(&p, &me, &g, NOW + 1), AddOutcome::AlreadyAGatherer);
        assert_eq!(list(&p, &me)[0].since, NOW, "a second press is not an edit");
    }

    /// ⛔ **A gatherer with no registered origin contributes no leg.**
    ///
    /// The alternative is `OriginFeedSource`'s first cut: an empty origin makes
    /// every fetch relative to the page, so the browser asks its own deployment
    /// for a stranger's mirrors and the 404 renders as *"this author has no
    /// feed"*. **A dropped leg is honest; an invented one is a wrong answer.**
    #[test]
    fn a_gatherer_we_have_no_route_to_contributes_no_leg() {
        let (p, me) = peers();
        let routed = peer_id(53);
        let unrouted = peer_id(54);
        assert_eq!(add(&p, &me, &routed, NOW), AddOutcome::Added);
        assert_eq!(add(&p, &me, &unrouted, NOW), AddOutcome::Added);

        assert_eq!(list(&p, &me).len(), 2, "both are in the durable list");
        assert!(legs(&p, &me).is_empty(), "neither has an origin yet");

        crate::content_site::origins::set_origin(
            &p,
            &me,
            &routed,
            "http://gatherer.example",
        );
        let legs = legs(&p, &me);
        assert_eq!(legs.len(), 1, "only the one we can reach");
        assert_eq!(legs[0].peer_id, routed);
        assert_eq!(legs[0].origin, "http://gatherer.example");
    }

    /// A row that does not decode costs that row and nothing else — and the
    /// guard is the **entity type**, because the tree is a universal namespace
    /// and a prefix promises nothing about what is under it (AP42).
    #[test]
    fn an_undecodable_row_is_skipped_and_the_rest_of_the_list_survives() {
        let (p, me) = peers();
        let good = peer_id(55);
        assert_eq!(add(&p, &me, &good, NOW), AddOutcome::Added);

        // A *follow* record, parked at a gatherer-shaped key. Deliberately a
        // real neighbouring type rather than junk: the two registries are one
        // path segment apart and both are keyed by a peer id, so this is the
        // confusion that can actually happen.
        let follow = crate::feed::Follow::new(&peer_id(56), NOW).to_entity().unwrap();
        p.seed_write(
            &me,
            app_paths::feed_gatherer_path(APP_ID, &me, "IMPOSTOR"),
            follow,
        );

        let rows = list(&p, &me);
        assert_eq!(rows.len(), 1, "the follow record is skipped, not adopted");
        assert_eq!(rows[0].peer_id, good);
    }

    /// The record round-trips, and the encoding is canonical ECF.
    #[test]
    fn a_gatherer_record_round_trips() {
        let row = GathererRow { peer_id: peer_id(57), since: NOW };
        let e = row.to_entity().unwrap();
        assert_eq!(e.entity_type, GATHERER_TYPE);
        assert_eq!(GathererRow::from_entity(&e).unwrap(), row);
        assert_eq!(
            e.data,
            to_ecf(&Value::Map(vec![
                (text("peer_id"), text(row.peer_id.clone())),
                (text("since"), uinteger(row.since)),
            ])),
            "the stored bytes are the canonical encoding, not whatever order we wrote"
        );
    }
}

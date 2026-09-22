//! The window index — the durable record of *which windows exist*, which is
//! the one thing `workspace/windows/{id}/state` cannot tell you.
//!
//! # The defect this closes
//!
//! `GUIDE-ENTITY-WORKBENCH-APP.md` §3 puts per-window state at
//! `app/{app-id}/workspace/windows/{window_id}/state` and §1 pins both that
//! prefix and the `(window_id, event, value)` action shape at **MUST**. §8 then
//! says per-window state should *"persist if the application offers session
//! resumption."* Nothing anywhere constrains `{window_id}`'s **lifetime**, and
//! both impls that exist made it a per-session counter from 1 — because the
//! action shape wants a small dense integer, which is a completely reasonable
//! thing to want.
//!
//! So one identifier does two incompatible jobs: a **session slot address**
//! (which pane is on screen now) and a **durable state key** (whose saved state
//! is this). Taking §8's persist arm with the second job unfilled is what
//! manufactures every symptom AP42 catalogued. The id reuse is how it shows;
//! **persisting without a roster is what it is.**
//!
//! The consequence that shows AP42's type discriminator is a guard and not a
//! fix: with ordinals and no index, **whether you get your state back depends on
//! the order you reopen windows in.** Last session had Shell=1, KB=2; reload;
//! open the KB first and it takes id 1, reads `app/state/shell`, correctly
//! refuses — and its own state, at id 2, is now unreachable for the rest of the
//! session. The guard turns wrong-adoption into no-adoption. It never makes the
//! right state findable.
//!
//! # What this is
//!
//! One entity on the primary peer listing the live windows as
//! `(id, type_name, peer_id)`. It buys three things, none of which needs the
//! path to change:
//!
//! 1. **Claiming.** At boot the previous session's entries become a claim table
//!    keyed `(type_name, peer_id)`. A window of that pair re-opens onto *its own*
//!    id, so [`crate::app_paths::window_state_path`] lands on its own state
//!    whatever order the user opens things in. This is the indirection that
//!    gets what type-scoping the path would have got, **without touching a
//!    cosigned MUST**.
//! 2. **A floor for `next_id`.** Fresh ids are allocated above every id the
//!    index knows, so an unclaimed window can never land on a stranger's slot.
//! 3. **An exact sweep.** State whose id is in neither the index nor a claim is
//!    unreachable by construction, so it can be deleted — as against a blanket
//!    "delete `workspace/windows/` at boot", which is the only sweep available
//!    without an index and which also deletes everything a user would want back.
//!
//! # Why `(type_name, peer_id)` is the claim key
//!
//! Because `boot_load` already treats that pair as a window's durable identity
//! for the one window it restores: *"Window ids are ephemeral → the durable
//! `(peer, type)` is the stable identifier, re-spawned each boot"*
//! (`BootSurface::Window`, `app.rs`). That was written for the maximized boot
//! surface and is correct there; this module is the same idea for every window.
//! It is also exactly the identity [`crate::window::WindowManager::find_open`]
//! already uses for singleton windows.
//!
//! Two windows of the same type on the same peer are a real configuration, and
//! the key does not distinguish them: the **first** to open claims the entry,
//! the second allocates fresh. That is a deliberate limit, not an oversight —
//! see the module's test of the same name.
//!
//! # Not "roster"
//!
//! `roster` is already taken in this tree for the **peer** roster
//! (`app_paths::roster_prefix`, `system/roster/`), which is a different durable
//! list of a different thing.
//!
//! # Type name
//!
//! `app/entity-browser/window-index` — app-internal per the workbench guide
//! §4.1.1, *not* `app/state/...`, because no cross-impl schema for this exists
//! yet. If arch takes the proposal this feeds, §4.1.1's own type-name promotion
//! path is how it becomes portable. **We do not get to write `app/state/` for
//! something one impl invented this week.**
//!
//! [AP30, AP40, AP41, AP42, AP44, D13, D16, `docs/SPEC-AMBIGUITIES.md` §1,
//!  `docs/plans/DESIGN-WINDOW-STATE-LIFECYCLE-AND-SESSION-RESUMPTION.md`]

use std::collections::HashMap;

use entity_entity::Entity;

use crate::peers::Peers;
use crate::window::WindowId;

/// Entity type of the window index.
///
/// App-internal (`app/{app-id}/{type}`), not `app/state/{type}` — see the module
/// docs. Checked on decode: this path is not a window-state path, so AP42's
/// reuse hazard does not reach it, but a decoder that trusts its path is the
/// habit that produced AP42 in the first place.
pub const INDEX_TYPE: &str = "app/entity-browser/window-index";

/// One live window, as the durable record sees it.
///
/// Deliberately **not** a snapshot of the window's contents — that is what
/// `windows/{id}/state` is for, and duplicating it here would be AP17 (an
/// authoritative source with a mirror left standing). This says only *that the
/// window exists, what type it is, and whose tree its state is in.*
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowIndexEntry {
    pub id: WindowId,
    /// The registry type name (`WindowType::name`), already canonicalized.
    pub type_name: String,
    /// The peer this window is bound to — **and therefore whose tree holds its
    /// state**, since `window_state_path` is qualified by the bound peer.
    pub peer_id: String,
}

/// The durable list of live windows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowIndex {
    pub entries: Vec<WindowIndexEntry>,
}

impl WindowIndex {
    /// Decode, returning `None` for *anything* we cannot vouch for — wrong
    /// type, unreadable bytes, not a map. `None` is the weakest claim (AP40)
    /// and its caller treats it as [`IndexLoad::Malformed`], which claims and
    /// sweeps nothing. A lossy `Default` here would be indistinguishable from a
    /// legitimately empty index, and an empty index authorizes a sweep.
    pub fn from_entity(entity: &Entity) -> Option<Self> {
        if entity.entity_type != INDEX_TYPE {
            return None;
        }
        let value: ciborium::Value = ciborium::from_reader(entity.data.as_slice()).ok()?;
        let map = value.as_map()?;
        let mut entries = Vec::new();
        for (k, v) in map {
            if k.as_text() != Some("windows") {
                continue;
            }
            let arr = v.as_array()?;
            for item in arr {
                // Every `?` below is load-bearing: one malformed row makes the
                // whole index untrustworthy, because a partial index still
                // authorizes a sweep and a sweep driven by a half-read list
                // deletes live state. Do not soften any of these to a `continue`.
                let fields = item.as_map()?;
                let mut id: Option<WindowId> = None;
                let mut type_name: Option<String> = None;
                let mut peer_id: Option<String> = None;
                for (fk, fv) in fields {
                    match fk.as_text() {
                        Some("id") => id = fv.as_integer().and_then(|i| WindowId::try_from(i).ok()),
                        Some("type") => type_name = fv.as_text().map(str::to_string),
                        Some("peer") => peer_id = fv.as_text().map(str::to_string),
                        _ => {}
                    }
                }
                match (id, type_name, peer_id) {
                    (Some(id), Some(type_name), Some(peer_id)) => entries.push(WindowIndexEntry {
                        id,
                        type_name,
                        peer_id,
                    }),
                    _ => return None,
                }
            }
        }
        Some(Self { entries })
    }

    pub fn to_entity(&self) -> Entity {
        let rows: Vec<entity_ecf::Value> = self
            .entries
            .iter()
            .map(|e| {
                entity_ecf::Value::Map(vec![
                    (
                        entity_ecf::Value::Text("id".into()),
                        entity_ecf::Value::Integer(e.id.into()),
                    ),
                    (
                        entity_ecf::Value::Text("type".into()),
                        entity_ecf::text(&e.type_name),
                    ),
                    (
                        entity_ecf::Value::Text("peer".into()),
                        entity_ecf::text(&e.peer_id),
                    ),
                ])
            })
            .collect();
        let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![(
            entity_ecf::Value::Text("windows".into()),
            entity_ecf::Value::Array(rows),
        )]));
        Entity::new(INDEX_TYPE, data).expect("window index entity is well-formed")
    }

    /// The claim table: `(type_name, peer_id)` → the id that pair held last
    /// session.
    ///
    /// **First entry wins** on a duplicate pair, matching the claim order at
    /// spawn: the first window of a pair to re-open takes the entry, and a
    /// second gets a fresh id. Building it the other way would hand the first
    /// re-opened window the *second* instance's state, which is worse than
    /// giving it nothing.
    pub fn claims(&self) -> HashMap<(String, String), WindowId> {
        let mut out: HashMap<(String, String), WindowId> = HashMap::new();
        for e in &self.entries {
            out.entry((e.type_name.clone(), e.peer_id.clone()))
                .or_insert(e.id);
        }
        out
    }

    /// One past the highest id this index knows about. Fresh ids start here so
    /// an unclaimed window cannot land on a slot that still holds state.
    pub fn next_id_floor(&self) -> WindowId {
        self.entries.iter().map(|e| e.id).max().unwrap_or(0) + 1
    }
}

/// How the boot-time index read resolved. Four outcomes, four words.
///
/// Split this finely on purpose — AP40, and the [`crate::window::Hydration`]
/// audit one layer down, where *"the read already answered"* and *"you moved
/// while we were reading"* shared a label and made a healthy boot
/// indistinguishable from a guard firing. Here the pair that must not merge is
/// **`NoIndex` vs `Malformed`**: *"you have never had one"* and *"you have one
/// and we cannot read it"* differ in exactly the way that decides whether a
/// sweep is safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexLoad {
    /// The index was read and its claims are installed. Carries the entry
    /// count, which is the only number an incident wants.
    Restored(usize),
    /// The tree **answered** and holds no index: a first boot, or the first
    /// boot after this feature shipped. No claims, and **no sweep** — the
    /// pre-index states under `workspace/windows/` are exactly the ones a
    /// blanket sweep would wrongly destroy.
    NoIndex,
    /// An entity is there and we cannot vouch for it. Claims and sweep are both
    /// withheld, same as `Unheard` — but it is *not* `Unheard`, because the
    /// round-trip worked and the fault is in the data.
    Malformed,
    /// The round-trip failed. **Changed nothing** (AP30 corollary (a)): a read
    /// that cannot answer must never be able to delete state or renumber
    /// windows.
    Unheard,
}

impl IndexLoad {
    pub fn label(self) -> &'static str {
        match self {
            IndexLoad::Restored(_) => "restored",
            IndexLoad::NoIndex => "no-index",
            IndexLoad::Malformed => "malformed",
            IndexLoad::Unheard => "unheard",
        }
    }

    /// May this outcome authorize deleting window state? Only a read we can
    /// vouch for. The three `false` arms are three different reasons and the
    /// enum keeps them apart; this collapses them at the single point where the
    /// collapse is correct.
    pub fn authorizes_sweep(self) -> bool {
        matches!(self, IndexLoad::Restored(_))
    }
}

/// The D13 line for the window index — one per boot, whatever happened.
///
/// Written as a function for the same reason `window_hydration::report` is: the
/// outcome that logs nothing is the one an incident needs, and five scattered
/// `tracing!` calls is how `NonePersisted` and the Direct-arm happy path both
/// ended up silent one layer down.
/// `swept` is on this line and not its own because a sweep is only ever a
/// consequence of the outcome beside it — reading *"restored 2 entries, swept 5
/// slots"* is the whole story, and the same two numbers in two places is how an
/// incident ends up correlating timestamps.
pub fn report(path: &str, outcome: IndexLoad, next_id: WindowId, swept: usize) -> IndexLoad {
    tracing::info!(
        path = %path,
        outcome = outcome.label(),
        entries = match outcome {
            IndexLoad::Restored(n) => n,
            _ => 0,
        },
        next_id,
        swept,
        "window index resolved against the durable tree"
    );
    outcome
}

/// Read the window index from the durable tree.
///
/// **`get_entity_async`, not `get_entity`** — AP41. The synchronous read answers
/// from the per-prefix cache mirror, which is never primed on the Worker arm for
/// a path nobody has subscribed, and races the store filling from IndexedDB on
/// the shipped Direct arm. This runs exactly once, on the boot path, before the
/// first window spawns; there is no frame budget to protect and no excuse for
/// the cold read.
pub async fn load(peers: &Peers, peer_id: &str, path: &str) -> (WindowIndex, IndexLoad) {
    match peers.get_entity_async(peer_id, path).await {
        Ok(Some(entity)) => match WindowIndex::from_entity(&entity) {
            Some(index) => {
                let n = index.entries.len();
                (index, IndexLoad::Restored(n))
            }
            None => {
                tracing::warn!(
                    path = %path,
                    entity_type = %entity.entity_type,
                    "window index is present but undecodable — claiming nothing and \
                     sweeping nothing"
                );
                (WindowIndex::default(), IndexLoad::Malformed)
            }
        },
        Ok(None) => (WindowIndex::default(), IndexLoad::NoIndex),
        Err(e) => {
            // NOT an answer. The error text is its own line because `report`
            // carries the outcome, not the cause.
            tracing::warn!(
                path = %path,
                error = %e,
                "window index read failed — keeping every window slot as-is rather than \
                 renumbering or sweeping"
            );
            (WindowIndex::default(), IndexLoad::Unheard)
        }
    }
}

/// Delete per-window state that no index entry can reach.
///
/// Only ever called when [`IndexLoad::authorizes_sweep`] — a sweep driven by an
/// index we could not read is indistinguishable from "delete the user's
/// workspace".
///
/// Scoped to `peer_id`'s own `workspace/windows/` prefix. Windows bound to
/// other peers write their state into *those* peers' trees, so a full sweep
/// would have to walk every peer; this walks the one the index lives on.
/// **That is a named limit** — unswept state on a secondary peer is inert
/// (unreachable, because ids are floored above the index) rather than wrong,
/// and it costs bytes, not correctness.
pub async fn sweep_unreachable(
    peers: &Peers,
    peer_id: &str,
    index: &WindowIndex,
    app_id: &str,
) -> usize {
    let live: std::collections::BTreeSet<WindowId> = index.entries.iter().map(|e| e.id).collect();
    let prefix = crate::app_paths::windows_prefix(app_id, peer_id);
    let listing = match peers.tree_listing_async(peer_id, &prefix).await {
        Ok(l) => l,
        Err(e) => {
            // Same rule as the read: no listing this boot changes nothing.
            tracing::warn!(
                prefix = %prefix,
                error = %e,
                "window-state listing failed — sweeping nothing"
            );
            return 0;
        }
    };
    let mut swept = 0;
    for id in ids_in_listing(&listing.iter().map(|e| e.path.clone()).collect::<Vec<_>>(), &prefix) {
        if live.contains(&id) {
            continue;
        }
        peers.dispatch_remove(
            peer_id,
            crate::app_paths::window_state_path(app_id, peer_id, id),
        );
        peers.dispatch_remove(
            peer_id,
            crate::app_paths::window_results_path(app_id, peer_id, id),
        );
        swept += 1;
    }
    if swept > 0 {
        tracing::info!(
            prefix = %prefix,
            swept,
            "swept per-window state no window index entry can reach"
        );
    }
    swept
}

/// Everything the boot path needs from the durable index, decided in one place.
#[derive(Debug, Clone)]
pub struct Resolution {
    /// The previous session's index — empty unless `outcome` is
    /// [`IndexLoad::Restored`].
    pub index: WindowIndex,
    pub outcome: IndexLoad,
    /// The lowest id fresh allocation may use. Always at least 1, and always
    /// above every occupied slot this boot could find out about — including on
    /// the paths where no index was readable, which is the case that would
    /// otherwise open a window straight onto a stranger's state.
    pub floor: WindowId,
    /// How many unreachable window slots were deleted (always 0 unless the
    /// outcome authorized a sweep).
    pub swept: usize,
}

/// Resolve the window index at boot: read it, sweep what it proves is
/// unreachable, and decide the fresh-id floor.
///
/// A free function over [`Peers`] rather than a method on the app, so the whole
/// policy — including the three withholding paths, which are the ones that
/// matter — is reachable from a native test without booting a browser.
///
/// **Order is load-bearing.** The sweep runs only after an index we can vouch
/// for, and the floor is computed from whatever authority *did* answer. The
/// failure shape this avoids is a read that times out (D23 makes that more
/// common, not less) being allowed to delete a workspace or renumber it.
pub async fn resolve_at_boot(peers: &Peers, peer_id: &str, app_id: &str) -> Resolution {
    let path = crate::app_paths::window_index_path(app_id, peer_id);
    let (index, outcome) = load(peers, peer_id, &path).await;
    if outcome.authorizes_sweep() {
        let swept = sweep_unreachable(peers, peer_id, &index, app_id).await;
        let floor = index.next_id_floor();
        return Resolution {
            index,
            outcome,
            floor,
            swept,
        };
    }
    // No index we can vouch for: claim nothing, delete nothing — but still
    // refuse to hand out an id some persisted slot is sitting on. The listing
    // is the only authority left, and this is what carries an upgrading profile
    // (whose `windows/` is full and whose index does not exist yet) through its
    // one transitional boot without landing a new window on old state.
    let prefix = crate::app_paths::windows_prefix(app_id, peer_id);
    let floor = match peers.tree_listing_async(peer_id, &prefix).await {
        Ok(listing) => {
            let paths: Vec<String> = listing.into_iter().map(|e| e.path).collect();
            ids_in_listing(&paths, &prefix)
                .into_iter()
                .max()
                .map_or(1, |m| m + 1)
        }
        // Neither authority answered. The floor stays at 1 — which is exactly
        // the behaviour that shipped before this module existed, so it is a
        // known limit rather than a regression, and AP42's type guard is what
        // stands behind it.
        Err(_) => 1,
    };
    Resolution {
        index,
        outcome,
        floor,
        swept: 0,
    }
}

/// Extract the window ids a listing of `workspace/windows/` covers.
///
/// Pulled out as a free function so it is testable without a store: the parsing
/// is where this goes wrong, and a listing shape that stopped matching would
/// silently sweep **nothing** (or, far worse, mis-parse and sweep something
/// live). Deliberately tolerant about what follows the id — `{id}/state`,
/// `{id}/results` and any future sibling all name the same window.
pub fn ids_in_listing(paths: &[String], prefix: &str) -> std::collections::BTreeSet<WindowId> {
    let mut out = std::collections::BTreeSet::new();
    for p in paths {
        let Some(rest) = p.strip_prefix(prefix) else {
            continue;
        };
        let head = rest.split('/').next().unwrap_or("");
        if let Ok(id) = head.parse::<WindowId>() {
            out.insert(id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: WindowId, type_name: &str, peer: &str) -> WindowIndexEntry {
        WindowIndexEntry {
            id,
            type_name: type_name.to_string(),
            peer_id: peer.to_string(),
        }
    }

    fn index(entries: Vec<WindowIndexEntry>) -> WindowIndex {
        WindowIndex { entries }
    }

    #[test]
    fn a_round_trip_preserves_every_entry() {
        let original = index(vec![
            entry(1, "Shell", "PEER1"),
            entry(4, "Knowledge Base", "PEER1"),
            entry(7, "Entity Browser", "PEER2"),
        ]);
        let decoded = WindowIndex::from_entity(&original.to_entity()).expect("decodes");
        assert_eq!(decoded, original);
    }

    #[test]
    fn an_empty_index_round_trips_as_empty_and_not_as_absent() {
        let decoded = WindowIndex::from_entity(&index(vec![]).to_entity()).expect("decodes");
        assert!(decoded.entries.is_empty());
    }

    /// AP42's habit, applied where the reuse hazard does not reach: decode by
    /// type, never by path. A window-state entity must not read as an index.
    #[test]
    fn another_types_entity_is_not_decoded_as_an_index() {
        let foreign = crate::views::chain_trace::model::ChainTraceState {
            chain_id: "chain-7".into(),
        }
        .to_entity();
        assert!(WindowIndex::from_entity(&foreign).is_none());
    }

    /// The distinction the whole sweep rests on: a row we cannot read makes the
    /// index `None` (→ `Malformed` → no sweep), **not** an empty index (which
    /// authorizes sweeping everything).
    #[test]
    fn a_malformed_row_fails_the_whole_index_rather_than_yielding_a_short_one() {
        let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![(
            entity_ecf::Value::Text("windows".into()),
            entity_ecf::Value::Array(vec![
                entity_ecf::Value::Map(vec![
                    (
                        entity_ecf::Value::Text("id".into()),
                        entity_ecf::Value::Integer(1i64.into()),
                    ),
                    (
                        entity_ecf::Value::Text("type".into()),
                        entity_ecf::text("Shell"),
                    ),
                    (
                        entity_ecf::Value::Text("peer".into()),
                        entity_ecf::text("PEER1"),
                    ),
                ]),
                // No `peer` field — a row we cannot place.
                entity_ecf::Value::Map(vec![(
                    entity_ecf::Value::Text("id".into()),
                    entity_ecf::Value::Integer(2i64.into()),
                )]),
            ]),
        )]));
        let entity = Entity::new(INDEX_TYPE, data).expect("well-formed");
        assert!(
            WindowIndex::from_entity(&entity).is_none(),
            "a partial index still authorizes a sweep, and a sweep driven by a \
             half-read list deletes live state"
        );
    }

    #[test]
    fn claims_are_keyed_by_type_and_peer() {
        let claims = index(vec![
            entry(1, "Shell", "PEER1"),
            entry(2, "Shell", "PEER2"),
            entry(3, "Knowledge Base", "PEER1"),
        ])
        .claims();
        assert_eq!(claims.get(&("Shell".into(), "PEER1".into())), Some(&1));
        assert_eq!(claims.get(&("Shell".into(), "PEER2".into())), Some(&2));
        assert_eq!(
            claims.get(&("Knowledge Base".into(), "PEER1".into())),
            Some(&3)
        );
    }

    /// The stated limit, pinned so it is a decision and not a surprise: two
    /// windows of one type on one peer collapse to one claim, and the first to
    /// re-open takes it.
    #[test]
    fn two_windows_of_one_type_on_one_peer_yield_one_claim_and_it_is_the_first() {
        let claims = index(vec![entry(2, "Shell", "PEER1"), entry(5, "Shell", "PEER1")]).claims();
        assert_eq!(claims.len(), 1);
        assert_eq!(
            claims.get(&("Shell".into(), "PEER1".into())),
            Some(&2),
            "the first entry wins, so the first re-opened window gets the first \
             instance's state rather than the second's"
        );
    }

    #[test]
    fn the_floor_clears_every_id_the_index_knows() {
        assert_eq!(
            index(vec![entry(3, "Shell", "P"), entry(9, "Storage", "P")]).next_id_floor(),
            10
        );
        assert_eq!(index(vec![]).next_id_floor(), 1, "an empty index floors at 1");
    }

    #[test]
    fn listing_ids_are_parsed_from_both_state_and_results_paths() {
        let prefix = "/PEER1/app/entity-browser/workspace/windows/";
        let ids = ids_in_listing(
            &[
                format!("{prefix}1/state"),
                format!("{prefix}1/results"),
                format!("{prefix}12/state"),
                format!("{prefix}notanumber/state"),
                "/PEER1/app/entity-browser/settings/ui".to_string(),
            ]
            .to_vec(),
            prefix,
        );
        assert_eq!(ids.into_iter().collect::<Vec<_>>(), vec![1, 12]);
    }

    /// Every outcome gets its own word, and the **count** is asserted so a
    /// fifth cannot quietly reuse one. Exactly the gate the `Hydration` audit
    /// landed on one layer down, for exactly the reason it landed on it.
    #[test]
    fn every_index_outcome_has_its_own_word() {
        let all = [
            IndexLoad::Restored(3),
            IndexLoad::NoIndex,
            IndexLoad::Malformed,
            IndexLoad::Unheard,
        ];
        let labels: std::collections::BTreeSet<&str> = all.iter().map(|o| o.label()).collect();
        assert_eq!(labels.len(), all.len(), "two outcomes share a label");
        assert_eq!(all.len(), 4, "a new outcome needs its own word and this count");
    }

    /// Only a read we can vouch for may delete anything — the three withholding
    /// outcomes are three different facts and one shared consequence.
    #[test]
    fn only_a_restored_index_authorizes_a_sweep() {
        assert!(IndexLoad::Restored(0).authorizes_sweep());
        assert!(!IndexLoad::NoIndex.authorizes_sweep());
        assert!(!IndexLoad::Malformed.authorizes_sweep());
        assert!(!IndexLoad::Unheard.authorizes_sweep());
    }
}

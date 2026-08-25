//! The cross-impl trust gate (TIER: integration — a real Direct peer,
//! the real entity-compute evaluator).
//!
//! Replays each fixture bundle through the full mount contract on the
//! Rust evaluator and asserts the per-tick state/port content hashes
//! equal the Go-side oracle, tick for tick, inputs included. Workbench's
//! anti-vacuity guard is inherited from the dump tool (a bundle whose
//! states don't evolve fails generation), and the differential is one we
//! have seen fail: corrupt one byte and the bundle import refuses
//! (`bundle::tests::corrupted_entity_is_refused`).
//!
//! A hash mismatch here means the two implementations disagree on
//! evaluation or canonical encoding — the exact divergence the browser
//! host must report loudly rather than assume away.

use crate::peers::Peers;
use crate::program_host::bundle::{digest_hex, digest_of_hash_string, Bundle, EMBEDDED_PROGRAMS};
use crate::program_host::descriptor::ProgramDescriptor;
use crate::program_host::host;

async fn run_oracle(key: &str) {
    let p = EMBEDDED_PROGRAMS.iter().find(|p| p.key == key).unwrap();
    let bundle = Bundle::parse(p.json).expect("bundle parses");
    let entities = bundle.verified_entities().expect("bundle hash-verifies");

    let peers = Peers::new_direct();
    let peer_id = peers.primary_peer_id().to_string();
    let ns = bundle.origin_peer.clone();

    // Materialize at the program's ORIGIN-namespace paths (the IR's
    // internal lookups are origin-qualified), then decode + seed.
    host::materialize_future(&peers, &peer_id, &ns, entities, |_, _| {})
        .await
        .expect("materialize");

    let desc_ent = peers
        .get_entity(&peer_id, &host::qualify(&ns, &bundle.descriptor_path))
        .expect("descriptor readable after materialize");
    let desc = ProgramDescriptor::decode(&desc_ent).expect("descriptor decodes");

    host::seed_future(&peers, &peer_id, &ns, &desc)
        .await
        .expect("seed");

    let state_path = host::qualify(&ns, &desc.state_path);
    let mut distinct = std::collections::HashSet::new();

    for (tick, oracle) in bundle.oracle.iter().enumerate() {
        // Scheduled input writes land BEFORE the tick runs (the dump
        // tool's replay order).
        for input in bundle.inputs.iter().filter(|i| i.tick == tick as u64) {
            let port = desc
                .input_ports
                .iter()
                .find(|p| p.name == input.port)
                .expect("input port exists");
            let entity =
                entity_entity::Entity::new(&input.entity_type, input.data.clone()).unwrap();
            host::input_future(&peers, &peer_id, &ns, &port.path, entity)
                .await
                .expect("input write");
        }

        host::tick_future(&peers, &peer_id, &ns, &desc)
            .await
            .unwrap_or_else(|e| panic!("{key}: tick {tick} faulted: {e}"));

        let state = peers
            .get_entity(&peer_id, &state_path)
            .expect("state readable");
        let ours = digest_hex(&state);
        let theirs = digest_of_hash_string(&oracle.state_hash).unwrap();
        assert_eq!(
            ours, theirs,
            "{key}: tick {tick}: state hash diverged from the Go oracle — \
             the two implementations do not agree on this program"
        );
        distinct.insert(ours);

        for (port_name, want) in &oracle.port_hashes {
            let port = desc
                .output_ports
                .iter()
                .find(|p| &p.name == port_name)
                .expect("oracle port exists in descriptor");
            let ent = peers
                .get_entity(&peer_id, &host::qualify(&ns, &port.path))
                .expect("port readable");
            assert_eq!(
                digest_hex(&ent),
                digest_of_hash_string(want).unwrap(),
                "{key}: tick {tick}: port {port_name} hash diverged"
            );
        }
    }

    // Anti-vacuity on OUR side too: agreement over a frozen program
    // proves nothing.
    assert!(
        distinct.len() >= bundle.oracle.len() / 2,
        "{key}: only {} distinct states across {} ticks — vacuous agreement",
        distinct.len(),
        bundle.oracle.len()
    );
}

#[tokio::test]
async fn life_matches_go_oracle_tick_for_tick() {
    run_oracle("life").await;
}

/// The interactive one, and the only fixture whose schedule drives **every
/// kind of control the standard controller has**: four axis bits (the cursor
/// d-pad), a momentary action that edits state under the cursor, one that
/// replaces the whole board, and one that toggles the paused flag.
///
/// That breadth is the point. `life` and `snake` bind no key-set at all and
/// `asteroids` predates control roles, so until this fixture the role parser
/// (`controls.rs`) had unit tests over hand-written keymaps and **no authored
/// program that exercised it** — the shape this repo keeps meeting, where the
/// mechanism is right and nothing drives it.
///
/// It is also the strictest agreement we ask of the two evaluators. The step
/// resolves a priority ladder (regen > toggle > paused-hold > the B3/S23 rule)
/// against edge-detected keys, so a divergence in *when* a press is observed —
/// not just in the automaton — shows up as a state-hash mismatch.
#[tokio::test]
async fn interactive_life_matches_go_oracle_across_every_control() {
    run_oracle("life-edit").await;
}

#[tokio::test]
async fn snake_matches_go_oracle_with_input_schedule() {
    run_oracle("snake").await;
}

#[tokio::test]
async fn asteroids_matches_go_oracle_with_key_schedule() {
    run_oracle("asteroids").await;
}

/// Restart semantics: reseed returns the program to state₀ — the mounted
/// tick sequence replayed from a reseed matches the oracle from tick 0
/// (determinism is the property save-state/replay rides on).
#[tokio::test]
async fn reseed_replays_deterministically() {
    let p = EMBEDDED_PROGRAMS.iter().find(|p| p.key == "life").unwrap();
    let bundle = Bundle::parse(p.json).unwrap();
    let entities = bundle.verified_entities().unwrap();
    let peers = Peers::new_direct();
    let peer_id = peers.primary_peer_id().to_string();
    let ns = bundle.origin_peer.clone();
    host::materialize_future(&peers, &peer_id, &ns, entities, |_, _| {})
        .await
        .unwrap();
    let desc_ent = peers
        .get_entity(&peer_id, &host::qualify(&ns, &bundle.descriptor_path))
        .unwrap();
    let desc = ProgramDescriptor::decode(&desc_ent).unwrap();

    for round in 0..2 {
        host::seed_future(&peers, &peer_id, &ns, &desc).await.unwrap();
        for (tick, oracle) in bundle.oracle.iter().take(4).enumerate() {
            host::tick_future(&peers, &peer_id, &ns, &desc).await.unwrap();
            let state = peers
                .get_entity(&peer_id, &host::qualify(&ns, &desc.state_path))
                .unwrap();
            assert_eq!(
                digest_hex(&state),
                digest_of_hash_string(&oracle.state_hash).unwrap(),
                "round {round} tick {tick} diverged after reseed"
            );
        }
    }
}

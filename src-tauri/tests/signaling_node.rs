//! The desktop backend really is a rendezvous — two peers meet through it.
//!
//! `signaling_node.rs`'s unit tests check the *shape* of the mount (the grant is
//! narrow, no reflectors are claimed). They cannot tell you whether a peer that
//! has never met this node can actually deposit an offer and have another peer
//! collect it, which is the entire feature. This does.
//!
//! # Why every test here runs ENFORCED
//!
//! The desktop default is `debug_open_grants` (`ENTITY_BROWSER_ENFORCE` unset),
//! which authorizes everything — so a node mounted with **no seed policy at
//! all** passes a naive round-trip, and would 403 the day anyone enforces. That
//! is not hypothetical: `signaling_seed_grants`' own doc records the extension
//! shipping exactly that bug, because *"the in-process live tests seed a
//! wildcard, which authorizes everything and so proves nothing about
//! admission"*. Running enforced is what makes these tests evidence.
//!
//! Mutation-checked: drop `.with_seed_policy(mount.seed_policy)` from
//! `spawn_node` and `two_peers_that_have_never_met_rendezvous_through_the_desktop_backend`
//! fails at the offer with a 403.

use std::sync::Arc;

use entity_peer::carrier::PeerCarrier;
use entity_peer::transport::{Connector, Listener, WebSocketConnector, WebSocketListener};
use entity_peer::{PeerBuilder, PeerConfig};
use entity_signaling::punch::Carrier;
use entity_signaling::RendezvousKey;

/// Stand up a backend peer mounting the rendezvous exactly the way
/// `start_backend_peer` does: bind first (so the endpoint is dialable), then
/// build with both halves of the mount decision.
///
/// `seed` distinguishes identities; `serve` is the toggle under test.
async fn spawn_node(seed: u8, serve: bool) -> (String, String, tokio::task::JoinHandle<()>) {
    let keypair = entity_crypto::Keypair::from_seed([seed; 32]);
    let peer_id = keypair.peer_id().to_string();

    // Bind BEFORE building — the endpoint the node advertises has to be the one
    // it actually listens on, and that is not known until the port is chosen.
    let listener = WebSocketListener::bind("127.0.0.1:0")
        .await
        .expect("node binds");
    let ws_addr = listener.local_addr();

    let mut builder = PeerBuilder::new().keypair(keypair).config(PeerConfig {
        // ENFORCED. See the module header — the open posture would make this
        // test pass with the admission grant deleted.
        debug_open_grants: false,
        ..PeerConfig::default()
    });

    if serve {
        let mount = entity_browser_tauri::signaling_node::mount(&ws_addr, &peer_id);
        builder = builder
            .with_seed_policy(mount.seed_policy)
            .handler(mount.handler);
    }

    let peer = builder.build().expect("node builds");
    let shared = peer.shared();
    peer.start_engines(&shared);
    let handle = tokio::spawn(async move {
        let _ = entity_peer::server::run(listener, shared).await;
    });
    // Keep the peer alive for the life of the listener task.
    std::mem::forget(peer);
    (peer_id, ws_addr, handle)
}

fn carrier_for(node_id: &str, node_addr: &str, seed: u8) -> PeerCarrier {
    PeerCarrier::new(
        node_id.to_string(),
        node_addr.to_string(),
        entity_crypto::IdentityKeypair::Ed25519(entity_crypto::Keypair::from_seed([seed; 32])),
        Arc::new(WebSocketConnector) as Arc<dyn Connector>,
        entity_hash::HASH_ALGORITHM_SHA256,
    )
}

/// A rendezvous key is [`RENDEZVOUS_KEY_LEN`] opaque bytes to the node — it
/// derives nothing and checks only the length (§1), so any distinct value
/// stands in for a real §2.2 derivation here.
///
/// Taken from the constant rather than written out: the width is **33**, not
/// the 32 a reader assumes from "it's a hash", and hard-coding the guess is how
/// this test first failed.
fn key(byte: u8) -> RendezvousKey {
    RendezvousKey::from_slice(&[byte; entity_signaling::RENDEZVOUS_KEY_LEN])
        .expect("RENDEZVOUS_KEY_LEN is the key width, by construction")
}

/// **The feature.** Two peers that have never met, and have no relationship
/// with the desktop backend beyond being able to reach its listener, exchange a
/// blob through it.
///
/// This is what a browser pair does on a LAN: the media rides host candidates
/// with zero reflectors (`make e2e-webrtc-lan`), but the offer still has to
/// reach the other side, and nothing but a rendezvous can carry it.
///
/// Note the two peers are **not** the node and are not known to it — the
/// `default` seed pattern is load-bearing precisely because peers that have
/// never met cannot be on an allow-list.
#[tokio::test]
async fn two_peers_that_have_never_met_rendezvous_through_the_desktop_backend() {
    let (node_id, node_addr, handle) = spawn_node(11, true).await;

    let alice = carrier_for(&node_id, &node_addr, 21);
    let bob = carrier_for(&node_id, &node_addr, 22);
    let k = key(7);

    alice
        .offer(&k, b"alice-was-here".to_vec())
        .await
        .expect("a peer the node has never met may deposit at a rendezvous key");

    let collected = bob
        .collect(&k)
        .await
        .expect("and another may collect from it");

    assert_eq!(
        collected.len(),
        1,
        "exactly the one deposit should be at this key, got {collected:?}"
    );
    assert_eq!(
        collected[0], b"alice-was-here",
        "the bytes must cross unchanged — the node is a bulletin board, not a participant"
    );

    handle.abort();
}

/// The bucket is **keyed**, not global. Two conversations through one desktop
/// node must not see each other's offers — otherwise a busy node would hand
/// every pair everyone else's handshakes, which is a privacy leak that no
/// signature check would catch (the blobs are real, just not yours).
#[tokio::test]
async fn deposits_at_different_keys_do_not_leak_into_each_other() {
    let (node_id, node_addr, handle) = spawn_node(12, true).await;

    let alice = carrier_for(&node_id, &node_addr, 31);
    let bob = carrier_for(&node_id, &node_addr, 32);

    alice.offer(&key(1), b"for-key-one".to_vec()).await.unwrap();
    bob.offer(&key(2), b"for-key-two".to_vec()).await.unwrap();

    let one = alice.collect(&key(1)).await.unwrap();
    let two = bob.collect(&key(2)).await.unwrap();

    assert_eq!(one, vec![b"for-key-one".to_vec()]);
    assert_eq!(two, vec![b"for-key-two".to_vec()]);

    let empty = alice.collect(&key(99)).await.unwrap();
    assert!(
        empty.is_empty(),
        "a key nobody deposited at is empty, not an error and not someone else's: {empty:?}"
    );

    handle.abort();
}

/// **Off means off**, and it must fail as *no such handler*, not as a silent
/// empty bucket.
///
/// The distinction is the whole reason this test exists: a node that accepted
/// the deposit and returned nothing would leave a browser waiting on a
/// rendezvous that will never complete, which presents as "that peer is
/// offline" — the failure mode this whole arc is trying to stop producing.
#[tokio::test]
async fn a_backend_with_the_rendezvous_off_refuses_rather_than_silently_swallowing() {
    let (node_id, node_addr, handle) = spawn_node(13, false).await;

    let alice = carrier_for(&node_id, &node_addr, 41);
    let result = alice.offer(&key(7), b"nobody-is-listening".to_vec()).await;

    assert!(
        result.is_err(),
        "a backend not serving rendezvous must refuse the deposit, not accept it \
         into a void — got {result:?}"
    );

    handle.abort();
}

/// What the node publishes about itself is what a peer needs to derive a
/// rendezvous key that other peers will actually use, and to size its deposits.
///
/// The endpoint assertion is the one that catches a real regression: build the
/// peer before binding and this becomes `0.0.0.0`, which every peer would
/// faithfully store and none could dial.
#[tokio::test]
async fn the_node_advertises_the_address_it_actually_listens_on() {
    let (node_id, node_addr, handle) = spawn_node(14, true).await;

    let mount = entity_browser_tauri::signaling_node::mount(&node_addr, &node_id);
    let ad = mount.handler.core().advertise();

    assert_eq!(
        ad.endpoint, node_addr,
        "advertise publishes the endpoint verbatim for peers to dial"
    );
    assert!(
        !ad.endpoint.contains("0.0.0.0"),
        "a node advertising the wildcard bind has published an address nobody can reach: {}",
        ad.endpoint
    );
    assert!(
        ad.reflection_endpoints.is_empty(),
        "we run no §9.3 STUN listener, so we must claim none: {:?}",
        ad.reflection_endpoints
    );
    // Limits come from the §5 pins, not from anything we invented.
    assert!(ad.limits.max_blob_bytes > 0);
    assert!(ad.limits.ttl_seconds > 0);

    handle.abort();
}

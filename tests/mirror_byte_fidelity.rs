//! **Arch's §13 item 1, run** — can a real store re-serve *another peer's exact
//! encoding* without normalizing it?
//!
//! The mirror model in `PROPOSAL-APP-CONVENTION-FEED` rests entirely on one
//! rule: **republish the original bytes.** A re-encoded entry no longer matches
//! its author's content hash, which turns a mirror from evidence into hearsay
//! and takes the whole assembly layer with it. Arch asked for this to be
//! falsified before anyone reads the schemas, on the grounds that a negative
//! result means rework rather than a caveat. This is that measurement.
//!
//! ## What makes this a test and not a tautology
//!
//! Round-tripping bytes *we* authored proves nothing — our encoder produced
//! them, so of course it reproduces them. **The subject is a foreign
//! publisher's encoding**, which we do not control and which may be valid CBOR
//! that our canonical encoder would never emit.
//!
//! So the payload here is deliberately **non-minimal CBOR**: the integer `1`
//! written in the `uint8` form (`0x18 0x01`) where a canonical encoder emits
//! the immediate form (`0x01`). Both are legal CBOR for the same value; only
//! one is what `to_ecf` produces. `a_normalizing_path_would_destroy_this_entity`
//! is the anti-vacuity guard — it asserts the payload really is non-canonical
//! and really would be broken by a normalizing round-trip, so a green result in
//! the other tests means byte fidelity and not a lucky choice of fixture.
//!
//! ## Why hashes are compared on `data`, never with `==`
//!
//! `impl PartialEq for Entity` compares **`content_hash` only**
//! (`core/entity/src/lib.rs`). For every other purpose that is right and for
//! this one it is exactly the wrong granularity — it would hide the difference
//! this file exists to detect. Every assertion below compares raw bytes.
//!
//! ## The second question, which arch did not ask and the model needs
//!
//! `an_entity_carries_no_signer` records what a mirrored entry can and cannot
//! prove on its own. Verification in this substrate is **root-anchored**: an
//! entity is authentic because it is reachable from its author's signed root
//! (`SignedSession::resolve` walks the trie and treats a miss as the origin
//! failing its own committed closure), and an `Entity` is three fields — type,
//! data, hash — with no signer among them. So *republishing the bytes* gets
//! integrity for free and **does not carry authorship**. That is a real
//! constraint on the mirror shape and it is stated here rather than discovered
//! later.

use std::sync::Arc;

use entity_crypto::Keypair;
use entity_ecf::{to_ecf, Value};
use entity_entity::Entity;
use entity_peer::PeerBuilder;
use entity_store::{MemoryContentStore, MemoryLocationIndex};

const FOREIGN_TYPE: &str = "app/feed/entry";

/// A foreign publisher's `data`: `{"n": 1}` with the integer in **non-minimal**
/// `uint8` form. Legal CBOR, and not what `to_ecf` emits.
///
/// `0xA1`      map, 1 pair
/// `0x61 0x6E` text(1) "n"
/// `0x18 0x01` unsigned, uint8 follows, value 1   ← canonical form is `0x01`
fn foreign_data() -> Vec<u8> {
    vec![0xA1, 0x61, 0x6E, 0x18, 0x01]
}

/// What our own canonical encoder produces for the same logical value.
fn canonical_data() -> Vec<u8> {
    to_ecf(&Value::Map(vec![(
        Value::Text("n".into()),
        Value::Integer(1u8.into()),
    )]))
}

fn store_and_read_back(entity: Entity) -> Entity {
    let peer = PeerBuilder::new()
        .keypair(Keypair::from_seed([7u8; 32]))
        .content_store(Arc::new(MemoryContentStore::new()))
        .location_index(Arc::new(MemoryLocationIndex::new()))
        .build()
        .expect("peer builds");
    let hash = peer.content_store().put(entity).expect("store accepts the foreign entity");
    peer.content_store().get(&hash).expect("the stored entity reads back")
}

/// **The anti-vacuity guard.** If this fails, the fixture is canonical after all
/// and every other test in this file is measuring nothing.
#[test]
fn a_normalizing_path_would_destroy_this_entity() {
    let foreign = foreign_data();
    let canonical = canonical_data();

    assert_ne!(
        foreign, canonical,
        "the fixture must be NON-canonical or this file proves nothing — \
         foreign={foreign:02x?} canonical={canonical:02x?}"
    );

    // And the difference is not cosmetic: it moves the content hash, which is
    // what a mirror's whole claim rests on.
    let authored = Entity::new(FOREIGN_TYPE, foreign).expect("foreign entity");
    let normalized = Entity::new(FOREIGN_TYPE, canonical).expect("normalized entity");
    assert_ne!(
        authored.content_hash, normalized.content_hash,
        "a normalizing re-encode must move the hash, or there is nothing to protect"
    );
}

/// **Arch's question, answered.** Foreign bytes in, through a real content
/// store, out through the exact call the publisher emits with.
#[test]
fn a_real_store_re_serves_a_foreign_encoding_byte_for_byte() {
    let foreign = foreign_data();
    let authored = Entity::new(FOREIGN_TYPE, foreign.clone()).expect("foreign entity");
    let authored_hash = authored.content_hash;

    // The wire hop: this is how the bytes would actually arrive from the
    // author's origin.
    let on_the_wire = entity_wire::encode_entity(&authored);
    let received = entity_wire::decode_entity(&on_the_wire).expect("decodes as it arrived");
    assert_eq!(
        received.data, foreign,
        "the wire codec must not normalize a foreign payload"
    );

    // The durable hop: a real store, put and get.
    let read_back = store_and_read_back(received);
    assert_eq!(
        read_back.data, foreign,
        "a real content store must hold the foreign payload verbatim"
    );
    assert_eq!(read_back.content_hash, authored_hash, "the hash must not move");

    // The emit hop: `RootProjector::finish` writes every blob with exactly this
    // call (`signed_root.rs` — `write_blob(base, h, &ecf_for_hash(&e.entity_type, &e.data))`).
    let emitted = entity_ecf::ecf_for_hash(&read_back.entity_type, &read_back.data);
    let as_the_author_emitted_it = entity_ecf::ecf_for_hash(FOREIGN_TYPE, &foreign);
    assert_eq!(
        emitted, as_the_author_emitted_it,
        "republishing must reproduce the author's bytes exactly"
    );

    // And the round-tripped entity still proves itself.
    read_back.validate().expect("the foreign entity still validates after the round trip");
}

/// The property the above rests on, isolated: the hash envelope embeds `data`
/// rather than re-encoding it, so a payload our encoder would never produce
/// still survives.
#[test]
fn the_hash_envelope_embeds_data_rather_than_re_encoding_it() {
    let foreign = foreign_data();
    let envelope = entity_ecf::ecf_for_hash(FOREIGN_TYPE, &foreign);

    let found = envelope
        .windows(foreign.len())
        .any(|w| w == foreign.as_slice());
    assert!(
        found,
        "the foreign payload must appear verbatim inside the hash envelope; \
         envelope={envelope:02x?}"
    );

    assert!(
        !envelope.windows(canonical_data().len()).any(|w| w == canonical_data().as_slice()),
        "the canonical form must NOT appear — its presence would mean a silent re-encode"
    );
}

/// **What republication does not buy.** Integrity travels with the bytes;
/// authorship does not. Recorded so the mirror shape is designed against the
/// substrate that exists rather than against per-entry signatures, which this
/// model does not have.
#[test]
fn an_entity_carries_no_signer() {
    let authored =
        Entity::new(FOREIGN_TYPE, foreign_data()).expect("foreign entity");

    // `validate()` is the strongest self-contained claim an entity can make,
    // and it is a claim about BYTES, not about a person: it recomputes the hash
    // over (type, data).
    authored.validate().expect("integrity is self-contained");

    // Integrity is genuinely checked — a tampered copy fails.
    let mut tampered_bytes = foreign_data();
    tampered_bytes[4] = 0x02;
    let tampered = Entity::new(FOREIGN_TYPE, tampered_bytes).expect("tampered entity");
    assert_ne!(
        tampered.content_hash, authored.content_hash,
        "a changed byte must change the hash"
    );

    // But nothing in the entity says WHO. An `Entity` is (type, data, hash);
    // authorship in this substrate comes from being reachable inside a peer's
    // SIGNED ROOT, which a lifted entity no longer sits under. So a mirror that
    // republishes bytes alone hands the reader integrity and not provenance —
    // the author's root and the trie path to the entry are what close that gap,
    // and they are extra bytes a mirror must deliberately carry.
    let restated = Entity::new(&authored.entity_type, authored.data.clone()).unwrap();
    assert_eq!(
        restated.content_hash, authored.content_hash,
        "anyone holding the bytes can reconstruct the entity identically — \
         which is exactly why the bytes alone cannot establish who wrote them"
    );
}

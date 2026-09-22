//! **`G-PIN-4` — one fixture, two publishers, identical site root.**
//!
//! `APP-CONVENTION-SEMANTIC-CONTENT-SITE` §9 lists a *reproducible-publish*
//! test as `[REQUIRED before ratification]`, and §2 pins what makes it possible:
//! *"Two peers with identical content under different prefixes produce the same
//! trie root hash"* (`EXTENSION-TREE` §3.2). The joint v1 lock signal is
//! cross-impl byte-equality on these cases, and §2 is explicit that
//! **"independent means built over different cores — two front ends linking the
//! same core cannot disagree about it and do not constitute two."**
//!
//! This file is the first half of that, run **unilaterally**, with no
//! coordination and no rig: `entity-workbench-go` already vendored a
//! deterministic emission of its publisher into this tree
//! (`tests/fixtures/crossimpl-go-site/`, over **`entity-core-go`**), and we
//! re-author the same four page entities over **`entity-core-rust`** and build
//! the trie ourselves. Two implementations, two cores, one binding set, one root
//! hash compared.
//!
//! ## What this establishes, stated at its real size
//!
//! - **The substrate agrees byte-for-byte.** Canonical ECF over the authored
//!   `(type, data)` produces the same content hash on both sides, and the
//!   `EXTENSION-TREE` v4.0.2 HAMT produces the same root over the same bindings.
//!   That is the `G-PIN-4` property, measured across independent cores.
//! - **It does NOT establish the site vocabulary.** Their fixture's leaves are
//!   `test/note`, not `app/site-manifest` / `app/site-page`. The convention's
//!   *format* half needs a fixture carrying the real app-tier types, and that is
//!   what the joint run with `entity-workbench-go` has to produce
//!   (`TRACKER-entity-workbench-go.md` `J-1`). Do not read a green here as §9
//!   discharged.
//!
//! ## The trap this file exists to keep the next author out of
//!
//! **The comparable artifact is the TRIE ROOT, never the published-root head.**
//! `system/peer/published-root` carries `published_at` — a wall-clock read
//! (`core/peer/src/published_root.rs`, `now_ms()`) — so its content hash, the
//! `system/signature/{head}.bin` binding named after it and two content shards
//! all move on every run. Measured here, not inferred: two `make site` runs of
//! one fixture under one pinned `--identity-seed` produced heads
//! `008615f3b44c09c7…` and `00a7b337ea9fd493…` with **`root_hash` identical**
//! (`00bc252f2a685c4a…`) and 13 of 15 content blobs shared. A `G-PIN-4` rig
//! that compares the head — which is what `make site-dist` prints under the
//! words *"signed root"* — reds one hundred percent of the time, and reds
//! **more** across two impls, where it would look like a real divergence.
//!
//! `EXTENSION-TREE` §3.2's determinism rule 3 says the same thing from the other
//! end: *"No timestamp — a snapshot is pure structural data."* The snapshot is
//! the comparand precisely because it is the artifact with no clock in it.
//!
//! ## Why the key sets can differ and the roots still must not
//!
//! Their site subgraph sits under `content/sites/{site_id}/`; ours sits under
//! `sites/{site_id}/`. Both are conformant — §2's v0.5 placement ruling makes a
//! site *"a free subgraph at any publisher-chosen tree path"* — and it does not
//! matter here, because a snapshot's bindings are keyed **relative to the
//! prefix** (§3.2: *"snapshot of `/alice_id/local/files/` and
//! `/bob_id/local/files/` both produce bindings keyed by the same relative
//! paths"*). Placement and peer-id fall out of the comparison by construction,
//! which is why `G-PIN-4` needs neither a shared keypair nor an agreed path.

#![cfg(all(test, not(target_arch = "wasm32")))]

use std::collections::BTreeMap;
use std::path::PathBuf;

use entity_ecf::{to_ecf, Value};
use entity_entity::Entity;
use entity_hash::Hash;
use entity_store::{ContentStore, MemoryContentStore};

/// The publisher of the vendored fixture. `entity-workbench-go` at `d940ce0`
/// over `entity-core-go` `7593618`; see [`super::crossimpl_go`] for how it is
/// re-cut.
const GO_PEER_ID: &str = "2KLv2nhwtPrLFd4BZFQuNK1ujtE74q8cVg7y8cYdcZZ5BL";

/// The CHAMP trie root their publisher signed. Pinned as a constant **and**
/// read back out of their bytes below: the constant is what a reader of this
/// file can check by eye, the read is what couples the gate to the artifact, and
/// asserting they agree is what makes a silently re-cut fixture fail loudly
/// instead of passing against a new root.
const GO_TRIE_ROOT_HEX: &str =
    "00f567bfbd1bb19b89a9d37881352fed1ac13278860f63c1320b9eec29f4503b89";

/// **The fixture, as authored inputs.** `(relative key, entity type, body)` —
/// the logical content a publisher is handed, *not* bytes copied out of their
/// emission. Everything downstream (the ECF encoding, the content hash, the key
/// hash, the bucket placement, the root) is recomputed here by our code; if any
/// of it were lifted, the comparison would be us agreeing with ourselves.
///
/// The keys are relative to their declared §3.3a prefix (`docs/`), which is what
/// the trie is actually keyed by. Two levels of nesting are deliberate on their
/// side — a flat site can pass a walk that a nested one fails.
const FIXTURE: [(&str, &str, &str); 4] = [
    ("index", "test/note", "# Home\n\nauthored bytes from the Go arm\n"),
    ("intro", "test/note", "# Intro\n\nsecond page\n"),
    ("deep/one", "test/note", "# Deep\n\nnested one level\n"),
    ("deep/two/leaf", "test/note", "# Leaf\n\nnested two levels\n"),
];

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/crossimpl-go-site")
}

/// A wire-hex hash (leading format byte + digest) as the content shard spells
/// it in a filename. `Hash::from_display` wants a `format:hex` tag we do not
/// have here, so the bytes are parsed and handed to `from_bytes`, which is the
/// same path a fetched pointer takes.
fn hash_from_wire_hex(hex: &str) -> Option<Hash> {
    if hex.len() % 2 != 0 {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok())
        .collect();
    Hash::from_bytes(&bytes?).ok()
}

/// Load every blob in the fixture's `content/` shard into a store, keyed by the
/// hash its own path spells.
///
/// Deliberately **not** trusting the filename: each body is decoded and its
/// hash recomputed by `Entity::new`, so a shard whose name and contents
/// disagree is a failure here rather than a silently wrong operand three
/// assertions later.
fn their_store() -> MemoryContentStore {
    let store = MemoryContentStore::new();
    let root = fixture_dir().join("content");
    let mut loaded = 0usize;
    for aa in std::fs::read_dir(&root).expect("the fixture has a content/ shard").flatten() {
        for bb in std::fs::read_dir(aa.path()).into_iter().flatten().flatten() {
            for f in std::fs::read_dir(bb.path()).into_iter().flatten().flatten() {
                let named = f.file_name().to_string_lossy().to_string();
                let bytes = std::fs::read(f.path()).expect("a fixture blob reads");
                let expected = hash_from_wire_hex(&named)
                    .unwrap_or_else(|| panic!("fixture blob {named} is not a wire-hex address"));
                // `verify_and_decode` re-hashes the body and refuses a mismatch,
                // so a shard whose name and contents disagree fails here rather
                // than becoming a silently wrong operand three assertions later.
                let ent = super::http_poll::verify_and_decode(&bytes, &expected)
                    .unwrap_or_else(|e| panic!("fixture blob {named} does not verify: {e:?}"));
                store.put(ent).expect("store a fixture blob");
                loaded += 1;
            }
        }
    }
    assert!(loaded >= 6, "the fixture shard came back near-empty ({loaded} blobs) — a vacuous run");
    store
}

/// Their signed root, read out of the fixture's own bytes: the head entity, the
/// trie root it commits to, its declared §3.3a prefix and its publish instant.
fn their_signed_root(store: &MemoryContentStore) -> (Entity, Hash, String, u64) {
    let head_ptr = fixture_dir()
        .join(GO_PEER_ID)
        .join("system/peer/published-root.bin");
    let ptr = std::fs::read(&head_ptr).expect("the fixture has a published-root pointer");
    // A `.bin` leaf is the bare 2-key `system/hash` pointer, not the 3-key wire
    // entity — `crack_pointer` is the reader the product uses for the same hop.
    let head_hash =
        super::http_poll::crack_pointer(&ptr).expect("their published-root pointer cracks");
    let manifest = store.get(&head_hash).expect("their head is in their own shard");
    let data = entity_types::PublishedRootData::from_entity(&manifest)
        .expect("their head decodes as a published-root");
    (manifest, data.root_hash, data.prefix, data.published_at)
}

/// Author one fixture page with **our** encoder, exactly as a publisher would.
fn author(entity_type: &str, body: &str) -> Entity {
    let data = to_ecf(&Value::Map(vec![(
        Value::Text("body".into()),
        Value::Text(body.into()),
    )]));
    Entity::new(entity_type, data).expect("the authored page is a well-formed entity")
}

/// Our publisher's binding set for the fixture: relative key → content hash.
fn our_bindings() -> BTreeMap<String, Hash> {
    FIXTURE
        .iter()
        .map(|(key, ty, body)| ((*key).to_string(), author(ty, body).content_hash))
        .collect()
}

// ---------------------------------------------------------------------------
// The gates
// ---------------------------------------------------------------------------

/// **Entity level.** Every page we author from the fixture inputs hashes to the
/// hash their publisher bound at the same key.
///
/// This is the half that fails first and the half that says *what* diverged, so
/// it runs before the root comparison: a root mismatch with no per-key
/// attribution is a red that costs a session to localize.
///
/// It also pins the direction of the claim — this is canonical ECF agreeing
/// across two independent cores over the authored `(type, data)`, which is
/// `ENTITY-CBOR-ENCODING` §4.1's whole contract and the precondition for every
/// other cross-impl hash claim in the ecosystem.
#[test]
fn every_authored_page_hashes_to_what_the_go_publisher_bound() {
    let store = their_store();
    let (_, root, prefix, published_at) = their_signed_root(&store);
    let theirs = entity_tree::trie::collect_all_bindings(&store, root, "");
    let ours = our_bindings();

    eprintln!(
        "cross-impl fixture: prefix={prefix:?} published_at={published_at} \
         root={} keys={}",
        root.to_hex(),
        theirs.len()
    );

    let mut divergent = Vec::new();
    for (key, our_hash) in &ours {
        match theirs.get(key) {
            None => divergent.push(format!(
                "  {key:?} — we bound {}, their trie has no such key",
                our_hash.to_hex()
            )),
            Some(their_hash) if their_hash != our_hash => divergent.push(format!(
                "  {key:?} — ours {}, theirs {}",
                our_hash.to_hex(),
                their_hash.to_hex()
            )),
            Some(_) => {}
        }
    }
    for key in theirs.keys() {
        if !ours.contains_key(key) {
            divergent.push(format!("  {key:?} — in their trie, absent from ours"));
        }
    }

    assert!(
        divergent.is_empty(),
        "the two publishers disagree about {} of {} bindings:\n{}",
        divergent.len(),
        theirs.len().max(ours.len()),
        divergent.join("\n")
    );
    assert_eq!(
        ours.len(),
        FIXTURE.len(),
        "the authored set shrank — a fixture row was dropped rather than compared"
    );
}

/// **`G-PIN-4`.** One fixture, two publishers over two independent cores,
/// identical site root.
///
/// The trie is built here from *our* bindings by *our* HAMT, and compared with
/// the root **their** signed root commits to. Nothing on our side of the
/// comparison came out of their bytes.
#[test]
fn the_site_root_is_identical_across_two_independent_publishers() {
    let theirs = their_store();
    let (_, their_root, their_prefix, _) = their_signed_root(&theirs);
    assert_eq!(
        their_root.to_hex(),
        GO_TRIE_ROOT_HEX,
        "the vendored fixture was re-cut and now commits to a different root — \
         re-derive the expectation from their emission rather than editing this constant blind"
    );

    let ours = MemoryContentStore::new();
    let our_root = entity_tree::trie::build_trie(&ours, &our_bindings())
        .expect("our HAMT builds over the fixture bindings");

    assert_eq!(
        our_root.to_hex(),
        their_root.to_hex(),
        "G-PIN-4 FAILED: two publishers, one fixture, two roots.\n  \
         ours   {}\n  theirs {} (declared prefix {their_prefix:?})\n  \
         Check the per-key gate first — it names the entity that diverged.",
        our_root.to_hex(),
        their_root.to_hex()
    );

    eprintln!(
        "G-PIN-4 PASS: {} bindings → root {} — entity-core-rust and entity-core-go agree byte-for-byte",
        FIXTURE.len(),
        our_root.to_hex()
    );
}

/// **The falsifier, as a test rather than as a claim in a comment.**
///
/// A root comparison between two stores is worth nothing unless a real
/// difference moves it — a rig that quietly authored the wrong thing, or built
/// over an empty binding set, would satisfy the gate above if the machinery were
/// insensitive. One byte changed in one body must move the root, and the empty
/// binding set must not collide with the real one.
///
/// Both directions are checked because they fail differently: an insensitive
/// hash gives the first, and a `build_trie` that silently ignored its operand
/// gives the second.
#[test]
fn one_changed_byte_moves_the_site_root() {
    let store = MemoryContentStore::new();
    let baseline = entity_tree::trie::build_trie(&store, &our_bindings()).expect("baseline builds");

    let mut perturbed = our_bindings();
    let (key, ty, body) = FIXTURE[0];
    perturbed.insert(key.to_string(), author(ty, &format!("{body}x")).content_hash);
    let moved = entity_tree::trie::build_trie(&store, &perturbed).expect("perturbed builds");
    assert_ne!(
        baseline.to_hex(),
        moved.to_hex(),
        "a changed page body left the site root where it was — the comparison in \
         `the_site_root_is_identical_across_two_independent_publishers` measures nothing"
    );

    let empty = entity_tree::trie::build_trie(&store, &BTreeMap::new()).expect("empty builds");
    assert_ne!(
        baseline.to_hex(),
        empty.to_hex(),
        "the fixture's root equals the EMPTY root — the bindings never reached the trie"
    );
}

/// **The published-root head is NOT the comparand, and this pins why.**
///
/// Their head and their trie root are different hashes over different preimages,
/// and only one of the two has a clock in it. A future author reaching for
/// *"compare the signed root"* — the words `make site-dist` prints — lands on the
/// head; this test is the thing that tells them what they would be comparing.
///
/// Stated as a fact about **their** artifact, which is all this file can see:
/// their `published_at` is a real instant they pinned deliberately
/// (`FixtureInstant`), which is exactly why their fixture is vendorable at all.
/// Ours is a live `now_ms()` with no seam — measured, and routed to
/// `entity-core-rust` rather than worked around here.
#[test]
fn the_head_carries_a_clock_and_the_trie_root_does_not() {
    let store = their_store();
    let (manifest, root, _, published_at) = their_signed_root(&store);

    assert_ne!(
        manifest.content_hash.to_hex(),
        root.to_hex(),
        "the head and the trie root are the same hash — this file's premise is wrong"
    );
    assert!(
        published_at > 0,
        "their head carries no timestamp, so the trap this test documents does not exist"
    );
    // The trie root's preimage is a `system/tree/snapshot/node`; the head's is a
    // `system/peer/published-root`. Different types, so no reader can confuse
    // them by accident — only a rig comparing hexes can.
    assert_eq!(manifest.entity_type, "system/peer/published-root");
    assert_eq!(
        store.get(&root).expect("their trie root node is in their shard").entity_type,
        "system/tree/snapshot/node"
    );
}

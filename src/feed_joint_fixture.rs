//! **`J-4` — `APP-CONVENTION-FEED`'s joint fixture, our half.**
//!
//! `entity-workbench-go` answered `W-4` **yes** (FEED this cycle, stage 1 only)
//! and accepted `J-5`'s split unchanged: **they produce, we consume and run the
//! rig.** This module is the artifact that makes their half *a small program
//! rather than a rig* — the same move that unblocked `G-PIN-4`'s link 2.
//!
//! - `tests/fixtures/feed-joint/feed.json` is the fixture as **authored input**:
//!   two pinned Ed25519 seeds, a page size, and five entries. Plain JSON, no
//!   Rust in it, readable by any implementation.
//! - `tests/fixtures/feed-joint/EXPECTED.json` is what **this** implementation
//!   computes from it — every entry's content hash, every detached signature,
//!   the index head, every index page, and the trie root over the §4.2-pinned
//!   keys.
//! - `README.md` beside them is the protocol for the second publisher.
//!
//! ## ⭐ What is comparable here is NOT what was comparable for a site, and
//! getting that wrong costs a session
//!
//! [`crate::content_site::gpin4_joint_fixture`]'s whole comparand is
//! **peer-independent**: a site's keys are site-relative and a site entity's
//! body carries no peer id, so the README can say *"the peer id and the keypair
//! do not matter"*. **Neither sentence survives here**, and both failures are
//! silent:
//!
//! | | site | feed |
//! |---|---|---|
//! | peer id in the body | no | **yes** — `FEED-R1` makes `author` equal the namespace, and every index page is a list of `EntityRef::pin(author, …)` |
//! | keys pinned by the convention | relative to the site root | **the index only.** §4.2 pins `app/feed/index` and `app/feed/index/{page}`; §2 says *"the cross-impl contract is the type tag, not the path"*, so where an **entry** lives is a local choice |
//!
//! Two consequences, and they are the reason this file exists rather than a
//! copy of the site one:
//!
//! 1. **The fixture pins a SEED, not a peer id.** A literal peer id would make
//!    entry bodies comparable and leave `FEED-R2`'s signature out of the
//!    comparison entirely — you cannot sign as a peer whose key you do not
//!    hold. Ed25519 is deterministic (RFC 8032), so a seed makes the peer id,
//!    the identity entity and every signature byte reproducible — so `J-4`'s
//!    own definition, *"plus each entry's detached signature"*, becomes
//!    checkable rather than aspirational.
//! 2. **There are TWO roots and only one of them is a comparand.**
//!    `index_root` is over the §4.2-pinned keys and is what the other seat
//!    compares. `feed_root_ours` adds our entry and signature keys, whose
//!    prefix is **ours** (`feed::entry_prefix`, stated as ours in its own doc),
//!    and exists only so that a change to our own binding layout reds here
//!    rather than surfacing as a cross-impl mystery. Same shape as the site
//!    fixture's `site_root_pages` / `site_root_full` split, for a different
//!    reason: there the second root was waiting on a type the other seat had
//!    not built, here it is waiting on a path the convention declines to pin.
//!
//! ## Why every row of `feed.json` is there
//!
//! Each is a case where two implementations can each be internally consistent
//! and still disagree, so a fixture without it goes green while the divergence
//! is live. The `_why` key on each entry carries the same reasoning at the row,
//! where somebody editing it will actually read it.
//!
//! ## Regenerating
//!
//! `FEED_JOINT_REGENERATE=1` rewrites `EXPECTED.json` from the current code.
//! That is the only way it should ever change, and changing it is a **wire
//! event**: the other seat compares against these bytes, so a regeneration that
//! was not accompanied by a deliberate `feed.json` edit means our encoder
//! moved, and the question is which side is right — not which file to update.

#![cfg(all(test, not(target_arch = "wasm32")))]

use std::collections::BTreeMap;
use std::path::PathBuf;

use entity_ecf::{text, uinteger};
use entity_hash::Hash;
use entity_store::MemoryContentStore;

use crate::content_site::signed_root::RootProjector;
use crate::embed::{EmbedData, EmbedNode, EmbedPayload};
use crate::entity_ref::{Anchor, EntityRef, Hint, HintTag};
use crate::feed::{
    entry_key, index_head_key, index_page_key, signature_key, FeedEntry, Reply,
};
use crate::feed_publish::{plan_index, signature_entity, PagedEntry};

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/feed-joint")
}

// ---------------------------------------------------------------------------
// Reading the authored input
// ---------------------------------------------------------------------------

/// A hash-valued field in `feed.json`: either a 66-char wire-hex literal, or
/// `{"entry": "<name>"}` naming an **earlier** entry.
///
/// **Forward references are refused rather than resolved in a second pass.**
/// Not a limitation of the harness: an entry's hash is a function of its bytes,
/// so an entry referring to a later one would have to be encoded before the
/// thing it names exists. A fixture format that *looked* like it allowed it
/// would be inviting the other seat to build a two-pass resolver for a shape
/// the convention cannot represent.
fn hash_field(v: &serde_json::Value, built: &BTreeMap<String, Hash>, what: &str) -> Hash {
    if let Some(hex) = v.as_str() {
        return hash_from_hex(hex, what);
    }
    let name = v
        .get("entry")
        .and_then(|x| x.as_str())
        .unwrap_or_else(|| panic!("{what}: a hash is a hex string or {{\"entry\": \"<name>\"}}"));
    *built.get(name).unwrap_or_else(|| {
        panic!(
            "{what}: names entry {name:?}, which is not an EARLIER entry in the fixture. \
             An entry's hash is a function of its bytes, so a forward reference cannot be encoded."
        )
    })
}

fn hash_from_hex(hex: &str, what: &str) -> Hash {
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .unwrap_or_else(|_| panic!("{what}: {hex:?} is not hex"))
        })
        .collect();
    Hash::from_bytes(&bytes).unwrap_or_else(|e| {
        panic!(
            "{what}: {hex:?} is not a wire hash ({e:?}) — a SHA-256 hash is 66 hex characters, \
             the leading `00` being the format varint, which is part of the address (V7 §1.2)"
        )
    })
}

/// One `entity-ref` from its JSON form. The shape mirrors the atom's own
/// fields rather than `REFERENCE` §3's URI string, deliberately: the string
/// form has its own 11-vector suite on **both** seats already
/// (`entitysdk/reference_test.go` carries our pinned literals verbatim), and
/// routing this fixture through it would make a FEED comparison fail for a
/// REFERENCE reason.
fn reference(v: &serde_json::Value, peers: &BTreeMap<String, String>, built: &BTreeMap<String, Hash>, what: &str) -> EntityRef {
    let tag = v.get("tag").and_then(|x| x.as_str()).unwrap_or_else(|| panic!("{what}: no tag"));
    let peer_name = v.get("peer").and_then(|x| x.as_str()).unwrap_or_else(|| panic!("{what}: no peer"));
    let peer = peers
        .get(peer_name)
        .unwrap_or_else(|| panic!("{what}: peer {peer_name:?} is not in the fixture's `peers`"));

    let mut r = match tag {
        "pin" => EntityRef::pin(
            peer.clone(),
            hash_field(v.get("hash").unwrap_or_else(|| panic!("{what}: pin with no hash")), built, what),
        ),
        "live" => {
            let path = v.get("path").and_then(|x| x.as_str()).unwrap_or_else(|| panic!("{what}: live with no path"));
            let mut live = EntityRef::live(peer.clone(), path);
            if let Some(seen) = v.get("seen") {
                live = live.with_seen(hash_field(seen, built, what));
            }
            live
        }
        other => panic!("{what}: tag {other:?} — §2.2 has exactly two, and a third intent is a third tag"),
    };

    if let Some(at) = v.get("at") {
        let field: Vec<String> = at["field"]
            .as_array()
            .unwrap_or_else(|| panic!("{what}: `at.field` is an array of names"))
            .iter()
            .map(|s| s.as_str().expect("a field name is a string").to_string())
            .collect();
        r = r.with_at(Anchor { field });
    }
    if let Some(via) = v.get("via") {
        let hints: Vec<Hint> = via
            .as_array()
            .unwrap_or_else(|| panic!("{what}: `via` is an array"))
            .iter()
            .map(|h| {
                Hint::new(
                    HintTag::from_token(h["tag"].as_str().expect("a hint tag is a string")),
                    h["value"].as_str().expect("a hint value is a string"),
                )
            })
            .collect();
        r = r.with_via(hints);
    }
    r
}

/// One `embed-node` body from its JSON form.
///
/// **The `params` value mapping is part of the fixture's contract and is stated
/// here because JSON cannot express it:** a JSON string is an ECF text value, a
/// JSON integer is an ECF **unsigned** integer. Both are in the fixture so that
/// an implementation which stringifies numbers into the open bag is caught by
/// the hash rather than by review.
fn body(v: &serde_json::Value, what: &str) -> EmbedNode {
    let payload_json = v.get("payload").unwrap_or_else(|| panic!("{what}: no payload"));
    let payload = match payload_json["tag"].as_str() {
        Some("inline") => {
            let text = payload_json["utf8"].as_str().unwrap_or_else(|| panic!("{what}: inline payload carries `utf8`"));
            EmbedPayload::Inline(text.as_bytes().to_vec())
        }
        Some("pointer") => EmbedPayload::Pointer(hash_from_hex(
            payload_json["hash"].as_str().unwrap_or_else(|| panic!("{what}: pointer payload carries `hash`")),
            what,
        )),
        other => panic!(
            "{what}: payload tag {other:?}. `child` is deliberately not in this fixture — it names \
             a sibling Embed entity and neither seat mints one, so a row for it would pin a shape \
             nobody can produce"
        ),
    };

    let mut data = EmbedData::new(
        payload,
        v["fallback"].as_str().unwrap_or_else(|| panic!("{what}: `fallback` is MANDATORY and non-empty (EMBED §3)")),
    );
    if let Some(params) = v.get("params").and_then(|p| p.as_object()) {
        for (k, val) in params {
            let value = if let Some(s) = val.as_str() {
                text(s.to_string())
            } else if let Some(u) = val.as_u64() {
                uinteger(u)
            } else {
                panic!("{what}: params value for {k:?} is neither a string nor an unsigned integer — see this module's `body`")
            };
            data.params.insert(k.clone(), value);
        }
    }
    EmbedNode::new(v["media_type"].as_str().unwrap_or_else(|| panic!("{what}: no media_type")), data)
}

/// Everything the fixture describes, built.
struct Authored {
    author: String,
    /// Oldest-first, the order they were authored.
    entries: Vec<(String, FeedEntry, Hash)>,
    page_size: usize,
}

fn author_fixture() -> Authored {
    let raw = std::fs::read_to_string(fixture_dir().join("feed.json"))
        .expect("the joint fixture is in the tree");
    let doc: serde_json::Value = serde_json::from_str(&raw).expect("the joint fixture is JSON");

    // Seeds → peer ids. `_comment` is prose and is not a peer.
    let peers: BTreeMap<String, String> = doc["peers"]
        .as_object()
        .expect("`peers` is a map of name → seed")
        .iter()
        .filter(|(name, _)| !name.starts_with('_'))
        .map(|(name, seed)| {
            let hex = seed.as_str().expect("a seed is hex");
            let mut bytes = [0u8; 32];
            assert_eq!(hex.len(), 64, "an Ed25519 seed is 32 bytes / 64 hex characters");
            for (i, b) in bytes.iter_mut().enumerate() {
                *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("a seed is hex");
            }
            (name.clone(), entity_crypto::Keypair::from_seed(bytes).peer_id().to_string())
        })
        .collect();
    let author = peers.get("author").expect("the fixture names an `author` peer").clone();

    let mut built: BTreeMap<String, Hash> = BTreeMap::new();
    let mut entries = Vec::new();
    for e in doc["entries"].as_array().expect("`entries` is an array") {
        let name = e["name"].as_str().expect("every entry is named").to_string();
        let what = format!("entry {name:?}");

        let mut entry = FeedEntry::new(
            author.clone(),
            e["created_at"].as_u64().unwrap_or_else(|| panic!("{what}: `created_at` is required")),
            body(&e["body"], &what),
        );
        if let Some(reply) = e.get("reply") {
            entry.reply = Some(Reply {
                root: reference(&reply["root"], &peers, &built, &format!("{what} reply.root")),
                parent: reference(&reply["parent"], &peers, &built, &format!("{what} reply.parent")),
            });
        }
        if let Some(context) = e.get("context") {
            entry.context = Some(reference(context, &peers, &built, &format!("{what} context")));
        }
        if let Some(prev) = e.get("prev") {
            entry.prev = Some(hash_field(prev, &built, &format!("{what} prev")));
        }
        if let Some(list) = e.get("attachments").and_then(|a| a.as_array()) {
            entry.attachments = list
                .iter()
                .enumerate()
                .map(|(i, a)| reference(a, &peers, &built, &format!("{what} attachments[{i}]")))
                .collect();
        }

        let hash = entry.to_entity().expect("the authored entry encodes").content_hash;
        assert!(
            built.insert(name.clone(), hash).is_none(),
            "two entries are named {name:?} — a symbolic reference could not say which"
        );
        entries.push((name, entry, hash));
    }

    Authored {
        author,
        entries,
        page_size: doc["page_size"].as_u64().expect("`page_size` is required") as usize,
    }
}

// ---------------------------------------------------------------------------
// Computing our half
// ---------------------------------------------------------------------------

/// What this implementation makes of the fixture. `index` is the comparand;
/// `ours` adds the keys whose prefix the convention leaves local.
struct Computed {
    /// §4.2-pinned keys only: `app/feed/index` + `app/feed/index/{n}`.
    index: BTreeMap<String, Hash>,
    /// …plus our entry and signature keys.
    ours: BTreeMap<String, Hash>,
    /// Per entry, in authored order: `(name, entry hash, signature key,
    /// signature entity hash)`.
    signed: Vec<(String, Hash, String, Hash)>,
    page_count: usize,
}

fn compute(a: &Authored) -> Computed {
    // The signature path is the production one, through the production
    // emitter: `feed_publish::signature_entity` over a `RootProjector` built
    // from the fixture's own seed. A second expression of *how this system
    // signs one entry* is exactly what C15 exists to stop, and a fixture is
    // the most tempting place to write one.
    let keypair = seed_keypair(a);
    let root = RootProjector::new(keypair).expect("the fixture's publisher builds");
    assert_eq!(
        root.peer_id(),
        a.author,
        "the projector's peer id must be the one the entries were authored under — otherwise the \
         signatures name a different publisher than the bodies do"
    );

    let mut signed = Vec::new();
    let mut ours = BTreeMap::new();
    let mut paged = Vec::new();
    for (name, entry, hash) in &a.entries {
        let sig = signature_entity(&root, hash).expect("the detached signature encodes");
        let key = signature_key(&a.author, hash);
        ours.insert(entry_key(hash), *hash);
        ours.insert(key.clone(), sig.content_hash);
        signed.push((name.clone(), *hash, key, sig.content_hash));
        paged.push(PagedEntry { hash: *hash, created_at: entry.created_at });
    }

    // The head's clock is the feed's own high-water mark, never a wall clock —
    // `publish_axes::head_clock`'s rule, restated here because a fixture that
    // called `SystemTime::now()` would move its own root on every run and be
    // uncomparable by construction.
    let head_clock = a.entries.iter().map(|(_, e, _)| e.created_at).max().unwrap_or(0);
    let plan = plan_index(&a.author, &paged, a.page_size, head_clock).expect("the index plans");

    let mut index = BTreeMap::new();
    index.insert(
        index_head_key().to_string(),
        plan.head.to_entity().expect("the head encodes").content_hash,
    );
    for page in &plan.pages {
        index.insert(
            index_page_key(page.page),
            page.to_entity().expect("a page encodes").content_hash,
        );
    }
    ours.extend(index.iter().map(|(k, v)| (k.clone(), *v)));

    Computed { index, ours, signed, page_count: plan.pages.len() }
}

fn seed_keypair(a: &Authored) -> entity_crypto::Keypair {
    let raw = std::fs::read_to_string(fixture_dir().join("feed.json")).expect("readable");
    let doc: serde_json::Value = serde_json::from_str(&raw).expect("JSON");
    let hex = doc["peers"]["author"].as_str().expect("the author's seed");
    let mut bytes = [0u8; 32];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("hex");
    }
    let kp = entity_crypto::Keypair::from_seed(bytes);
    assert_eq!(kp.peer_id().to_string(), a.author);
    kp
}

fn root_of(b: &BTreeMap<String, Hash>) -> Hash {
    entity_tree::trie::build_trie(&MemoryContentStore::new(), b).expect("the trie builds")
}

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

fn expected_path() -> PathBuf {
    fixture_dir().join("EXPECTED.json")
}

fn describe(a: &Authored, c: &Computed) -> serde_json::Value {
    serde_json::json!({
        "_comment": "COMPUTED by entity-browser-rust from feed.json. Not hand-editable — see the \
                     module doc on src/feed_joint_fixture.rs. `index_root` is the EXTENSION-TREE \
                     v4.0.2 trie root over `index_bindings` and is THE comparand. \
                     `feed_root_ours` additionally covers keys whose prefix APP-CONVENTION-FEED \
                     leaves to the publisher, so it is our own drift detector and NOT a \
                     cross-impl expectation. Neither is a published-root head: that entity \
                     carries a wall clock and is not comparable.",
        "convention": "APP-CONVENTION-FEED — §2.3 entries, §4.2 index, §1.1 (FEED-R2) signatures",
        "author_peer_id": a.author,
        "page_size": a.page_size,
        "page_count": c.page_count,
        "index_root": root_of(&c.index).to_hex(),
        "feed_root_ours": root_of(&c.ours).to_hex(),
        "index_bindings": c.index.iter()
            .map(|(k, h)| (k.clone(), serde_json::Value::String(h.to_hex())))
            .collect::<serde_json::Map<_, _>>(),
        "entries": c.signed.iter().map(|(name, hash, sig_key, sig_hash)| serde_json::json!({
            "name": name,
            "entry": hash.to_hex(),
            "signature_key": sig_key,
            "signature": sig_hash.to_hex(),
        })).collect::<Vec<_>>(),
        "entry_keys_are_ours": {
            "prefix": crate::feed::entry_prefix(),
            "why": "APP-CONVENTION-FEED §2 makes the cross-impl contract the TYPE TAG, not the \
                    path, so where an entry lives is a local choice and a second seat is not \
                    expected to match this. Compare `entries[].entry` by hash, in order. The \
                    signature key is a different case: V7 §3.5's invariant pointer pins it, so \
                    `signature_key` IS an expectation.",
        },
    })
}

/// **Our half of `J-4` is pinned and does not drift.**
///
/// Every entry hash, every detached signature, the head, every page and the
/// §4.2-keyed trie root are compared against `EXPECTED.json` — the file the
/// other seat compares *their* publisher against. So a change to our
/// `app/feed/*` encoding reds here, in this repo, on the commit that makes it,
/// rather than surfacing as an unexplained cross-impl divergence later.
///
/// **Per-key first and per-key named**, which is the one thing
/// `entity-workbench-go` asked back for when they accepted `J-4`: *a bare root
/// mismatch costs a session to localize.*
#[test]
fn the_feed_fixture_encodes_to_the_hashes_we_published_to_the_other_seat() {
    let authored = author_fixture();
    let computed = compute(&authored);
    let doc = describe(&authored, &computed);

    if std::env::var("FEED_JOINT_REGENERATE").is_ok() {
        std::fs::write(
            expected_path(),
            format!("{}\n", serde_json::to_string_pretty(&doc).expect("serializes")),
        )
        .expect("EXPECTED.json is writable");
        eprintln!(
            "FEED_JOINT_REGENERATE: rewrote EXPECTED.json — index_root {}",
            doc["index_root"].as_str().unwrap_or("?")
        );
        return;
    }

    let raw = std::fs::read_to_string(expected_path())
        .expect("EXPECTED.json is in the tree — FEED_JOINT_REGENERATE=1 to mint it");
    let expected: serde_json::Value =
        serde_json::from_str(&raw).expect("EXPECTED.json is JSON");

    // Per key, before any root.
    let mut drift = Vec::new();
    let expected_index = expected["index_bindings"].as_object().expect("index_bindings");
    for (key, hash) in &computed.index {
        match expected_index.get(key).and_then(|v| v.as_str()) {
            None => drift.push(format!("  {key:?} — now {}, not in EXPECTED.json", hash.to_hex())),
            Some(e) if e != hash.to_hex() => {
                drift.push(format!("  {key:?} — now {}, published {e}", hash.to_hex()))
            }
            Some(_) => {}
        }
    }
    for key in expected_index.keys() {
        if !computed.index.contains_key(key) {
            drift.push(format!("  {key:?} — in EXPECTED.json, no longer emitted"));
        }
    }
    // Then per entry, naming the row rather than the hash.
    let expected_entries = expected["entries"].as_array().expect("entries");
    for (i, (name, hash, sig_key, sig_hash)) in computed.signed.iter().enumerate() {
        let Some(e) = expected_entries.get(i) else {
            drift.push(format!("  entry {name:?} — authored, not in EXPECTED.json"));
            continue;
        };
        let published = |field: &str| e[field].as_str().unwrap_or("(absent)").to_string();
        if e["name"].as_str() != Some(name.as_str()) {
            drift.push(format!("  position {i} — now {name:?}, published {:?}", published("name")));
        }
        if e["entry"].as_str() != Some(&hash.to_hex()) {
            drift.push(format!("  entry {name:?} — now {}, published {}", hash.to_hex(), published("entry")));
        }
        if e["signature_key"].as_str() != Some(sig_key.as_str()) {
            drift.push(format!(
                "  entry {name:?} signature KEY — now {sig_key:?}, published {:?}",
                published("signature_key")
            ));
        }
        if e["signature"].as_str() != Some(&sig_hash.to_hex()) {
            drift.push(format!(
                "  entry {name:?} signature — now {}, published {}",
                sig_hash.to_hex(),
                published("signature")
            ));
        }
    }
    assert!(
        drift.is_empty(),
        "the joint fixture's encoding moved on {} row(s). entity-workbench-go is comparing their \
         publisher against EXPECTED.json, so regenerating is a WIRE EVENT, not a test fix — \
         establish which side is right first:\n{}",
        drift.len(),
        drift.join("\n")
    );

    assert_eq!(
        expected["author_peer_id"].as_str(),
        Some(authored.author.as_str()),
        "the peer id derived from the fixture's seed moved — every entry body and every index \
         page carries it, so this is not a labelling change"
    );
    assert_eq!(
        root_of(&computed.index).to_hex(),
        expected["index_root"].as_str().unwrap_or_default(),
        "the index root moved without any index binding moving — the trie construction changed"
    );
    assert_eq!(
        root_of(&computed.ours).to_hex(),
        expected["feed_root_ours"].as_str().unwrap_or_default(),
        "our own full-feed root moved — this one is NOT a cross-impl expectation, so a red here \
         with `index_root` green means our entry or signature KEY LAYOUT moved, not our encoding"
    );

    // Anti-vacuity. Every loop above is satisfied by an emptied fixture, and
    // the per-entry loop is satisfied by an EXPECTED.json with more rows than
    // we authored.
    assert_eq!(authored.entries.len(), 5, "the fixture authors five entries");
    assert_eq!(expected_entries.len(), 5, "and EXPECTED.json pins five");
    assert_eq!(
        computed.page_count, 3,
        "five entries at page_size 2 is three pages — a multi-page index with a PARTIAL last \
         page, which is the shape §4.3 rule 1's cost argument is about"
    );
    assert_eq!(computed.index.len(), 4, "one head + three pages");

    eprintln!(
        "J-4 (FEED vocabulary) — our half: {} entries, {} pages, index_root {}",
        authored.entries.len(),
        computed.page_count,
        root_of(&computed.index).to_hex()
    );
}

/// **The optional keys are ABSENT, not empty — the divergence the fixture's
/// first row was shaped around.**
///
/// `reply`, `context`, `prev` and `attachments` are all conditional on our side
/// and would be `omitempty` on a Go struct. Two implementations that disagree
/// about *empty vs absent* produce different bytes for the same authored
/// content and neither looks wrong on its own. Asserted on the encoded key set
/// rather than on a round trip, because a round trip is exactly what cannot see
/// this.
#[test]
fn an_absent_optional_is_not_an_empty_one() {
    let authored = author_fixture();
    let (name, _, _) = &authored.entries[0];
    assert_eq!(name, "plain", "the floor row is first");

    let encoded = authored.entries[0].1.to_entity().expect("encodes").data;
    let value: ciborium::Value =
        ciborium::from_reader(encoded.as_slice()).expect("the authored entry decodes");
    let keys: Vec<String> = value
        .as_map()
        .expect("a map")
        .iter()
        .filter_map(|(k, _)| k.as_text().map(str::to_string))
        .collect();
    assert_eq!(
        keys,
        vec!["body".to_string(), "author".to_string(), "created_at".to_string()],
        "an entry with no reply, context, prev or attachments must encode exactly those three \
         keys, in ECF's canonical order (length, then lexical) — this is the byte sequence the \
         other seat reproduces, and an `omitempty` disagreement shows up here first"
    );

    // …and the loaded row really does carry them, or the assertion above is
    // measuring an empty fixture.
    let loaded = &authored.entries[3].1;
    assert!(loaded.context.is_some() && !loaded.attachments.is_empty());
}

/// **The head emits no `oldest` key, and that is §4.2's declared default rather
/// than an omission.**
///
/// A publisher who has dropped nothing emits no key; *absent* is a real zero.
/// Pinned separately from the hash comparison because the hash cannot say
/// *which* key is missing, and this is the one whose default is itself a fact —
/// the collapse that cost us `min_rollback_index` in another subsystem.
#[test]
fn a_head_that_has_dropped_nothing_carries_no_oldest_key() {
    let authored = author_fixture();
    let computed = compute(&authored);
    let head_hash = computed.index.get(index_head_key()).expect("the head is bound");

    let paged: Vec<PagedEntry> = authored
        .entries
        .iter()
        .map(|(_, e, h)| PagedEntry { hash: *h, created_at: e.created_at })
        .collect();
    let clock = authored.entries.iter().map(|(_, e, _)| e.created_at).max().unwrap();
    let plan = plan_index(&authored.author, &paged, authored.page_size, clock).expect("plans");
    let head = plan.head.to_entity().expect("encodes");
    assert_eq!(head.content_hash, *head_hash);

    let value: ciborium::Value =
        ciborium::from_reader(head.data.as_slice()).expect("the head decodes");
    let keys: Vec<String> = value
        .as_map()
        .expect("a map")
        .iter()
        .filter_map(|(k, _)| k.as_text().map(str::to_string))
        .collect();
    assert_eq!(
        keys,
        vec!["current".to_string(), "updated_at".to_string()],
        "the head must carry `current` + `updated_at` and NOT `oldest` — §4.2 declares 0 as the \
         default, so emitting it would be a different byte sequence for the same fact"
    );
}

/// **The pages fill oldest-first and read newest-first within a page, and the
/// fixture's index is the shape that can tell the two apart.**
///
/// Both orders reversed round-trips perfectly, so a gate that only re-read the
/// index would pass with them swapped. This one asserts against the authored
/// order directly, which is what an independent publisher has to reproduce from
/// `feed.json` alone.
#[test]
fn page_zero_holds_the_oldest_entries_and_lists_them_newest_first() {
    let authored = author_fixture();
    let paged: Vec<PagedEntry> = authored
        .entries
        .iter()
        .map(|(_, e, h)| PagedEntry { hash: *h, created_at: e.created_at })
        .collect();
    let clock = authored.entries.iter().map(|(_, e, _)| e.created_at).max().unwrap();
    let plan = plan_index(&authored.author, &paged, authored.page_size, clock).expect("plans");

    let hash_of = |name: &str| {
        authored.entries.iter().find(|(n, _, _)| n == name).map(|(_, _, h)| *h).expect(name)
    };

    assert_eq!(plan.pages.len(), 3);
    assert_eq!(
        plan.pages[0].entries,
        vec![
            EntityRef::pin(authored.author.clone(), hash_of("params")),
            EntityRef::pin(authored.author.clone(), hash_of("plain")),
        ],
        "page 0 holds the OLDEST two entries, listed newest-first within the page (§4.5)"
    );
    assert_eq!(
        plan.pages[2].entries,
        vec![EntityRef::pin(authored.author.clone(), hash_of("chained"))],
        "the last page is the PARTIAL one and holds the newest entry"
    );
    assert_eq!(
        plan.head.current, 2,
        "the head names the highest page in use, which is the partial one"
    );
}

/// **A symbolic reference to a later entry is refused rather than resolved.**
///
/// The fixture format looks like it could allow one, and an implementer reading
/// only `feed.json` might build a two-pass resolver. It cannot be represented:
/// an entry's hash is a function of its bytes, so an entry naming a later one
/// would have to be encoded before the thing it names exists. Asserted so the
/// refusal is a property of the harness rather than of the current fixture
/// happening not to contain one.
#[test]
fn a_forward_reference_is_refused_because_it_cannot_be_encoded() {
    let built = BTreeMap::new();
    // The refusal is a panic, so the default hook would print a backtrace for a
    // test that is passing. Silenced for the duration and restored after.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        hash_field(&serde_json::json!({ "entry": "written-later" }), &built, "test")
    }))
    .expect_err("a forward reference must not resolve");
    std::panic::set_hook(hook);
    let msg = err
        .downcast_ref::<String>()
        .map(String::as_str)
        .unwrap_or("");
    assert!(
        msg.contains("forward reference"),
        "the refusal must say WHY, or the next implementer reads it as a harness limitation: {msg}"
    );
}

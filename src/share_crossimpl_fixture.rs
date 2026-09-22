//! **`B-7` — `app/share/*` bodies from OUR encoder, for the other seat to
//! vendor.**
//!
//! `entity-workbench-go` asked for this when the `WC-2` convergence closed, and
//! the reason is the asymmetry that convergence left behind:
//!
//! > our gates build the record and publication bodies from the CDDL and from
//! > their emitter's field spellings, which catches a rename on **our** side
//! > and cannot catch one on theirs — and theirs does the same in the mirror.
//!
//! `src/share.rs` has thirty-two gates and **every fixture in every one of them
//! is authored by the encoder under test**. That is the population argument
//! this repo has paid for twice already: *a test population you generated
//! cannot contain the shape you are missing.* The only cure is bytes crossing
//! the boundary, and that is what this module emits.
//!
//! It is the mirror image of `tests/fixtures/crossimpl-go-site/` — their bytes,
//! frozen, in our tree — which is how the **site** convergence was measured.
//!
//! ## What is emitted, and why it is not a whole published tree
//!
//! `B-7` needs **bodies**, not a tree: their reader decodes an entity, so what
//! has to cross is the canonical hashable body —
//! `entity_ecf::ecf_for_hash(type, data)`, byte-for-byte what a publisher puts
//! at `content/{aa}/{bb}/{hex}` and what their `fetch` already parses. No peer,
//! no signed root, no keypair: **nothing in `app/share/*` carries a peer id**,
//! because §4 makes the *namespace* the publisher and `decode_share` takes it
//! as an argument rather than trusting a `from` field. That is the opposite of
//! `feed-joint`, where the peer id is in the bytes and the fixture has to pin a
//! seed — the two conventions differ on exactly this axis and it is worth
//! knowing which one you are in.
//!
//! ## Regenerating
//!
//! `make test-one T=share_crossimpl EXTRA_RUN_ENV="-e SHARE_CROSSIMPL_REGENERATE=1"`
//!
//! and that is the only way it should ever change. **A regeneration not
//! accompanied by a deliberate edit to [`rows`] is a wire event, not a test
//! fix** — the other seat is decoding these bytes.

#![cfg(all(test, not(target_arch = "wasm32")))]

use std::path::PathBuf;

use entity_entity::Entity;
use entity_hash::Hash;

use crate::share::{
    decode_share, share_entity, Audience, AudienceMember, AudienceOrigin, Share, ShareTarget,
};

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/share-crossimpl")
}

/// The publisher the bodies are decoded under. **Not in the bytes** — §4 makes
/// the namespace the publisher, so this is here only so the `EXPECTED.json`
/// rows can state what a reader should end up with, and so a seat that *does*
/// read a `from` field out of the body has something to disagree with.
const PUBLISHER: &str = "2K42FX8pASWDrXaAVsGXMNJbAkCVBuVwXf4RuwFNnmyYis";

const GRANTEE_A: &str = "2KLv2nhwtPrLFd4BZFQuNK1ujtE74q8cVg7y8cYdcZZ5BL";
const GRANTEE_B: &str = "2KHPSRBHu13dCYEo4AQW8rqZ1yLVE2Hmu3faXJUy3zcs8L";

fn blob_hash() -> Hash {
    // An opaque content address — a share is *what gives a hash a filename*, so
    // the blob itself is not part of what crosses. 33 bytes, `00` format varint.
    let mut digest = [0u8; 32];
    digest[0..4].copy_from_slice(&[0xd0, 0xc5, 0x1e, 0x17]);
    digest[31] = 1;
    Hash::new(0, &digest)
}

/// The fixture, as `(name, why it is here, share)`.
///
/// Every row is a case where two implementations can each be internally
/// consistent and still disagree, so a fixture without it goes green while the
/// divergence is live.
fn rows() -> Vec<(&'static str, &'static str, Share)> {
    vec![
        (
            "record-one-member",
            "THE FLOOR for `app/share/record`: one audience entry, a note, a blob target. \
             `audience` is an ARRAY OF ENTITIES (`{type, data}`), not a list of peer-id strings \
             — a seat that flattens it to strings produces a plausible record with different \
             bytes and a reader that cannot tell `via` from absent.",
            Share {
                title: "quarterly-figures.pdf".into(),
                target: ShareTarget::Blob(blob_hash()),
                audience: Audience::Direct(vec![AudienceMember::direct(GRANTEE_A, 1_757_462_400_000)]),
                note: Some("The numbers behind Tuesday's deck.".into()),
                created_at: 1_757_462_400_000,
                size: None,
                from: PUBLISHER.into(),
            },
        ),
        (
            "record-self-only",
            "SHARE-7's vector, and the most valuable row here: an EMPTY audience is SELF-ONLY, \
             never public. The `audience` key is PRESENT and empty. A seat that reads it as \
             public, or that omits the key when the array is empty, is wrong in the direction \
             that discloses — and both mistakes are invisible in a round trip against yourself.",
            Share {
                title: "draft-notes".into(),
                target: ShareTarget::Prefix(format!("/{PUBLISHER}/local/files/drafts/")),
                audience: Audience::self_only(),
                note: None,
                created_at: 1_757_466_000_000,
                size: None,
                from: PUBLISHER.into(),
            },
        ),
        (
            "record-two-members-group-origin",
            "TWO entries, so authored order is exercised, and the second carries `via: group` — \
             §2.3's other origin token, which records that a group membership produced the entry \
             AT AUTHORING TIME and is explicitly not resolved at check time. Also NO `note`, so \
             the optional key must be ABSENT rather than an empty string.",
            Share {
                title: "retro-2026-09".into(),
                target: ShareTarget::Prefix(format!("/{PUBLISHER}/local/files/retro/")),
                audience: Audience::Direct(vec![
                    AudienceMember::direct(GRANTEE_A, 1_757_469_600_000),
                    AudienceMember {
                        grantee: GRANTEE_B.into(),
                        via: Some(AudienceOrigin::Group),
                        added_at: 1_757_469_601_000,
                    },
                ]),
                note: None,
                created_at: 1_757_469_600_000,
                size: None,
                from: PUBLISHER.into(),
            },
        ),
        (
            "record-member-without-via",
            "`via` is OPTIONAL (§2.3) and this entry has none, so the key must be ABSENT inside \
             the entry's `data` — not `\"direct\"` defaulted in on the way out. A seat that \
             defaults it is emitting a claim about how the member got there.",
            Share {
                title: "one-off.txt".into(),
                target: ShareTarget::Blob(blob_hash()),
                audience: Audience::Direct(vec![AudienceMember {
                    grantee: GRANTEE_B.into(),
                    via: None,
                    added_at: 1_757_473_200_000,
                }]),
                note: None,
                created_at: 1_757_473_200_000,
                size: None,
                from: PUBLISHER.into(),
            },
        ),
        (
            "publication",
            "`app/share/publication` — §2.5's public, pull-only half, and it carries NO \
             `audience` key at all. SHARE-8 makes a publication carrying one INVALID rather than \
             something to skip silently: §2.6's must-ignore would otherwise launder the one \
             shape this type exists to exclude into an ordinary-looking row.",
            Share {
                title: "entity-core-overview.pdf".into(),
                target: ShareTarget::Blob(blob_hash()),
                audience: Audience::Public,
                note: Some("Public — pull-only, no grant, and we cannot know who fetched it.".into()),
                created_at: 1_757_476_800_000,
                size: None,
                from: PUBLISHER.into(),
            },
        ),
    ]
}

/// One row's canonical hashable body — exactly the bytes at
/// `content/{aa}/{bb}/{hex}` in a published tree.
fn body_bytes(ent: &Entity) -> Vec<u8> {
    entity_ecf::ecf_for_hash(&ent.entity_type, &ent.data)
}

fn describe(name: &str, why: &str, ent: &Entity, share: &Share) -> serde_json::Value {
    // Decoded back through the PRODUCTION decoder, so the JSON states what a
    // conformant reader should end up with rather than what we put in. A row
    // whose encode and decode disagree is a defect we would rather find here
    // than have the other seat report.
    let round_tripped = decode_share(ent, PUBLISHER).expect("our own encoder round-trips");
    assert_eq!(
        round_tripped.title, share.title,
        "the fixture's own round trip disagrees on `title` for {name}"
    );
    serde_json::json!({
        "name": name,
        "why": why,
        "type": ent.entity_type,
        "content_hash": ent.content_hash.to_hex(),
        "body_file": format!("bodies/{}", ent.content_hash.to_hex()),
        "decodes_to": {
            "title": round_tripped.title,
            "target": match &round_tripped.target {
                ShareTarget::Blob(h) => serde_json::json!({ "tag": "blob", "hash": h.to_hex() }),
                ShareTarget::Prefix(p) => serde_json::json!({ "tag": "prefix", "path": p }),
            },
            "audience": match &round_tripped.audience {
                Audience::Public => serde_json::Value::Null,
                Audience::Direct(m) => serde_json::Value::Array(m.iter().map(|e| serde_json::json!({
                    "grantee": e.grantee,
                    "via": e.via.map(|v| v.as_token()),
                    "added_at": e.added_at,
                })).collect()),
            },
            "audience_is_self_only": round_tripped.audience.is_self_only(),
            "note": round_tripped.note,
            "created_at": round_tripped.created_at,
            // Stated because it is the field most likely to be looked for and
            // is deliberately NOT in the bytes.
            "from": round_tripped.from,
        },
    })
}

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

/// **The `app/share/*` bodies we hand the other seat are pinned and do not
/// drift.**
///
/// Emitting them is only half of `B-7`; the other half is that they cannot
/// change under the other seat without us noticing, which is what this asserts.
///
/// **Per-row and per-field before any hash**, the same ordering the FEED joint
/// fixture uses and for the same stated reason: a bare hash mismatch names the
/// entity and not the field, and localizing it costs a session.
#[test]
fn the_share_bodies_we_hand_the_other_seat_are_pinned() {
    let rows = rows();
    let mut entities = Vec::new();
    let mut manifest = Vec::new();
    for (name, why, share) in &rows {
        let ent = share_entity(share).expect("the fixture share encodes");
        manifest.push(describe(name, why, &ent, share));
        entities.push((name.to_string(), ent));
    }

    let doc = serde_json::json!({
        "_comment": "COMPUTED by entity-browser-rust. Not hand-editable — see the module doc on \
                     src/share_crossimpl_fixture.rs. Each row's `body_file` holds the CANONICAL \
                     HASHABLE BODY, byte-for-byte what a publisher writes to \
                     content/{aa}/{bb}/{hex}. No peer id appears in any of these bytes: \
                     APP-CONVENTION-SHARE §4 makes the NAMESPACE the publisher, so a reader is \
                     handed it rather than reading a `from` field.",
        "convention": "APP-CONVENTION-SHARE §2.2 (record), §2.3 (audience-entry), §2.5 (publication)",
        "for": "entity-workbench-go, ask B-7",
        "publisher": PUBLISHER,
        "entities": manifest,
    });

    let bodies = fixture_dir().join("bodies");
    if std::env::var("SHARE_CROSSIMPL_REGENERATE").is_ok() {
        // Rewrite from scratch: a stale body left behind from a retired row is
        // a file the other seat may still be decoding.
        let _ = std::fs::remove_dir_all(&bodies);
        std::fs::create_dir_all(&bodies).expect("the bodies directory is writable");
        for (_, ent) in &entities {
            std::fs::write(bodies.join(ent.content_hash.to_hex()), body_bytes(ent))
                .expect("a body is writable");
        }
        std::fs::write(
            fixture_dir().join("EXPECTED.json"),
            format!("{}\n", serde_json::to_string_pretty(&doc).expect("serializes")),
        )
        .expect("EXPECTED.json is writable");
        eprintln!("SHARE_CROSSIMPL_REGENERATE: rewrote {} bodies", entities.len());
        return;
    }

    let raw = std::fs::read_to_string(fixture_dir().join("EXPECTED.json"))
        .expect("EXPECTED.json is in the tree — SHARE_CROSSIMPL_REGENERATE=1 to mint it");
    let expected: serde_json::Value = serde_json::from_str(&raw).expect("EXPECTED.json is JSON");
    let expected_rows = expected["entities"].as_array().expect("entities");

    let mut drift = Vec::new();
    for (i, row) in manifest.iter().enumerate() {
        let name = row["name"].as_str().unwrap_or("?");
        let Some(e) = expected_rows.get(i) else {
            drift.push(format!("  {name} — emitted, not in EXPECTED.json"));
            continue;
        };
        for field in ["name", "type", "content_hash"] {
            if row[field] != e[field] {
                drift.push(format!(
                    "  {name}.{field} — now {}, published {}",
                    row[field], e[field]
                ));
            }
        }
        if row["decodes_to"] != e["decodes_to"] {
            drift.push(format!(
                "  {name} decodes differently:\n      now       {}\n      published {}",
                row["decodes_to"], e["decodes_to"]
            ));
        }
    }
    for e in expected_rows.iter().skip(manifest.len()) {
        drift.push(format!("  {} — in EXPECTED.json, no longer emitted", e["name"]));
    }
    assert!(
        drift.is_empty(),
        "the share bodies moved on {} row(s). entity-workbench-go vendors these bytes and decodes \
         them, so regenerating is a WIRE EVENT, not a test fix:\n{}",
        drift.len(),
        drift.join("\n")
    );

    // The bodies on disk are the bytes, not a description of them. A manifest
    // that agreed while the files had gone stale would be the worst of both.
    for (name, ent) in &entities {
        let path = bodies.join(ent.content_hash.to_hex());
        let on_disk = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("{name}: {} is missing ({e}) — the other seat vendors this file", path.display()));
        assert_eq!(
            on_disk,
            body_bytes(ent),
            "{name}: the body on disk is not what this encoder now produces"
        );
        // …and it really is the hashable form: re-hashing it must give the
        // address it is filed under, which is the property their reader checks.
        assert_eq!(
            Entity::new(&ent.entity_type, ent.data.clone())
                .expect("re-encodes")
                .content_hash,
            ent.content_hash,
            "{name}: the body does not hash to its own filename"
        );
    }

    // Anti-vacuity: every loop above is satisfied by an empty fixture, and the
    // two rows that carry the convention's sharpest distinctions are the ones
    // a tidy-up would drop first.
    assert_eq!(rows.len(), 5, "the fixture carries five rows");
    assert!(
        manifest.iter().any(|r| r["type"] == "app/share/publication"),
        "a fixture with no publication cannot measure the §2.5 split at all"
    );
    let self_only = manifest
        .iter()
        .find(|r| r["name"] == "record-self-only")
        .expect("SHARE-7's row is present");
    assert_eq!(
        self_only["decodes_to"]["audience_is_self_only"],
        serde_json::Value::Bool(true),
        "the self-only row must decode as self-only — if it ever reads as public, that is the \
         disclosure SHARE-7 exists to catch, and we would be publishing it as an expectation"
    );

    eprintln!("B-7 — {} share bodies pinned for entity-workbench-go", entities.len());
}

/// **A publication carries no `audience` key, and a record's empty audience is
/// PRESENT and empty.** SHARE-7 and SHARE-8 from the emitting side.
///
/// Asserted on the encoded key set rather than through a decode, because a
/// decode is exactly what cannot see the difference between *absent* and
/// *present and empty* once both have been normalized into a variant.
#[test]
fn a_publication_omits_the_audience_key_and_a_self_only_record_carries_an_empty_one() {
    let by_name: std::collections::BTreeMap<&str, Share> =
        rows().into_iter().map(|(n, _, s)| (n, s)).collect();

    let keys_of = |share: &Share| -> Vec<String> {
        let ent = share_entity(share).expect("encodes");
        let v: ciborium::Value = ciborium::from_reader(ent.data.as_slice()).expect("decodes");
        v.as_map()
            .expect("a map")
            .iter()
            .filter_map(|(k, _)| k.as_text().map(str::to_string))
            .collect()
    };

    let publication = keys_of(&by_name["publication"]);
    assert!(
        !publication.contains(&"audience".to_string()),
        "a publication must carry NO audience key — SHARE-8 makes one INVALID, not ignorable: {publication:?}"
    );

    let self_only = keys_of(&by_name["record-self-only"]);
    assert!(
        self_only.contains(&"audience".to_string()),
        "a self-only record must carry a PRESENT, empty audience — omitting it would make it \
         indistinguishable on the wire from a publication, which is the disclosure SHARE-7 \
         names: {self_only:?}"
    );

    // The member-level optional, same rule one layer in.
    let ent = share_entity(&by_name["record-member-without-via"]).expect("encodes");
    let v: ciborium::Value = ciborium::from_reader(ent.data.as_slice()).expect("decodes");
    let audience = v
        .as_map()
        .expect("a map")
        .iter()
        .find(|(k, _)| k.as_text() == Some("audience"))
        .expect("audience is present")
        .1
        .as_array()
        .expect("audience is an array");
    let data = audience[0]
        .as_map()
        .expect("an entry is a map")
        .iter()
        .find(|(k, _)| k.as_text() == Some("data"))
        .expect("the entry carries data")
        .1
        .as_map()
        .expect("data is a map");
    let member_keys: Vec<&str> = data.iter().filter_map(|(k, _)| k.as_text()).collect();
    assert_eq!(
        member_keys,
        vec!["grantee", "added_at"],
        "a member with no `via` must omit the key, leaving exactly `grantee` + `added_at` in \
         ECF's canonical order — defaulting `via` to \"direct\" emits a claim about how the \
         member got there that the author never made"
    );
}

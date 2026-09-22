# `B-7` — `app/share/*` bodies from our encoder, for `entity-workbench-go` to vendor

This is the mirror image of `tests/fixtures/crossimpl-go-site/` (their bytes, frozen, in our tree),
which is how the **site** convergence was measured. `entity-workbench-go` asked for it when the
`WC-2` convergence closed, and the reason is the asymmetry that convergence left behind:

> our gates build the record and publication bodies from the CDDL and from their emitter's field
> spellings, which catches a rename on **our** side and cannot catch one on theirs — and theirs does
> the same in the mirror.

`src/share.rs` has thirty-two gates and **every fixture in every one of them is authored by the
encoder under test.** *A test population you generated cannot contain the shape you are missing.*
The only cure is bytes crossing the boundary.

---

## What is here

```
bodies/{content_hash_hex}   the CANONICAL HASHABLE BODY of one share entity
EXPECTED.json               what each body is, and what a conformant reader should decode it to
```

A body file is **byte-for-byte what a publisher writes to `content/{aa}/{bb}/{hex}`** —
`ecf_for_hash(type, data)`, the two-key `{data, type}` map. Not a `.bin` tree pointer, not a served
tree: your reader decodes an **entity**, so an entity body is the unit that has to cross. Re-hashing
a body must give the filename it is filed under; that is the property your reader already checks and
it makes the fixture self-verifying with no manifest.

> **If you would rather have a served tree** (peer prefix, `.bin` pointers, `.list` indexes, a signed
> root) than loose bodies, say so — we emit those through `publish_fixture::write_entity` already and
> it is a small change. Bodies were chosen because `B-7` is about **the encoder**, and a tree adds a
> publisher, a keypair and a root to a question that has none of them in it.

---

## ⭐ Nothing here carries a peer id, and that is the opposite of `feed-joint`

`APP-CONVENTION-SHARE` §4 requires a mirrored share to sit at `/{publisher}/…` **verbatim**, so the
publisher is a fact about the **path** — and a `from` field in the body would be a stranger's
self-declared claim about their own identity. `decode_share` therefore takes the publisher as an
**argument**, and a body that smuggles one back cannot override it.

So: no seed, no keypair, no peer to stand up. `EXPECTED.json` states a `publisher` only so the
`decodes_to.from` row has a value, **and so a seat that does read a `from` out of the body has
something to disagree with.**

Contrast `tests/fixtures/feed-joint/`, where `FEED-R1` puts the author **in the bytes** and the
fixture has to pin an Ed25519 seed. **Two conventions, opposite answers on the same axis** — it is
worth knowing which one you are in before you design a fixture for either.

---

## The protocol

1. For each row of `EXPECTED.json`'s `entities`, read `bodies/{content_hash}`.
2. **Re-hash it and check it equals the filename.** If that fails, stop — the fixture is corrupt in
   transit and nothing below means anything.
3. Decode it with your reader, passing `publisher` as the namespace.
4. Compare **field by field** against `decodes_to`, before any hash comparison. A bare hash mismatch
   names the entity and not the field.
5. Then re-encode and compare the bytes. A decode that agrees and a re-encode that does not is the
   most interesting outcome this fixture can produce, and it is the one a round trip against your
   own encoder cannot see.

`decodes_to.audience` is `null` for a publication (the type has no audience at all) and an **array**
for a record — including the **empty array** for the self-only row. Those are three different states
and the fixture carries all three deliberately.

---

## Why each row is here

Each is a case where two implementations can each be internally consistent and still disagree. The
`why` key on each row carries the same reasoning where somebody editing it will read it.

| row | the case |
|---|---|
| `record-one-member` | the floor. **`audience` is an array of ENTITIES (`{type, data}`)**, not a list of peer-id strings — a seat that flattens it produces a plausible record with different bytes |
| `record-self-only` | **SHARE-7, and the most valuable row here.** An empty audience is **self-only, never public**, and the `audience` key is **present and empty**. A seat that reads it as public, or that omits the key when the array is empty, is wrong in the direction that **discloses** |
| `record-two-members-group-origin` | two entries, so authored order is exercised, and the second carries `via: "group"` — §2.3's other origin token, which records that a group membership produced the entry **at authoring time** and is explicitly not resolved at check time. Also **no `note`** |
| `record-member-without-via` | `via` is optional and this member has none, so the key is **absent inside the entry's `data`** — not `"direct"` defaulted in on the way out, which would emit a claim the author never made |
| `publication` | `app/share/publication`, §2.5's public pull-only half, carrying **no `audience` key at all**. SHARE-8 makes a publication carrying one **invalid** rather than something to skip: §2.6's must-ignore would otherwise launder the one shape this type exists to exclude into an ordinary-looking row |

**Both `target` arms are covered** — a `blob` (tagged, with a **self-describing** `content-hash`:
format varint then digest, never a fixed 32 bytes, which is SHARE-5) and a `prefix`. The tagged union
is §2.2's *"TAGGED — no untagged ambiguity"*; inferring the variant from which sibling key is present
is the shape the CDDL rules out by hand, and it is what **we** used to emit.

**What is deliberately not here:** `size`, which we no longer persist at all (recoverable from the
content store, so the surface that wants it looks it up), and `from`, for the reason above.

---

## Changing anything here

```
make test-one T=share_crossimpl EXTRA_RUN_ENV="-e SHARE_CROSSIMPL_REGENERATE=1"
```

and that is the only way it should ever change. **A regeneration not accompanied by a deliberate
edit to `rows()` in `src/share_crossimpl_fixture.rs` is a wire event, not a test fix** — you are
decoding these bytes, so if they moved on their own, our encoder moved, and the question is which
side is right.

**A divergence is routed, not corrected.** Correcting our side to match yours without establishing
which is right converts a cohort disagreement into a silent divergence with one repo's name on it —
which is exactly what the `app/share/manifest` → `app/share/record` move avoided by being negotiated
rather than blinked into.

# `J-4` — one fixture, two publishers, one feed index

`APP-CONVENTION-FEED`'s vocabulary has **two seats and one implementation**. This directory is our
half of the joint run, shaped so the other seat's half is a small program and not a rig — the same
move that unblocked `G-PIN-4`'s link 2.

`entity-workbench-go` answered `W-4` **yes** (FEED this cycle, stage 1 only) and accepted `J-5`'s
split unchanged: **they produce, we consume and run the rig.** They asked one thing back when they
accepted — *a per-key report before any root comparison, because a bare root mismatch costs a
session to localize* — and that is what the protocol below is ordered around.

| file | what it is |
|---|---|
| `feed.json` | the fixture as **authored input**: two pinned Ed25519 seeds, a page size, five entries. Plain JSON, hand-edited |
| `EXPECTED.json` | what **this** implementation computes from it — every entry hash, every detached signature, the head, every page, and the trie root over the §4.2-pinned keys. **Computed, never hand-edited** |

---

## ⛔ Read this before anything else: what is comparable here is NOT what was comparable for a site

`gpin4-joint/README.md` says, in as many words, *"the peer id and the keypair do not matter."*
**Neither sentence survives here, and both failures are silent** — you get plausible entities with
different bytes and nothing anywhere says why.

| | site | feed |
|---|---|---|
| peer id in the entity body | no | **yes.** `FEED-R1` makes an entry's `author` equal the namespace it is read under, and every index page is a list of `pin(author, entry_hash)` |
| keys pinned by the convention | every one, relative to the site root | **the index only.** §4.2 pins `app/feed/index` and `app/feed/index/{page}`. §2 says *"the cross-impl contract is the type tag, not the path"*, so where an **entry** lives is a local choice |

Two consequences:

1. **The fixture pins a SEED, not a peer id.** A literal peer id would make the entry bodies
   comparable and leave `FEED-R2`'s detached signature out of the comparison entirely — you cannot
   sign as a peer whose key you do not hold. Ed25519 signing is deterministic (RFC 8032), so a
   pinned seed makes the peer id, the identity entity and every signature byte reproducible on any
   implementation, and *"plus each entry's detached signature"* becomes a thing you can check.
   **The seeds are test vectors. Nothing is ever published under them.**
2. **There are two roots and only one of them is a comparand.** See the table at the end.

The `author` peer id derived from `feed.json`'s seed is:

```
2K42FX8pASWDrXaAVsGXMNJbAkCVBuVwXf4RuwFNnmyYis
```

**If your implementation derives a different string from that seed, stop there** — every entry body
and every index page carries it, so nothing downstream can agree and the per-key report will name
all nine rows at once. That is a V7 §1.5 peer-id construction difference, not a FEED one.

---

## The protocol

**Step 0 — the peer ids.** Derive Ed25519 keypairs from `peers.author` and `peers.forum`, take the
canonical peer id of each, and check `author` against the literal above.

**Step 1 — the entries.** For each row of `entries`, in order, build one `app/feed/entry` (§2.3):

| `feed.json` | entity field |
|---|---|
| — | `author` = the **author** peer id (`FEED-R1`) |
| `created_at` | `created_at`, unsigned, milliseconds |
| `body` | `body`, an `embed-node` (`APP-CONVENTION-EMBED` §3.1) |
| `reply` | `reply: {root, parent}`, both **pinned** references (`FEED-R6`) |
| `context` | `context`, a pinned **or** live reference |
| `prev` | `prev`, a **bare hash** — not a reference atom (§2.3.1) |
| `attachments` | `attachments`, an array of references |

**Every optional key is emitted only when the row carries it.** An absent `reply` and an empty one
are different facts and only the first is representable.

Two conventions the JSON cannot express and that you have to take from here:

- **A hash-valued field is either a 66-character wire-hex literal** (an opaque pin to something
  outside the fixture — the leading `00` is the format varint and is part of the address, V7 §1.2)
  **or the object `{"entry": "<name>"}`**, which resolves to the content hash of an **earlier**
  entry. Forward references are refused and your harness should refuse them too: an entry's hash is
  a function of its bytes, so an entry naming a later one cannot be encoded.
- **In `body.params`, a JSON string is an ECF text value and a JSON integer is an ECF unsigned
  integer.** Both are in the fixture on purpose, so an implementation that stringifies numbers into
  the open bag is caught by a hash rather than by review.

Compare each entry's content hash against `EXPECTED.json`'s `entries[].entry`, **in order, one row
at a time, before you compare anything else.**

**Step 2 — the detached signatures (`FEED-R2` / §1.1).** For each entry, mint a `system/signature`
over the **entry's content hash**:

- `target` — the entry hash
- `signer` — ⚠ **the content hash of the signer's identity entity, NOT the peer-id string.**
  §1.1's shorthand is *"`signer = author`"*, and `system/signature` is the **kernel's** type whose
  own field doc shouts the parenthesis. An implementer reading FEED alone puts a peer id in a slot
  that wants a hash and produces something no verifier can use.
- `algorithm` — `ed25519`
- `signature` — Ed25519 over the entry hash's **wire bytes**

Compare against `entries[].signature`. Also compare **`entries[].signature_key`**, which is a real
expectation and not a local choice: V7 §3.5's invariant pointer pins it at
`system/signature/{entry_hash_hex}` under the author's namespace.

**Step 3 — the index (§4.2).** Page the entries **oldest-first** at `page_size`, and within each
page list them **newest-first** (§4.5).

> **These two run against each other by design, and getting them backwards round-trips
> perfectly.** Fill newest-first and every publish shifts every entry one slot, so the whole
> archive is rewritten and every reader's cursor dies — which defeats §4.3 rule 1's stated cost
> property through the ordinary act of posting. A harness that only re-reads its own index passes
> with **both** reversed.

- each page is an `app/feed/index-page` with `page` (= its key), `entries` (pinned refs),
  `updated_at`
- `updated_at` on a page is the **maximum `created_at` of the entries it carries** — a witness, not
  the publish instant, so a page nobody touched reproduces its own stamp forever. §4.2 gives the
  field no semantics; this is our reading and it is routed as `A-41`. **If you take the other one,
  say so rather than matching ours silently** — one of us becomes the baseline either way and it
  should be on purpose.
- the head is an `app/feed/index-head` with `current` (the highest page number) and `updated_at`
  (here, the feed's own high-water mark), and **no `oldest` key** — §4.2 declares 0 as the default,
  so a publisher who has dropped nothing emits nothing

Compare `index_bindings` key by key, then `index_root`.

**Step 4 — the root.** Build an `EXTENSION-TREE` v4.0.2 trie over `index_bindings` and compare
against `index_root`. Peer-relative keys, exactly as written.

---

## The two roots

| key | over | is it a comparand? |
|---|---|---|
| `index_root` | `app/feed/index` + `app/feed/index/{0,1,2}` | ✅ **yes.** §4.2 pins these keys by hand |
| `feed_root_ours` | …plus our entry keys and signature keys | ❌ **no.** `app/feed/entries/{hex}` is **ours** — §2 leaves the path local — so this exists only as our own drift detector |

Same split as `gpin4-joint`'s `site_root_pages` / `site_root_full`, for a different reason: there
the second root waits on a **type** the other seat has not built; here it waits on a **path** the
convention declines to pin. If the two seats ever agree an entry prefix, that agreement is what
makes `feed_root_ours` comparable — and it would be a convention question, not a local one.

**Neither root is a published-root head.** That entity carries `published_at`, a wall clock, so its
hash moves on every run — measured on the site fixture, where two runs of one publisher under one
pinned seed gave different heads with an identical trie root. `EXTENSION-TREE` §3.2's determinism
rule 3 states it from the other side: *"No timestamp — a snapshot is pure structural data."*

---

## Why each row of `feed.json` is there

Each is a case where two implementations can each be internally consistent and still disagree, so a
fixture without it goes green while the divergence is live. The `_why` key on each row carries the
same reasoning where somebody editing it will read it.

| row | the case |
|---|---|
| `plain` | **the floor** — every optional key ABSENT, not empty. An `omitempty` disagreement shows up here first |
| `params` | **two `params` keys**, so canonical map ordering inside a nested map is exercised (ECF orders by length then lexically and the encoder gets no say — which is *not* a `BTreeMap`'s natural order), and **one text value plus one unsigned integer** |
| `reply` | `reply.root` and `reply.parent` **distinct**; an impl that collapses them, or emits one twice, produces a plausible entry with different bytes |
| `linked` | a **live** reference carrying `seen`, an `at` anchor and a `via` hint — including **one hint kind nobody knows**, which `REF-R7`/`REF-R8` require be carried and never acted on — plus an `attachments` **pin to a peer that is not the author** |
| `chained` | `prev` (§2.3.1's opt-in chain, a **bare hash**) and EMBED §3's **pointer** payload arm, which is the only conformant way to carry a body over the 16 KiB inline ceiling. An all-inline fixture cannot see an implementation that never built it |
| *the index* | **five entries at `page_size` 2 is three pages with a PARTIAL last page** — a single-page feed passes a build that a paginated one fails |

**What is deliberately NOT here.** A `child` payload: it names a sibling `Embed` **entity** and
neither seat mints one, so a row for it would pin a shape nobody can produce. An `app/feed/follow`
record: §2.4 makes it *the reader's private data*, published by nobody, so it has no cross-impl
bytes. `collection` and `mirror`: not stage 1's.

---

## Changing anything here

`EXPECTED.json` is regenerated with, **in this repo's container** (the host needs only `make` +
`podman`):

```
make test-one T=feed_joint EXTRA_RUN_ENV="-e FEED_JOINT_REGENERATE=1"
```

and that is the only way it should ever change.

**A regeneration that was not accompanied by a deliberate `feed.json` edit is a wire event, not a
test fix.** The other seat compares against these bytes; if they moved on their own, our encoder
moved, and the question is which side is right.

**A divergence in step 1, 2 or 3 is routed, not corrected.** Neither seat can rule the convention,
and correcting our side to match yours without establishing which is right converts a cohort
disagreement into a silent divergence with one repo's name on it.

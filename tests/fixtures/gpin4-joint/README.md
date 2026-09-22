# `G-PIN-4` — one fixture, two publishers, one site root

`APP-CONVENTION-SEMANTIC-CONTENT-SITE` §9 lists a **reproducible-publish** check as
`[REQUIRED before ratification]`, and says outright that *"the fixtures, the bytes and the run are
the implementations' and the conformance oracle's."* This directory is our half of it, shaped so the
other seat's half is a small program and not a rig.

**G-PIN-4 decomposes into three links.** Both seats reached that decomposition independently and
agreed on which one was unmeasured:

| link | state |
|---|---|
| **1. source directory → site entity** | `source/` + `EXPECTED-INGEST.json` — **NOT a conformance surface; see below.** The convention specifies no authoring format, so two conformant publishers may lower one markdown file differently |
| **2. site entity → canonical bytes** | `site.json` + `EXPECTED.json`; **AGREED, measured both directions.** Forward: `make crossimpl-site` runs this fixture through their `entitysdk` — 8 keys byte-identical. Backward: they decoded our August emission through their types and re-encoded byte-identically (3 manifests, 11 pages) |
| **3. binding set → CHAMP root** | **AGREED, measured both directions.** Forward: `make crossimpl-site` rebuilds `site_root` through `entity-core-go/core/tree` and it matches. Backward: their reader walked our signed root, collected 15 bindings and rebuilt it in a fresh store |

**`make crossimpl-site` is how you run links 2 and 3 — one command, no packet.** It reads
`site.json`, builds the entities through **entity-workbench-go's own types** and the trie through
**entity-core-go's own `core/tree`**, and prints a per-key report. It consumes their code; it does
not model it. Source: `tools/crossimpl/gpin4/main.go`.

## What link 1 is and is not

**`APP-CONVENTION-SEMANTIC-CONTENT-SITE` specifies no authoring format.** There is no `render/`
directory, no `site.manifest.json` and no `+++` frontmatter block anywhere in its twelve sections.
§0 calls frontmatter *"optional local flavor not the contract"* — a clause **this repo contributed**,
per the v0.4.2 provenance line — and §4's CDDL says *"title-only is conformant; **MAY** derive title
from first H1"*.

So a source-lowering difference is a **permitted** difference, not a conformance failure, and the
two divergences listed further down are precisely the case the spec already blesses both ways. There
is also nothing to compare against today: `entity-workbench-go` has no source-directory site ingest
(no TOML parsing, no `+++` handling, no manifest-file reader, on any branch — their
`workbench/ingest_tree.go` is a *doc-tree* ingest emitting `doc/markdown-file`).

**These files are kept and are useful.** They pin our own ingest against silent drift, and they make
our lowering choices legible if the two seats ever choose to converge on an authoring format. What
they are not is G-PIN-4's deciding link — which is what this file used to say.

So this directory carries **two fixtures, at two different layers**, and neither is redundant:

- **`source/`** — a real content-team `render/` directory: `site.manifest.json`, markdown pages with
  `+++` TOML frontmatter, one `.html` page, one asset. This is link 1's input.
- **`EXPECTED-INGEST.json`** — what **our** ingest makes of `source/`, field by field, plus **two**
  trie roots. Read the `roots_are_split_because` key before comparing.
- **`site.json`** — the fixture as **authored input at the entity layer**, bypassing any source
  parsing. Plain JSON. This is link 2's input.
- **`EXPECTED.json`** — every site-relative key → content hash and the trie root, from `site.json`.

**Why both.** `site.json` isolates the *encoder* — it pins what the bytes are given agreed field
values, with no markdown or frontmatter parsing in the way. `source/` pins the *ingest* — which is
where trailing newlines, title derivation, slug derivation and nav construction actually diverge.
A gate with only the first can't see a lowering disagreement; a gate with only the second can't tell
a lowering disagreement from an encoding one.

**And they cover different cases on purpose.** `site.json` carries a nav section with **no `target`**
(the spec's `nav-node.? target`); our source-directory ingest **cannot produce one** — `parse_nav`
derives a header's target from its children — so that arm is reachable only from the entity layer.
Pinned by `no_nav_node_from_a_source_directory_lacks_a_target`.

`G-PIN-4` passes when a second publisher, over a **different core**, computes the same values.
§2 is explicit about why that qualifier matters: *"independent means built over different cores — two
front ends linking the same core cannot disagree about it and do not constitute two."*

## The protocol for the second publisher

**Link 2 (the encoder) — start here, it is smaller:**

1. Read `site.json`. Build one `app/site-manifest` from `site_id` / `title` / `nav` / `params`, and
   one `app/site-page` per entry in `pages` from `format` / `body` / `frontmatter`.
2. Bind them at **site-relative** keys: `manifest`, and `pages/{slug}`.
3. Compare each entity's content hash against `EXPECTED.json`'s `bindings`. **Do this before
   comparing roots** — a bare root mismatch costs a session to localize, and the per-key comparison
   names the entity that diverged.
4. Build an `EXTENSION-TREE` v4.0.2 trie over that binding set and compare the root against
   `site_root`.

**Link 1 (the ingest) — a shared authoring note, NOT a conformance gate. Read "What link 1 is and is
not" below before comparing anything here:**

1. Read `source/` with your own source-directory ingest.
2. Compare against `EXPECTED-INGEST.json`'s `entities` — manifest fields, and per page the `slug`,
   `format`, `frontmatter` and `body`. **Field by field, before any hash.**
3. Compare `site_root_pages`. Leave `site_root_full` until you model `app/site-asset` — see the asset
   section below, which changed on 2026-09-10: the row is now reproducible on our side, and what is
   still missing is a counterpart to compare it to.

**A divergence in link 1 is routed, not corrected** — it is a disagreement about what a source
directory means, and neither side's reading is privileged. Two we already expect you to hit:

- **`glossary` has no frontmatter block, and our ingest stores `frontmatter: {"title": ""}`** — an
  empty string, not an absent key. Our own `.html` path deliberately removes an empty title, with the
  reason written down (*"an empty title would render as a blank breadcrumb, which reads as broken"*).
  The markdown path does the opposite. **We pinned it rather than repairing it**, because `{"title":
  ""}` and an absent key are different bytes and the fix has to be one decision taken by both seats,
  not a silent repair made while you are comparing against these bytes.
- **`site.manifest.json`'s `tagline` and `theme` land in `params`**, alongside `root`. The tree model
  has no dedicated field for either and `params` is the spec's open attribute bag.

Nothing above needs a shared keypair, an agreed tree path, a network, or a pinned clock. See below
for why each of those drops out.

## The asset row: REPRODUCIBLE as of 2026-09-10, not yet COMPARABLE

> **This section said the opposite until 2026-09-10, and the correction is the useful part.**
> It read *"the asset row is known-incomparable"* because **our own representation was
> unreproducible**: `app/site-asset` put the raw bytes inline at any size, so there was no chunker on
> the path to disagree about parameters with, and §6.1's canonical `chunk_size` MUST was **bypassed**
> rather than untested. `entity-workbench-go` had diagnosed the symptom (*"nothing in either tree
> exercises chunking"*) and recommended an asset over 16 KiB; the diagnosis was right and the fixture
> alone could not reach §6.1. **Arch made the reasoning normative** —
> `APP-CONVENTION-SEMANTIC-CONTENT-SITE` §4 now carries a `[MUST]`: an asset whose bytes exceed
> `inline-payload`'s `.size (1..16384)` ceiling MUST use a `pointer` payload, *"placing the bytes in
> the content store where §6.1's canonical chunking governs them"*, with the note that on an
> all-inline path the MUST *"is not failing, it is unreachable."*

**We implement the pointer arm.** `assets/figures/big.svg` (32,047 bytes) now encodes as
`{media_type, payload: {tag: "pointer", hash}}` — `APP-CONVENTION-EMBED` §3's tagged union, reused
rather than restated — and its bytes live in the content store as a `system/content/blob` plus chunks,
chunked at §6.1's locked default (**1 MiB-average FastCDC**, min/avg/max 256 KiB / 1 MiB / 2 MiB).

So `EXPECTED-INGEST.json`'s asset row now pins something a second implementation can reproduce
**without seeing our code**: `payload.blob` is a function of the source bytes and the canonical
chunker's parameters, and nothing else. Our own gate asserts exactly that equality rather than
comparing our pipeline against itself.

**The two roots stay, for a different and weaker reason.** `entity-workbench-go` does not model
`app/site-asset` at all (their `assets/{name}` is reserved for post-v1 passive-Embed work), so there
is still nothing on the other side to compare the row *against*. What changed is which side the gap
is on:

| | before | now |
|---|---|---|
| our asset bytes reproducible by another impl | **no** — inline, chunker bypassed | **yes** — canonical FastCDC blob hash |
| a second impl models `app/site-asset` | no | no |

Compare **`site_root_pages`** until a second seat implements the type. Compare **`site_root_full`**
the day one does — and note that it now *should* match, which is a claim the previous version of this
section could not make.

**If you are the seat implementing it:** the row you need to reproduce is the asset's
`payload.blob`, and it needs only the source file and the canonical chunker. The `content` array in
`EXPECTED-INGEST.json` lists the blob and chunk entities by hash so a mismatch names which one moved
rather than only moving the root.

## Four things that make this comparable at all

**1. The comparand is the trie root, never a published-root head.** `system/peer/published-root`
carries `published_at`, a wall-clock read, so its content hash moves on every run — measured here,
not inferred: two runs of one fixture under one pinned identity seed produced heads
`008615f3b44c09c7…` and `00a7b337ea9fd493…` with **`root_hash` identical** and 13 of 15 content
blobs shared. A rig comparing the head — which is what `make site-dist` prints under the words
*"signed root"* — reds one hundred percent of the time, and across two implementations it reds
looking like a real divergence. `EXTENSION-TREE` §3.2's determinism rule 3 states the property from
the other side: *"No timestamp — a snapshot is pure structural data."*

**2. Placement does not matter.** Our site subgraph is at `/{peer}/sites/{site_id}/…`; workbench-go's
is at `/{peer}/content/sites/{site_id}/…`. Both are conformant — §2's v0.5 ruling makes a site *"a
free subgraph at any publisher-chosen tree path"* — and **relative to the site root both spell
`manifest` and `pages/{slug}`**, which is what the trie is keyed by. §3.2 again: *"snapshot of
`/alice_id/local/files/` and `/bob_id/local/files/` both produce bindings keyed by the same relative
paths."*

**3. The peer id and the keypair do not matter**, for the same reason — neither appears in a
site-relative key or in a leaf's `(type, data)`.

**4. The substrate is already known to agree.** `src/content_site/crossimpl_reproducible_publish.rs`
runs the same comparison over workbench-go's vendored `entity-core-go` emission
(`tests/fixtures/crossimpl-go-site/`) and is green: canonical ECF and the v4.0.2 HAMT produce
identical hashes and an identical root across the two cores. What is **not** yet established, and
what this fixture is for, is the `app/site-*` **vocabulary**.

## Why each row of `site.json` is there

Each is a case where two implementations can each be internally consistent and still disagree, so a
fixture without it goes green while the divergence is live:

| Row | The case |
|---|---|
| `glossary` | a page with **no frontmatter** — the key must be **absent**, not an empty map (`omitempty` on their struct, conditional on ours; an empty map is different bytes) |
| `about` | **two** frontmatter keys, so map ordering is exercised — ECF orders canonically (length, then lexical) and the encoder gets no say |
| `Reference` nav entry | a section with **no `target`** (spec `nav-node.? target`) — optional-absent, one level in |
| `Guide` nav entry | **nested `children`**, emitted by both sides only when non-empty |
| `raw` | a **non-default `format`** (`html`), so `format` is not a constant a hardcoded default could match by accident |
| `guide/advanced/internals` | **two levels** of page nesting, so the trie has interior nodes — a flat site can pass a build a nested one fails |

## Changing anything here

`EXPECTED.json` is regenerated **in this repo's container** — the host needs only `make` + `podman`:

```
make test-one T=gpin4_joint EXTRA_RUN_ENV="-e GPIN4_REGENERATE=1"
```

and that is the only way it should ever change.

> This line used to read `GPIN4_REGENERATE=1 cargo test --bin entity-browser gpin4_joint`, which is
> a **host** invocation and fails on a podman-only box — `openssl-sys` cannot find `openssl.pc`, and
> the build dies before any test runs. Corrected 2026-09-12, when the FEED joint fixture needed the
> same command and it did not work. *A documented invocation is a coupling no compiler maintains;
> run it before you write it down.*

**A regeneration that was not accompanied by a deliberate `site.json` edit is a wire event, not a
test fix.** The other seat compares against these bytes; if they moved on their own, our encoder
moved, and the question is which side is right.

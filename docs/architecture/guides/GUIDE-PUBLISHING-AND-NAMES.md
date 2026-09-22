# Publishing & Names — the trust chain, end to end

> **The publishing model lives in one place:** [`REFERENCE-PUBLISHING-PIPELINE.md` §0.1](../specs/REFERENCE-PUBLISHING-PIPELINE.md#01-the-model-in-one-screen--read-this-before-changing-anything-that-publishes) — *publish translates an input into the tree, then projects the tree.* This document covers names and the registry; it does not restate the model.

**Audience:** anyone publishing content or names to the entity network, and
anyone implementing a consumer of it. **Scope:** the three things you can
publish — a **site**, a **name registry**, and the **app** that reads them — how
each is emitted, verified and served, what a consumer checks, and exactly what
is *not* yet closed.

> **The one-sentence version.** A publisher signs a root over its own content
> tree; a registry — a *different* key — signs `name → peer-id` bindings and a
> root over those; a consumer pins **one peer-id** and walks two signed roots to
> a page, trusting the web server for nothing.

Every command and number in this guide was run on 2026-08-18 against `dev`. Where
something is unproven, it says so.

---

## 1. Three identities, and the user's is not one of them

This is the thing most worth getting straight before any command, because the
diagram is not what people assume:

```
  ┌──────────────────┐        ┌──────────────────┐        ┌──────────────────┐
  │  REGISTRY key    │        │  PUBLISHER key   │        │  the USER's       │
  │                  │        │  (one per domain)│        │  browser peer     │
  │ signs            │        │ signs            │        │                   │
  │  name → peer-id  │──────▶ │  the content     │        │  involved in      │
  │  bindings        │  names │  tree's root     │        │  NEITHER          │
  └──────────────────┘        └──────────────────┘        └──────────────────┘
          ▲                            ▲
          │ the ONE string             │ learned, never configured
          │ a consumer pins            │
     ┌────┴─────────────────────────────────┐
     │  resolve: name → binding → peer-id   │
     │           → that peer's root → page  │
     └──────────────────────────────────────┘
```

- The **registry key** and the **publisher key** are deliberately different. One
  key doing both jobs means trusting a name-issuer to also *be* the thing it
  names. `tools/local-federation.sh` keeps them separate for exactly this reason,
  and so do the two durable identities on disk (§2.1).
- The **user's browser peer** is not registered, nothing is published under it,
  and no signature mentions it. If you find yourself wanting to publish under the
  visitor's identity, the model has been misread.
- The pinned string is a **peer-id, not a public key**. For Ed25519 canonical
  form a peer-id *embeds* its 32 public-key bytes (`PeerId::derive_public_key`,
  `hash_type = identity`), so pinning is a decode, not a key fetch. There is no
  key-distribution problem to solve. The legacy SHA-256 form does *not* carry its
  key and genuinely needs one out of band — `PinnedPublisher::from_peer_id`
  returns `None` for it rather than guessing.

---

## 2. Publish a site

### 2.1 Identity first — it is the thing consumers pin

A publish is only useful if its peer-id is **stable across republishes**, because
every binding that names it, and every consumer that pinned it, is keyed on that
id. Two ways to fix it:

| | where the key lives | use for |
|---|---|---|
| **durable** (default) | `{ENTITY_DATA_DIR}/publish/keypair` | a real deployment publishing from one machine/CI |
| **`--identity-seed=<64 hex>`** | nowhere — derived | reproducible builds, fixtures, multi-domain scripts |

Generate a real seed with `openssl rand -hex 32` and keep it wherever you keep
secrets. **A registry has its own separate durable identity**
(`{ENTITY_DATA_DIR}/registry/keypair`) — see §3, and note the consequence in **§3.3**:
verifying a registry needs the `registry` verb, not `publish --verify`.

### 2.2 Emit

```bash
make site OUT=dist-mysite IDENTITY_SEED=$(cat publisher.seed)
```

`make site` is headless and containerized (host needs only `make` + `podman`).
The full knob table — `INGEST`, `PREFIX`, `LIVE`, `SURFACE`, `DEPLOY_CONFIG`,
`CONFIG_SITE` and the rest — lives in
[`GUIDE-DEPLOYMENT-AND-CONFIGURATION.md` §6](./GUIDE-DEPLOYMENT-AND-CONFIGURATION.md#6-the-make-site-command-surface),
which is authoritative for it. This guide covers only what the *trust chain*
needs.

What lands in the output directory:

```
{OUT}/{peer-id}/sites/{site}/pages/{slug}.bin   tree leaves — `system/hash` POINTERS
{OUT}/{peer-id}/sites.list                      a listing artifact (transport-trusted)
{OUT}/{peer-id}/system/peer/published-root      the SIGNED ROOT  ← the trust anchor
{OUT}/{peer-id}/system/signature/{hash}.bin     the detached signature
{OUT}/content/{aa}/{bb}/{hex}                   the bodies, content-addressed
{OUT}/sites/…                                   legacy-web `.html` projection
```

Two shapes here surprise people, and both cost us a bug:

1. **Trie interior nodes appear in no listing.** They are reachable only by hash
   from the signed root. An emit path that skips `write_entity` produces a tree
   that verifies pointer-by-pointer and cannot be *walked* — and the failure
   surfaces to a consumer as `Ok(None)`, indistinguishable from "that page does
   not exist".
2. **`published-root` is not a `system/hash` pointer.** It is the 3-key *wire*
   entity, because verification reads its `content_hash`, which the bare hashable
   form does not carry. It deliberately has no `.bin` suffix so the pointer sweep
   does not mistake it for one.

### 2.3 Verify — before you upload, not after a visitor complains

```bash
make site OUT=dist-mysite IDENTITY_SEED=$(cat publisher.seed) VERIFY=1
```

`--verify` reads an **already-published** directory (it returns before any emit,
so it needs the identity and nothing else — not the sources). It checks four
independent properties:

| check | catches |
|---|---|
| every `.bin` pointer cracks and names a blob that exists | a truncated upload |
| every blob's bytes hash to the address claiming them | a tampered or corrupted body |
| bytes are the **canonical** pre-image, not merely hash-equal | appended garbage — `ciborium` ignores trailing bytes, so this passes a naive hash check |
| the signed root verifies **and its closure is complete** | a withheld interior node: the shape where every pointer resolves and a pinned consumer still resolves *nothing* |

Exit codes: `0` clean · `2` defect found · `1` could not run. **Orphan blobs are
reported, not failed** — nothing links to them, and a tree mid-cutover
legitimately holds blobs its pointers have not adopted.

> **The fourth row is new as of this audit and was previously unenforceable.**
> The closure walk discovered candidate hashes by scanning bytes and only
> enqueued ones *already present*, so a missing node could never be counted —
> the guard existed, was recorded as landed, and could not fire. Measured on a
> 24-name registry: withholding one interior node took the closure from 56 to 54
> blobs and reported **`0 missing`, exit 0**. It is now a structural check — a
> trie node *declares* its children, so each declared child is required to exist —
> and the same case reports `1 missing`, names the withheld hash *and the parent
> that declared it*, and exits 2. Gate:
> `registry_verify_passes_a_clean_tree_and_fails_a_withheld_interior_node`.

### 2.4 Serve

Two properties, and you do not get to skip either:

**CORS.** If the app and the content are on different origins — which is what
real domains are — the content origin **MUST** send
`Access-Control-Allow-Origin`. `python3 -m http.server` does not. `curl` will not
tell you: a native client happily reads a response a browser discards. Use
`tools/cors-serve.py` locally.

> ⚠️ **The normative CDN runbook is not in this corpus.**
> `RUNBOOK-CDN-BROWSER-DEPLOYMENT.md` — which `EXTENSION-NETWORK` Amendment 5
> points at normatively for exactly these headers — has not been published
> alongside the specification it is cited from. Routed for pull-in
> (`ROUTING-2026-08-18-a`); until it lands, this section and
> `tools/cors-serve.py` are the working reference, and the runbook wins on
> overlap once it arrives.

**Cache headers keyed on mutability.** Content is hash-addressed and therefore
immutable forever; the signed root is the one mutable file, and a stale one
pins a consumer to an old tree. Measured on `tools/cors-serve.py`:

```
GET /foundation/{peer}/system/peer/published-root
  → 200 · Access-Control-Allow-Origin: *  · Cache-Control: no-store
GET /foundation/{peer}/sites.list
  → 200 · Cache-Control: no-store
GET /foundation/content/{aa}/{bb}/{hex}
  → 200 · Cache-Control: public, max-age=31536000, immutable
```

---

## 3. Publish a name registry

A registry is **the same emitter at different keys** (`EXTENSION-REGISTRY` §7.4 —
*"the registry is itself a coral reef, a static publisher in dormancy"*). No live
peer is required at any step.

### 3.1 Emit

```bash
make registry \
  REGISTRY_OUT=dist-registry \
  TTL_DAYS=30 \
  SEED=--identity-seed=$(cat registry.seed) \
  BIND='--bind=example.org=2KGTrr4L…skz9sS@https://example.org
        --bind=docs.example.org=2KFRBJ9f…5D2sS@https://docs.example.org'
```

Or the CLI directly:

```bash
entity-browser registry OUT_DIR --bind=NAME=PEER_ID@ORIGIN [--bind=...] \
  [--ttl-days=N] [--identity-seed=HEX]
entity-browser registry OUT_DIR --verify
```

Four rules the emitter enforces, each refusing **before anything is written** and
naming **every** offender rather than the first:

1. **`@ORIGIN` is mandatory** (arch D10). A binding with no `transports` resolves
   to a peer-id the consumer has no way to reach. `EXTENSION-REGISTRY` §4.1.2
   promises a §6.5 transport fall-through, and §6.5.4 makes profile discovery
   out-of-band in v1 — so for a *statically published* target there is nothing to
   fall through to. The §4.1.2 **pin** carve-out is untouched: a pin is the
   user's own assertion, so reaching that peer is the user's problem.
2. **The target must be a peer-id.** Added by this audit, found by typing the
   wrong thing: `--bind=example.test=foundation@…`, a directory slug where the id
   belonged, emitted a fully signed binding whose only symptom appeared on a
   *consumer's* machine as *"the named peer-id carries no public key"*. The
   operator, the one person who could fix it, saw a success message. The bar is
   `PeerId::validate` — well-formed in either Ed25519 form — not
   pinnable-by-us, because refusing a legacy-form target would be this emitter
   deciding what other consumers may resolve.
3. **`ttl` cannot be null.** Not a flag with a default — there is no way to
   express it. §6a.4 permits `null`; arch's D3 makes non-null a MUST, because a
   null-TTL binding whose revocation a hostile origin **withholds** is
   permanently unrevokable. `issued_at + ttl` is the one check on this path a
   hostile byte-server cannot influence. **Both are milliseconds** — emitting
   seconds yields a permanently-expired binding whose resolve returns `None`, the
   same answer as a bad signature or a revocation.
4. **Names are normalized once, by the shared function.** NFC, **no case
   folding** (`normalize_name(name, "none")`). Emitter and resolver call the same
   upstream function; re-deriving "what normalization probably means" on one side
   is how the two ends drift apart silently.

Measured output of a 4-name registry:

```
registry published → dist-federation/registry (4 binding(s), ttl 30 day(s))
  registry peer: 2KBLkCxvkgobuauPA6zPfKarpuRRnnWHL98n8Gv1GNmybr
  signed root: 00de12ed300f7bd1 (seq 0, 12 keys, 2 trie nodes)
```

### 3.2 What is committed, and by what

| artifact | key | authenticated by |
|---|---|---|
| binding body | `system/registry/binding/{hash}` | its content address |
| by-name index | `system/registry/binding/by-name/{norm}` | **the binding's own signature** |
| §5.2 signature | `system/signature/{binding-hash}` | the registry key |
| identity entity | (reachable by hash only) | its content address |
| name listing | `system/registry/binding/by-name.list` | **nothing — transport-trusted** |

The association is committed by the **per-binding signature**, not by the index:
the signature covers a body carrying `name`, so `sig(R, {name, target_peer_id})`
*is* the commitment. The identity entity must be projected or every signature
verifies nothing — `sig.signer` resolves through it. The by-name index is one
entity under **two** trie keys (`record_hash`, never a second `record`), or the
content address stops being the dedup.

### 3.3 Verify

```bash
entity-browser registry dist-registry --identity-seed=$(cat registry.seed) --verify
```

`make registry` runs this automatically after every emit. **Use this verb, not
`publish --verify`** — the two resolve *different* durable identities, so
`publish` would look under the publisher peer-id for a tree the registry emit
wrote under the registry one, and report a clean tree as unverifiable.

Measured on the 4-name registry: *13 pointers, 13 verified, 0 broken; signed root
verifies against the publisher key, 10 blobs in its closure, 0 missing.*

### 3.4 Enumeration is a menu, not an inventory

`by-name.list` tells a browser what names to *offer*; it commits to nothing. A
hostile origin can hide a name or invent one (an invented one fails to resolve).

The commonly-repeated reason for this — *"a signed root cannot answer what keys
exist"* — is **false**, and was refuted by arch (`1782d6d`).
`EXTENSION-TREE` §3.1's leaf is `[key, value_hash]`, so the keys are *in* the
nodes. The two reasons a browse surface was still held are **both now closed**:

1. ~~The browser-side reader has no trie decoder.~~ **`SignedSession::enumerate`**
   walks the HAMT from the verified `root_hash` and returns the key set under a
   prefix. Publish at `prefix: system/registry/binding/by-name/` and the trie's
   key set **is** the name set. `entity-tree` is a shared dependency now, not
   native-only — it is a pure codec and builds for wasm32 unchanged.
2. ~~Even the signed walk is silently short.~~ True of upstream
   `collect_bindings_into`, which is why **you must not call it**: it skips a
   missing `Entry::Link` with a bare `if let Some(..)` and returns a `BTreeMap`,
   not a `Result`, so withholding one interior node hides names with no error and
   a root hash that still verifies (measured: 1 of 24). Ours **fails** —
   `IncompleteWalk` on a declared child it cannot fetch, a verification error on
   a node that does not decode or is of the wrong type. That is arch's D9 for the
   one operation that did not satisfy it; resolution already did.
   Gate: `enumerating_a_signed_root_fails_on_a_withheld_node_rather_than_shortening`,
   mutation-checked by restoring the tolerant behaviour.

**What a signed enumeration does and does not claim.** It is authoritative about
what *this root commits to* — not about what the publisher knows. That is
strictly stronger than the `.list` artifacts, which commit to nothing, and it is
weaker than omniscience. **A surface may now show a signed listing; it must still
resolve anything it is about to act on**, and it must treat an unresolvable entry
as a FAILURE rather than the end of a branch.

---

## 4. Publish the app (the WASM route)

The browser app is **one generic bundle**, shaped per-domain by a fetched JSON
file — never rebuilt per customer.

```bash
make wasm            # debug → dist/
make wasm-release    # optimized (opt-level=z + LTO + wasm-opt -Oz) → dist/
make dist-web        # the release bundle as a tarball → artifacts/
make build-serve     # release build THEN serve on :8081
```

Three things to know:

- **`trunk` WIPES `dist/`, so anything you mean to KEEP must not live there.**
  This audit found `make federation` defaulting to `dist/federation` — publish a
  federation, build the app, and the federation silently vanished; the next serve
  404s and reads as a broken publish. Outputs that must outlive a build now sit in
  siblings (`dist-federation`, `dist-registry`, `dist-publish`), and yours should
  too. **`make site`'s own `OUT=dist/static-demo` default is the deliberate
  exception**: it is published *in order to* be served out of `dist/` by
  `make serve` in the same breath, and is expected to be re-emitted. If you are
  publishing something you will still want after the next `make wasm`, pass an
  `OUT` outside `dist/`.
- **A green build can be a cached build.** Check the artifact timestamps of any
  build you intend to quote — `ls -l dist/*.wasm` — and treat a suspiciously fast
  "Finished" as unverified. A sibling-repo change once left `make test`, `make
  lint` and `make wasm` green for hours because nothing rebuilt `entity-sdk`.
- **Worker mode needs a secure-context origin.** OPFS is secure-context-gated,
  and without it the Worker bootstrap fails and the app **silently falls back**
  to the main-thread arm. `localhost` / `127.0.0.1` / `https` are secure; a plain
  http *hostname* is not.

Deployment posture (`entity-deployment.json`, startup surface, kiosk lock,
origins) is [`GUIDE-DEPLOYMENT-AND-CONFIGURATION.md`](./GUIDE-DEPLOYMENT-AND-CONFIGURATION.md).

---

## 5. Consume: resolve a name

### 5.1 Stand the whole thing up locally

```bash
make federation          # N domains + one registry, all signed, all --verify'd
./tools/cors-serve.py dist-federation 8099
```

`dist-federation/MAPPING.txt` carries the `name → peer-id → slug` table and the
one pin. Edit the `DOMAINS` table at the top of `tools/local-federation.sh` to
change what gets seeded — that table **is** the seeding.

### 5.2 Drive it from the browser

In the app's Shell (browser only — the fetch is `window.fetch`, whose futures are
`!Send`, so the native build says so rather than pretending):

```
name pin <registry-peer-id> http://localhost:8099/registry
name resolve entitychurch.org
  → entitychurch.org → 2KGTrr4L…skz9sS  [http://localhost:8099/foundation]
    checked: association=true  name=true  revocation=true  expires <ms>
name open entitychurch.org               # asks which site — it does not guess
name open entitychurch.org demo          # landing page from the site's own manifest
name open entitychurch.org demo index
  → sites/demo/pages/index verified — 689 bytes, hash 007466324d5d9c5d
name pins                                # how many publishers this tab holds a floor for
```

### 5.2a A deployment can seed the pin, so a user types nothing

Nobody will paste a peer-id. `/entity-deployment.json` may carry one:

```bash
make site OUT=out/ DEPLOY_CONFIG=1 \
  REGISTRY_PIN=<registry-peer-id>@https://registry.example

# …or the CLI directly, which is what the make target runs:
entity-browser publish out/ --deployment-config \
  --registry-pin=<registry-peer-id>@https://registry.example
```

**Use the `make` route for anything you intend to ship.** A podman-only host has
no `cargo`, so for a while the only way to pin from such a host was to publish
without one and edit the emitted JSON afterwards — which produces the same two
strings and *skips the validation this flag exists to perform*. A pin that gets
past a hand-edit fails later, at a visitor's browser, where it is
indistinguishable from a registry that is merely down.

```json
"name_registry_pin": { "peer_id": "2KBLkCxv…", "origin": "https://registry.example" }
```

`make federation` does this for every domain it emits, all pointing at the one
registry it just published (`registry --peer-id` answers the identity question
before the registry tree exists). Four rules the implementation holds you to:

- **It seeds; it never overwrites.** A `name pin` typed in the Shell outranks it
  for that tab. `name pins` says which is in force and where it came from — a
  name resolving through a registry the user never chose must not be silent.
- **It rides the durable config**, not the fetched document: a returning profile
  never re-fetches `/entity-deployment.json`, so a pin read at fetch time would
  apply on a cold boot and silently not on a warm one.
- **An origin with no peer-id is not a pin** and is dropped whole. The peer-id
  *is* the key; pinning an origin would trust the origin.
- **The emitter refuses a non-canonical peer-id**, because this consumer derives
  the verification key out of the peer-id itself and has nowhere to put an
  out-of-band one.

There is still **no default registry shipped for everybody**, deliberately:
shipping our globs before arch ratifies `name_format_dispatch` is how two app
tiers ship two, and a private name leaks to whichever they picked. A *deployment*
saying which registry it trusts is the opposite move — it binds only itself.

### 5.3 What a resolve actually checks

Hop 1 walks the **registry's signed root** to `binding/by-name/{name}` and checks:

- **the association is committed** — the walk, not a host-served pointer. A
  static host that repoints one pointer file can otherwise answer
  `foundation.example` with the binding legitimately issued for
  `protocol.example`; walking the root removes the choice.
- **`binding.name` equals the name asked for** (arch D1) — done *anyway*, because
  the signature already covers it and a resolver that ignores it is discarding a
  commitment it holds. It is what keeps this correct if the read ever becomes
  host-trusted again.
- **a non-null TTL, and not expired** (D3).
- **the §6a.6 by-target revocation key, probed *inside* the signed tree** — so
  "not revoked" is an absence the root committed to, not one the host chose.

Hop 2 pins the peer-id hop 1 named, at the origin its `transports` carried, and
walks **that** peer's signed root to the page. The origin is trusted for nothing
at either hop.

Every success carries `NameEvidence` recording which checks ran. Nothing in it is
a boolean a caller can set — a resolved name and an unchecked one must not look
alike.

### 5.4 Privacy: our resolve never sends the name

Measured, not argued. Because the walk is content-addressed, every request is
`content/{aa}/{bb}/{hex}` and the key is matched *inside* a node already fetched:

- a **hit** never puts the name on the wire
  (`resolving_a_name_never_puts_that_name_on_the_wire`)
- a **miss** costs 4 requests, none carrying the name
  (`a_name_the_registry_does_not_carry_dies_in_the_trie_without_being_sent`)

This is why a `peer-issued` backend over a signed root may sit in a catch-all
glob while `dns-txt` / `well-known-url` / `did-web` — which *transmit* the name —
must require an explicit marker. **The property is conditional on the signed-root
walk**: a consumer trusting the host-served `by-name` pointer puts the name in a
URL path and loses it. Arch's D1 is a privacy prerequisite, not only an integrity
one. Full reasoning:
`docs/architecture/reviews/PROPOSAL-NAME-FORMAT-DISPATCH-DEFAULTS-AND-THE-NAME-BLIND-BACKEND.md`.

### 5.5 Hold one session per publisher, for the lifetime of the process

The rollback floor (`seq`) lives in a `SignedSession`. Build a client per fetch
and a hostile origin rolls a site back **page by page** — `seq 1` for one page,
`seq 0` for the next — while every signature and every hash still verifies,
because both trees really were published by that key. Nothing else in the chain
catches it.

**And a republish has to advance `seq`, which it did not until 2026-08-19.**
Every CLI invocation built a fresh in-memory publisher peer, so there was no
prior head to chain off and *every* emit published `seq 0` — which made the floor
above unable to see a rollback at all, because two trees at zero never go
backwards. The emitter now adopts the head already in the output directory
(reading it *before* the clean that would delete it, and carrying the prior
signature too, so an unchanged republish can still project one). If you write a
new emit path, it inherits this from `RootProjector`; if you write one that
cleans its own output first, read the head before you clean.

> **Verified 2026-08-20 on the shipped path**, because a fix to a
> rollback defence that nobody re-runs is a claim: two `make registry` emits into
> the same directory, second one carrying an extra binding, reported
> `seq 0` → `seq 1`, each `--verify` clean (`0 missing`).
>
> **`make federation` is the exception, and knowing why matters.**
> `tools/local-federation.sh` does its own `rm -rf "$OUT"` *before* the emitter
> runs, so the prior head is gone at the shell level and **every federation run
> republishes `seq 0`**. That is correct for a local rig standing up a whole
> federation from scratch, and it means the rig **never exercises the sequence
> advance** — do not read a green `make federation` as evidence about rollback.
> It also means `make federation` is the wrong tool for *updating* a federation
> that consumers have already seen: use `make site` / `make registry` into the
> stable directories, which adopt the head that is already there.

Use `session_cache::session_for(peer_id, origin)`. It is keyed on the
**publisher, never on `(publisher, origin)`**: keying on the origin hands a
hostile *mirror* a fresh floor of zero by serving the same tree from a second
URL — the same rollback wearing a hostname. Known limit, recorded rather than
guessed: **the first origin wins**, so a publisher that genuinely moves needs the
origin threaded through `PinnedPublisher`, which is a trust-chain refactor and
was not done.

---

## 6. What is NOT closed

Stated precisely, because a chain that is 90% verified and described as verified
is worse than one described accurately.

| # | open item | bound today | whose |
|---|---|---|---|
| **F2** | A static host can **withhold a revocation** by serving an *older signed root*. Not a forgery — a rollback. | the session `seq` floor *within* a session; across a cold start, the binding's **TTL** | model-level; TTL is the answer |
| **D9 detection** | **Now arch's and specified** — `PROPOSAL-TREE-WALK-COMPLETENESS` §4 (`ROUTING-2026-08-18-j`). It was never a design question we were waiting on; it was a defect in the spec's text, and the same tolerant walk is in **all three engines**. | our own resolve path already satisfies the invariant (§6.1) | arch + the engine seats |
| ~~**`name_format_dispatch` defaults**~~ | **CLOSED, in our favour** — `EXTENSION-REGISTRY` v1.8 (arch `8bfc9b6`) re-keyed §4.1 step 2 from *remoteness* to **name transmission**, and `peer-issued` resolved per §6a.4 through the signed root is now an explicit **MAY** in the catch-all (§4.1a row 6). Both halves settled: the glob list is a *filter* with no precedence, and `priority` decides who answers. `default_rules()` matches all six ratified rows (`our_table_matches_the_ratified_4_1a_rows`). | — | — |
| ~~**Foreign layouts**~~ | **CLOSED both halves.** The consumer reads the advertised profile (`PinnedPublisher` carries a `PublishLayout`, not an origin string; `session_cache::session_for_layout` is the entry point), and the publisher emits one (`signed_root::write_transport_profile`). `http_poll_origin` — whose F6 verdict *"a layout we cannot consume"* was itself wrong, since it refused every conformant publisher that is not our emitter — is gone. Gates: `the_shipped_consumer_resolves_a_go_published_page_over_their_advertised_layout`, `a_publish_advertises_its_own_endpoint_and_a_consumer_enters_through_it`. | — | — |
| **`session_for` is first-origin-wins** | A publisher first met at a *typed* origin keeps that layout even if a later binding advertises a real one — the floor outranks the layout, and the order of first contact decides. Reachable on a four-site federation by visiting in the wrong order. | correctness, not integrity: the wrong *layout* 404s, it does not admit forged bytes | ours — the fix threads the origin through `PinnedPublisher`, a trust-chain refactor |
| **A published-then-withheld revocation** | Our fixture cannot emit a revocation, so the one shape where a blob exists *only* on the revocation path is **untested**. Everything around it is proven; this is named as a gap rather than covered. | — | ours |
| **Registry-browser surface** | B16b's product surface (a name bar, site-window integration) is not built. The `name` verb is the spike that proves the path — **and it is a spike, not a wire: it prints bytes to the scrollback and navigates nothing.** See §7, which is the honest statement of where the chain stops. | — | ours |
| ~~**The seeded pin's browser path**~~ | **CLOSED** — `a_deployment_that_seeds_a_registry_pin_resolves_a_name_with_nothing_typed` stages a real `/entity-deployment.json`, serves it from its own port (a different origin, which is what makes it a cold boot and therefore the only kind that reads the document), types no pin, and resolves. Mutation-checked. | — | — |

### 6.1 What we verified about arch's completeness invariant

Arch's `ROUTING-2026-08-18-j` §4 states the rule the whole chain needs:

> **The absence of a node is never an answer.** A node that *resolved* and did not
> contain the key → `not_found`. A node that *did not resolve* → a failed walk.

Two halves, and they landed in different places:

- **Publisher side** — our `--verify` was the defect (F8) and is now the reference
  implementation: completeness is structural, and the error names the declaring
  parent as well as the missing child.
- **Consumer side** — **already conformant**, which arch did not expect. Resolution
  goes through the pump, so a withheld blob becomes a transport failure and
  `Absent` means only "every node on the path resolved and none held the key".
  Measured: 6 blobs on a resolve's walk, each withheld in turn, **none** became an
  answer.

The half that is genuinely open is **enumeration**: a signed name listing would go
through upstream `collect_all_bindings`, which *is* the tolerant walk. That is why
§3.4 says an unresolvable entry must be treated as a failure and never as the end
of a branch.

---

## 7. Two consumers, two trust models — and they are not wired together

Everything above §6 describes **one** consumer: `resolve_name`, reached from the
Shell's `name` verb. The app ships a **second** one — the **Site Browser**
window, which is the surface a user actually browses with. They do not share a
trust model, they do not share a discovery mechanism, and **nothing connects
them**. Measured 2026-08-20; stated here because a reader who has got this far
will otherwise assume the guide describes the product.

|  | **`name` verb** (Shell) | **Site Browser** (the window) |
|---|---|---|
| how a foreign publisher is found | a pinned registry → a signed binding | `entity-deployment.json`'s `origins` map, seeded at boot |
| what is trusted | the **publisher key**, pinned; the origin for nothing | **the origin**, for the whole path→hash mapping |
| integrity check | signed root + signature + `seq` floor + revocation probe | the body hashes to the pointer the origin just served |
| rollback defence | the session `seq` floor | **none** |
| what it does on success | prints 160 bytes of preview into the scrollback | renders the page, caches it, adds it to the rail |

**The asymmetry is the point, and it is not a bug in either one.** The Site
Browser's two-hop check (`http_poll::verify_and_decode`) is a real integrity
gate against a *corrupted* or *truncated* transfer — the body must be the
canonical pre-image of the hash it was fetched under. What it cannot detect is a
**lying origin**, because the origin also supplied the pointer: serve a different
hash at `sites/x/pages/index.bin` and a matching body at that address, and every
check passes. That is exactly the hole the signed root closes, and the Site
Browser does not read one. Measured 2026-08-24: `resolve_name` has **two**
callers in `src/` — the Shell's `name` verb and `views/registry_browser/model.rs`
— and `session_cache` / `signed_fetch` still have **no call site anywhere on the
Site Browser's fetch path**: `resolver.rs`, `http_poll.rs`, `views/content_site/`
and `dom/content_site.rs` do not mention a signed root at all. The one
non-consumer use is the *emitter* validating a pin's peer-id (`publish.rs`),
which is the opposite direction.

**The second caller is the whole of §7.2's change, and note what it does and does
not do.** The Registry Browser resolves through the full signed chain and then
hands the Site Browser an **origin** — so the resolution is verified and the
subsequent page fetch is not. That is the seam this section is about, now
reachable by a user rather than only by a deployment.

This is defensible today only because of *who supplies the origins*: they come
from the deployment's own `entity-deployment.json`, so a visitor is trusting the
site they already loaded the app from — the same trust a normal web page asks
for, and no more. **It stops being defensible the moment a foreign origin can
arrive from anywhere else** — a resolved name, a user-typed URL, a link from
another publisher. Whichever of those ships first is what makes wiring the signed
path into the Site Browser a prerequisite rather than an improvement.

### 7.1 "Am I on a different domain?" — what the UI actually tells you

- **In the directory rail: yes, partially.** A row's subline reads `owned` for
  your own sites and `cached · {host}` for a fetched foreign one
  (`dom/site_directory.rs::subline`), plus a visit count. So the *rail* names
  the host it came from.
- **On the page itself: no.** The nav bar carries the **site title** and nothing
  about the peer or the origin (`dom/content_site.rs::render_nav_bar`). Once you
  have clicked through, there is no persistent indicator of whose tree you are
  reading. A cross-peer `entity://` link followed from a page changes publisher
  silently.
- **Nowhere: which key signed it** — because on this path nothing did.

The gap worth naming precisely: the rail answers *"which host served this"*,
which is a **transport** fact. The question the trust chain makes answerable is
*"which key published this"*, which is an **identity** fact. Today only the
`name` verb can answer the second, and it has no UI to say it in.

### 7.2 "Do you have to link to the site?"

To reach a foreign peer's site in the Site Browser, that peer needs a registered
origin. There are now **three** ways one exists:

1. **The deployment declares it** — `origins` in `entity-deployment.json`, seeded
   durably at boot with `put_if_absent`, then `warm_peer_sites` fetches each
   peer's `sites.list` so their sites appear in the rail on first paint. This is
   the curated-federation shape.
2. **A link from a page already loaded** — `entity://{peer}/sites/{id}/pages/{p}`
   (`content_site::location::classify_link`). This resolves **only if that peer
   already has a registered origin**; otherwise the resolve is bounded and ends
   in `ResolveError::Unreachable` rather than spinning. So a link *alone* is
   still not enough.
3. **A resolved name** — the Registry Browser's *Open in Site Browser*
   (`RegistryBrowser::open_in_site_browser`) registers the origin the signed
   binding carried, under the system peer, and warms that publisher's sites into
   the rail. **This is a user decision, not a deployment one.**

> **This section said the opposite until 2026-08-24, and the correction is the
> point.** It read: *"there is no affordance anywhere in the app to add an
> origin … the origin registry is deployment-owned … the naming chain terminates
> in a scrollback preview."* All three clauses were true when written and route
> 3 spent them. A "we cannot do X" paragraph has an expiry, and the expiry is
> somebody shipping X — here, four commits later, in this same repo.

**The consequence is the one §7 exists to name: the B-3 trigger has fired.**
§7's own rule is that the trust work becomes a prerequisite on *"any path by
which bytes reach the renderer from an origin the deployment did not supply."*
A registry-resolved origin is exactly that, and it is live. Stated precisely,
because the severity is easy to overstate in either direction:

- **Nothing was wired to claim `verified`.** `verified_at` is still hard `None`
  with the reason inline, and every foreign row still renders `not verified`.
  The D2 violation §7 warns about — a verified *resolution* laundering an
  unverified *fetch* — has **not** shipped.
- **The exposure did not change in kind; the origin set widened.** The two-hop
  check is the same in both routes: real against corruption, silent about
  authorship. What moved is *whose judgement bounds the origin set* — it was the
  deployment operator, and it is now also the registry operator.
- **A registry-resolved origin is not an arbitrary one.** It arrives inside a
  binding that was signature-checked through the registry's signed root, with
  the D1 name check, a non-null TTL and the §6a.6 revocation probe — *better*
  provenance than a deployment-config origin, which is an unsigned JSON file.
  The residual risk is narrow: **a host named by a legitimate signed binding
  serves bytes the publisher never signed.** That needs the *named* host to be
  hostile or compromised, not any attacker.

So the honest status is: the condition fired, the labelling is still honest, and
wiring the signed path into the Site Browser is now **load-bearing rather than
optional**. The argument that made the gap defensible — *the deployment decided
every origin in advance* — is no longer available; the replacement argument is
trust in the registry operator, and it holds only while that is the same party.

### 7.3 The smallest honest next step

Not "build a name bar". The first wire is `name open` handing its verified result
to the Site Browser instead of formatting a preview — which immediately forces
the two design questions this section exists to surface, and neither should be
answered by accident:

- **A verified page and an origin-trusted one must not render identically.**
  Whatever indicator gets added, `§7.1`'s distinction is the one it has to carry:
  *which key*, not *which host*.
- **`session_for_layout` is the entry point, and it is keyed on the publisher.**
  A Site Browser that reaches a foreign tree any other way gets its own `seq`
  floor of zero and the rollback comes back (§5.5).

## 8. Where the code lives

| concern | file |
|---|---|
| site emit + `--verify` (both trees) | `src/content_site/publish.rs` |
| signed-root projection | `src/content_site/signed_root.rs` |
| registry emit + `--verify` + CLI parse | `src/content_site/registry_publish.rs` |
| consuming a signed root (the pump) | `src/content_site/signed_fetch.rs` |
| one session per publisher (the `seq` floor) | `src/content_site/session_cache.rs` |
| the two-hop resolve + evidence | `src/content_site/named_site.rs` |
| glob dispatch / disclosure classification | `src/content_site/name_dispatch.rs` |
| the `name` shell verb | `src/views/shell/model.rs` |
| the local federation rig | `tools/local-federation.sh`, `tools/cors-serve.py` |

Deeper, and authoritative where they overlap with this guide:

- `RUNBOOK-CDN-BROWSER-DEPLOYMENT` — CORS, cache headers, the browser vector.
  **Currently legacy-tree only** — see the §2.4 note and `ROUTING-2026-08-18-a`
- `docs/architecture/reviews/BUILDOUT-SIGNALING-AND-NETWORK-EXTENSIONS.md` —
  connectivity buildout and its open items
- `docs/plans/AUDIT-NAMING-AND-PUBLISHING-ARC-2026-08-18.md` — the audit this
  guide's corrections came out of
- upstream `EXTENSION-REGISTRY`, `EXTENSION-TREE`, `EXTENSION-NETWORK`,
  `GUIDE-RESOLUTION` — the spec wins on all of it

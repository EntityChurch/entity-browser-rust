# Reference — The Publishing Pipeline (source → tree → CDN → live site)

> **⚠ Partially superseded (pipeline cutover).** The `render → tree → CDN → live`
> *arc* below is still accurate, but the **papers-specific Makefile wrapper it
> uses as its running example is gone** — `make site-papers`, `publish-papers-preflight`,
> and every `PAPERS_*` / `PRERENDERED` / `SKIP_STAGE0` variable were removed, and
> apps ingest is now the single `APPS_DIST=<dir>` parameter. The generic pipeline is
> unchanged: use `make site INGEST=<dir>` / `make site-serve INGEST=<dir>`.
> For the current, tool-agnostic contract see
> **`docs/architecture/guides/PUBLISH-INGEST-FORMAT.md`** (§7 records the cutover).
> Treat the `publish-papers` command examples here as historical.

**Status:** reference for the publish/ingest/serve *arc* (source → tree → CDN →
live site), including images (the embed/asset arc). **Read this when** you touch
publish/ingest/serve, onboard the DevOps R2 step, or extend the flow (cross-site
links, static-export images) — alongside the note above.

Companion doc: `REFERENCE-CONTENT-SITE-APP.md` (the app-level picture of the
Content Site surface this pipeline feeds).

---

## 0. The one-line model

> A content site is **content-addressed entities in a peer's tree**. Publishing
> = serialize that tree (plus a WASM single-page app and a tiny deployment
> config) into a **fully static directory**. Any dumb static file server — the
> Python dev server, an R2 bucket behind a CDN — serves it. The SPA boots in the
> browser, fetches the tree over plain HTTP, and renders it live.

There is **no application server** *on this road* — see §0.0 for the other one.
`dist/` is the whole product. "Will the CDN work?" → yes, because R2 and
`python -m http.server` are interchangeable static file servers, and the SPA
fetches **same-origin-relative** (or from an explicit origin map — see §5).

### 0.0 ⭐ TWO THINGS ARE CALLED PUBLISHING, AND ONLY ONE OF THEM HAS A VERB

**Read this before §0.1.** This distinction has been got backwards at least
three times across two seats — once by arch, recorded against themselves in
`ROUTING-2026-09-15-d` §1, and twice here — always in the same direction, and the
reason is mechanical rather than conceptual: **one of the two meanings has a
verb, a flag, an out-dir and a whole pipeline behind it, so it looks like the
real one.**

| | **the tree road** (live) | **the static road** (snapshot) |
|---|---|---|
| the act | write an entity into your peer's tree | project that tree into a directory and sign a root over it |
| what makes it public | a connected peer asks and you serve it, like everything else in your tree | a static file server hands the bytes to anyone, whether you are running or not |
| the verb | none — `put` is the whole of it | `entity-browser publish` |
| who reads it | `feed_peer::PeerFeedSource`, `remote_read` — a peer over the transport | `SignedSession` over HTTP — a browser, `DirFetcher`, another impl |
| the authority | **the tree** | **the tree** — this is a snapshot *of* it |

**The tree road is publishing.** Writing an `app/feed/entry` into your own tree
*is* posting; `src/feed_compose.rs`'s four verbs do exactly that and reach no
out-dir. The static road is an **additional, operational** act: it freezes a
version others can pull while you are asleep, offline, behind a NAT, or simply
not running. It does not make the content public — it makes it *available
without you*.

⇒ **When a sentence in this system needs the word "publish", decide which road it
is on before reaching for the machinery.** Most confusion here is a sentence that
means the tree road being answered with the static road's tooling.

#### What is actually built on each road, measured — so nobody re-derives it

Stated because the gaps are asymmetric, and reading either road's code alone
gives a wrong picture of the other.

- **A browser peer is not a publisher, and that is a fact about construction.**
  Through `Peers::new_direct_with_connector` — the construction `PeerManager`
  gives the app — nothing calls `PeerBuilder::with_published_root`, so no prefix
  is tracked and a tree write mints no signed root. Verified 2026-09-16: **zero
  callers in `src/`**, and in `entity-core-rust` only its own tests. Armed by
  hand it works (~176 µs/put, one mint per write, *including* a window-state
  persist) — so arming it is a decision about whether every profile pays a mint
  per write, not a missing feature.
- **The desktop peer serves the SPA and never its own tree.**
  `src-tauri/src/app_server.rs` is one exact-key lookup against the embedded
  `frontendDist` (`assets.get(lookup)`); `EXTENSION-NETWORK` §6.5.6's live
  serving mode is unimplemented. So the one component here with a real
  long-lived tree cannot expose it over HTTP.
- **`publish` cannot publish a peer's tree.** `resolve_publish_source` builds a
  **fresh in-memory peer** each run and seeds it; only the *keypair* is durable.
  There is no verb naming a long-lived native store and no flag naming a
  subtree. That is `F3`, it is pre-existing, and it applies to sites exactly as
  it does to a feed.
- **A live read works today and is one leg of a priority list, not an
  alternative to the other.** `feed_route::plan` tries live then published,
  because a publisher may serve at only one — and an *empty* answer from one leg
  is not evidence about the other (`feed_route`, and the gate asserting the
  second leg is still consulted). ⚠ The live leg is the **weaker** read right
  now: FEED §4.2's index is a **publish artifact**, built by `plan_index` on the
  way out and never written into the tree, so a key-resolving live reader falls
  through to §4.3 rule 6's prefix enumeration and *reconstructs* the order the
  index would have carried.

### 0.1 The model in one screen — read this before changing anything that publishes

**⭐ THIS IS THE CANONICAL STATEMENT OF THE PUBLISHING MODEL.** If you are about
to write it down somewhere else, link here instead. It is one function:

```
                    ┌───────────── src/content_site/publish.rs, resolve_publish_source ─────────────┐
  an authored   ──► │   ingest::ingest_path(disk)                        read_all_sites             │
  input             │   apps::ingest::ingest_into        ──► THE TREE ──► read_all_app_sets         │ ──► ONE projector
  (render/ dir,     │   feed_ingest::ingest_path                         read_owned_feed            │      → project + sign
   posts/ dir,      │   or seed_demo_site_set                                                       │
   demo seed, …)    └──────────────────────────────────────────────────────────────────────────────┘
                                                          the axes: src/publish_axes.rs
```

**Four consequences, each of which has been got wrong at least once:**

1. **`publish` translates an input INTO the tree, then projects the TREE.** The
   tree is the authority. An authored directory is *one workflow into it*, not
   the model — several can coexist (a `render/` dir, an in-tree process, an
   editor that saves and publishes).
2. **The clean is snapshot semantics, not destruction.** `publish.rs` removes
   `{base}/{peer_id}` (and `content/` only when no sibling publisher is present)
   and re-projects. Re-projecting the current tree *is* the operation; **not**
   re-projecting is what leaves an output stale against the tree it claims to
   snapshot.
3. **One projection, one `RootProjector`, one root.** `finish` builds the trie
   over the bindings *that projector recorded*, so **two projector runs into one
   directory do not compose** — the second signs a root naming only its own axis
   and the first is silently un-named. Gated:
   `feed_publish.rs::a_second_axis_signed_by_its_own_projector_un_names_the_first`.
   ⇒ **anything that wants to be in the signed root has to be in the tree
   `publish` projects.** A separate verb writing into the same out-dir is the
   mistake.
4. **What enters the projection is `src/publish_axes.rs` — one list, four rows.**
   It was a hardcoded enumeration of two L5 conventions (`emit_owned_sites` +
   `for set in app_sets`) until 2026-09-10, when the feed axis landed and paid
   for the table, and the §6 mirror made it four on 2026-09-12. **A fifth
   convention is a row plus an `impl PublishAxis`**, and the compiler will not
   let it skip an obligation: name, tree prefix, `incoming()`, `carried_peers()`,
   `project()`.
   - The prefix matters because **the clean is wholesale** — `{base}/{peer}/` goes
     in one `remove_dir_all`, so an axis nobody listed is not left alone, it is
     deleted. Every axis therefore also owes a term in `run_plan`.
   - **The residue that stays per-convention is tabulated in that module's doc**
     (the legacy-web `.html` export, `--bare-root`, the plan's per-unit naming,
     the `http_poll` URL builders) rather than left to be rediscovered.

### 0.2 The four axes

| axis | tree prefix | reader | ingest flag | `make` variable | worked example |
|---|---|---|---|---|---|
| sites (`APP-CONVENTION-SEMANTIC-CONTENT-SITE`) | `sites/` | `content_site::read::read_all_sites` | `--ingest=<dir>` · `--demo-sites` · `--no-sites` (**exactly one, required**) | `INGEST=<dir>` · `DEMO_SITES=1` · `NO_SITES=1` | `examples/entity-demo/` |
| apps | `apps/` | `apps::read::read_all_app_sets` | `--ingest-apps=<dir>` | `APPS_DIST=<dir>` | the `entity-apps` repo's `dist/` |
| feed (`APP-CONVENTION-FEED`) | `app/feed/` | `feed_tree::read_owned_feed` | `--ingest-feed=<dir>` | `FEED=<dir>` | `examples/entity-demo/feed/` |
| mirrors (`APP-CONVENTION-FEED` §6) | `app/feed/mirrors/` **+ each carried author's segment** | `feed_gather::gather_timeline` — somebody ELSE's tree, not ours | `--gather=<peer_id>@<dir>` *(repeatable)* | `GATHER='<peer_id>@<dir> …'` | `tests/fixtures/crossimpl-go-feed/peer-root` |

**The `make` column is the *authoring* entry point and it is not cosmetic.** Every
containerized publish target bind-mounts only this repo, so a source anywhere else on the
host is invisible to the publish; the variables are staged (`stage_publish_sources`) into
repo-local dirs and the publish is handed the *staged* path. A raw `--ingest-feed=` at a
`make` invocation therefore names a directory the container cannot see. **`FEED=` arrived
2026-09-12, two days after `--ingest-feed`** — so for two days the third axis was reachable
from a bare `cargo run` and from no `make` target, which on a podman-only host means it was
reachable by nobody. *An axis with a flag and no staged variable is an axis a person cannot
publish.*

⭐ **SECOND INSTANCE, 2026-09-16, and it is NOT an axis — so the rule is wider than the row it
was written in.** `--window-target` shipped 2026-09-12 with a row in the flag table, three CLI
refusals and a browser gate, and **no `make` variable**, so for four days the only aimed-window
deployment reachable on a podman-only host was none. It was found by needing it: a site-free
`--deployment-config` refuses `--surface=window` *unless* a target is named, and there was no way
to name one through `make`. ⇒ ***the deliverable for any publish flag an operator must set is the
flag, the `make` variable, and a worked example*** — not only for the `--ingest*` family, which is
what the `FEED=` lesson looked like when it had one instance. `WINDOW_TARGET=` is wired into all
three deployment-config verbs; `site-bare` gained the site arm and staging in the same commit, for
the same reason.

⛔⭐⭐ **THIRD INSTANCE, 2026-09-17, AND IT WAS SITTING IN THIS TABLE AS A STATED EXEMPTION.** The
mirrors row carried *"— (no make variable: the value is a `peer@dir` pair, and the dir is another
publisher's out-tree rather than an authored source)"* from the day `--gather` shipped. **Both
clauses of that reason are true and neither answers the paragraph three lines above it:** the
constraint is the **bind mount**, not the provenance of the directory or the shape of the argument,
and another publisher's out-tree outside this repo is exactly as invisible to the container as an
authored source outside it. So the fourth axis was reachable by nobody on the supported host for five
days **behind a reason rather than behind an omission**, which is harder to find because the cell
reads as considered. ⇒ ***an exemption's reason must answer the rule's reason, not the rule's
subject*** — and the cheap check is to re-read the paragraph explaining why a column exists before
writing *N/A* into it. `GATHER=` is staged like its three siblings (each source under the **author's**
peer id — `parse_gathers` already refuses one author twice, so that id is unique by the rule that
matters), wired at all three staged-flag call sites, and run before being written down:

```
make site OUT=<dir> NO_SITES=1 \
  GATHER='<their peer_id>@tests/fixtures/crossimpl-go-feed/peer-root' \
  IDENTITY_SEED=<64 hex>
```

→ 34 entries, 34 attributable, one gathered view, two pages. ⚠ **Two cuts into different directories
are byte-identical in the mirror head, both pages and every carried entity, and
`system/peer/published-root` differs** — it carries a wall-clock `published_at` with no seam (`K-3`),
which is why the trie root and not that head is the comparand.

The fourth row is the only one whose reader points outside this peer and the
only one with a non-empty `carried_peers`; §0.2a is what that costs.

⭐ **THE FIRST ROW IS THE ONLY ONE THAT EVER HAD A DEFAULT, AND THE DEFAULT INVENTED
CONTENT — closed 2026-09-16, on the release-coordination seat's `B-4`.** Absent `--ingest`,
`resolve_publish_source` seeded `demo` + `entity-info`; apps and feed were both
`if let Some(dir)` and genuinely optional. **The asymmetry was not a decision anybody
made** — when the seeding was written, every publish had sites — and the argument against
it was already in this repo, one axis over, as `--ingest-feed`'s own comment: *"a publish
that invented an empty feed would claim every site publisher has one."*

**The cost is not two unwanted sites; it is that the clean is wholesale.** `run_projection`
removes `{base}/{peer}` and rebuilds it, so a publish that *meant* `--ingest` and omitted it
did not add the demo set beside a domain's real sites — it **replaced** them, under that
domain's own identity, and exited `0`. *"You forgot `--ingest`"* and *"this domain has no
sites"* were the same command line, so no guard at any layer could tell them apart.

**Three arms, exactly one, and silence is refused** (`SiteSource` / `parse_site_source`).
They are 1:1 with the release-coordination seat's estate-configuration axis vocabulary — `papers` →
`--ingest`, `builtin` → `--demo-sites`, `none` → `--no-sites` — which is where the ask came
from, and their board had already reached the same rule for the same reason: *"all three are
required and none of them defaults."* **`--verify` is exempt and that ordering is
load-bearing**: it reads an output tree and resolves no source, so demanding an arm for it
would refuse a verification for a reason with no bearing on it.

**A site-free publish emits no site chrome** (`static_export::export_site_set` returns before
`write_root_index` / `write_landing_redirect`) and no `home_site` key. Found by a gate, not
by reading: the first cut published a feed-only tree that still served a `sites/index.html`
listing nothing and redirected `/` to it.

Three things about the feed axis that are decisions, not details:

- **A peer has ONE feed** — §4.2 pins the index at `/{peer}/app/feed/index` — so
  the reader is `read_owned_feed`, not `read_all_feeds`. The sweep is over
  entries.
- **The entries are the authored fact; the §4 index is DERIVED at publish time**,
  like `sites/index.html`. The cost, stated: a *backdated* post shifts every
  entry after it and rewrites the archive from its insertion point. Appending —
  the ordinary case — touches only the last page.
- **`created_at` is required in the post's `+++` block and is never taken from
  the file's mtime**, which `git clone` rewrites: the same posts would otherwise
  produce different entities, hashes and page boundaries on every machine.
  Likewise the §4.2 head is stamped from the feed's own newest post and **not**
  from a wall clock, or the signed root would move on every run and `G-PIN-4`'s
  *one fixture, two publishers, identical root* comparand would be unreachable
  for any tree carrying a feed.

**What is genuinely missing:** `resolve_publish_source` builds a **fresh
in-memory** peer each run — the *keypair* is durable, the *content* is assembled
at publish time. **There is no verb that reads a long-lived native store**, which
is what *"publish cannot publish a peer's tree"* means. It applies to sites
identically and is what blocks *"the desktop app posts"*.

### 0.2a ⭐ The fourth axis is a MIRROR, and it IS a row — what it cost to make it one

`APP-CONVENTION-FEED` §6's gatherer is `src/feed_mirror.rs`, the verb is
`publish --gather=<peer_id>@<dir>`, and the row is
`publish_axes::MirrorAxis`. **This section used to say a mirror is NOT a
row**, and listed three questions to answer first. All three are answered
below; **one of them was answered the other way from how it was posed**, and
that correction is the part worth reading.

**1. A fourth axis, not a second subgraph of the feed axis.** The deciding
argument is the *plan's* per-unit term, not tidiness: `run_plan` exists to name
**which** thing a republish would delete, and folded into the feed's term a
publisher carrying posts without re-gathering would be told *"0 post(s)
REMOVED"* on the run that deletes every gathered view they hold. A post and a
gathered view are different units.

The prefix overlap is real and is now **asserted rather than implied** —
`app/feed/mirrors/` nests inside `app/feed/` because §6 puts it there, and
`the_only_nesting_between_two_axes_is_the_one_feed_6_puts_there` pins exactly
which pair overlaps, so a fifth convention colliding with an existing prefix by
accident fails instead of quietly sharing another axis's accounting. Both
statements are true at once because the clean is wholesale over
`{base}/{peer}/`: every prefix in the table is nested inside *that*.

**2. `tree_prefix` does not have to lie — a row states its foreign half.**
`PublishAxis::carried_peers` is the fifth obligation, **undefaulted** so a fifth
convention gets `error[E0046]` rather than an empty list it never chose. Three
rows answer it in one line; the mirror is the only non-empty answer in the
table, because a carried entry is bound at its own **author's** address, which
is what lets a consumer reach it with the reader it already has.

⛔ **And the consequence that a "carried bodies just survive" reading misses:
the POINTERS survive the clean and their BLOBS do not.** `write_entity` puts
every body into the **shared** `content/{aa}/{bb}/{hex}` store whoever it
belongs to; only the `.bin` pointer is peer-scoped. `projected_peer_ids` reads
`{base}/sites/`, and a carried author has no site projection now or ever — so
left to that check alone, the next publish deletes the blobs and leaves the
pointers naming them. **Remove-then-un-name: this repo's own rule run
backwards**, and a tree that no longer resolves what it still names.

The content clean therefore asks a broader question — `foreign_trees`, *is
anybody else's tree here at all* — and it asks it **only in the direction that
is safe to be wrong in**. ⭐ ***Enumerating to decide what NOT to remove is
safe; enumerating to decide what to remove is how a publish destroys a co-hosted
publisher's tree (AP52/AP53).*** The predicate cannot tell a gathered author's
segment from a sibling publisher's, which is exactly why it is only ever allowed
to say *"leave it alone"*. Cost, stated: orphan blobs accumulate, which
`--verify` already lists and §7's origin-wide keep-set is the real answer to.
Gate: `a_republish_that_does_not_re_gather_leaves_no_dangling_pointer`.

**3. ⛔ `--verify` IS fixed by an arm — and the version of this section that
said otherwise was wrong.** It reasoned: the sweep is rooted at
`{base}/{peer_id}/`, a mirror's carried bodies are bound under each author's
segment, so *"this one is the wrong root and no extra arm fixes it"*. Two facts,
both measured rather than reasoned, make it an arm after all:

- **The blobs are not foreign at all** — see above; the shared content store
  already holds them and the closure fetcher already reaches them by hash.
- **A mirror record DECLARES what it carries.** `entries` is `FEED-R28`-pinned,
  so each row is a `(peer, hash)` coordinate — the same structural handle a trie
  node's children and a site asset's blob give.

⇒ **the general rule holds with no exception: every declaring type owes an arm
in `run_verify`, in the commit that introduces it.** What differs here is only
that the declaration names a *pair*, so the arm checks two legs — the bytes, and
the foreign key a consumer resolves them by. Either alone passes a tree that
does not serve. Gate:
`verify_follows_a_mirror_and_fails_on_either_half_of_what_it_declares`, with
both legs falsified separately.

⭐⭐ **AND THE ARM EARNED ITS KEEP ON ITS FIRST RUN, WHICH IS THE WHOLE
ARGUMENT FOR THE RULE.** Against a fixture carrying one post over EMBED §3's
16 KiB inline ceiling it reported *"the post appears in the index and its body
is empty"* — because `plan_mirror` carried the entry and its signature and
**not the blob its pointer body names**. The identical dangling-reference defect
`publish_feed` shipped with, one convention over, invisible to every gate in
`feed_mirror` and `feed_fetch` because all of them run against map-backed
doubles that serve whatever they were handed. *A test population you generated
cannot contain the shape you are missing.* `MirrorPlan::content` is the fix and
`GatherError::BodyClosureMissing` is the refusal — **on the plan, not on its
callers** (AP44), because a `content` argument a caller may pass empty is a step
the next caller forgets and whose consequence is not an error but a silently
hollow publication.

⚠ **A hypothesis about this that measured WRONG, in the alarming direction.** We
expected a mirror publish to make every carried author look like a **sibling
publisher**, permanently disabling the shared `content/` clean and printing *"N
other publisher(s) already at this origin"* about peers who never published
there. **It does not:** `projected_peer_ids` reads `{base}/sites/`, and a carried
author has no site projection. *One function, four minutes — and reasoning about
it would have produced a design decision answering a problem that does not
exist.* (What it *did* leave open is the pointer/blob asymmetry in point 2,
which is a different and real one.)


## 1. End-to-end flow

```
 ┌─ CONTENT RENDERER ──────┐   ┌─ THIS APP ──────────────────────────────────┐   ┌─ DEVOPS ────┐
 │ render/ (Go)            │   │ publish verb (Rust)                         │   │             │
 │  paper.md + figures  ─► │ ► │  ingest disk→tree ─► serialize tree→dist/   │ ►│ push dist/  │
 │  emits a site dir:      │   │   • entity-native .bin  ({peer}/sites/…)    │   │  → R2 bucket│
 │   site.manifest.json    │   │   • content blobs       (content/<hash>)    │   │  (CDN front)│
 │   pages/*.md            │   │   • legacy static .html (sites/{peer}/…)    │   │             │
 │   assets/figures/*.png  │   │   • SPA: *.wasm, *.js, index.html, sw.js    │   │             │
 │   (::embed + ![]())     │   │   • entity-deployment.json (home/origins)   │   │             │
 └─────────────────────────┘   └─────────────────────────────────────────────┘   └─────────────┘
                                                                                         │
                                              ┌──────────────────────────────────────────┘
                                              ▼
 ┌─ BROWSER (the live site) ─────────────────────────────────────────────────────────────────┐
 │ load index.html → boot WASM SPA → read entity-deployment.json (home_site, origins, posture) │
 │ → boot into the home site overlay → http_poll fetches page entities from the origin         │
 │ → render markdown→sanitized HTML → resolve <img> via the asset TWO-HOP → data: URL paints   │
 └─────────────────────────────────────────────────────────────────────────────────────────────┘
```

## 2. Ownership boundary (who owns which stage)

| Stage | Owner | Artifact in / out |
|---|---|---|
| Author + render content | **your renderer** | `paper.md` + `output/figures/*` → a site dir (`site.manifest.json`, `pages/`, `assets/figures/`) |
| Ingest + publish | **this app** (`make site-papers`) | site dir → `dist/` (static) |
| Push to CDN | **DevOps** (not yet wired) | `dist/` → R2 bucket behind a CDN |
| Run the site | **the browser** | static files → live SPA |

The papers↔app contract (image grammars, `assets/**`, content-addressing) is the
ingest surface described in §4 and §8. The app↔DevOps boundary is §7.

## 3. The command

`make site-papers` (`Makefile:235`) is the whole pipeline. It expands to:

```bash
# 1. build the papers render engine
go build -C $PAPERS_REPO/render -o render ./...
# 2. clean the render-out (belt — papers now cleans its own --output; /tmp-guarded)
# 3. render the domain → a tree of site dirs
$PAPERS_REPO/render --site billslab --repo . --skip-stage0 --output /tmp/papers-render
# 4. ingest the WHOLE domain into a tree + serialize to dist/ (THE publish step)
#    NOTE: --live is EMPTY (same-origin). The emitted config + banner are
#    relative, so this SAME dist/ runs at localhost here AND dropped on a CDN
#    root with no rebuild. We do NOT bake a domain. (See §5.)
cargo run --bin entity-browser -- publish dist \
    --ingest=/tmp/papers-render/billslab \
    --deployment-config --config-site=billslab-main \
    --surface=window --window-type="Site Browser" --live=
# 5. serve dist/ statically (dev only; R2 replaces this in prod)
python3 -m http.server 8081 --directory dist
```

### The `publish` verb — flag reference (`src/content_site/publish.rs`)

| Flag | Effect |
|---|---|
| *(positional)* `dist` | output directory (the static bundle) |
| `--ingest=<dir>` | **site arm 1 of 3** — read a site dir tree from disk into the tree first |
| `--demo-sites` | **site arm 2 of 3** — publish the bundled `demo` + `entity-info` set, *on purpose*. This is what an absent `--ingest` silently meant until 2026-09-16 |
| `--no-sites` | **site arm 3 of 3** — publish **no** sites: a feed-only or apps-only domain. Refused with `--bare-root` / `--html-only` (both project a site); with `--deployment-config` it emits no `home_site` and requires `--surface=chrome`, or `--surface=window` with a `--window-target` |
| `--deployment-config` | also emit `entity-deployment.json` (home site, origins, posture) |
| `--config-site=<id>` | the SPA's **home site** (what it boots into) |
| `--surface=<chrome\|site\|window>` | cold-boot **surface** baked into the deployment config (default `window`) |
| `--window-type=<name>` | for `--surface=window`, the maximized window type (default `Site Browser`) |
| `--window-target=<ref>` | for `--surface=window`, **what that window opens at** — an `APP-CONVENTION-REFERENCE` §3.1 address (`entity+ref://{peer}/{path}`). Refused at the CLI if it is unreadable, if no viewer handles it, or if its viewer is not the `--window-type` beside it. Absent = today's behaviour: the window opens showing nothing in particular |
| `--locked` | for `--surface=site`, emit the kiosk lock (no toggle, no peer creation) |
| `--live=<origin>` | the HTTP origin the SPA fetches content from. **Empty ⇒ same-origin** (portable; see §5/§7) |
| `--prefix=<p>` | host many isolated peers under one domain at `/{prefix}` (multi-tenant; empty ⇒ root) |
| `--html-only` | emit only legacy static `.html`, skip the entity-native `.bin` data |
| `--bare-root` | render a **single** site at the domain root (the no-JS SSG opt-out; `Layout::BareRoot`) |
| `--site=<id>` | publish only this site out of the set |
| `--ingest-apps=<dir>` | ingest an app set (entity-apps `dist/`) alongside the sites. `--ingest-games=<dir>` is an accepted alias |
| `--ingest-feed=<dir>` | ingest a directory of authored posts (`*.md` with a `+++` TOML block carrying **`created_at`**) as the peer's `APP-CONVENTION-FEED` archive — the **third publish axis**. The date is required and never taken from the file's mtime, which `git clone` rewrites; see `src/feed_ingest.rs` |
| `--gather=<peer_id>@<dir>` | *(repeatable)* read that author's feed out of the tree they published at `<dir>` and carry it into this publish as an `APP-CONVENTION-FEED` §6 **mirror** — the **fourth publish axis**. ⚠ **A DIRECTORY, not an origin**: this tree has no native HTTP client, so the topology it serves is publishers sharing one hosting scope (`src/feed_gather.rs` leads with the bound). One author per flag, because §6.0.1 derives one address per subject and two gathers of one author would write both at one key |
| **`--plan`** | resolve the source and report **what would change, writing nothing**. Has its own exit-code contract (`run_plan`) |
| **`--verify`** | prove an **already-published** tree resolves — every pointer, every body hashing to its address, the whole closure walkable (`run_verify`). **For a registry use `registry --verify`, not this** — different durable identity |
| `--allow-out-of-set-links` | downgrade an out-of-set `site:`/`entity://` target from a build **failure** to a warning. `--strict-links` is **accepted and ignored** — it asks for today's default, and other repos' pipelines still pass it |
| **`--set-home`** | this publish **moves the domain's home site**. Load-bearing: the home publish owns the domain-level fields and a secondary publish contributes only its `origins` entry, so without this a second peer defers to the existing home. A re-key *is* a deliberate home move |
| **`--supersede=OLD=NEW`** | *(repeatable)* the succession this domain **declares**. A consumer cannot infer it for anyone but the home peer — `origins` is a map, and a key leaving as another arrives is ambiguous between a re-key and one tenant replacing another |
| `--registry-pin=PEER_ID@ORIGIN` | the §7.4 preloaded name registry this deployment seeds. Same spelling as `registry --bind`, deliberately |
| `--identity-seed=<64 hex>` | publish under a **specific** system identity. Default is the durable publisher keypair under `{ENTITY_DATA_DIR}/publish/` |
| `--demo-identity` | the fixed demo publisher seed (dev/testing only). `--identity-seed` wins over it |

> **This table is the whole flag set as of 2026-09-16** (25 entries, cross-checked against
> `grep -oE '"--[a-z-]+' src/content_site/publish.rs`). It was **10 of 22** until the 09-10 audit — the
> missing twelve included `--verify`, `--set-home` and `--supersede`, all three load-bearing.
> `tools/publish-doc-check.py` (in `make lint`) now fails the build if a flag is added to `publish.rs`
> without a row here, which is how `--ingest-feed` arrived in the same commit as its code.
> **`registry` and `builds` are separate verbs with their own flags**, not part of this table; `main.rs`'s
> usage text is canonical for those.

## 4. `dist/` layout — what a CDN serves

A publish of billslab (11 sites / 397 pages) = **71 MB, 1183 files** — *measured 2026-08-24; a
figure, not a live reading.*

```
dist/
├── index.html                          # SPA entry
├── entity-browser-<hash>.wasm  (~20 MB) # the app (browser thread)
├── entity-browser-<hash>.js            # wasm-bindgen glue
├── entity-worker_bg.wasm       (~15 MB) # worker peer (only used in ?worker=1)
├── entity-worker.js / -loader.js
├── sw.js                               # service worker (cache-first asset delivery)
├── entity-deployment.json              # ← the per-domain config (§5)
├── {peer-id}/sites/{site}/…            # ENTITY-NATIVE .bin — what the live SPA fetches
│     ├── pages/<slug>.bin              #   page entities (CBOR; body speaks ::embed)
│     ├── assets/figures/<name>.bin     #   asset POINTERS (58 B: {type:system/hash, data:<hash>})
│     └── site.manifest / pages.list…
├── content/<aa>/<bb>/<full-hash>       # CONTENT-ADDRESSED blobs (the real image bytes live here, once)
└── sites/{peer-id}/{site}/….html       # LEGACY static HTML (no-JS / SEO fallback)
```

Three content roots, by design (the `[B2]` split):
- **`{peer}/sites/…` (entity-native `.bin`)** — the source of truth the live SPA
  reads over `http_poll`. Page bodies, manifests, and asset *pointers*.
- **`content/<hash>`** — content-addressed blob store. A figure referenced by N
  pages is stored **once** here; each reference is a 58-byte pointer. This is the
  dedup substrate and the second hop of asset resolution (§6).
- **`sites/{peer}/…html`** — dumb legacy HTML for no-JS clients / crawlers. Today
  it does **not** carry images (static-export images = deferred, §9).

## 5. `entity-deployment.json` — the per-domain knob (and the R2 step)

One generic WASM bundle serves **N domains**; this 341-byte file is the only
per-domain difference. Fetched at boot (`src/deployment_config.rs`), precedence
**persisted > fetched > build-time**.

```json
{
  "home_site":   { "peer": "2KEB3…", "site": "billslab-main", "loc": "" },
  "origins":     { "2KEB3…": "http://localhost:8081" },
  "surface":     "window",
  "window_type": "Site Browser",
  "site_mode":   { "enabled": true, "locked": false, "show_toggle": true }
}
```

- **`home_site`** — what the SPA boots into.
- **`origins`** — `peer-id → base URL` for the published peer. **Default is the
  empty string = same-origin**: at runtime the SPA expands `""` to
  `window.location.origin` and fetches content from **whatever host served it**.
  An explicit absolute value (`--live=https://host`) is a **deliberate pin** —
  rarely needed. It is **NOT** a cross-domain federation mechanism (see below).
- **`surface` (+ `window_type`) / `site_mode`** — cold-boot surface + posture
  (chrome/window/site; overlay on/locked/toggle). A locked kiosk also emits
  `peer_creation_enabled: false`.

**Portability — the contract (DevOps):** publish with **empty `--live`** (the
default of `make site-papers` / `site-serve`). The `origins` value is `""`,
the static→live banner is root-relative, and the **same `dist/` is portable to
any URL served at the domain ROOT** — localhost, R2 preview, R2 prod — **with no
rebuild and no domain to specify**. This is the intended operator story: pop out
a bundle, drop it anywhere at the root, it works. *(Caveat: portability holds at
the root; a subdirectory deploy is not yet zero-config — tracked debt.)*

**What `origins` is NOT.** It is **not** how the system reaches *another domain's*
content. Cross-domain navigation is **entity-native**: you follow an **entity
link**, the **registry** resolves `name → peer-id → transport`, and the
**resolver** fetches the tree. That machinery is part of the entity system and is
out of scope for this static-publish pipeline. A classical `<a href="https://…">`
web link is a **deliberate escape hatch** that takes the reader *out* of the
entity system — used explicitly, not the default.

## 6. Runtime — how the live SPA renders a page + its images

1. Boot reads `entity-deployment.json`, resolves the home `SiteRef`, registers
   origins, boots into the site overlay.
2. **Page fetch** — `http_poll` (`src/content_site/http_poll.rs`) GETs the page
   entity `.bin` from the origin; the body is canonical `::embed` markdown.
   Cached-foreign content is written through to the local store
   (`resolver.rs::persist_to_cache`).
3. **Render** — `render.rs::markdown_to_html` lowers `::embed` → a **sanitized**
   `<img alt src>` (no raw HTML / `onerror`).
4. **Asset TWO-HOP** — for each `<img>`, `dom/content_site.rs::rewrite_images`
   resolves the site-local ref (gated by `paths.rs::asset_name_from_ref` — only
   `assets/…`, never `://`/`//`/`/abs`/`data:`/`..`):
   - **hop 1**: read the asset *pointer* entity (`{type:system/hash, data:<hash>}`),
   - **hop 2**: read the *content blob* by that hash,
   then set `src` to a `data:<mime>;base64,…` URL. Unresolved ⇒ `src` stripped
   (degrades to alt text, **never** fetches off-site).
   - On the HTTP arm, `http_poll::resolve_closure_via` pre-fetches a page's embed
     assets via the same two-hop (best-effort; a missing asset → alt, never fatal).
5. Resolution happens **DOM-side, not in the render output**, to keep the
   overlay's every-frame equality compare cheap at papers scale.

## 7. CDN / R2 deployment (the one un-wired step)

`dist/` is a static directory. To go live, DevOps:
1. `aws s3 sync --delete dist/ s3://<r2-bucket>/ --endpoint <r2>` (or rclone/wrangler).
   `--delete` prunes orphans on a **re**-publish (deleted pages, swapped-out
   blobs). For the full edit/add/delete/identity-churn republish mechanics, see
   [`../guides/GUIDE-REPUBLISH-AND-INCREMENTAL.md`](../guides/GUIDE-REPUBLISH-AND-INCREMENTAL.md).
2. Front it with the CDN; serve `index.html` for the SPA route, byte-serve the rest.
3. **Content-type matters**: `.wasm` → `application/wasm`, `.json` → `application/json`,
   the `.bin` files are opaque (`application/octet-stream` is fine — the SPA reads them).
4. **No build-time coupling to the destination.** Publish with the default empty
   `--live` (same-origin) and the **same bytes work at every URL** — localhost, R2
   preview, R2 prod. No server logic, no env, no per-domain rebuild, no domain to
   specify. DevOps just `sync dist/ → bucket`.
5. **Serve at the bucket/domain ROOT.** Portability holds at the root this
   release; a subdirectory mount (`host/sub/`) is not yet zero-config (tracked
   debt). If you must mount under a path, that's the case to revisit before
   relying on it.

Caching: `sw.js` already does cache-first asset delivery in-browser; on the CDN,
the content-addressed `content/<hash>` blobs are **immutable** (safe for
long/`immutable` cache headers); `index.html` + `entity-deployment.json` should
be short-TTL so a redeploy is picked up.

## 8. Code surface — the pieces it touches

`src/content_site/` (**32 files** as of 2026-09-10) is the home of the pipeline. The table below
names the load-bearing ones and is **not** an inventory:

| File | Role |
|---|---|
| `publish.rs` | the `publish` verb — arg parse, orchestration, `--bare-root` |
| `publish_fixture.rs` | `emit_site` / `emit_owned_sites` — serialize tree → `dist/` (pages, manifests, **asset blobs + pointers**, static html) |
| `ingest.rs` | disk site dir → tree entities; walks `assets/**`; normalizes `![]()`→`::embed`; skips `.placeholder` |
| `read.rs` | `OwnedSite` (+ `.assets` closure) — read a site subgraph back out |
| `embed.rs` | the embed standard — `markdown_to_embed`, `embed_to_markdown_image`, `parse_embeds`/`embed_refs`, `base64_encode` |
| `format.rs` | `SiteAsset` (content-addressed), manifests, `media_type_for_path` |
| `render.rs` | markdown→sanitized HTML; `format:html`→a sandboxed document; `::embed` lowering |
| `paths.rs` | tree path helpers + `asset_name_from_ref` (**the security gate**) |
| `http_poll.rs` | remote fetch — pages + `asset_bin_url`/`fetch_asset` (the two-hop), `resolve_closure_via` |
| `resolver.rs` | local/cached/remote resolution; `ResolvedPage.assets`; `persist_to_cache` write-through |
| `static_export.rs` | legacy `.html` emit (`Layout::Projection` / `BareRoot`); rewrites **both** `href` (`rewrite_hrefs`) **and** `src` (`rewrite_srcs`), and emits the asset files — see §9 |
| `deployment_config.rs` (`src/`) | parse/apply `entity-deployment.json` |
| `discovery.rs`, `origins.rs`, `cache.rs`, `prefs.rs`, `location.rs` | site enumeration, origin roster, foreign-site cache, prefs, link resolution |
| `dom/content_site.rs` (`src/`) | the WASM DOM read path — `make_asset_resolver`, `rewrite_images`, `rewrite_links` |

## 9. Proven vs deferred

**Proven live:** ingest → publish → static `dist/` → served → live
SPA boot → page fetch (http_poll) → render → **image two-hop → `data:` URL paints**.
All three image origins (curated SVG, compute `::embed` PNG, authored
content-addressed PNG) verified in a real browser, zero leaked srcs. Direct arm +
remote/http_poll arm both exercised. Permanent guard: the demo-SVG assertion in `tests/e2e_worker.rs` Phase 19-img.

**Proven live + pinned:** **intra-domain cross-site links** (§10
below). A `site:{site_id}/{page}` body link projects to a sibling site under the
same peer; proven on the seeded two-site demo and guarded by
`static_export.rs::intra_domain_cross_site_link_projects_to_sibling_site`.

**Landed since this list was written** (corrected 2026-09-10 — each was recorded
here as deferred while shipping, which is the expensive direction: *a shipped
capability recorded as deferred reads as work still owed*):

- **Static-export images — SHIPPED.** `static_export.rs` rewrites `src` as well
  as `href` (`rewrite_srcs`) and emits the asset files at
  `sites/{peer}/{site}/assets/{name}`. Measured in a published tree: `dist-site`
  carries `…/demo/assets/figures/demo.svg` and the page carries
  `<img src="/sites/…/demo.svg">`. The no-JS surface is no longer alt-text-only.
- **The name→peer-id layer — SHIPPED.** `registry_publish.rs` emits a signed
  name registry and `named_site.rs` resolves one; `make registry` is the verb.
  Inter-domain linking is no longer blocked on *"the registry still being built"*.

**Deferred (non-blocking):**
- **DevOps push to the bucket** — the static-bundle → CDN step (§7). **Owned by
  `<devops-tree>`, and its status is theirs to state, not ours** — see
  `docs/status/TRACKER-<devops-tree>.md` rather than trusting a status
  asserted here. (This row previously read *"not code; just un-wired"*, which was
  a claim about somebody else's estate.)
- **Worker-arm image pin** — code-verified + Direct/remote live-proven; a live
  e2e pin on `?worker=1` is belt-and-suspenders.
- **Feeds are not described anywhere in `docs/architecture/specs/`.** Four
  `app/feed/*` types, a publisher, a reader and a window exist; every account of
  them is in `AGENTS.md` or a dated `docs/status/` handoff. See
  `docs/plans/AUDIT-THE-PUBLISHING-PIPELINE-AND-WHY-ITS-MODEL-IS-UNFINDABLE-2026-09-10.md`
  §8 F4.

## 10. Cross-site linking — the settled contract

This is **not an open question.** The link vocabulary is settled by the
upstream semantic content-site application convention: `link-ref` blesses
exactly three forms, and the convention pins the URL projection. We implement
all three in `location.rs::classify_link` and project them in
`static_export.rs::static_href` / `dom/content_site.rs::rewrite_links`.

| As written in a page body | Meaning | Classifier (`location.rs`) | Static href / live nav |
|---|---|---|---|
| `./about`, `../x`, `intro` | in-site, dir-relative to the current page | `InSite` | same site |
| `/docs/intro` | in-site, root-absolute | `InSite` | same site |
| **`site:{site_id}/{page}`** | **cross-site, SAME peer (intra-domain)** | `CrossSite` | `/sites/{peer}/{site_id}/{page}.html` |
| `entity://{peer}/sites/{site}/pages/{page}` | cross-peer (inter-domain) | `CrossPeer` | `/sites/{peer}/{site}/{page}.html` |
| `https://`, `http://`, `mailto:` | leaves the system | `External` | verbatim, `target=_blank` |

**URL projection (§11 of the spec):** `{base}/sites/{peer_id}/{site_id}/{page}`.
`sites` is the SITE convention's reserved first-segment word at the NETWORK
§6.5.6 demux (Amendment 9). One `dist/` serves the WASM SPA at `/` plus the
static tree under `sites/…` → same server, same origin, links resolve locally.

**What papers must emit (the entire papers-facing ask).** A domain (e.g.
billslab) publishes all its sites under **one peer-id**, each site ingested with
its own `site_id` (domain-prefixed, e.g. `billslab-research`). A link from one
site to another under that domain is **cross-site, same peer** → papers emit
**`site:{target_site_id}/{page}`**. That is the only special form; everything
within a single site stays relative as today. Papers funnel page→page links
through one chokepoint (`render/resolve.go pageLink`) — for a cross-site target
it emits the `site:` form; we resolve it. No registry, no network hop, fully
resolved at ingest/render time.

**Three surfaces, one resolver.** `classify_link` + `resolve_target`
(`location.rs`) is the single classifier; it feeds (1) the live **window**
(`model.rs::navigate`→`go_to` switches `site_id` on the same peer), (2) the live
**overlay** (the deployed site-mode preview — delegates to the same
`model.navigate`), and (3) **static export** (`static_href`→`projection_href`).
A cross-site link works identically on all three (`site:` form, intra-domain).

**Footgun to know (D13).** `resolve_in_site` clamps `..` at the site root. If a
cross-site link is mistakenly authored as an escaping *relative* path
(`../../other-site/page.md`) instead of the `site:` form, it does **not** cross —
it clamps and resolves to a (wrong) in-site page, silently. The contract above
(emit `site:`) is what avoids this; an ingest-time escape detector is a candidate
hardening if a corpus ever ships escaping relative links.

## 11. Knobs cheat-sheet

| Want | Do |
|---|---|
| Publish billslab + serve locally | `make site-papers` (portable, same-origin) |
| Portable bundle for any CDN/R2 **root** | the default — **empty `--live`** (same-origin); drop `dist/` anywhere at the root |
| Deliberately pin the banner/config to one origin | `--live=https://<public-url>` (rare; not for cross-domain nav — that's registry/resolver) |
| Locked content-site deployment | `--surface=site --locked` |
| Multi-tenant (many peers, one domain) | `--prefix=<tenant>` per peer; never mix with a root peer |
| One site at the domain root (SSG) | `--bare-root` |
| Legacy HTML only | `--html-only` |
```

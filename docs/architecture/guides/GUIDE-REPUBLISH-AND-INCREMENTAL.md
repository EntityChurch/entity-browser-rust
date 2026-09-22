# Republish & Incremental Update Guide

> **The publishing model lives in one place:** [`REFERENCE-PUBLISHING-PIPELINE.md` §0.1](../specs/REFERENCE-PUBLISHING-PIPELINE.md#01-the-model-in-one-screen--read-this-before-changing-anything-that-publishes) — *publish translates an input into the tree, then projects the tree.* This document covers republish and incremental emission; it does not restate the model.

**Audience:** anyone re-running `publish` against a live deployment (DevOps,
content/site authors, the deploy tool). **Scope:** what a *re*-publish into an
existing `dist/` actually changes — per edit / add / delete / identity change —
and how that maps onto a CDN sync. **Companion:** deploy posture is
[`GUIDE-DEPLOYMENT-AND-CONFIGURATION.md`](./GUIDE-DEPLOYMENT-AND-CONFIGURATION.md);
the on-disk layout is [`REFERENCE-PUBLISHING-PIPELINE.md`](../specs/REFERENCE-PUBLISHING-PIPELINE.md).

> Every claim here was verified empirically against `examples/entity-demo`
> (publish → re-publish, edit, add, delete, identity swap; diff the `dist/`
> trees by content hash). The observations are reproducible with the commands
> shown.

---

## 1. The mental model (why republish is safe & near-incremental)

A publish emits three roots into `dist/` (`--prefix` nests them all; empty = root):

| Root | What | Addressing |
|---|---|---|
| `content/{aa}/{bb}/{hash}` | the **content-addressed blob store** — every page/asset body | **path = content hash** (immutable: identical bytes ⇒ identical path) |
| `{peer}/sites/{site}/…​.bin` | **entity-native pointers** — small `system/hash` refs a live peer ingests | stable tree path; body is a hash pointer |
| `sites/{peer}/{site}/…​.html` | the **legacy-web projection** (no-JS static site) | stable path; body is rendered HTML |
| `entity-deployment.json` | the per-domain config (always at the served root) | fixed path |

Two properties make republish predictable:

1. **Content-addressed ⇒ dedup + immutability.** A body's blob path *is* its
   hash. Unchanged bodies keep the exact same `content/…` path across every
   republish; changed bodies get a *new* path (the old one is not overwritten in
   place — it's simply no longer referenced).
2. **Clean-then-rewrite, scoped per publish.** Each run wipes the roots it owns
   before rewriting: **`sites/` and `content/` wholesale**, but **`{peer}/` only
   for the publishing peer** (`run_projection`'s clean step). So a same-identity
   republish self-prunes orphaned blobs and pages; an *identity change* does not
   prune the previous identity's `{peer}/` subtree (see [§5](#5-identity-churn-the-one-that-orphans)).

**The publisher identity is durable and stable** — the default keypair
(`persistence::publisher_keypair`, under `ENTITY_DATA_DIR`) is load-or-generated
once and reused, so the `{peer}` in every path is stable across republishes.
Keep that keypair; the peer-id *is* the site address (deep-links, `origins`).

> **A static publish IS signed — and which reader checks the signature is the
> part to get right.** This note said the opposite until 2026-08-24, and had
> already been corrected once before that; it is the standing shape of a
> "we cannot do X" claim whose expiry is somebody shipping X.
>
> **`entity-browser publish` emits a signed root.** Every publish projects the
> HAMT trie closure plus a signed `system/peer/published-root`
> (`src/content_site/signed_root.rs`, `RootProjector`), carrying a monotonic
> `seq` chained off the previous publish and a signature at
> `system/signature/{hex}`. `--verify` checks the closure and the trust anchor.
> A consumer that pins the publisher's key therefore gets **origin-independent**
> integrity: it walks the HAMT *from the signed root*, so the origin is trusted
> for nothing, and a rollback to an earlier publish is refused by the `seq`
> floor.
>
> **Two readers, two trust models — do not read the first as covering the
> second.** The naming chain (`name resolve` / `name open` →
> `content_site::signed_fetch::SignedSession`) walks the signed root and holds
> that floor for the life of the session. The **Site Browser's** navigate path
> does not: it fetches over the `http_poll` two-hop, checking the content body
> against the hash the `.bin` pointer claimed while taking the pointer itself on
> origin trust. That proves the tree is *self-consistent*, not that it is
> *ours*. It is defensible today only because the origins it will use come from
> the deployment's own `entity-deployment.json` or from a signature-checked
> registry binding — and the rail says `not verified` on those rows rather than
> claiming otherwise.
>
> So *"does a partial republish invalidate a signature?"* **does** have a
> subject: a partial upload that leaves the served tree missing a blob the
> signed root commits to is an *incomplete walk*, which a pinning consumer
> reports as such rather than as a missing page. That is the concrete reason
> §7's cut-over advice matters for a large deployment — see
> [§7 Deploying a republish](#7-deploying-a-republish-cdn-sync).

---

## 2. Case A — identical republish (no source change)

**Result: byte-for-byte identical `dist/`, same peer-id.** Deterministic and
idempotent — re-running publish on unchanged input is a no-op at the content
level (every blob hash, pointer, and HTML file is identical).

> Note: the files are physically *rewritten* (fresh mtimes) even when
> byte-identical — relevant only to the CDN sync strategy, see [§6](#6-deploying-a-republish-cdn-sync).

## 3. Case B — edit one page (change a body)

Editing `pages/about.md` and republishing touches **exactly**:

| Change | Path | Why |
|---|---|---|
| **new blob** | `content/{new-hash}` | the edited body (new bytes ⇒ new hash) |
| **removed blob** | `content/{old-hash}` | the old body — no longer referenced, pruned by the content-clean |
| **content changed, path stable** | `{peer}/sites/demo/pages/about.bin` | the pointer now references the new hash |
| **content changed, path stable** | `sites/{peer}/demo/about.html` | re-rendered with the new body |

**Everything else is byte-identical** — `entity-deployment.json`, the site
manifest/`pages.list`, and *every other site* (a second site in the same publish
is untouched). An edit is the narrowest possible change: **1 pointer + 1 HTML
rewritten, 1 blob swapped, zero orphans.**

## 4. Case C — add one page (an "add an article" release)

Adding `pages/changelog.md` **and** a `nav` entry for it in the site manifest,
then republishing:

- **Added:** `changelog.bin` (pointer) + `changelog.html` + its content blob(s).
- **Content changed (stable paths):** the site's `manifest.bin` + `pages.list`
  (the page set grew), **and every sibling page's `.html`** — because the shared
  **nav bar** was rebuilt to include the new link, so each rendered page changes.
- **Unchanged:** every sibling page's **`.bin` pointer and body blob** (their
  bodies didn't change — only the HTML projection's nav did), other sites, and
  `entity-deployment.json`.

So an add is incremental at the **entity-native layer** (only the new page's
`.bin`/blob is added; siblings' data is untouched) but re-renders the **whole
HTML projection's nav**. That HTML re-render is cheap (small files) and correct —
the nav must gain the new entry everywhere.

## 5. Case D — delete a page (and the dangling-link gotcha)

Deleting `pages/theory.md` from the source and republishing:

- **Cleanly pruned:** `theory.bin`, `theory.html`, and the theory body blob all
  **disappear** — no orphan artifacts (the wholesale `sites/`+`content/` clean
  plus content-addressing guarantee it). `pages.list` updates to the smaller set.

**⚠️ Gotcha — deleting the `.md` is NOT enough.** The site's **nav is authored
separately** in `site.manifest.json`'s `nav` array. If you delete `theory.md`
but leave its nav entry, publish drops the page (it renders 6 pages instead of 7
— it just skips the missing file) **but every sibling page still renders a
`Theory` link to `theory.html`, which no longer exists → dangling 404 links.**

**It is no longer silent** — see the warning below.

**Correct page-removal workflow:**
1. Delete the page file (`pages/theory.md`).
2. Remove its entry from `site.manifest.json`'s `nav` (and any `children`).
3. Fix any **inline body links** to it in other pages (`[Theory](…)`).
4. Republish → verify no dangling links: `grep -rl "theory.html" dist/` returns nothing.

Removing the nav entry (step 2) is what clears the dangling links — verified: with
the entry removed, `grep -rl theory.html dist/` = 0.

> **✅ IMPLEMENTED — this section's recommended ratchet landed and this guide did
> not say so.** `warn_dangling_nav_links` runs at the top of `run_projection`
> (`src/content_site/publish.rs`), before anything is written, and prints one
> line per offender:
>
> ```
> publish: WARNING — site "x" nav item "Theory" → "/theory" resolves to no page
>          (a dangling 404 link; delete the nav entry or add the page)
> publish: 1 dangling nav link(s) — the site ships but those links 404.
> ```
>
> **Warn, not fail** — deliberately. The site still publishes; a hard failure
> would block a legitimate publish over a cosmetic link. Gated by
> `dangling_nav_targets_flags_only_unresolved_in_site_links` and by
> `bundled_demo_set_has_no_dangling_nav_links`, which keeps the shipped demo set
> as the clean baseline.
>
> **Scope, so a warning is read correctly:** only **in-site** targets are
> checked. Cross-site (`site:other/x`), cross-peer and external targets resolve
> against trees this publish cannot see, and section headers (empty target) are
> skipped. A nav entry pointing at the site root resolves via the manifest's
> declared root page.
>
> *(Corrected 2026-08-17, after a measurement run against the real content tree
> reported four warnings while this guide still said the check did not exist.
> The ratchet landed in `2a63435f`; the guide is where it failed to land.)*

## 6. Case E — identity churn (the one that orphans)

Publishing with a **different identity** into the same `dist/` (a lost keypair,
or `--demo-identity` vs the durable default) is the one genuinely messy case:

- The **`sites/` HTML projection is cleaned wholesale**, so the old peer's HTML
  under `sites/{old-peer}/` is *removed* and only `sites/{new-peer}/` remains.
- But **`{peer}/` is cleaned per-peer**, so the old identity's
  **`{old-peer}/sites/…​.bin` pointer subtree LINGERS as an orphan** — the clean
  only wiped the *new* peer's (empty) dir.
- **All deep-links and `origins` that referenced the old peer-id break** — the
  site address changed (`sites/{peer}/…` moved).

**Avoid identity churn.** Keep the durable publisher keypair
(`ENTITY_DATA_DIR/publish/keypair`) — with it, every republish reuses the same
peer-id and none of this happens. If churn is unavoidable (a deliberate identity
rotation), do a **clean publish into a fresh `dist/`** rather than republishing
over the old tree, so no stale `{old-peer}/` subtree ships. (The content store is
identity-independent — content-addressed by body — so blobs dedup across
identities regardless.)

---

## 6b. You cannot publish just one thing (measured, 2026-08-17)

**A publish is whole-tree-per-`(prefix, identity)`.** `run_projection` removes
`{out}/{prefix}/sites` — plus `{out}/{prefix}/content` and `{out}/{prefix}/{peer}`
when emitting `.bin` — **wholesale, before rewriting**. So a publish carrying a
*subset* of the source set **deletes** everything it left out.

Measured by doing it, on a real seven-site tree — publishing one site into the
populated `dist/`:

| | site dirs | blobs | app bins | size |
|---|---|---|---|---|
| before | 7 | 1,862 | 35 | 103 MB |
| after | **2** | **68** | **0** | **2.7 MB** |

**The driver must own the complete source set.** Every republish ingests every
site plus the apps dist, or the omissions disappear. The cost of doing so is
nil — a full publish of that tree is **~7.4 s** — so this is a *knowledge*
requirement, not a performance one.

**It is not silent any more.** Publish now reads what is already projected
*before* the clean and names every site about to vanish:

```
publish: WARNING — site "entity-core-go" is already published here but is NOT in
         this publish; the projection is replaced wholesale, so it will be REMOVED
publish: 5 site(s) will disappear. A publish replaces the whole projection under
         this prefix — ingest the COMPLETE source set, or publish this site under
         its own --prefix.
```

**Warn, don't fail** — removing a site *is* a legitimate publish, and a hard
failure would make the legitimate case unreachable. Gated by
`a_site_left_out_of_a_publish_is_named_as_about_to_be_removed`
(mutation-checked) and `projected_site_ids_reads_the_layout_and_tolerates_an_absent_tree`.

**The clean itself is correct, and is deliberately scoped.** Stale artifacts must
not linger, and the scope is *this* prefix — a sibling peer's tree under another
prefix survives untouched. That makes **`--prefix` the real escape hatch** for
publishing one site independently, at a stated cost: `content/` lives *under* the
prefix, so per-prefix publishing **loses cross-site blob dedup** (the same body
shared by two sites is stored once per prefix, not once overall).

> **Incrementality lives at the upload layer, not the publish layer.** Publish is
> cheap and total; the sync is what makes a republish a 12 KB event instead of a
> 103 MB one (§7).

### 6b.1 `--plan` — find out before you run

`--plan` resolves the sources exactly as a real publish would, reports what would
change, and **writes nothing**:

```bash
make site PLAN=1 OUT=dist INGEST=<content-root> APPS_DIST=<apps-dist>
# or:  entity-browser publish dist --plan --ingest=… --ingest-apps=…
```

```
publish --plan: dist/sites/2KEB3Bn… (peer 2KEB3Bn…)
publish --plan: 4 present, 3 incoming — 3 kept, 0 added, 1 REMOVED
publish --plan:   - legacy-site  (REMOVED — not in this publish)
publish --plan: 1 site(s) would be REMOVED. … (exit 2)
```

**Exit codes are the interface — they are stable, and a gate may bind to them:**

| code | meaning |
|---|---|
| `0` | the plan removes nothing — safe to publish |
| `2` | the plan **removes** at least one site |
| `1` | the plan could not be produced (a real error) |

So this is a complete gate, with no extra flags and no parsing:

```bash
make site PLAN=1 OUT=dist INGEST=… && make site OUT=dist INGEST=…
```

**It reports; it does not decide.** A legitimate removal is *reported*, never
refused — a plan that could veto a deliberate act would simply get bypassed, and
then it would be telling you nothing at all. Pass the same flags you are about to
run: a plan of a different command predicts a different publish.

`--bare-root` owns no `sites/{peer}/` projection, so it has no add/remove set;
`--plan` says so and exits `0` rather than inheriting a verdict about a
projection it does not manage.

Gated by `plan_reports_the_delta_and_writes_nothing` — driven through the real
CLI entry, and **mutation-checked on the assertion that matters**: with the
early return removed the plan publishes, and the test fails on *"`--plan` must
not write into the output directory"*. The plan and the publish-time warning
share **one** definition of what disappears (`dropped_site_ids`), pinned by its
own test, because a `--plan` that disagreed with the publish it predicts would be
worse than no plan at all.

### 6b.2 Where the mechanics end and the process begins

This repo owns the **mechanics and the standard**: the publish contract, the
ingest format, the projection layout, the warnings, and the exit codes above.
They are deliberately **general-purpose** — the tool reads an ingest tree and
writes an output directory, and knows nothing about which sources are canonical,
which bucket it lands in, or when a cutover is safe. Anyone can run it against
any backend.

Whoever **operates** a pipeline owns the process: the complete source set, the
sync (with `--delete` and `--size-only`, §7), scheduling, and the cutover. The
publish tool's job is to make a correct process *expressible* — which is what
`--plan` and §6b's warning are for — not to encode one particular deployment's
policy.

**If the process needs a mechanic that does not exist, that is a requirement for
this repo, not a workaround for the driver.** Say what the pipeline needs to
know and it belongs here, next to the behaviour it describes.

## 7. Deploying a republish (CDN sync)

Because the layout is **content-addressed and path-stable**, a plain
`aws s3 sync --delete dist/ s3://<bucket>/` (or rclone/wrangler) is correct and
near-incremental:

- **New blobs** (edits/adds) upload; **changed HTML** (nav re-renders, edited
  pages) uploads; **orphaned blobs/pages** (from edits, deletes) are pruned by
  `--delete`; **orphaned `{old-peer}/` subtrees** (identity churn) are pruned
  too. `--delete` is what keeps the bucket from accumulating cruft.
- **One caveat — unchanged files are physically rewritten each publish (fresh
  mtimes).** A default `s3 sync` compares size + mtime, so it may re-upload
  byte-identical blobs. Since `content/…` paths *are* content hashes, a path
  match already proves the bytes match — so **`--size-only`** (skip when size
  matches) avoids needless re-uploads of the immutable blob store safely. The
  mutable `sites/…html` + `entity-deployment.json` are few and small; let them
  sync normally. A re-upload is only a bandwidth cost — never a correctness risk
  (idempotent).

**Rule of thumb:** `aws s3 sync --delete` = correct every time; add `--size-only`
(or scope it to the `content/` prefix) when you want to skip re-uploading the
unchanged content-addressed blobs.

> **⚠️ Large, live deployments — do NOT republish in place.** Publish's clean
> step wipes `sites/`/`content/`/`{peer}/` *before* rewriting, so re-publishing
> straight over a live tree opens a **destructive window** (the site is broken
> until the full re-upload finishes) and re-uploads everything. For a big
> deployment, use **blue-green**: publish a new version to a fresh location,
> verify, then cut over — preserving the old for rollback. Making that cheap is
> a design we hold internally and have not built: a shared append-only content
> store so the immutable blobs are never re-uploaded, versioned trees, and an
> atomic cutover. **Nothing in the tool does this for you today** — blue-green
> here means two output directories and a cut-over you perform yourself.

---

## 8. Quick reference

| You did… | dist/ changes | Orphans? | Deploy |
|---|---|---|---|
| **Nothing** (republish) | none (byte-identical) | none | no-op |
| **Edited a page** | 1 pointer + 1 HTML rewritten; blob swapped | none (self-pruned) | sync uploads 2 + 1 new blob, prunes 1 |
| **Added a page** (+ nav entry) | new `.bin`/HTML/blob; all HTML nav re-rendered; manifest/`pages.list` | none | sync uploads the new + re-rendered HTML |
| **Deleted a page** (.md + nav entry) | page's 3 artifacts pruned; `pages.list` shrinks | none | `--delete` prunes the 3 |
| **Deleted a page** (.md only) | page pruned; **dangling nav 404s remain — but publish WARNS by name** (§5) | dangling links | ⚠️ remove the nav entry too |
| **Changed identity** | new peer tree; **old `{peer}/` subtree orphaned**; deep-links break | `{old-peer}/` | prefer a fresh `dist/`; keep the durable keypair |
| **Published a SUBSET** of the sites | every omitted site **DELETED** — publish warns by name (§6b) | — | ingest the complete set, or use `--prefix` |

**Two things the deploy tail cannot do today** (`content-publish`, not this
repo): it has **no prune/delete**, so a removed page stays live in the bucket at
its old URL even though `dist/` self-pruned; and `plan_uploads` walks every file
with no HEAD or size compare, so every republish re-uploads the whole tree even
when the delta is 12 KB. §7's `--size-only`/`--delete` advice is correct and the
deploy tooling implements neither half — close that before the first real
publish, because it is the difference between *"republish is safe"* and
*"republish is safe and a delete actually disappears."*

**Reproduce:** publish twice into a scratch `dist/` with a fixed
`ENTITY_DATA_DIR`, mutate `examples/entity-demo`, and diff by content hash:
`(cd dist && find . -type f -exec sha256sum {} \; | sort -k2)`.

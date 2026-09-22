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

There is **no application server**. `dist/` is the whole product. "Will the CDN
work?" → yes, because R2 and `python -m http.server` are interchangeable static
file servers, and the SPA fetches **same-origin-relative** (or from an explicit
origin map — see §5).

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
4. **What enters the projection is `src/publish_axes.rs` — one list, three rows.**
   It was a hardcoded enumeration of two L5 conventions (`emit_owned_sites` +
   `for set in app_sets`) until 2026-09-10, when the feed axis landed and paid
   for the table. **A fourth convention is a row plus an `impl PublishAxis`**,
   and the compiler will not let it skip an obligation: name, tree prefix,
   `incoming()`, `project()`.
   - The prefix matters because **the clean is wholesale** — `{base}/{peer}/` goes
     in one `remove_dir_all`, so an axis nobody listed is not left alone, it is
     deleted. Every axis therefore also owes a term in `run_plan`.
   - **The residue that stays per-convention is tabulated in that module's doc**
     (the legacy-web `.html` export, `--bare-root`, the plan's per-unit naming,
     the `http_poll` URL builders) rather than left to be rediscovered.

### 0.2 The three axes

| axis | tree prefix | reader | ingest flag |
|---|---|---|---|
| sites (`APP-CONVENTION-SEMANTIC-CONTENT-SITE`) | `sites/` | `content_site::read::read_all_sites` | `--ingest=<dir>` |
| apps | `apps/` | `apps::read::read_all_app_sets` | `--ingest-apps=<dir>` |
| feed (`APP-CONVENTION-FEED`) | `app/feed/` | `feed_tree::read_owned_feed` | `--ingest-feed=<dir>` |

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
| `--ingest=<dir>` | read a site dir tree from disk into the tree first (else publishes the seeded demo set) |
| `--deployment-config` | also emit `entity-deployment.json` (home site, origins, posture) |
| `--config-site=<id>` | the SPA's **home site** (what it boots into) |
| `--surface=<chrome\|site\|window>` | cold-boot **surface** baked into the deployment config (default `window`) |
| `--window-type=<name>` | for `--surface=window`, the maximized window type (default `Site Browser`) |
| `--locked` | for `--surface=site`, emit the kiosk lock (no toggle, no peer creation) |
| `--live=<origin>` | the HTTP origin the SPA fetches content from. **Empty ⇒ same-origin** (portable; see §5/§7) |
| `--prefix=<p>` | host many isolated peers under one domain at `/{prefix}` (multi-tenant; empty ⇒ root) |
| `--html-only` | emit only legacy static `.html`, skip the entity-native `.bin` data |
| `--bare-root` | render a **single** site at the domain root (the no-JS SSG opt-out; `Layout::BareRoot`) |
| `--site=<id>` | publish only this site out of the set |
| `--ingest-apps=<dir>` | ingest an app set (entity-apps `dist/`) alongside the sites. `--ingest-games=<dir>` is an accepted alias |
| `--ingest-feed=<dir>` | ingest a directory of authored posts (`*.md` with a `+++` TOML block carrying **`created_at`**) as the peer's `APP-CONVENTION-FEED` archive — the **third publish axis**. The date is required and never taken from the file's mtime, which `git clone` rewrites; see `src/feed_ingest.rs` |
| **`--plan`** | resolve the source and report **what would change, writing nothing**. Has its own exit-code contract (`run_plan`) |
| **`--verify`** | prove an **already-published** tree resolves — every pointer, every body hashing to its address, the whole closure walkable (`run_verify`). **For a registry use `registry --verify`, not this** — different durable identity |
| `--allow-out-of-set-links` | downgrade an out-of-set `site:`/`entity://` target from a build **failure** to a warning. `--strict-links` is **accepted and ignored** — it asks for today's default, and other repos' pipelines still pass it |
| **`--set-home`** | this publish **moves the domain's home site**. Load-bearing: the home publish owns the domain-level fields and a secondary publish contributes only its `origins` entry, so without this a second peer defers to the existing home. A re-key *is* a deliberate home move |
| **`--supersede=OLD=NEW`** | *(repeatable)* the succession this domain **declares**. A consumer cannot infer it for anyone but the home peer — `origins` is a map, and a key leaving as another arrives is ambiguous between a re-key and one tenant replacing another |
| `--registry-pin=PEER_ID@ORIGIN` | the §7.4 preloaded name registry this deployment seeds. Same spelling as `registry --bind`, deliberately |
| `--identity-seed=<64 hex>` | publish under a **specific** system identity. Default is the durable publisher keypair under `{ENTITY_DATA_DIR}/publish/` |
| `--demo-identity` | the fixed demo publisher seed (dev/testing only). `--identity-seed` wins over it |

> **This table is the whole flag set as of 2026-09-10** (23 entries, cross-checked against
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

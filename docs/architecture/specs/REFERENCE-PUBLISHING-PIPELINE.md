# Reference — The Publishing Pipeline (source → tree → CDN → live site)

**Status:** authoritative reference. Describes the *working, live-proven*
pipeline that turns papers' rendered content into a browsable content-site
deployment — including images (the embed/asset arc). **Read this when** you
touch publish/ingest/serve, onboard the DevOps R2 step, or come back to extend
the flow (cross-site links, static-export images).

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

## 1. End-to-end flow

```
 ┌─ PAPERS TEAM ───────────┐   ┌─ THIS APP ──────────────────────────────────┐   ┌─ DEVOPS ────┐
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
| Author + render content | **papers team** | `paper.md` + `output/figures/*` → a site dir (`site.manifest.json`, `pages/`, `assets/figures/`) |
| Ingest + publish | **this app** (`make publish-papers`) | site dir → `dist/` (static) |
| Push to CDN | **DevOps** (not yet wired) | `dist/` → R2 bucket behind a CDN |
| Run the site | **the browser** | static files → live SPA |

The papers↔app contract (image grammars, `assets/**`, content-addressing) is the
ingest surface described in §4 and §8. The app↔DevOps boundary is §7.

## 3. The command

`make publish-papers` (`Makefile:235`) is the whole pipeline. It expands to:

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
    --config-profile=tutorial --live=
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
| `--config-profile=<full\|tutorial\|strict-site>` | cold-boot posture baked into the deployment config |
| `--live=<origin>` | the HTTP origin the SPA fetches content from. **Empty ⇒ same-origin** (portable; see §5/§7) |
| `--prefix=<p>` | host many isolated peers under one domain at `/{prefix}` (multi-tenant; empty ⇒ root) |
| `--html-only` | emit only legacy static `.html`, skip the entity-native `.bin` data |
| `--bare-root` | render a **single** site at the domain root (the no-JS SSG opt-out; `Layout::BareRoot`) |

## 4. `dist/` layout — what a CDN serves

A real, current publish of billslab (11 sites / 397 pages) = **71 MB, 1183 files**:

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
  "home_site": { "peer": "2KEB3…", "site": "billslab-main", "loc": "" },
  "origins":   { "2KEB3…": "http://localhost:8081" },
  "profile":   "tutorial",
  "site_mode": { "enabled": true, "locked": false, "show_toggle": true }
}
```

- **`home_site`** — what the SPA boots into.
- **`origins`** — `peer-id → base URL` for the published peer. **Default is the
  empty string = same-origin**: at runtime the SPA expands `""` to
  `window.location.origin` and fetches content from **whatever host served it**.
  An explicit absolute value (`--live=https://host`) is a **deliberate pin** —
  rarely needed. It is **NOT** a cross-domain federation mechanism (see below).
- **`profile` / `site_mode`** — cold-boot posture (overlay on/locked/toggle).

**Portability — the contract (DevOps):** publish with **empty `--live`** (the
default of `make publish-papers` / `publish-serve`). The `origins` value is `""`,
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
1. `aws s3 sync dist/ s3://<r2-bucket>/ --endpoint <r2>` (or rclone/wrangler).
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

`src/content_site/` (17 files) is the home of the pipeline:

| File | Role |
|---|---|
| `publish.rs` | the `publish` verb — arg parse, orchestration, `--bare-root` |
| `publish_fixture.rs` | `emit_site` / `emit_owned_sites` — serialize tree → `dist/` (pages, manifests, **asset blobs + pointers**, static html) |
| `ingest.rs` | disk site dir → tree entities; walks `assets/**`; normalizes `![]()`→`::embed`; skips `.placeholder` |
| `read.rs` | `OwnedSite` (+ `.assets` closure) — read a site subgraph back out |
| `embed.rs` | the embed standard — `markdown_to_embed`, `embed_to_markdown_image`, `parse_embeds`/`embed_refs`, `base64_encode` |
| `format.rs` | `SiteAsset` (content-addressed), manifests, `media_type_for_path` |
| `render.rs` | markdown→sanitized HTML; `::embed` lowering |
| `paths.rs` | tree path helpers + `asset_name_from_ref` (**the security gate**) |
| `http_poll.rs` | remote fetch — pages + `asset_bin_url`/`fetch_asset` (the two-hop), `resolve_closure_via` |
| `resolver.rs` | local/cached/remote resolution; `ResolvedPage.assets`; `persist_to_cache` write-through |
| `static_export.rs` | legacy `.html` emit (`Layout::Projection` / `BareRoot`); **rewrites `href` not `src` — images deferred** |
| `deployment_config.rs` (`src/`) | parse/apply `entity-deployment.json` |
| `discovery.rs`, `origins.rs`, `cache.rs`, `prefs.rs`, `location.rs` | site enumeration, origin roster, foreign-site cache, prefs, link resolution |
| `dom/content_site.rs` (`src/`) | the WASM DOM read path — `make_asset_resolver`, `rewrite_images`, `rewrite_links` |

## 9. Proven vs deferred

**Proven live:** ingest → publish → static `dist/` → served → live
SPA boot → page fetch (http_poll) → render → **image two-hop → `data:` URL paints**.
All three image origins (curated SVG, compute `::embed` PNG, authored
content-addressed PNG) verified in a real browser, zero leaked srcs. Direct arm +
remote/http_poll arm both exercised. Permanent guard: demo-SVG e2e pin (`d10dab0`).

**Proven live + pinned:** **intra-domain cross-site links** (§10
below). A `site:{site_id}/{page}` body link projects to a sibling site under the
same peer; proven on the seeded two-site demo and guarded by
`static_export.rs::intra_domain_cross_site_link_projects_to_sibling_site`.

**Deferred (non-blocking):**
- **DevOps R2 push** — the static-bundle → bucket step (§7). Not code; just un-wired.
- **Inter-domain (Type-2) cross-site links** — jump to a peer by *name*, resolved
  by the registry/discovery layer (`entity://{name}/…`). Not ours; registry layer
  still being built. The classifier already produces a `CrossPeer` target for the
  `entity://{peer}/sites/{site}/pages/{page}` form (§11) — what's missing is the
  name→peer-id resolution, which is the registry's job.
- **Static-export images** — `static_export.rs` rewrites `href` not `src` and
  copies no asset files (S5). The no-JS/SEO surface shows alt text only.
- **Worker-arm image pin** — code-verified + Direct/remote live-proven; a live
  e2e pin on `?worker=1` is belt-and-suspenders.

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
| Publish billslab + serve locally | `make publish-papers` (portable, same-origin) |
| Portable bundle for any CDN/R2 **root** | the default — **empty `--live`** (same-origin); drop `dist/` anywhere at the root |
| Deliberately pin the banner/config to one origin | `--live=https://<public-url>` (rare; not for cross-domain nav — that's registry/resolver) |
| Locked content-site deployment | `--config-profile=strict-site` |
| Multi-tenant (many peers, one domain) | `--prefix=<tenant>` per peer; never mix with a root peer |
| One site at the domain root (SSG) | `--bare-root` |
| Legacy HTML only | `--html-only` |
```

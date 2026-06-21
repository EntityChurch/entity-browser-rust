# ⭐ REFERENCE — The Content Site App

**Status: authoritative closeout reference.** This is the single place to start
when you return to the Content Site app — what it is, how it works, why it works
the way it does, what we decided and why, the code surface, the UX, and what is
deliberately left undone. It consolidates the prior topic docs (publishing
pipeline, link resolution, peer-general cache, web projection, site-flow/config)
into one app-level picture; where it overlaps a more specialized doc, the
specialized doc remains the deeper authority and is cross-referenced inline.

> If this doc disagrees with the upstream cross-impl guides (the semantic
> content-site application convention, the protocol + extension specs), **the
> upstream guides win.** This doc is the app-side consolidation, not the
> protocol authority.

---

## 0. What it is, in one paragraph

The Content Site app turns the entity tree into **browsable, publishable
websites**. A "site" is a content-addressed subgraph in a peer's tree
(`/{peer}/sites/{site}/…`): a manifest (identity + a human nav menu), markdown
pages, and asset blobs. The app **renders** a site two ways — as a normal app
**window** and as a full-screen immersive **Site Mode overlay** — through one
host-agnostic DOM renderer. It can **resolve** sites from three places (the
local tree, a durable cache of foreign peers' sites, or a remote peer over plain
HTTP). And it can **publish** a site set to a static `dist/` bundle (entity-native
`.bin` + legacy `.html` + content-addressed blobs + a per-domain deployment
config) that a dumb CDN serves and the live SPA boots into. The tree is the data
model end to end; there is no separate site database.

The publishing pipeline and live rendering (images and all) are **proven live**
against the real billslab corpus (11 sites, ~397 pages). It is **non-gating** for
the research preview. The big remaining arc is making the window
**site-aware** (add / delete / edit sites) — the "create your own sites" stretch
goal (§14).

---

## 1. The two surfaces + the one renderer

There are exactly two surfaces, and they share **one** pure DOM renderer
(`src/dom/content_site.rs::render`). The renderer takes a renderer-neutral
`SiteRenderOutput`, a `SiteNavHost`, and an `AssetResolver`, and builds DOM into
a container. It touches no tree/model/peer state — that is the model's job.

```rust
pub enum SiteNavHost {
    Window(WindowId),          // a Content Site window section — nav routes to that window
    Overlay { can_exit: bool },// the full-screen #site-layer overlay — app-level surface
}
```

| | **Content Site Window** (`SiteNavHost::Window`) | **Site Mode Overlay** (`SiteNavHost::Overlay`) |
|---|---|---|
| Host DOM | the window's shadow-DOM section | light-DOM `#site-layer` (full page) |
| Controller | `views/content_site/mod.rs` → `ContentSiteWindow` | `dom/site_overlay.rs` → `SiteOverlay` |
| Nav state | per-window, at the window's state path | app-level, at an overlay state path |
| Extra chrome | the **directory rail** (`dom/site_directory.rs`) — a list of all sites this peer holds | an **"Exit Site ▲"** control (only when `can_exit`) |
| Nav action | `Action::SiteNavigate{window_id,…}` / `SiteBack` | `Action::SiteOverlayNavigate{…}` / `SiteOverlayBack` |
| Reactivity | window dirty-flag (subscription) rebuilds the section | per-frame render with a `SiteRenderOutput`-equality **rebuild guard** |

**Why two surfaces.** The overlay is the immersive reading experience (no entity
machinery visible); the window is the "meta" view that also lets you see/manage
the set of sites. They are the same renderer because a site looks identical
either way — only link-click routing and the exit/rail chrome differ.

`can_exit` = `site_mode.show_toggle && enabled`. In a **locked** (strict-site)
deployment it is `false`, so the overlay renders **no** exit control — closing
BUG-1 ("Exit strands you in chrome with no way back"). The `ToggleSiteMode`
action is *also* guarded (no-ops when `locked`) as defense in depth; `?chrome=1`
is the operator escape hatch (§11).

---

## 2. The data model (entity types + tree layout)

### 2.1 Entity types (`src/content_site/format.rs`)

| Type tag | Struct | Fields |
|---|---|---|
| `app/site-manifest` | `SiteManifest` | `site_id`, `title`, `nav: Vec<NavItem>`, `params: BTreeMap<String,String>` (sorted, byte-stable). Landing page = `params.root` (default `"index"`). **No page-collection field** — see §2.3. |
| `app/site-page` | `SitePage` | `format` (`"markdown"` default \| `"html"` escape hatch), `body`, `frontmatter: BTreeMap` (`"title"` is the well-known key). |
| `app/site-asset` | `SiteAsset` | `media_type` (IANA, e.g. `image/png`), `bytes: Vec<u8>`. Content-addressed → identical `(media_type,bytes)` dedups in the store. |

`NavItem { label, target, children: Vec<NavItem> }` — empty `target` = a section
header; `children` is emitted only when non-empty (a flat nav is byte-stable and
back-compatible). Codec is `entity_ecf::to_ecf` + `ciborium`, decode is lossy
(malformed → default, never panics).

### 2.2 Tree placement (`src/content_site/paths.rs`) — a **free subgraph**

```
/{peer_id}/sites/{site_id}/manifest
/{peer_id}/sites/{site_id}/pages/{page}          # page may nest: guide/advanced/internals
/{peer_id}/sites/{site_id}/assets/{name}
```

A site is a **free subgraph at any publisher-chosen path**, NOT under
`system/content/…` (the v0.4.2 defect, corrected in v0.5). `sites` is a reserved
first-segment literal at the NETWORK demux layer; peer-ids are long base58, so
`sites/` is unambiguous. The site's capability scope is its own subgraph root
(`site_prefix(peer,site)`).

Key helpers: `manifest_path`, `page_path`, `pages_prefix`, `asset_path`,
`asset_name_from_ref` (the **security gate** — extracts `figures/x.png` from
`assets/figures/x.png`, **rejects external/escaping refs**), `parse_manifest_path`
(inverse).

### 2.3 Discovery is lazy (`.list`, not a manifest field)

Pages are discovered **level-by-level via `.list`** enumeration, never via a
manifest `pages` field (dropped in v0.4.1). The manifest is a **cover, not a
collection store** — this avoids the "download the whole index" anti-pattern at
scale (a huge index becomes an index-of-indexes; our `.list` *is* that
recursively). Ordering floor: lexicographic-by-name (a renderer presentation
rule, not a tree property). `discovery.rs` provides `list_child_pages` (one
level over the local/cached tree), `children_from_slugs` (the remote-`pages.list`
form), `list_sites`, `scan_local_sites`, and the async `list_all_sites` (a
universal `system/query` filtered by `SITE_MANIFEST_TYPE`).

---

## 3. The link model (one resolver, three surfaces)

All link logic lives in `src/content_site/location.rs` + `resolver.rs` and is
shared by **all three surfaces** (live window, live overlay, static export) so a
link resolves identically everywhere.

```rust
pub enum LinkTarget {
    InSite   { page },                       // directory-relative body link
    CrossSite{ site_id, page },              // site:{site_id}/{page} — same peer, other site
    CrossPeer{ peer_id, site_id, page },     // entity://{peer}/sites/{site}/pages/{page}
    External { url },                        // http(s):// , mailto: — leaves the system
}
fn classify_link(href, current: &Location) -> LinkTarget
```

### 3.1 In-site resolution (`resolve_in_site`) — the one convention

| Form | Result (from current `research/model/grounding`) |
|---|---|
| `/docs/intro` (leading `/`) | root-absolute → `docs/intro` |
| `./about`, bare `intro` | directory-relative → `research/model/about`, `research/model/intro` |
| `../notes/x.md` | parent-relative, **`.md` stripped at final slug** → `research/notes/x` |
| `../../../escape` | `..` **clamps at site root** → `escape` |
| `page.md#section?q` | fragment/query dropped (pages are whole files) → `page` |
| `/` | site root → `""` (renders `manifest.root`) |

**Rule:** body links resolve directory-relative; nav + app-generated links
(breadcrumbs, sidebar, section-index) are authored **root-absolute** so they
resolve identically from any page. This fixed the billslab 404 storm (0/12,419
broken, was 5,797).

### 3.2 Cross-site links — the `site:` contract

A link within one domain (same peer, different site) is written
**`site:{target_site_id}/{page}`**. A relative path like `../../other-site/x`
does **not** cross a boundary — the resolver clamps `..` at the site root and
silently lands wrong. The papers team's one-line ask: emit `site:` for
cross-site links. URL projection is `{base}/sites/{peer}/{site}/{page}`.

### 3.3 Click wiring (`dom/content_site.rs`)

`rewrite_links` walks the mounted markdown's `<a href>`: in-system links get
`preventDefault` + a nav action; `External` links get `target="_blank"
rel="noopener noreferrer"` and keep default navigation (they leave the system).
`wire_nav` attaches the click → push action → repaint.

---

## 4. Embeds & assets (the image story)

### 4.1 The embed standard (`src/content_site/embed.rs`)

One canonical inline form: `::embed[fallback-text]{ref=assets/figures/x.png}`.
Ingest **normalizes up** into it: markdown `![alt](src)` → `::embed[alt]{ref=src}`
(`markdown_to_embed`). For HTML render it **lowers back down** to a plain
`<img>` (`embed_to_markdown_image`). `embed_refs(body)` lists the asset refs
(deduped, order preserved). v1 embeds are **passive only** (no active/code
embeds — that is the G1-gated compute closure, deferred).

### 4.2 Asset resolution = a two-hop (`make_asset_resolver`, `rewrite_images`)

Assets are **content-addressed** and resolved in two hops:

1. **pointer** — read the asset pointer entity at the asset path (a
   `{type:"system/hash", data:<33-byte hash>}` leaf).
2. **blob** — read the content body by that hash (`content/{aa}/{bb}/{hex66}`),
   verify `Hash::compute(type,data)` matches (integrity gate), then inline it as
   a `data:` URL on the `<img>`.

`make_asset_resolver` uses the cache's **selector/path split**: the *store
selector* is always the **bound peer** (MY store — owned and cached-foreign
assets both live there); the asset path's peer-segment is the page's owning peer.
Resolution is L0/sync (Direct reads the store; Worker reads the cache mirror).
**Unresolved refs have their `src` removed** — the browser never fetches an
off-site/404 URL; the image degrades to its `alt`. Proven live with all three
image origins (curated SVG, computed PNG, authored PNG).

### 4.3 Render security (`src/content_site/render.rs`, F-CONTENT-1)

`render_page_body` renders `markdown` (default) via `markdown_to_html`
(tables/strikethrough/tasklists; embeds lowered to `<img>`). Raw HTML blocks/inline
**escape to text** (no `<script>` passthrough). `format:"html"` is rendered as
**escaped text**, not raw — there is no sanitizer, so there is no raw passthrough.

---

## 5. Rendering & reactivity

The model (`views/content_site/model.rs`) builds the renderer-neutral
`SiteRenderOutput` (`views/content_site/output.rs`):

```rust
struct SiteRenderOutput {
    site_title, nav: Vec<NavLink>, breadcrumbs: Vec<Crumb>, sidebar: Vec<SectionLink>,
    can_go_back, page_title, body_html,          // markdown already rendered to sanitized HTML
    peer: Option<String>, site_id, current_page, // location, for relative link classification
    error: Option<String>, loading,              // loading = async HTTP-poll Pending
}
```

`PartialEq`/`Eq` on this struct back the overlay's **rebuild guard**: the overlay
renders every active frame but only rebuilds the DOM when the output (or
`can_exit`) actually changes — an idle frame is a cheap compare with no DOM churn.
Window reactivity is the standard `WindowWatch` subscription dirty-flag.

The model also computes the **active-trail** (`in_section`: a nav item stays
highlighted across its whole top-level section), **breadcrumbs** (collapsing a
trailing `index`), and the **sidebar** (`.list`-derived, top-level + the active
section expanded one level). When a foreign origin is unreachable but the
manifest is cached, it emits a **manifest-pinned shell** (chrome from the durable
manifest + a synthetic notice page) — see §8.

---

## 6. Navigation & responsive UX

This is the UX layer hardened to closeout. The nav bar had a "can't get
out" bug (a long flat nav shoved Exit/Share off-screen) and was unusable on
mobile (fixed-width sidebars ate the screen; the dropdown clipped off-screen).
All fixed.

### 6.1 The nav bar (`render_nav_bar`)

Always visible: a **left cluster** — back affordance + the **site title, which is
a clickable Home link** (a `⌂` glyph; navigates to `/` = the manifest root). The
rest is **two layouts the responsive CSS swaps between**:

- **Desktop** (`.cs-nav-desktop`): up to `NAV_INLINE_MAX` (=4) nav items inline,
  the surplus under a **"More ▾"** dropdown (the active page is always kept
  inline — swapped in if it would land in the overflow), then a right cluster
  (Share + overlay Exit) pinned via `margin-left:auto` so it can never be
  displaced.
- **Mobile (≤768px):** the desktop region is hidden; a **hamburger ☰** opens a
  single **vertical dropdown** (`.cs-nav-menu`) with *every* nav link + Share +
  Exit. The panel is viewport-anchored (`left:8px; right:8px`) so nothing clips
  off-screen. No inline/overflow split on mobile — that mixed thing is desktop-only.

### 6.2 Collapsible sidebars

Both side columns collapse on mobile behind a native-feeling toggle (DOM-held
`cs-open` class, flipped by a button listener; survives idle frames, resets on
the next rebuild):

- the overlay/window **section sidebar** (`.cs-sidebar`, `render_sidebar`) →
  "Contents ▾" toggle, **closed by default** so page content shows first;
- the window-only **directory rail** (`.cs-rail`, `dom/site_directory.rs`) →
  "Sites ▾" toggle.

On mobile the body (`.cs-body`) and window row (`.cs-window-row`) stack
vertically (`flex-direction:column; overflow:auto`).

### 6.3 The responsive stylesheet

The shared renderer injects its own `<style>` (`RESPONSIVE_CSS`) as a root-scoped
element. This is **why it works in both surfaces**: the overlay mounts into the
light DOM `#site-layer` (which never gets `dom/style.rs::DOM_STYLES`), and a
`<style>`'s rules are scoped to the containing root — so the same injected sheet
also reaches the window's rail in the shadow DOM. Class names are `cs-*` to avoid
collisions. A scoped `.cs-main, .cs-main * { box-sizing:border-box }` reset fixed
right-edge body-text clipping on mobile (content was `width:100%`+padding under
the default `content-box`). The breakpoint is `@media (max-width:768px)`, matching
the main app's existing responsive rules.

**Known minor (deferred polish):** the *windowed* (non-maximized) Content Site
window on a very small screen stacks a lot of vertical chrome (palette + window
title + Sites toggle + nav bar + Contents toggle) before content. The
overlay/maximized path — the normal mobile case — is clean.

---

## 7. Discovery, caching & foreign sites

You can browse a **foreign** peer's site; the app caches it durably as **more
data in your own tree**, partitioned by peer-id. This is the clean model: owned
vs cached is a **path property** (the peer-segment), not a flag, and
`discovery::list_sites` enumerates cached sites for free — no special
foreign-site picker.

### 7.1 The field-split (three records)

| Record | Owner | Path | Holds |
|---|---|---|---|
| **A — content** | the foreign peer | `/{foreign}/sites/{S}/…` (in MY store) | manifest + pages, byte-faithful, content-addressed |
| **B — provenance** (SDK-tier) | me | `/{me}/system/cache/{foreign}/sites/{S}/provenance` | `CacheProvenance { last_reconciled, pinned_root_hash, source_transport }` |
| **C — prefs** (app-tier) | me | `/{me}/app/entity-browser/site-cache/{peer}/sites/{S}/prefs` | `SitePrefs { visit_count, bookmarked, is_home, keep_offline, last_viewed_page }` |

This is the **git model**: immutable content objects + a mutable manifest "ref" +
out-of-band metadata. Provenance and prefs are *about* the cache, so they live in
my namespaces, not in the content tree.

### 7.2 Cache-read-first + write-through (`resolver.rs::MultiResolver`)

Resolution order each frame: **(1)** read the local/cached tree first (hit → no
origin needed, works offline + on reload); **(2)** on an HTTP-poll completion,
**write through** to the cache via `persist_to_cache` (guarded by a `persisted`
HashSet → once per location per session). `persist_to_cache` always writes the
manifest + provenance + embed assets; it writes **page bodies only if
`keep_offline`** is set (O3 manifest-pinned default). A loud guard validates the
foreign peer-id before any cross-namespace write (prevents silent drops).

### 7.3 Manifest-pinned offline shell (O3)

If the origin is unreachable but the manifest is cached, the model renders a
**shell** — title, nav, breadcrumbs from the durable manifest + a synthetic
notice — so structure stays browsable with no network. Toggle `keep_offline` to
seed all pages for true offline.

### 7.4 Worker-arm cache warming (the subscription rule)

On the Worker arm the in-memory cache **mirror** is cold on reload until
subscribed. `site_overlay.rs::ensure_foreign_watches` subscribes `/{foreign}/sites/`
for every routable peer **each frame** (routes warm after boot); the snapshot
delivery warms the mirror so cache-reads hit. Without the subscription, durable
reads return `None` — the classic "Direct passes / Worker empty" trap.

---

## 8. Transport (`resolver.rs`, `http_poll.rs`, `read.rs`)

`MultiResolver` routes by peer:
- **local** (`peer == me` or `None`) → `LocalTreeResolver`, sync `Ready`.
- **registered origin** → `HttpPollResolver`, async `Pending` then fill+repaint.
- **unregistered, no local tree** → grace window (`UNREACHABLE_GRACE_MS = 8000`;
  native has no clock so never trips) then `ResolveError::Unreachable` (no more
  infinite loading).

`HttpPollResolver` is the Amendment-6 **two-hop** fetch: a `.bin` tree leaf is a
`system/hash` **pointer**; `crack_pointer` extracts the hash, `content_url`
fetches the blob at `/content/{aa}/{bb}/{hex66}`, `verify_and_decode` re-encodes
canonically and checks the hash (integrity). It fetches manifest → page → embed
assets (best-effort). One-hop enumeration artifacts `pages.list` /
`sites.list` (newline-delimited) make a remote site navigable (sidebar + deep
pages). `read.rs` is the local recursive reader (`read_site` / `read_all_sites` →
`OwnedSite`) used by publish.

---

## 9. The publishing pipeline

**Authoritative deep-dive: `REFERENCE-PUBLISHING-PIPELINE.md`.** Summary:

```
papers render dir ──(ingest)──▶ entity tree ──(read)──▶ emit ──▶ dist/ ──▶ CDN ──▶ live SPA
   (other team)                  (our peer)              │
                                                         ├─ {peer}/sites/{site}/…manifest.bin/pages/…/assets/…  (entity-native, SPA source of truth)
                                                         ├─ content/{aa}/{bb}/{hex66}                            (content-addressed blobs, deduped, long-TTL)
                                                         ├─ sites/{peer}/{site}/{slug}.html + sites.list + index (legacy no-JS/SEO fallback; images deferred)
                                                         └─ entity-deployment.json                               (per-domain posture + origins; optional)
```

- **`cargo run -- publish <dir>`** (`publish.rs`) orchestrates read + emit.
  Flags: `--ingest=<dir>` (disk → tree, `ingest.rs`), `--html-only`,
  `--bare-root`/`--site=ID` (one site at domain root, no entity branding —
  `Layout::BareRoot`), `--live=<origin>` (banner deep-link; **empty default =
  same-origin, portable**), `--deployment-config`, `--config-profile=`,
  `--config-site=`, `--prefix=<path>` (hosting-scope nesting; empty = byte-identical root).
- **`publish_fixture.rs::emit_owned_sites`** writes the entity-native `.bin` +
  content blobs; **`static_export.rs`** writes the legacy `.html` arm (rewriting
  in-site/cross-site/cross-peer links to projection paths, synthesizing
  section-index pages for bare dirs).
- **Ownership boundary:** papers *render* → this app *publishes* → DevOps *push to R2/CDN*.
  Each stage has an explicit contract. The bundle is **portable by design** — the
  same `dist/` bytes work at any root URL with no rebuild (the SPA expands an empty
  `origins` to `window.location.origin` at runtime).

---

## 10. Configuration, site mode & boot (`app.rs`, `session_config.rs`, `deployment_config.rs`)

### 10.1 Five-layer precedence (governs **posture**, not the origin registry)

1. **URL overrides** (ephemeral, never persisted): `?site=`, `?chrome=1`,
   `?boot_window=`, `?fastpaint=`.
2. **Durable session config** — `/{me}/…/settings/session` (a returning user wins).
3. **Per-domain `entity-deployment.json`** — fetched cold-boot only.
4. **Build-time defaults** — `ENTITY_PROFILE`, `ENTITY_HOME_*` env (baked).
5. **Hard default** — `Profile::Full`, bundled demo.

The **site-origin registry is NOT a preference** — it is re-derived/registered at
boot from the deployment config, not carried as user state.

### 10.2 `entity-deployment.json` (one small file per domain)

`home_site {peer,site,loc}` (boot destination) · `origins {peer-id → base URL}`
(default `""` = same-origin) · `profile`/`site_mode` (cold-boot posture). One
generic WASM bundle serves N domains; each domain ships this file.

### 10.3 Three profiles (the posture knob)

- **`full`** (default): full entity chrome + a toggle into Site Mode.
- **`tutorial`** (published default): overlay-first + a small "open live" banner + toggle back.
- **`strict-site`**: locked kiosk — site only, no chrome, no toggle, no escape (except `?chrome=1`).

### 10.4 Site mode, deep links & the escape hatch

- `apply_site_mode()` (per frame, idempotent) reads the config, ORs in the
  ephemeral `site_deeplink_active` and `chrome_override`, and sets the container
  mode (`mode-site` vs `mode-dom`) + status-bar/toggle visibility.
- `?site={peer}/{site}/{page}` (`site_deeplink_override`) navigates the overlay
  and forces it on for the session (never persisted). The `self` sentinel
  (`paths::SELF_PEER`) resolves to the local system peer (same-origin safe); a
  real peer-id is registered as a same-origin origin so the resolver HTTP-polls
  the published `.bin`. This is the static→live round-trip (the banner fix).
- **`?chrome=1`** (also `?unlock=1`) forces chrome + exposes the toggle — the
  operator escape from a locked deployment; it wins over a deep-link.
- `ToggleSiteMode` refuses when `locked` (unless `chrome_override`), clears the
  ephemeral deep-link override (user action wins), and flips off the **last
  visible** state (not the persisted config, which can desync from ephemeral
  overrides).

---

## 11. Code surface map

| Path | Role |
|---|---|
| `src/content_site/format.rs` | entity types (`SiteManifest`/`SitePage`/`SiteAsset`/`NavItem`) + media-type inference |
| `src/content_site/paths.rs` | tree paths, URL projection, deep-link forms, `asset_name_from_ref` (security gate), prefix normalization |
| `src/content_site/location.rs` | `Location`, `classify_link`, `LinkTarget`, `resolve_in_site` (the one in-site rule) |
| `src/content_site/resolver.rs` | `ContentResolver` trait, `LocalTreeResolver`/`HttpPollResolver`/`MultiResolver`, cache-read-first + write-through, grace |
| `src/content_site/http_poll.rs` | Amendment-6 two-hop fetch, `crack_pointer`/`verify_and_decode`, `*_bin_url`, `pages.list`/`sites.list` |
| `src/content_site/discovery.rs` | `list_child_pages`, `children_from_slugs`, `list_sites`, `scan_local_sites`, async `list_all_sites`, `refresh_site_index` |
| `src/content_site/cache.rs` | `CacheProvenance` ledger; `manifest_hash_hex`; provenance read/write |
| `src/content_site/prefs.rs` | `SitePrefs` (visit/bookmark/home/keep_offline) read/write/update |
| `src/content_site/origins.rs` | `peer-id → origin` registry (`set_origin`/`get_origin`/`list_origins`) |
| `src/content_site/embed.rs` | embed standard; `markdown_to_embed`/`embed_to_markdown_image`/`embed_refs`; `base64_encode` |
| `src/content_site/render.rs` | `render_page_body`, `markdown_to_html` (sanitized; raw HTML neutralized) |
| `src/content_site/read.rs` | recursive tree reader → `OwnedSite` |
| `src/content_site/publish.rs` | `publish` CLI verb + flags |
| `src/content_site/publish_fixture.rs` | emit entity-native `.bin` + content blobs |
| `src/content_site/static_export.rs` | legacy `.html` projection + link rewriting + `Layout::{Projection,BareRoot}` |
| `src/content_site/ingest.rs` | disk site dir → tree entities (papers render output) |
| `src/deployment_config.rs` | parse/apply/emit `entity-deployment.json`; `expand_origin` |
| `src/session_config.rs` | `SessionConfig`, `Profile`, `home_site`, `DEMO_SITE_ID`, site-mode state |
| `src/views/content_site/{mod,model,output}.rs` | window controller, `ContentSiteModel`, renderer-neutral output shapes; `ensure_demo_site` |
| `src/dom/content_site.rs` | host-agnostic renderer: `render`, `render_nav_bar`, sidebar, breadcrumbs, content, asset/image rewrite, `RESPONSIVE_CSS` |
| `src/dom/site_overlay.rs` | `SiteOverlay` — full-page overlay, rebuild guard, subscription wiring |
| `src/dom/site_directory.rs` | window-only directory rail |
| `src/action.rs` | `Site{Navigate,Back,Open,BookmarkToggle,KeepToggle}`, `SiteOverlay{Navigate,Back}`, `ToggleSiteMode` |

---

## 12. Key decisions & rationale (consolidated)

| Decision | Why | Status |
|---|---|---|
| Two surfaces, one renderer (`SiteNavHost`) | a site looks identical either way; only routing/chrome differ | ✅ shipped |
| Site = free subgraph at `/{peer}/sites/{site}/…` (not `system/content`) | L5 sites are first-class; `sites` is a reserved demux word | ✅ v0.5 |
| Manifest = cover (identity + nav), discovery is lazy `.list` | avoids "download the index" at scale | ✅ v0.5 |
| Markdown + inline `::embed` directive; passive only | universal/portable; rounds-trips to an entity; active embeds G1-gated | ✅ v0.5 |
| One link resolver, three surfaces; body=dir-relative, generated=root-absolute, `.md` stripped | no surface drift; fixed the 404 storm | ✅ shipped |
| Cross-site = `site:{site}/{page}` | relative paths can't cross (clamp at root) | ✅ shipped (papers ask pending) |
| Content-addressed assets, two-hop resolution, unresolved → `src` removed | offline-safe, dedup, XSS-safe (no off-site fetch) | ✅ proven live |
| Foreign sites cached at natural path in MY store; field-split (content/provenance/prefs) | owned-vs-cached is a path property; git object/ref model | ✅ P1/P3 shipped |
| Cache-read-first + write-through; manifest-pinned offline shell | reload/offline work; structure always browsable | ✅ shipped |
| Worker-arm `/{foreign}/sites/` subscription each frame | warms the mirror; avoids Direct-passes/Worker-empty | ✅ shipped |
| Publish = entity `.bin` + content blobs + legacy `.html` + deployment.json | live source of truth + dumb-CDN fallback + per-domain posture | ✅ proven live |
| Portable bundle: empty `--live` = same-origin | same bytes work at any URL, no rebuild | ✅ shipped |
| 5-layer config precedence; origin registry is NOT a preference | operator + user + dev flexibility; warm boot re-derives routes | ✅ shipped |
| Profiles full/tutorial/strict-site; locked refuses toggle; `?chrome=1` escape | choose chrome posture; never strand the user (BUG-1) | ✅ shipped |
| Nav: desktop inline+More, mobile hamburger; pinned Share/Exit; clickable Home | a long nav can never hide the exit; mobile is usable | ✅ shipped (this session) |
| Responsive `<style>` injected root-scoped; `box-sizing` reset | works in both light-DOM overlay and shadow-DOM window | ✅ shipped (this session) |

---

## 13. Known deferred / open items

| Item | Why deferred | Pointer |
|---|---|---|
| **Cross-DOMAIN links** (`entity://{name}/…`, name→peer) | needs the registry/discovery layer (still being built); explicit peer-id form works | classifier emits `CrossPeer` today |
| **Static-export images** | live SPA renders images; static is an archive layer — `static_export.rs` rewrites `href` not `src`, copies no assets | collect blobs → `assets/` → rewrite `<img>` |
| **Share control still emits `self`** | same class as the (fixed) banner bug; wrong for foreign/cached sites | give it the real-peer-id treatment |
| **Site-escape silent clamp (D13)** | the `site:` contract avoids it; candidate ingest-time escape detector | `resolve_in_site` clamps `..` silently |
| **DevOps R2 push** | operator step, not code: `aws s3 sync dist/ → bucket` | `REFERENCE-PUBLISHING-PIPELINE §7` |
| **Durable hosting peer** | publish source is an ephemeral seeded peer; real hosting loads a durable tree | parallel SDK gap |
| **Windowed-mobile chrome stacking** | minor; overlay path is clean | §6.3 |
| **Directory rail → dropdown** | P3 UX redesign | peer-general-cache P4 |
| **Site-aware window (add/delete/edit)** | the big pre-release arc — see §14 | P4 |
| **Registry / federation; L5 repos & spaces** | post-preview; sites is the shipped L5 convention, repos/spaces are exploratory | web-projection design |

---

## 14. Stretch goal — "create your own sites": can we do it?

**Short answer: yes, and most of the substrate already exists — the missing piece
is an editing UI, not new architecture.** The honest assessment:

**What already works in our favor.**
- The tree **is** the data model. A site is just three entity types
  (`SiteManifest`/`SitePage`/`SiteAsset`) at `/{me}/sites/{site}/…`. Creating a
  site = writing those entities to my own peer (exactly what `ensure_demo_site`
  and `ingest.rs` already do).
- We already **write** sites (ingest from disk; demo seed), **read** them back
  (`read.rs`), **render** them (window + overlay), and **publish** them. The full
  round-trip exists for content we didn't author in-app.
- Window state is entity-backed and reactive; editing-in-tree is the established
  pattern (write → subscription → rebuild). An editor would `put` a `SitePage`
  and the view would refresh for free.
- The directory rail already enumerates owned vs cached sites; the actions
  (`SiteOpen`/`SiteBookmarkToggle`/…) and the per-peer writer seam
  (`writer_handle_for`) are in place.

**What's missing (the actual work).**
1. **Create/delete a site** — UI to write an initial manifest at a new
   `site_id` (and remove the subgraph). Small.
2. **Edit a page** — a markdown editor pane that `put`s `SitePage` to the tree;
   live preview is near-free (re-render on the subscription). Medium.
3. **Manage nav + assets** — edit the manifest's `nav`; upload an image →
   `SiteAsset` (content-addressed) → insert an `::embed`. Medium.
4. **Make the window site-aware** — owned-vs-cached affordances, the
   add/delete/edit surface (the P4 arc).

**The verdict.** No architectural blockers — the hard parts (content-addressing,
the entity formats, rendering, publishing, the persistence/reactivity loop) are
done and proven. "Create your own sites" is a **UI build on a finished
substrate**, not a research problem. The main design questions are editor UX
(raw markdown vs WYSIWYG — keep it markdown, per the landscape study) and how
much to lean on the existing reactive preview. It's a real, scoped feature, not a
moonshot.

> Adjacent forward note (not part of this app): the plan to **rename the
> Knowledge Base window to a "Notes" app** for clarity. That's a separate, small
> rename/reframe and doesn't touch the Content Site surfaces — tracked
> independently.

---

## 15. Test coverage & live-proven

**Live-proven:** full publish pipeline (billslab, 11 sites/~397
pages, end-to-end); image two-hop (SVG + computed PNG + authored PNG, zero leaked
`src`); `site:` cross-site links on all three surfaces; static→live deep-link
banner (real peer-id); HTTP-poll remote boot into a foreign peer; durable
foreign-site cache across reload; manifest-only offline shell; static `.html`
export; the nav/mobile UX (WebDriver drives at 360–1280px).

**Permanent e2e (`tests/e2e_worker.rs`, Worker arm):** site browsing + deep nav +
active-trail (Phase 20a–d), the tree-driven sidebar over the Worker cache mirror
(Phase 20c), foreign-site cache durability (Phase 21/21b), `?site=` deep-link
boot (Phase 26), boot-into-foreign-home (Phase 27). Native guards include
`intra_domain_cross_site_link_projects_to_sibling_site` and the demo-seed/SVG
pins. Discipline: permanent e2e stays papers-independent (demo fixtures) and
short; real-content validation is a one-off WebDriver drive.

**Suite at closeout:** 513 native unit + 11 Worker-arm e2e, clippy clean, WASM
builds. (If e2e ever returns 0/11, suspect the Selenium container wedged —
restart it per `tools/e2e/README.md`, not the code.)

---

## 16. Disciplines that earned their place here

- **Trace values, don't theorize substrate.** The "No sites yet" bug *looked*
  like a Worker cache/subscription race; it was a per-peer writer/reader store
  divergence in our own code (writer used the primary, reader the backend peer).
  Add `tracing::debug!(found=…)` and re-run before blaming the arm.
- **Never split arm choice off the primary for a per-peer op.** Use
  `Peers::writer_handle_for(peer_id)`, not the primary-bound handle. (`AGENTS.md`
  arm-split footgun.)
- **Worker-arm e2e is mandatory for any tree-reading surface.** Native tests
  share one store and hide arm asymmetry; assert the user-visible result on the
  Worker arm.
- **Never strand the user.** Pinned exit chrome, the lock-refuses-toggle guard,
  and `?chrome=1` exist because each closed a real "trapped in a locked/overflowed
  UI" bug. Chrome must never be displaceable by content.
- **Verify through the real delivery path.** Green ≠ working — every UX claim in
  §6 was checked by driving the actual WASM build in a real browser at real
  viewport widths, not just by tests passing.

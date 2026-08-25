# Deployment & Configuration Guide

**Audience:** anyone deploying Entity Browser to a domain (DevOps, the deploy
tool, content/site authors). **Scope:** how a published deployment is shaped —
peer identity, the per-domain config file, the startup surface & posture, the
publish command surface, and the concrete recipes (deploy a site / lock a kiosk /
apps-only with no site overlay).

This is the **quick, self-contained reference**. The deeper authoritative docs
it draws from are linked at the bottom ([§10](#10-where-to-go-deeper)); where
this guide and those overlap, those win.

> **TL;DR**
> - **One generic WASM bundle** is shaped per-domain by a small JSON file,
>   `/entity-deployment.json`, fetched at boot. **No per-domain rebuild.**
> - The **published peer-id is deterministic** — stable across re-publishes.
>   You don't manage or rotate it.
> - The **`surface`** field sets the cold-boot surface directly (no preset
>   names): `chrome` (the workspace), `window` (a maximized window — e.g. a Site
>   Browser — via `window_type`), or `site` (the full-viewport overlay). The
>   granular posture (`site_mode`, `peer_creation_enabled`) is set alongside it,
>   not bundled behind a preset. A **locked kiosk** = `surface: "site"` +
>   `site_mode.locked: true` + `peer_creation_enabled: false`.
> - **To deploy apps with no forced site:** `surface: "chrome"` plus
>   `site_mode: { "enabled": false, "show_toggle": false }`. Games/Apps work
>   normally; users land on chrome, never the site.

---

## 1. The mental model

Entity Browser ships as **one** size-optimized WASM single-page app (the
"SPA"). It is **not** rebuilt per customer/domain. Instead:

```
   ┌─────────────────────────┐         ┌──────────────────────────────┐
   │  one generic WASM bundle │  +      │  /entity-deployment.json      │
   │  (index.html + .wasm)    │         │  (small, per-domain, fetched  │
   │  identical everywhere    │         │   at boot over plain HTTP)    │
   └─────────────────────────┘         └──────────────────────────────┘
                    │                              │
                    └──────────────┬───────────────┘
                                   ▼
                    cold-boot posture for THIS domain
            (which site, locked or not, apps-only, origins, …)
```

At boot the SPA does a single `GET /entity-deployment.json` from its own
origin. If served, it shapes the cold boot. If absent (404), unreachable, or
unparseable, the SPA **silently** falls back to its build-time defaults and
boots normally — the config never blocks or fails boot (this is the **D16
honesty** rule). A default bundle with no config file boots byte-identically to
having no config system at all.

Two artifacts therefore make up a deployment:

1. **The SPA bundle** (`dist/` — `index.html`, `*.wasm`, `*.js`, `sw.js`).
2. **The published content + config** written alongside it by `make site`
   (`sites/…`, `content/…`, `{peer}/…`, and optionally `entity-deployment.json`).

DevOps drops the whole `dist/` directory on a CDN / R2 bucket at the domain
root. That's the deploy.

---

## 2. Persistent peer-ids (PIDs)

There are **two distinct peer-ids**. Don't conflate them.

### 2.1 The published peer-id — *you choose the identity; stable per seed*

When you `make site`, all content is keyed under a single peer-id:

```
sites/{peer_id}/{site}/…          ← legacy-web .html projection
{peer_id}/sites/{site}/…          ← entity-native .bin content data
{peer_id}/apps/{set}/…            ← embedded apps (games/tools)
```

That `peer_id` is derived from the publisher's keypair. **By default the publish
identity is DURABLE**: `make site` loads-or-generates a keypair under
`{ENTITY_DATA_DIR}/publish/keypair` (`persistence::publisher_keypair`), so the
**first** publish mints a stable identity and every later publish reuses it — the
peer-id is the site address, so it must not drift per run. Three ways to pick it:

| How | Identity |
|---|---|
| *(default)* | the durable `{ENTITY_DATA_DIR}/publish/keypair` — minted once, reused |
| `IDENTITY_SEED=<64-hex>` (`--identity-seed=`) | a SPECIFIC 32-byte system seed (same hex form as `entity_system_seed`), so each site/deployment gets its own stable peer-id |
| `DEMO_IDENTITY=1` (`--demo-identity`) | the fixed demo publisher seed (`2KEB3…`) — dev/testing only |

> **Container gotcha (already wired):** the containerized `make site` runs
> `podman run --rm`, so `~/.entity` inside it is ephemeral and the durable key
> would regenerate every run. The Makefile points `ENTITY_DATA_DIR` at the
> repo-local, gitignored `PUBLISH_DATA_DIR` (default `.entity-publish/`, visible
> in-container via the parent mount) for **both** `site` and `site-serve`,
> so the identity persists and both modes publish under the same peer-id.
> Override the location with `PUBLISH_DATA_DIR=…`. **Treat `.entity-publish/` like
> a private key — it is one; never commit it.**

**For a given seed the peer-id is deterministic** — re-running `make site`
with the same `IDENTITY_SEED` re-emits the same `sites/{peer_id}/…` layout, so
deep-links, `origins` entries in `entity-deployment.json`, and cross-site
references keyed to that peer-id keep working across deploys. **A malformed seed
fails the build** (it won't silently fall back to the demo identity).

**Managing seeds:** a deployment's seed is yours to generate and keep
(out-of-band — treat it like a private key; it *is* the publisher's secret).
Generate any 32-byte value as 64 hex chars (e.g. `openssl rand -hex 32`) and
reuse it for every re-publish of that deployment. Different deployments →
different seeds → isolated peer-ids that never collide.

**You usually don't need to copy the peer-id by hand:** when you publish with
both `IDENTITY_SEED` *and* `DEPLOY_CONFIG=1`, the emitted `entity-deployment.json`
is **auto-keyed** to the derived peer-id (`home_site.peer` + the `origins` key),
so the config and the content always agree. The build also prints the peer-id if
you need it for a hand-written config or cross-site `origins`.

> **Forward note (seam):** the publish *identity* is now durable (the
> load-or-generate keypair above), but the publisher still seeds the bundled demo
> site *content* when you don't `--ingest` real content. The remaining seam is
> loading a full persisted peer **directory** (its durable keypair AND its real
> pre-seeded content/tree) instead of ingesting a folder — same stable-peer-id
> guarantee, just sourcing the content from a live peer store. Nothing downstream
> changes.

### 2.2 The runtime per-browser peer-id — *each visitor's own identity*

Every browser/device that opens the SPA generates **its own** "system peer" from
a seed stored in `localStorage` under the key `entity_system_seed`
(`persistence.rs::system_seed`):

- **First visit:** a fresh 32-byte seed is generated and persisted.
- **Every later reload:** the same seed is read back → the **same** identity is
  reconstructed. The derived peer-id names the IndexedDB database
  (`entity-peer-{id}`), which is what makes the visitor's local state **durable
  across reloads**.
- **Clearing browser storage** (or a fresh incognito session) ⇒ a new identity.

This is the **viewer's** key, used to own their *local* tree (window state,
their own peers, cached foreign sites). It is **never** the publisher's key.
Visitors fetch your published content over plain HTTP via `origins`; they do not
need — and never receive — the publisher's private key. **For deployment you
only ever think about the published peer-id (§2.1).**

> Multi-tab note: a single Web-Lock leader (keyed on the system-seed id) holds
> the durable IndexedDB store; additional tabs in the same browser stay
> in-memory on purpose to avoid last-writer-wins corruption. This is automatic;
> nothing to configure. See [`MODEL-PEER-LIFECYCLE-AND-STARTUP`](#10-where-to-go-deeper).

---

## 3. The configuration file: `entity-deployment.json`

Served at the **origin root** (`/entity-deployment.json`), regardless of any
content `--prefix`. Every field is **optional**; a partial config overrides only
what it names and inherits the rest. Unknown keys are ignored.

### 3.1 Full schema

```json
{
  "surface": "chrome",
  "window_type": "Site Browser",
  "home_site": { "peer": "<published-peer-id>", "site": "<site-id>", "loc": "" },
  "origins": { "<published-peer-id>": "" },
  "site_mode": { "enabled": true, "show_toggle": true, "locked": false },
  "fast_paint": true,
  "peer_creation_enabled": true
}
```

| Field | Type | Meaning |
|---|---|---|
| `surface` | `"chrome" \| "site" \| "window"` | Cold-boot **surface** — the primary axis (see [§4](#4-surface--posture)). Set the surface directly; the granular posture fields below are set alongside it, not bundled behind a preset name. Absent ⇒ inherit the build-time default surface. |
| `window_type` | `string` | Only for `surface: "window"` — which window type to boot maximized (e.g. `"Site Browser"`, `"Games"`, `"Apps"`, `"Entity Tree"`). Must be a registered window type. |
| `home_site` | `{ peer, site, loc }` | The startup site — where a maximized Site Browser (`surface: "window"`) or the site overlay (`surface: "site"`) lands, and what the home toggle opens. `peer` = the published peer-id; `site` = a site id that exists in the publish; `loc` = optional page/path within the site (`""` = the site's index). |
| `origins` | `{ peerId: originString }` | Where each hosting peer's published artifacts live, so the resolver can HTTP-poll them. **`""` = same origin** (the SPA expands it to `window.location.origin` at runtime — the common CDN case). A root-relative `"/sub"` = same origin under a prefix. A concrete `"https://host"` = cross-origin. |
| `site_mode.enabled` | `bool` | Whether site mode exists at all. `false` ⇒ no overlay, no toggle, ever. |
| `site_mode.show_toggle` | `bool` | Whether the ⛶ site toggle appears in the status bar. The toggle shows iff `show_toggle && enabled`. |
| `site_mode.locked` | `bool` | Kiosk lock — `true` removes every chrome↔site escape (the escapable⇄locked axis). |
| `fast_paint` | `bool` | Phase-1 fast-paint kill switch (paints the site shell before peers boot, for site-first deployments). Leave default unless debugging a paint flash. |
| `peer_creation_enabled` | `bool` | Capability gate — set `false` to forbid minting new peers **independent of surface** (so a `chrome` deployment can still disable creation, and a locked kiosk sets it `false` here). |

### 3.2 Precedence (highest wins)

```
1. URL overrides            ?site=…  ?boot_window=…  ?chrome=1        (dev/showcase, never persisted)
2. Durable persisted config a returning user's own saved settings     ← always wins on a warm boot
3. THIS fetched config      /entity-deployment.json                   ← shapes a COLD boot
4. Build-time env defaults  ENTITY_STARTUP_SURFACE / ENTITY_HOME_*    (baked fallback)
5. Hard default             chrome (workspace, local demo)
```

The deployment config shapes a **cold** boot only — the SPA fetches it just
when no durable config exists yet. A returning user's persisted choices always
win. (This is why testing a config change may require clearing storage or a
fresh profile — your own previous session is winning at level 2.)

---

## 4. Surface → posture

There is **no profile preset layer** anymore. A startup posture *is* a **surface**
plus a **granular posture**, set directly — you configure exactly the surface and
the fields you mean, not an opaque preset name.

### 4.0 The three boot surfaces (what "boots into" means)

The `surface` field maps 1:1 to `boot_surface`
(`src/session_config.rs::BootSurface`) — three kinds, all applied in
`app.rs::boot_load`:

| `surface` | What you see | CSS | Notes |
|---|---|---|---|
| `"chrome"` | the windowed workspace (status bar + windows) | `mode-dom` | the full explorable browser; **apps-only deployments** land here |
| `"window"` | **one named window (`window_type`), spawned + maximized** | `mode-dom` | still normal chrome underneath — un-maximize, open other windows. General: a Site Browser, Games, Apps, Entity Tree, … |
| `"site"` | the **site overlay** (a single site, full-viewport) | `mode-site` | the locked/kiosk surface — fragile, no window chrome |

The surface is **config, not an architecture fork** — the system peer always
exists regardless. At boot, `boot_load` derives the runtime `active`
(is-the-overlay-showing) purely from `surface` — only `"site"` lights the overlay
— so boot lands per config, never per a previous session's toggle.

### 4.1 The two axes

Set them independently:

- **Surface** (above) — `chrome` / `window` (+ `window_type`) / `site`.
- **Escapable ⇄ locked** — `site_mode.locked` (and `show_toggle`): an escapable
  surface keeps the chrome↔site toggle; a locked one removes every escape.
- Plus the orthogonal **capability** gate `peer_creation_enabled` (MAP §5) — may
  the user mint peers here — independent of the surface.

The three postures people usually want, spelled out as explicit fields:

| Goal | `surface` | `site_mode` | `peer_creation_enabled` |
|---|---|---|---|
| **The workspace** (apps-only, or an explorable browser) | `"chrome"` | `{ enabled: false, show_toggle: false }` (apps-only) or defaults | `true` |
| **Show my site, forgiving** (maximized Site Browser, escapable) | `"window"` + `window_type: "Site Browser"` | `{ enabled: true, show_toggle: true, locked: false }` | `true` |
| **Locked kiosk** (full-viewport overlay, no escape) | `"site"` | `{ enabled: true, show_toggle: false, locked: true }` | `false` |

- A **`"window"`** surface hydrates the window to `home_site` on open
  (`ContentSiteState::initialize`), so the user lands *on the home site* — but
  inside the normal chrome: they can un-maximize, open other windows, browse the
  directory rail. Deliberately **not** the site overlay — the overlay is
  kiosk-like and fragile; a maximized window is intuitive and forgiving for the
  everyday "show my sites" deployment. And `window_type` is general — a
  single-app kiosk (e.g. one Game maximized) is just `surface: "window"` with a
  different type.
- A **`"site"`** surface boots the **site overlay** (`BootSurface::Site`) — that
  overlay *is* the kiosk surface. Add `site_mode.locked: true` +
  `peer_creation_enabled: false` for a true locked kiosk; leave
  `show_toggle: true` for an escapable overlay (a non-kiosk full-viewport site).

> **Escape hatch (any surface, incl. locked):** appending **`?chrome=1`** to the
> URL forces the chrome surface and re-exposes the toggle — the operator escape
> out of a locked kiosk, e.g. if you lock yourself out during testing. It is
> ephemeral (never persisted). (A user-facing "enter locked mode" control in
> Settings is intentionally **not** shipped yet — it awaits a deliberate,
> confirmed flow with a documented recovery story, so a tester can't strand
> themselves.)

---

## 5. Deployment recipes

### 5.1 Apps only — no site overlay (the "just the apps" deployment)

Boot to chrome, never the site, no site entry point at all. Games/Apps windows
work normally (apps ride every publish — see [§7](#7-embedded-apps--games)).

`entity-deployment.json`:

```json
{
  "surface": "chrome",
  "site_mode": { "enabled": false, "show_toggle": false }
}
```

`enabled: false` disables the overlay entirely and `show_toggle: false`
suppresses the ⛶ toggle (`exposes_toggle()` ⇒ `false`). `home_site`/`origins` are
unnecessary here. Emit it with:

```bash
make site OUT=dist DEPLOY_CONFIG=1 SURFACE=chrome \
     --ingest-apps=../entity-apps/dist
```

…then hand-edit the emitted `site_mode` block to the above, **or** simply have
the deploy tool write `entity-deployment.json` directly (every field optional).

> **Note:** a publish currently requires **at least one site** in the tree
> (`"no sites found — nothing to publish"`). For an apps-only deployment you
> still publish a site (the demo seed is fine), but `surface: "chrome"` +
> `site_mode.enabled: false` means users never see it.

### 5.2 A content site, escapable (users can reach the workspace)

Boots into a **maximized Site Browser window** landed on `home_site` — the
everyday "show my sites" deployment. Users start on the site but keep the full
chrome (un-maximize, open other windows, browse the rail). See [§4.1](#41-the-two-axes).

```json
{
  "surface": "window",
  "window_type": "Site Browser",
  "home_site": { "peer": "<published-peer-id>", "site": "<site-id>", "loc": "" },
  "origins": { "<published-peer-id>": "" },
  "site_mode": { "enabled": true, "show_toggle": true, "locked": false }
}
```

```bash
make site OUT=dist DEPLOY_CONFIG=1 SURFACE=window WINDOW_TYPE="Site Browser" CONFIG_SITE=<site-id>
```

(This is the publish default — `SURFACE`/`WINDOW_TYPE` unset gives exactly this.)

> Want the full-viewport **site overlay** for a non-kiosk site instead of the
> windowed browser? Use `SURFACE=site` **without** `LOCKED` (overlay + an escape
> toggle), or set the durable `boot_surface` to `Site` in Settings → Startup
> surface.

### 5.3 A locked public site / kiosk

```json
{
  "surface": "site",
  "home_site": { "peer": "<published-peer-id>", "site": "<site-id>", "loc": "" },
  "origins": { "<published-peer-id>": "" },
  "site_mode": { "enabled": true, "show_toggle": false, "locked": true },
  "peer_creation_enabled": false
}
```

```bash
make site OUT=dist DEPLOY_CONFIG=1 SURFACE=site LOCKED=1 CONFIG_SITE=<site-id>
```

`--locked` (from `LOCKED=1`) emits the explicit `site_mode` lock **and**
`peer_creation_enabled: false` — the kiosk posture, spelled out (no preset).
(`?chrome=1` still lets an operator escape — see [§4](#4-surface--posture).)

### 5.4 Bare static site (no SPA, no entity chrome at all)

If you want a *plain* static website with none of the Entity Browser runtime — a
pure SSG output — use bare-root mode. One site rendered at the domain root, no
`sites/{peer}/{site}/` prefix, no branding, no WASM:

```bash
make site-bare SITE=<site-id> OUT_BARE=dist-bare
```

This is the "Entity Browser is also just a site generator" output. No
`entity-deployment.json`, no peer-id in the path, no apps.

**The `.html` path never touches the content store.** The static export writes
self-contained `.html` (+ `assets/` files) with links rewritten to plain URLs —
there is **no `content/{hash}` blob store, no two-hop, no duplication**. The
content-addressed store (`content/{aa}/{bb}/{hex}` + the `{peer}/…​.bin`
pointers) is emitted **only** by the entity-native `.bin` form, which exists so a
live WASM peer can ingest + hash-verify the site. Consequences worth knowing:

- **The default `make site` writes the site TWICE** — `.html` *and* `.bin`
  (the two representations serve different consumers: dumb CDN vs. live peer).
  That double-write is inherent to the default, not a bug. For a pure static
  site, `HTML_ONLY=1` (or bare-root, always HTML-only) skips the `.bin` entirely
  — half the output, zero content store.
- **The content store dedups identical bytes across sites** within the `.bin`
  form, so the `.bin` itself isn't wasteful — but a *republish* re-emits it
  (the in-place clean wipes `content/` first), which is the re-upload cost the
  blue-green **append-only content store** design targets (see
  `docs/architecture/reviews/DESIGN-REPUBLISH-BLUE-GREEN-AND-CONTENT-STORE-SPLIT.md`),
  **not** anything the static `.html` path does.

---

## 6. The `make site` command surface

`make site` is headless/native (no browser): it builds a peer, seeds or
ingests its sites + apps, reads them back off the tree, and projects them to
`OUT`. Knobs:

| Variable | Default | Effect |
|---|---|---|
| `OUT=<dir>` | `dist/static-demo` | Output directory. **Must stay under the repo tree** (publish runs in a container with only the repo bind-mounted; an absolute `/tmp/x` writes into the container's throwaway fs and never reaches the host — enforced by `CHECK_IN_TREE`). |
| `INGEST=<dir>` | — | Source sites from a content-team `render/` emit (disk→tree) instead of the bundled demo seed. One site dir, or a parent of site dirs. **May point anywhere** — it is staged into a repo-local dir for the container automatically; you do not copy anything by hand. |
| `APPS_DIST=<dir>` *(flag `--ingest-apps`)* | bundled demo seed | Source the embedded apps from an entity-apps `dist/` (split into games/apps by entry type). Staged like `INGEST`, so it may point anywhere. |
| `PLAN=1` | off | Resolve the sources and report what the publish **would** add/keep/**remove**, writing nothing. Exit `0` = nothing removed · `2` = removes something · `1` = error, so `make site PLAN=1 … && make site …` is a gate. **Covers sites *and* app sets**: a publish replaces everything under the peer prefix, so omitting `APPS_DIST` deletes `{peer}/apps/**`. |
| `PREFIX=<path>` | empty (root) | Per-peer **hosting scope** — nest all content (`.html`, `.bin`, origin) under `{OUT}/{PREFIX}/…` so one domain can host many isolated peers. Empty = domain root, byte-identical to un-prefixed. Validated (no leading/trailing `/`, no `..`, not `sites`/`content`). |
| `LIVE=<origin>` | empty (same-origin) | The "open in live entity browser" banner target + the deployment-config origin. **Empty = same-origin** (relative — the same `dist/` works at localhost and on any CDN root, no rebuild). Set a concrete `https://host` only for a deliberate cross-origin pin. **Never `LIVE=http://localhost` for a shipped bundle** (guarded — it bakes a loopback that serves a content-less shell off your machine). |
| `HTML_ONLY=1` | both forms | Skip the entity-native `.bin` content data (dumb-CDN-only — no live overlay, just the static `.html`). |
| `DEPLOY_CONFIG=1` | off | Also emit `/entity-deployment.json` so a generic SPA on this origin boots into the published home. |
| `SURFACE=<chrome\|site\|window>` | **`window`** | The startup surface written into the emitted config. A typo **fails the build**. |
| `WINDOW_TYPE=<name>` | **`Site Browser`** | For `SURFACE=window`, which window type to boot maximized (must be a registered type). Ignored otherwise. |
| `LOCKED=1` | off | For `SURFACE=site`, emit the kiosk lock (`site_mode` no-toggle-locked + `peer_creation_enabled: false`). Rejected for other surfaces. |
| `CONFIG_SITE=<id>` | demo site | The home site written into the emitted config. Must be among the published sites or the build **fails**. |
| `IDENTITY_SEED=<64-hex>` | demo publisher seed | The **system identity** to publish under (the same hex seed form as the runtime `entity_system_seed`) → its own stable peer-id under `sites/{peer}/…`. Generate with e.g. `openssl rand -hex 32`; reuse per deployment. A malformed seed **fails the build**. See [§2.1](#21-the-published-peer-id--you-choose-the-identity-stable-per-seed). |
| `REGISTRY_PIN=<PEER_ID[@ORIGIN]>` *(flag `--registry-pin=`)* | empty (no pin) | Seed the §7.4 **preloaded name registry** into the emitted config (`name_registry_pin`), so a visitor resolves names through that registry without pinning one by hand. Same `PEER_ID@ORIGIN` spelling as `registry --bind`; a bare peer-id means same-origin. **Requires `DEPLOY_CONFIG=1`** — the pin rides in that file, and the publish *refuses* the combination without it rather than emitting a pin nothing carries. The peer-id must be **canonical form**: it is the verification key, so a legacy-form id is refused at the emitter (there is nowhere in a deployment config to put an out-of-band key). Honoured by `site`, `site-dist`, `site-serve` and `tauri-bundle`; `site-bare` emits no config, so it takes no pin. |

> **Why this is a flag and not a hand-edit.** `name_registry_pin` is two strings
> in a JSON file, so injecting it after the publish looks equivalent — and it is
> not. The emitter validates the pin (`parse_registry_pin`) and refuses a
> peer-id a consumer could never use, *on the operator's machine, where the
> refusal can be read*. A hand-edited pin skips exactly that check and fails
> instead at a visitor's browser, where it is indistinguishable from a registry
> that is simply down. Audit F9's rule, on a new field.

> ⚠️ **Default surface gotcha:** with `DEPLOY_CONFIG=1` and **no** `SURFACE`, the
> emitted surface defaults to **`window`** + `WINDOW_TYPE="Site Browser"` (boots
> into a maximized, escapable Site Browser). Pass `SURFACE` explicitly when you
> want a different posture (e.g. `SURFACE=chrome` apps-only, or
> `SURFACE=site LOCKED=1` for a kiosk).

Higher-level convenience targets:

- **`make site-dist` — the artifact you upload.** ⚠️ **`make site` alone is the
  content half only.** Its root `index.html` is a redirect to `/sites/` and
  there is no wasm, so uploading that to a bucket root **replaces the live SPA
  with a redirect page** and orphans every app bundle (they are `.bin` files
  under `{peer}/apps/**` reachable only through the SPA — nothing in the static
  HTML projection links to them). `site-dist` emits the production shape into
  `dist-site/`: SPA at the apex, content under `sites/` + `content/`, apps, and
  `/entity-deployment.json`. It is `wasm-release` **then** publish into the same
  dir — that order is load-bearing (trunk wipes its dist dir; the publish cleans
  only `sites/`, `content/`, `{peer}/`) — and it runs `--verify` at the end.
  `DEPLOY_CONFIG` defaults to **1** here, because without the config the SPA
  boots to its own seed and every published site appears missing. Takes the same
  knobs as `make site`; override the output with `SITE_DIST_OUT=<dir>`.
- **`make site-serve`** — rebuild the SPA, publish sites (the bundled demo, or
  `INGEST=<dir>`) + optional apps (`APPS_DIST=<dir>`) into an isolated `/tmp` copy,
  and serve on one origin (`:8081`). The one-command end-to-end round-trip on your
  machine. Serves the SPA at `/` and the static sites at `/sites/`. Add
  `DEPLOY_CONFIG=1 CONFIG_SITE=<id>` to boot the SPA into a published site as a
  cache-backed foreign-site overlay (the real remote-peer path).
- **`make site-bare`** — bare static site ([§5.4](#54-bare-static-site-no-spa-no-entity-chrome-at-all)).

---

## 7. Embedded apps & games

The JS-apps platform (games + tools) reads its catalog + bundles **off the
published tree exactly like sites**, so **every full publish carries every app
set** — no flag required for them to ship. They live under
`{peer}/apps/{set}/…` (`games` / `apps`, split by the entry `type` in
entity-apps' `index.json`).

- Provide real apps with `APPS_DIST=<entity-apps/dist>` (a **pre-built** dist dir;
  the pipeline consumes it via `--ingest-apps` — it never builds it).
- Without a real apps dir, a minimal demo seed is published.
- The live Games/Apps window fetches a bundle on click-through, like a site
  asset, over the same origin as the sites.

So an **apps-only deployment** ([§5.1](#51-apps-only--no-site-overlay-the-just-the-apps-deployment))
is: publish with `--ingest-apps`, set `surface: "chrome"` + `site_mode` off. Users
open the Apps / Games windows from the menu.

---

## 8. The `dist/` layout (what DevOps ships)

After `make site OUT=dist DEPLOY_CONFIG=1 …` (empty `PREFIX`, the standard
single-tenant root deploy):

```
dist/
├── index.html                      # the SPA (untouched by publish)
├── *.wasm  *.js  sw.js             # the SPA bundle
├── entity-deployment.json          # ← per-domain config, ALWAYS at the root
├── sites/{peer}/{site}/…/index.html   # [B1] legacy-web .html projection (no-JS)
├── content/{xx}/{yy}/{hash}        # [B2] content-addressed .bin blobs
└── {peer}/                         # [B2] entity-native pointers (what a live peer ingests)
    ├── sites/{site}/…
    └── apps/{set}/…
```

- With a non-empty `PREFIX`, the content roots (`sites/`, `content/`,
  `{peer}/`) nest under `{PREFIX}/…`, **but `entity-deployment.json` stays at the
  served root** (it's fetched from `/entity-deployment.json`). The config's
  `origins[peer]` then points at `/{PREFIX}` so the resolver finds the content.
- **Publishing into a populated `dist/` is safe** — publish cleans only the
  roots it owns (under the prefix) and never touches `index.html` or the bundle.
- DevOps deploy = `aws s3 sync --delete dist/ → bucket` (or R2 equivalent). With
  the default same-origin (`LIVE` empty), the same `dist/` works at any domain
  root with **no rebuild**.
- **Re-publishing an existing deployment** (edit / add / delete a page, or the
  incremental-update mechanics + the CDN-sync strategy) has its own guide:
  [`GUIDE-REPUBLISH-AND-INCREMENTAL.md`](./GUIDE-REPUBLISH-AND-INCREMENTAL.md).
  Short version: content-addressed + path-stable ⇒ `aws s3 sync --delete` is
  correct and near-incremental; **keep the durable publisher keypair** so the
  peer-id (the site address) never churns.

> **Subdirectory caveat:** portability is guaranteed at the domain **root**
> only. A subdirectory deploy (`host/sub/`) is known, deliberate debt — see
> [`ANALYSIS-PUBLISH-PORTABILITY-AND-ORIGIN-MODEL`](#10-where-to-go-deeper).

### 8.1 Bundling content into the Tauri desktop app

**The same static publish works for the desktop app — no separate server.** Tauri
does **not** run an HTTP server for content; `tauri::generate_context!()`
(`src-tauri/src/lib.rs`) **embeds `../dist` into the app binary** at compile time,
and the WebView serves it as the app's **same-origin** (a `tauri://localhost`
custom protocol). Root-relative fetches — `/{peer}/sites/{site}/manifest.bin`,
`/content/{xx}/{yy}/{hash}`, `/entity-deployment.json` — resolve against that
origin and are served straight from the embedded bundle. So a same-origin publish
(empty `LIVE`) into `dist/` is delivered by Tauri exactly as a CDN would deliver
it, just embedded and offline.

The three ways content reaches the desktop app:

1. **Author in-app** — create/edit a site in the Site Creator; it persists to
   IndexedDB and renders from the local tree with **no fetch at all**. (This is
   what a `make tauri-run` user sees for the seeded demo, and for anything they
   make; IDB durability across restart is verified on WebKitGTK — 2026-07-02.)
2. **Bake published content into the bundle** — use **`make tauri-bundle`**, which
   sequences the publish into the build:
   ```bash
   # sites + apps baked in, boots into <site-id>; INGEST/APPS_DIST may be ANY path.
   make tauri-bundle-run CONFIG_SITE=<site-id> INGEST=<sites-dir> APPS_DIST=<entity-apps/dist>
   ```
   The app boots the SPA, reads the embedded `entity-deployment.json`, and
   resolves `/{peer}/sites/…` + `/{peer}/apps/…` **same-origin** from the bundle.
   Verified end-to-end on WebKitGTK/Tauri (2026-07-02): sites render and apps
   launch, all served offline from the embedded bundle.
   - **Do NOT use plain `make tauri` for this** — its `wasm-release` step re-runs
     trunk, which **wipes `dist/`** and drops the published content. `tauri-bundle`
     exists precisely to order it right: wasm-release → publish INTO dist/ → embed.
   - `INGEST` / `APPS_DIST` can point **anywhere**; the recipe stages them into the
     repo first (the publish container only mounts this repo) — no hand-copying.
     Omit `INGEST` and the built-in demo set is baked; omit `APPS_DIST` and no apps
     ship.
3. **Point at a remote origin** — publish to a CDN with a concrete `LIVE=https://…`
   (or set `origins[peer]` in the config), and the desktop app HTTP-polls the
   remote (the CSP allows `connect-src … http: https:`). Content lives on the
   server, not in the binary — an updatable-without-reinstall deployment.

> **Two load-bearing desktop gotchas (learned the hard way, 2026-07-02):**
>
> - **A returning app keeps its durable config, which WINS over the baked
>   `entity-deployment.json`** (`persisted > fetched > build-time`, `app.rs`
>   `boot_load`). So a fresh install boots into the baked content, but if you've
>   run the app before (or created any site), it ignores a newly-baked bundle and
>   keeps the old posture. To re-test a bundle, clear the profile first:
>   `rm -rf ~/.local/share/systems.entity.browser` (the app identifier).
> - **Apps are sandboxed `srcdoc` iframes of inline-script HTML**, and a srcdoc
>   frame inherits the embedder's CSP — so the Tauri CSP (`src-tauri/tauri.conf.json`)
>   must allow the frames' inline scripts or every app is a **blank white iframe**.
>   The CSP now carries `script-src … 'unsafe-inline' 'unsafe-eval'` **and**
>   `"script-src"` in `dangerousDisableAssetCspModification` (without the latter,
>   Tauri injects a nonce that nullifies `'unsafe-inline'` — the same trap as the
>   `style-src` grayscale bug). The sandbox (opaque origin, `allow-scripts` only)
>   remains the real isolation boundary. The browser deployment has no CSP, so
>   apps always ran there — this was Tauri-only.

---

## 9. Gotchas & escape hatches (read before shipping)

- **Your own previous session wins.** A returning visitor's durable config
  (precedence level 2) beats the deployment config. To test a config change,
  clear site storage / use a fresh profile, or open the System Recovery console
  (`?systemrecovery=1`) to inspect/clear state.
- **`DEPLOY_CONFIG=1` defaults to `SURFACE=window` (Site Browser)** if `SURFACE`
  is unset — set it explicitly for a different posture ([§6](#6-the-make-publish-command-surface)).
- **`?chrome=1`** escapes any locked deployment (operator break-glass).
- **`?site=<peer>/<site>/<page>`** deep-links the overlay to a specific page
  (ephemeral, never persisted) — useful for showcase links.
- **Never bake a loopback origin** (`LIVE=http://localhost…`) into a shipped
  bundle — publish warns loudly, but it serves a content-less shell.
- **A publish needs at least one site** — apps-only deployments still publish a
  (possibly demo) site, hidden via `surface: "chrome"` + `site_mode` off.
- **A bad `SURFACE` or a `CONFIG_SITE` not among published sites fails the
  build** — by design, so a broken config never ships.
- **Deleting a page needs a manifest edit too.** Removing a `pages/*.md` file
  and republishing prunes the page cleanly, but the site's **nav** is authored
  separately in `site.manifest.json` — leave the nav entry and you ship
  **dangling 404 links** (publish does *not* warn). Remove the `nav` entry (+ any
  inline links) as well. Full workflow:
  [`GUIDE-REPUBLISH-AND-INCREMENTAL.md` §5](./GUIDE-REPUBLISH-AND-INCREMENTAL.md#5-case-d--delete-a-page-and-the-dangling-link-gotcha).
- **Config failures are silent at boot** — a missing/garbled
  `entity-deployment.json` falls through to defaults; it never wedges boot
  (D16). The flip side: a typo'd field is simply ignored, so verify the served
  file with the recipes above.

---

## 10. Where to go deeper

Authoritative deeper docs (these win where they overlap with this guide):

- **Publishing pipeline (source→tree→`dist/`→CDN→live SPA), live-proven with
  images:** `docs/architecture/specs/REFERENCE-PUBLISHING-PIPELINE.md`
- **Boot-closure + deployment-config design (precedence, the cut-2b mechanism):**
  `docs/plans/DESIGN-BOOT-CLOSURE-AND-DEPLOYMENT-CONFIG.md` (and the
  arc handoff `docs/plans/HANDOFF-WEB-PROJECTION-BUILD-ARC.md`)
- **Hosting model — single-tenant root vs multi-tenant `--prefix`, `origins`
  roster, remote `.list` discovery:** see the hosting-model §7/§8 design
  (`project_hosting_model_and_prefix` memory + the HOSTING-MODEL doc)
- **Publish portability + origin model (same-origin default, subdir debt, what
  `origins` is and isn't):**
  `docs/architecture/reviews/ANALYSIS-PUBLISH-PORTABILITY-AND-ORIGIN-MODEL.md`
- **Ingest format (disk→tree, the `--ingest` content contract):**
  `docs/architecture/guides/PUBLISH-INGEST-FORMAT.md`
- **Content Site app (the two surfaces, data model, link resolver, caching):**
  `docs/architecture/specs/REFERENCE-CONTENT-SITE-APP.md`
- **Entity JS-Apps platform (the games/tools window, app contract, sizing):**
  `docs/architecture/specs/REFERENCE-ENTITY-JS-APPS-PLATFORM.md`
- **Peer lifecycle, identity, the three peer sets, startup/delete:**
  `docs/architecture/reviews/MODEL-PEER-LIFECYCLE-AND-STARTUP.md`
- **Persistent system peer + durability substrate (IDB default, roster):**
  `docs/plans/DESIGN-PERSISTENT-SYSTEM-PEER-AND-DURABILITY-SUBSTRATE.md`

Code entry points:

- Deployment config parse/apply/precedence — `src/deployment_config.rs`
- Profiles, posture presets, `BootSurface` — `src/session_config.rs`
- Publish command (CLI, `--deployment-config`, identity seed, `dist/` layout) —
  `src/content_site/publish.rs`
- Runtime system-peer seed / per-browser identity — `src/persistence.rs`
  (`system_seed`)
- Boot application of the config — `src/app.rs` (`boot_load`) and
  `src/boot_fast_paint.rs`

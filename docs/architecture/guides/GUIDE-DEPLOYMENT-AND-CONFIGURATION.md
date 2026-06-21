# Deployment & Configuration Guide

**Audience:** anyone deploying Entity Browser to a domain (DevOps, the deploy
tool, content/site authors). **Scope:** how a published deployment is shaped —
peer identity, the per-domain config file, the posture profiles, the publish
command surface, and the concrete recipes (deploy a site / lock a kiosk /
apps-only with no site overlay).

This is the **quick, self-contained reference**. The deeper authoritative docs
it draws from are linked at the bottom ([§10](#10-where-to-go-deeper)); where
this guide and those overlap, those win.

> **TL;DR**
> - **One generic WASM bundle** is shaped per-domain by a small JSON file,
>   `/entity-deployment.json`, fetched at boot. **No per-domain rebuild.**
> - The **published peer-id is deterministic** — stable across re-publishes.
>   You don't manage or rotate it.
> - The **`profile`** field sets the cold-boot posture: `full` (chrome-first,
>   **no site overlay**), `tutorial` (site overlay, escapable), `strict-site`
>   (locked kiosk).
> - **To deploy apps with no forced site overlay:** `profile: "full"` plus
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
2. **The published content + config** written alongside it by `make publish`
   (`sites/…`, `content/…`, `{peer}/…`, and optionally `entity-deployment.json`).

DevOps drops the whole `dist/` directory on a CDN / R2 bucket at the domain
root. That's the deploy.

---

## 2. Persistent peer-ids (PIDs)

There are **two distinct peer-ids**. Don't conflate them.

### 2.1 The published peer-id — *you choose the identity; stable per seed*

When you `make publish`, all content is keyed under a single peer-id:

```
sites/{peer_id}/{site}/…          ← legacy-web .html projection
{peer_id}/sites/{site}/…          ← entity-native .bin content data
{peer_id}/apps/{set}/…            ← embedded apps (games/tools)
```

That `peer_id` is derived from a **32-byte system seed** — the same hex form as
the runtime `entity_system_seed`. **You supply it per publish** with
`--identity-seed=<64-hex>` (Makefile: `IDENTITY_SEED=<hex>`), so each
site/deployment publishes under **its own** identity. With no `--identity-seed`,
publish falls back to the fixed demo publisher seed (the first-push default):

```rust
// src/content_site/publish.rs
const DEMO_PUBLISH_SEED: [u8; 32] = *b"entity-demo-publisher-seed-v1\0\0\0";
fn publish_identity_keypair(seed: [u8; 32]) -> Keypair { Keypair::from_seed(seed) }
```

**For a given seed the peer-id is deterministic** — re-running `make publish`
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

> **Forward note (seam):** the publisher still seeds the bundled demo site set
> when you don't `--ingest` real content. When the durable native-peer-load path
> lands, the seam can additionally accept a persisted peer *directory* (load its
> own durable keypair) instead of a raw seed — same stable-peer-id guarantee,
> just a different way to point at the identity. Nothing downstream changes.

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
  "profile": "full",
  "home_site": { "peer": "<published-peer-id>", "site": "<site-id>", "loc": "" },
  "origins": { "<published-peer-id>": "" },
  "site_mode": { "enabled": true, "show_toggle": true, "locked": false },
  "fast_paint": true,
  "peer_creation_enabled": true
}
```

| Field | Type | Meaning |
|---|---|---|
| `profile` | `"full" \| "tutorial" \| "strict-site"` | Cold-boot **posture preset** (see [§4](#4-profiles--posture)). Swaps in the whole preset; other fields below then *merge on top*. |
| `home_site` | `{ peer, site, loc }` | The startup site — where a `Site` boot lands and the home toggle opens. `peer` = the published peer-id; `site` = a site id that exists in the publish; `loc` = optional page/path within the site (`""` = the site's index). |
| `origins` | `{ peerId: originString }` | Where each hosting peer's published artifacts live, so the resolver can HTTP-poll them. **`""` = same origin** (the SPA expands it to `window.location.origin` at runtime — the common CDN case). A root-relative `"/sub"` = same origin under a prefix. A concrete `"https://host"` = cross-origin. |
| `site_mode.enabled` | `bool` | Whether site mode exists at all. `false` ⇒ no overlay, no toggle, ever. |
| `site_mode.show_toggle` | `bool` | Whether the ⛶ site toggle appears in the status bar. The toggle shows iff `show_toggle && enabled`. |
| `site_mode.locked` | `bool` | Kiosk lock — `true` removes every chrome↔site escape. |
| `fast_paint` | `bool` | Phase-1 fast-paint kill switch (paints the site shell before peers boot, for site-first deployments). Leave default unless debugging a paint flash. |
| `peer_creation_enabled` | `bool` | Capability gate — set `false` to forbid minting new peers **independent of profile** (so a `full` chrome deployment can still disable creation without becoming a locked site). |

### 3.2 Precedence (highest wins)

```
1. URL overrides            ?site=…  ?boot_window=…  ?chrome=1     (dev/showcase, never persisted)
2. Durable persisted config a returning user's own saved settings  ← always wins on a warm boot
3. THIS fetched config      /entity-deployment.json                ← shapes a COLD boot
4. Build-time env defaults  ENTITY_PROFILE / ENTITY_HOME_*         (baked fallback)
5. Hard default             Full (chrome-first, local demo)
```

The deployment config shapes a **cold** boot only — the SPA fetches it just
when no durable config exists yet. A returning user's persisted choices always
win. (This is why testing a config change may require clearing storage or a
fresh profile — your own previous session is winning at level 2.)

---

## 4. Profiles → posture

`profile` selects one of three presets (`src/session_config.rs::Profile::preset`):

| `profile` | boots into | Site Overlay on boot? | ⛶ site toggle | peer creation | typical use |
|---|---|---|---|---|---|
| **`full`** | chrome (empty-state) | **NO** — chrome-first | shown* | yes | the workspace; **apps-only deployments** |
| **`tutorial`** | the site overlay | **YES**, fully **escapable** (toggle back to chrome) | shown | yes | a content site you want users in by default but free to explore the chrome |
| **`strict-site`** | the site overlay | **YES**, **locked kiosk** | hidden | no | a locked public content site / kiosk |

\* the toggle only appears when `site_mode.exposes_toggle()` = `show_toggle && enabled`.

`full` lands on the chrome (now the first-run empty-state tutorial, no auto-opened
window). `tutorial` and `strict-site` boot straight into the site overlay; the
difference is the escape: `tutorial` keeps the toggle and is unlocked,
`strict-site` hides the toggle and sets `locked: true`.

> **Escape hatch (all profiles, incl. locked):** appending **`?chrome=1`** to the
> URL forces the chrome surface and re-exposes the toggle — the operator escape
> out of a locked kiosk, e.g. if you lock yourself out during testing. It is
> ephemeral (never persisted).

---

## 5. Deployment recipes

### 5.1 Apps only — no site overlay (the "just the apps" deployment)

Boot to chrome, never the site, no site entry point at all. Games/Apps windows
work normally (apps ride every publish — see [§7](#7-embedded-apps--games)).

`entity-deployment.json`:

```json
{
  "profile": "full",
  "site_mode": { "enabled": false, "show_toggle": false }
}
```

`enabled: false` disables the overlay entirely and `show_toggle: false`
suppresses the ⛶ toggle (`exposes_toggle()` ⇒ `false`). `home_site`/`origins` are
unnecessary here. Emit it with:

```bash
make publish OUT=dist DEPLOY_CONFIG=1 CONFIG_PROFILE=full \
     --ingest-apps=../entity-apps/dist
```

…then hand-edit the emitted `site_mode` block to the above, **or** simply have
the deploy tool write `entity-deployment.json` directly (every field optional).

> **Note:** a publish currently requires **at least one site** in the tree
> (`"no sites found — nothing to publish"`). For an apps-only deployment you
> still publish a site (the demo seed is fine), but `profile: full` +
> `site_mode.enabled: false` means users never see it.

### 5.2 A content site, escapable (users can reach the workspace)

```json
{
  "profile": "tutorial",
  "home_site": { "peer": "<published-peer-id>", "site": "<site-id>", "loc": "" },
  "origins": { "<published-peer-id>": "" },
  "site_mode": { "enabled": true, "show_toggle": true, "locked": false }
}
```

```bash
make publish OUT=dist DEPLOY_CONFIG=1 CONFIG_PROFILE=tutorial CONFIG_SITE=<site-id>
```

### 5.3 A locked public site / kiosk

```json
{
  "profile": "strict-site",
  "home_site": { "peer": "<published-peer-id>", "site": "<site-id>", "loc": "" },
  "origins": { "<published-peer-id>": "" },
  "site_mode": { "enabled": true, "show_toggle": false, "locked": true }
}
```

```bash
make publish OUT=dist DEPLOY_CONFIG=1 CONFIG_PROFILE=strict-site CONFIG_SITE=<site-id>
```

(`?chrome=1` still lets an operator escape — see [§4](#4-profiles--posture).)

### 5.4 Bare static site (no SPA, no entity chrome at all)

If you want a *plain* static website with none of the Entity Browser runtime — a
pure SSG output — use bare-root mode. One site rendered at the domain root, no
`sites/{peer}/{site}/` prefix, no branding, no WASM:

```bash
make publish-bare SITE=<site-id> OUT_BARE=dist-bare
```

This is the "Entity Browser is also just a site generator" output. No
`entity-deployment.json`, no peer-id in the path, no apps.

---

## 6. The `make publish` command surface

`make publish` is headless/native (no browser): it builds a peer, seeds or
ingests its sites + apps, reads them back off the tree, and projects them to
`OUT`. Knobs:

| Variable | Default | Effect |
|---|---|---|
| `OUT=<dir>` | `dist/static-demo` | Output directory. **Must stay under the repo tree** (publish runs in a container with only the repo bind-mounted; an absolute `/tmp/x` writes into the container's throwaway fs and never reaches the host). |
| `INGEST=<dir>` | — | Source sites from a content-team `render/` emit (disk→tree) instead of the bundled demo seed. One site dir, or a parent of site dirs. |
| `INGEST_APPS=<dir>` *(flag `--ingest-apps`)* | bundled demo seed | Source the embedded apps from an entity-apps `dist/` (split into games/apps by entry type). |
| `PREFIX=<path>` | empty (root) | Per-peer **hosting scope** — nest all content (`.html`, `.bin`, origin) under `{OUT}/{PREFIX}/…` so one domain can host many isolated peers. Empty = domain root, byte-identical to un-prefixed. Validated (no leading/trailing `/`, no `..`, not `sites`/`content`). |
| `LIVE=<origin>` | empty (same-origin) | The "open in live entity browser" banner target + the deployment-config origin. **Empty = same-origin** (relative — the same `dist/` works at localhost and on any CDN root, no rebuild). Set a concrete `https://host` only for a deliberate cross-origin pin. **Never `LIVE=http://localhost` for a shipped bundle** (guarded — it bakes a loopback that serves a content-less shell off your machine). |
| `HTML_ONLY=1` | both forms | Skip the entity-native `.bin` content data (dumb-CDN-only — no live overlay, just the static `.html`). |
| `DEPLOY_CONFIG=1` | off | Also emit `/entity-deployment.json` so a generic SPA on this origin boots into the published home. |
| `CONFIG_PROFILE=<full\|tutorial\|strict-site>` | **`tutorial`** | The profile written into the emitted config. A typo **fails the build**. |
| `CONFIG_SITE=<id>` | demo site | The home site written into the emitted config. Must be among the published sites or the build **fails**. |
| `IDENTITY_SEED=<64-hex>` | demo publisher seed | The **system identity** to publish under (the same hex seed form as the runtime `entity_system_seed`) → its own stable peer-id under `sites/{peer}/…`. Generate with e.g. `openssl rand -hex 32`; reuse per deployment. A malformed seed **fails the build**. See [§2.1](#21-the-published-peer-id--you-choose-the-identity-stable-per-seed). |

> ⚠️ **Default profile gotcha:** with `DEPLOY_CONFIG=1` and **no**
> `CONFIG_PROFILE`, the emitted profile defaults to **`tutorial`** (boots into
> the site, escapable). Always pass `CONFIG_PROFILE` explicitly so the posture is
> intentional. *(The Makefile comment that says "default strict-site" is stale —
> the CLI default in `src/content_site/publish.rs` is `tutorial`.)*

Higher-level convenience targets:

- **`make publish-serve`** — rebuild the SPA, publish ALL sites + apps into an
  isolated `/tmp` copy, and serve on one origin (`:8081`). The one-command
  end-to-end round-trip on your machine. Serves the SPA at `/` and the static
  sites at `/sites/`.
- **`make publish-papers`** — the cross-team loop: render the content team's
  `render/` engine → ingest disk→tree → publish both forms + emit
  `entity-deployment.json` → serve. The SPA boots into the ingested site as a
  cache-backed foreign-site overlay (exercises the real remote-peer path).
- **`make publish-bare`** — bare static site ([§5.4](#54-bare-static-site-no-spa-no-entity-chrome-at-all)).

---

## 7. Embedded apps & games

The JS-apps platform (games + tools) reads its catalog + bundles **off the
published tree exactly like sites**, so **every full publish carries every app
set** — no flag required for them to ship. They live under
`{peer}/apps/{set}/…` (`games` / `apps`, split by the entry `type` in
entity-apps' `index.json`).

- Provide real apps with `--ingest-apps=<entity-apps/dist>` (or `APPS_REPO=<path>`
  for the convenience targets, which run entity-apps' `build.py` first).
- Without a real apps dir, a minimal demo seed is published.
- The live Games/Apps window fetches a bundle on click-through, like a site
  asset, over the same origin as the sites.

So an **apps-only deployment** ([§5.1](#51-apps-only--no-site-overlay-the-just-the-apps-deployment))
is: publish with `--ingest-apps`, set `profile: full` + `site_mode` off. Users
open the Apps / Games windows from the menu.

---

## 8. The `dist/` layout (what DevOps ships)

After `make publish OUT=dist DEPLOY_CONFIG=1 …` (empty `PREFIX`, the standard
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
- DevOps deploy = `aws s3 sync dist/ → bucket` (or R2 equivalent). With the
  default same-origin (`LIVE` empty), the same `dist/` works at any domain root
  with **no rebuild**.

> **Subdirectory caveat:** portability is guaranteed at the domain **root**
> only. A subdirectory deploy (`host/sub/`) is known, deliberate debt — see
> [`ANALYSIS-PUBLISH-PORTABILITY-AND-ORIGIN-MODEL`](#10-where-to-go-deeper).

---

## 9. Gotchas & escape hatches (read before shipping)

- **Your own previous session wins.** A returning visitor's durable config
  (precedence level 2) beats the deployment config. To test a config change,
  clear site storage / use a fresh profile, or open the System Recovery console
  (`?systemrecovery=1`) to inspect/clear state.
- **`DEPLOY_CONFIG=1` defaults to `tutorial`** if `CONFIG_PROFILE` is unset —
  always set it explicitly ([§6](#6-the-make-publish-command-surface)).
- **`?chrome=1`** escapes any locked deployment (operator break-glass).
- **`?site=<peer>/<site>/<page>`** deep-links the overlay to a specific page
  (ephemeral, never persisted) — useful for showcase links.
- **Never bake a loopback origin** (`LIVE=http://localhost…`) into a shipped
  bundle — publish warns loudly, but it serves a content-less shell.
- **A publish needs at least one site** — apps-only deployments still publish a
  (possibly demo) site, hidden via `profile: full` + `site_mode` off.
- **A bad `CONFIG_PROFILE` or a `CONFIG_SITE` not among published sites fails
  the build** — by design, so a broken config never ships.
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

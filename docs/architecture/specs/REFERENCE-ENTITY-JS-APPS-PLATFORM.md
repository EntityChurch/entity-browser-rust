# Reference — Entity JS-Apps Platform (embedded HTML apps)

**What this is.** A platform for running standalone HTML/JavaScript apps inside
the entity system. The first app set is **games** (from the `entity-apps` repo),
but the machinery is app-set-agnostic — this is how *any* self-contained JS/HTML
app is ingested, stored, distributed, and run on top of the entity tree. The
"Games" window is the worked example that proves the platform.

This report explains the whole thing end-to-end: the contract, the data model,
the runtime, the sizing/display rules, the publish/distribution path, the code
surface, and what's done vs. remaining.

The app contract this platform consumes is authored in the `entity-apps` repo
(`entity-apps/docs/EMBEDDING.md`).

---

## 1. The big picture

```
  entity-apps repo                 OUR app (entity-browser)                  live web
  ────────────────                 ──────────────────────────                ────────
  source + build.py                ingest → tree → sandboxed iframe          publish → CDN
       │                                  │                                       │
   dist/<id>.html  ──ingest──▶  /{peer}/apps/games/bundles/<id>  ──render──▶  iframe(game)
   dist/index.json ──ingest──▶  /{peer}/apps/games/catalog        ──grid──▶   launcher
                                        │                                       │
                                   save-state ◀── postMessage ──▶ game     publish .bin ──▶ fetch-on-click
```

A JS developer writes a **self-contained HTML app** (one `.html` file, no network,
built for `iframe sandbox="allow-scripts"`). We **ingest** it into a peer's entity
tree as content-addressed data, **render** it by dropping the bundle into a
sandboxed iframe and speaking a tiny postMessage protocol, and **persist its
save-state** back into the tree. The same content **publishes** to a static `dist/`
and is **fetched on click-through** when the app is served live — exactly the
content-site pipeline, a different data type.

**Why this is the right shape:** the bundle is opaque, isolated, and host-agnostic;
the entity system provides identity, content-addressing, durable storage, and
distribution. The app developer owns the app; the platform owns identity +
storage + delivery. Storage is the host's job — which is *our* job, and it lands
naturally on the entity-backed-state pattern.

---

## 2. The app contract (what an entity JS app is)

Authored in `entity-apps`; the contract we consume is `entity-apps/docs/EMBEDDING.md`.

- **One self-contained `.html` bundle** (`dist/<id>.html`). Zero external refs
  (no CDN, no runtime network). Built for `sandbox="allow-scripts"` — removing the
  iframe leaves nothing behind. 37 KB–184 KB each.
- **A catalog** `dist/index.json`: `[{ id, name, description, saves }]`.
- **The postMessage protocol** (the only channel; the sandbox blocks everything
  else). Messages carry `source`: `entity-app` (app→host) / `entity-host` (host→app).

  | dir | type | meaning |
  |---|---|---|
  | app→host | `ready-for-init` | mounted, wants saved state → host replies `init` |
  | app→host | `state` `{state}` | "persist this" (opaque, keyed by app id) |
  | app→host | `ready` / `closed` / `error` | running / torn down / `create()` threw |
  | host→app | `init` `{state}` | the saved object, or `null` for a fresh start |
  | host→app | `viewport` `{width,height,safe}` | iframe size + safe-area insets |
  | host→app | `request-state` / `destroy` | flush latest / tear down |

  **150 ms fallback:** if the host never answers `ready-for-init`, the app starts
  fresh — so even a dumb host yields a working (stateless) app.
- **The app owns its own layout.** The SDK canvas (`sdk/canvas.js`,
  `.entity-canvas { flex:1; min-height:0 }`) fills whatever box the host gives it
  via a `ResizeObserver`; board games (`sdk/board.js`, `measureFit`) draw a
  **square** sized to `min(width,height)`, centered. **So the host owns the box;
  the game fits itself into it.** This is the key fact behind the sizing rules (§5).

---

## 3. The data model (entities + tree layout)

Code: `src/apps/format.rs`, `src/apps/paths.rs`.

Three CBOR entity types (thin wrappers, lossy `from_entity`, deterministic
`to_entity` so identical bytes content-address + dedup):

- **`app/app-catalog`** (`AppCatalog`) — `entries: [{id,name,description,saves}]`.
- **`app/app-bundle`** (`AppBundle`) — `html: String` (the self-contained bundle).
- **`app/app-save`** (`AppSave`) — `state: String` (opaque per-app save JSON).

Tree layout (a free subgraph under the owning peer, sibling to `/{peer}/sites/…`):
```
/{peer}/apps/games/catalog            the catalog entity (the launcher list)
/{peer}/apps/games/bundles/{id}       one content-addressed bundle blob per game
```
Save-state is *app-tier* frontend state under OUR peer (not the content subgraph):
```
/{peer}/app/entity-browser/apps/games/state/{id}    opaque save, keyed by game id
```
`apps/{set}` is the convention prefix (`set = games` today); a non-game set joins
without a new reserved word. Paths: `apps::paths::{games_catalog_path,
games_bundle_path}`; save path: `app_paths::game_save_path`.

---

## 4. The runtime — the Games window

Code: `src/views/games/mod.rs` (window), `src/dom/games.rs` (DOM + host loop).

A normal **Peer-scoped window** (16th type, in `window_registry`). Two views,
chosen by view-state:

- **Launcher grid** (`dom::games::render_grid`) — one card per catalog entry
  (name + description). Clicking emits a `select_game` window event with the id.
- **Player** (`dom::games::render_player`) — a back bar ("← Games") + the
  **sandboxed iframe** running the bundle. `sandbox="allow-scripts"` (and *not*
  `allow-same-origin`): opaque origin, can run JS but can't touch our DOM/storage/
  origin; `srcdoc` inlines the bundle (no extra fetch). A crash stays in the frame.

**State (all entity-backed):**
- *Which game is open* = window view-state (`GamesViewState { selected }`) at the
  per-window state path — survives rebuilds and reload.
- *Catalog + bundles* live in the tree (§3); the window reads them via L0
  `get_entity`. The watch subscribes to the `apps/games/` prefix + window-state
  (NOT the save path — that would rebuild and kill a running game).
- *Save-state*: the host loop persists `state` messages to the tree keyed by id;
  on `ready-for-init` it reads the tree and replies `init` with the saved object,
  so a game restores across navigation **and reload**.

**The host loop** (`render_player`): a `window` `message` listener (captured
**directly**, since a shadow-DOM iframe isn't reachable via `getElementById`;
removed on rebuild/drop so listeners never stack). On `ready-for-init` → post
`init {state}` + `viewport {width,height,safe:0}`. On `state` → `AppSave` →
`writer.put` (the save path).

**Demo seed:** `ensure_demo_games` bakes **all 11** entity-apps games (catalog
from their `index.json`, bundles via `include_str!`) into the tree on first open,
so the grid is fully populated offline. This is a *demo/local-dev crutch* (~730 KB
in the bundle); the production path is publish → live fetch (§6, §8).

---

## 5. The sizing / display rules (how it looks "right")

**The principle (from §2):** the game fills whatever box the host gives it and
fits its own content (square board centered, etc.). So getting the look right is
entirely about **choosing the box** — and the box must work in our windows, which
can be **maximized (definite height)** or **tiled (auto height — the window
shrink-wraps to content)**.

Rules (`GAMES_PLAYER_CSS` in `dom/games.rs`):
- The player is a **centered, square-capped "stage"** in a scrollable area:
  `.gm-stage-area { flex:1; min-height:560px; display:flex; justify-content:center;
  align-items:stretch; overflow:auto }` → `.gm-stage { width:100%; max-width:680px;
  max-height:680px }` → `.gm-frame { flex:1 }`.
- **`align-items:stretch` (flex), NOT percentage height or `aspect-ratio` or
  container-query units.** Hard-won: flex stretch fills the box in *both* a
  definite parent (maximized) and an auto-height parent (tiled); `height:100%`
  silently resolves to `min-height` when the parent isn't "definite", and
  `cqmin`/`aspect-ratio` **collapse to ~0** in the auto-height tiled window. Flex
  stretch + a `min-height` floor + a `max` square cap is the only combination that
  is robust across both.
- Result: **maximized → a clean 680×680 square card** centered (was: stretched
  edge-to-edge); **tiled → 680×558** (min-height floor; was: 680×360 wide-short
  with the board letterboxed into a tiny square). Board games fill the card; card/
  menu games center fine; the wide-short letterbox and the giant-stretch are both
  gone.
- **Phones (`@media max-width:640px`):** full-bleed (drop the cap/border) so the
  game fills the viewport.
- 680 is entity-apps' own reference-host convention (`templates/index.html`,
  `max-width:680px`). Their manifests carry **no dimensions** today; if the
  platform broadens to arbitrary aspect ratios, the natural next step is a manifest
  field (preferred size/aspect) the host reads instead of the 680 square default.

---

## 6. Distribution — publish + live fetch

Publish is a **generic "put this structured data where the app can pull it"**
mechanism — the same two-hop `.bin` content data as a content site, a different
subgraph. Code: `content_site::publish_fixture::emit_games`,
`content_site::http_poll::{fetch_games_catalog, fetch_games_bundle}`,
`apps::read::read_all_games` / `apps::ingest::ingest_into`.

- **Games are part of EVERY publish.** `resolve_publish_source` seeds the source
  peer's games into the tree (the bundled demo set = all baked games, or an
  entity-apps `dist/` via `--ingest-games=<dir>` as an override) and reads them
  back with `read_all_games` — exactly like sites. `run_projection` then always
  emits whatever games are on the tree. No flag is required to publish games;
  `publish dist` alone carries the full set.
- **Emit** (`emit_games`, reuses the site `write_entity` primitive): writes
  `{peer}/apps/games/catalog.bin` + `bundles/{id}.bin` (each = a `system/hash`
  pointer) + the content blobs under `content/`, under the **same publish peer**
  as the sites, so a live app fetches `{peer}/apps/games/…` from the same origin.
  Verified: a bare `publish dist` (no flags) emits the catalog + all 11 bundles.
- **Fetch transport** (`fetch_games_{catalog,bundle}`): the two-hop fetch+verify
  over HTTP, identical in shape to a site asset. Native round-trip test pins it.

This is the "ingest the apps, drop them in the tree, download on click-through
when live" model the operator specified — one pipeline, many data types.

---

## 7. End-to-end flows

**Author → play (local/demo):** `entity-apps` `build.py` → `dist/` → (baked seed)
`ensure_demo_games` writes catalog + 11 bundles into the tree → open Games window →
grid → click → bundle read from tree → sandboxed iframe → postMessage handshake →
play → `state` persisted to tree → restores on reload.

**Author → deploy (live):** `publish dist` (games always included; `--ingest-games=<dist>`
to override the set) → `dist/` with
`apps/games/*.bin` + content blobs → served on a CDN/same-origin → live app fetches
catalog + bundle-on-click via the two-hop (the §8 remaining piece wires the window
to this) → identical play/save experience, bundles arriving on demand.

---

## 8. Status — done vs. remaining

**Done + verified (540 native tests · clippy · WASM · live headless Firefox):**
- Data model (`apps::format`/`paths`), ingester (`apps::ingest::read_dist`).
- Games window: launcher grid (all 11 games), sandboxed-iframe player, back-nav,
  per-game save-state to the tree (restored across navigation + reload).
- Sizing/display: square-capped centered stage, robust in tiled + maximized.
- `viewport` message (host contract honored).
- Publish-emit (`emit_games` + `--ingest-games`) + fetch transport
  (`fetch_games_*`), native round-trip tested; a real entity-apps publish verified.

**8a, live consumer wiring — ✅ DONE.** The window fetches the
catalog + bundle-on-click **from a registered origin** when absent locally, caching
into MY store at the foreign peer's natural `/{peer}/apps/games/…` path — the same
shape as `precache_origin_sites`. Pieces (`src/views/games/mod.rs`):
- `games_source(peers, me) -> (peer, Option<origin>)` — **foreign-first** resolution:
  a registered foreign origin (the publish peer) wins over the locally-baked demo set
  (prefer one whose catalog is already cached, else the first registered origin;
  `peer == me` skipped as owned/local; no origins → `(me, None)` = baked dev set).
- `GamesWindow::ensure_fetched` (wasm) — `spawn_local`s `fetch_games_{catalog,bundle}`
  and writes the result via `writer_handle_for(self.peer_id)` (route by my peer, write
  the foreign path), in-flight guarded by a `HashSet` key (`catalog`/`bundle:{id}`).
- `render_dom` reads at the resolved games peer's path, kicks a fetch on a miss;
  **saves stay under `self.peer_id`**. The factory subscribes each routable foreign
  peer's `apps/games/` prefix so a cached write flips dirty → the continuous rAF loop
  re-renders (no manual repaint). Bound: an origin registered *after* the window opens
  needs a re-open to be watched (same as the content-site window).

`games_source` is native-tested; the end-to-end consumer fetch was **verified through
the real delivery path** (headless Firefox): a `dist/` published with a
deployment-config origin for the publish peer, served fresh (system peer ≠ publish
peer → foreign route). The http.server access log shows `GET …/apps/games/catalog.bin`
on Games-window open (grid populated from the foreign path — not the system-peer baked
seed) and `…/bundles/chess.bin` on click-through (board rendered). The baked demo seed
still runs in every build (drop the ~730 KB `include_str!` crutch as the follow-up
below — now unblocked).

**Deferred — Tauri CSP:** `srcdoc` inherits the embedder CSP, and Tauri's
`script-src` lacks `'unsafe-inline'`, so inline game scripts would be blocked
*there* (the browser arm ships no CSP → fine). Fix via a `blob:` src or a scoped
`frame-src`. Desktop-only follow-up.

---

## 9. Code surface map

| Area | Files |
|---|---|
| Data model | `src/apps/format.rs` (catalog/bundle/save), `src/apps/paths.rs` |
| Ingest (disk→entities) | `src/apps/ingest.rs` (`read_dist`) |
| Window | `src/views/games/mod.rs` (window, view-state, `ensure_demo_games`, fixtures) |
| DOM + host loop + sizing | `src/dom/games.rs` (`render_grid`, `render_player`, `GAMES_PLAYER_CSS`, postMessage) |
| Save-state path | `src/app_paths.rs::game_save_path` |
| Registry | `src/window_registry.rs` (16th type) |
| Publish-emit | `src/content_site/publish_fixture.rs::emit_games`; `src/content_site/publish.rs` (`--ingest-games`) |
| Fetch transport | `src/content_site/http_poll.rs::{games_catalog_bin_url, games_bundle_bin_url, fetch_games_catalog, fetch_games_bundle}` |
| web-sys features | `Cargo.toml` (`HtmlIFrameElement`, `MessageEvent`) |
| Demo fixtures | `src/views/games/fixtures/` (all 11 `.html` + `index.json`) |

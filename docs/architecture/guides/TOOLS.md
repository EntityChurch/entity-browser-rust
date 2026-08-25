# Tools & commands reference — entity-browser-rust (Entity Browser)

The complete command surface of this repo: every `make` target, both
binaries, the `entity-browser publish` CLI, the build-time and runtime
knobs, the helper scripts, the Tauri IPC commands, and the external tools the
build invokes. If you can type it or set it, it should be here.

> **The door is `make`.** A bare host with only `make` + `podman` can build and
> test (the core targets run inside the toolchain container from `Dockerfile`).
> Serve / desktop / E2E targets touch the host (python3 / a display / Selenium)
> and are called out below. Full build setup: [`../../../README.md`](../../../README.md).

---

## 1. Binaries (`[[bin]]`)

| Binary | Built by | What it is |
|---|---|---|
| **`entity-browser`** | `make wasm` (WASM, via Trunk) / `cargo build` (native stub) | The app. On `wasm32` it's the browser/Tauri-WebView frontend. The **native** build has no UI — it's the headless home of the `publish` pipeline (§4); bare invocation prints a redirect to the active targets. |
| **`entity-worker`** | `make wasm` (WASM only, via Trunk) | The dedicated Web Worker peer host for the `?worker=1` (Worker + OPFS) arm. Trunk builds it as a **separate worker bundle** from the same `make wasm` (`index.html`: `<link data-trunk rel="rust" data-bin="entity-worker" data-type="worker">`). `#[cfg(target_arch = "wasm32")]` — native builds skip it. |

---

## 2. `make` targets

### Build & test — bare-box, run **inside the podman toolchain image**

| Target | Does |
|---|---|
| `make image` | Build the toolchain image (`Dockerfile`: Rust 1.94.1 + `wasm32` + Trunk 0.21.14 + binaryen + webkit2gtk). Other targets depend on it. |
| `make build` | Alias for `make wasm` — the conventional bare-box entry point. |
| `make wasm` | Debug browser build → `dist/` (builds **both** `entity-browser` + `entity-worker` bundles), then `check-dist`. **Run after any change** — native tests can't catch WASM-only compile errors. |
| `make wasm-release` | Size-optimized release build → `dist/` (opt-level=z + fat LTO + `wasm-opt -Oz`). |
| `make wasm-measurement` | Debug build with the `measurement` feature (per-frame L0-call counters to the console). |
| `make test` | Native unit + peer-integration + the other integration suites. Bare-box green — the Selenium E2E suite is gated OFF (see `e2e` feature below). |
| `make lint` | `cargo clippy`. |
| `make check-dist` | Deploy-staleness guard (`tools/check-dist.sh`): verifies `dist/` is internally consistent (index.html references only bundles that exist + are non-empty). |

### Desktop (host — needs webkit2gtk + a display)

| Target | Does |
|---|---|
| `make tauri` | Build release WASM, then `cargo build` the Tauri backend (`src-tauri/`). The cargo build runs in-container; **launching** needs a desktop session. |
| `make tauri-run` | `make tauri` then launch `./src-tauri/target/debug/entity-browser-tauri` with stdout logs. |

### Serve (host — needs `python3`)

| Target | Does |
|---|---|
| `make serve` | Serve the current `dist/` on `:8081` (no rebuild). `PORT=<n>` to override. |
| `make build-serve` | `make wasm-release` then serve the freshly-built `dist/`. |

### Publish / static export

| Target | Does | Where it runs |
|---|---|---|
| `make publish` | Render the site set to static `.html` (legacy-web) **and** `.bin` (entity-native) → `OUT` (default `dist/static-demo`). See §4 for the underlying CLI + flags. | in-container |
| `make publish-bare` | Render **one** site at the domain root (bare SSG, no entity branding) → `OUT_BARE`. | in-container |
| `make publish-serve` | Rebuild SPA + publish sites (bundled demo, or `INGEST=<dir>`) + optional apps (`APPS_DIST=<dir>`) into an isolated `/tmp` dir + serve one origin (`:8081`). Accepts the same projection flags as `publish` (`DEPLOY_CONFIG`, `CONFIG_SITE`, …). | host (python3) |

### E2E (host — needs an external Selenium-firefox container on `:4444`)

| Target | Does |
|---|---|
| `make e2e-worker` | `make wasm`, then `cargo test --features e2e --test e2e_worker -- --nocapture --test-threads=1`. The suite is `#![cfg(feature = "e2e")]`, so it ONLY compiles/runs with `--features e2e` (this target). See `tools/e2e/README.md` for the Selenium setup + the WebDriver screenshot recipe. |

### Deprecated

| Target | Does |
|---|---|
| `make native` | Prints a redirect — there is no native UI build; the only render path is DOM (WASM). |

---

## 3. `make` variables

Resource caps (per-container ceilings — [`../../release-readiness`](.) → `RESOURCE-CAPS.md` in the meta repo):

| Var | Default | Meaning |
|---|---|---|
| `CAP_MEM` | `6g` | Hard memory ceiling per container (sized from the measured cold-build peak). |
| `CAP_SWAP` | `=CAP_MEM` | Keep equal to `CAP_MEM` → zero swap → clean OOM at the cap instead of host thrash. |
| `CAP_PIDS` | `4096` | Max procs/threads (run only). |
| `CAP_CPUS` | `6` | CPU cores at runtime (run only). |
| `CAP_CGROUP_PARENT` | (empty) | Optional host slice to nest under (e.g. `dev-heavy.slice`). |

Override per-machine via env (`CAP_MEM=4g make build`) or an untracked, gitignored `caps.local.mk`.

**Build output / isolation** (lets two builds run concurrently against the same source — see [`../reviews/BUILD-FLOW-REVIEW.md`](../reviews/BUILD-FLOW-REVIEW.md)):

| Var | Default | Meaning |
|---|---|---|
| `DIST` | `dist` | Trunk's WASM output dir (`trunk build --dist`). The `publish-*` family overrides it to `dist-publish`. |
| `TARGET_DIR` | `target` | Cargo build dir (`CARGO_TARGET_DIR`). The `publish-*` family overrides it to `target-publish`. |

Because of these, **`make tauri-run` (uses `dist`/`target`) and `make publish-serve` (uses `dist-publish`/`target-publish`) can run at the same time** without clobbering each other. The cargo registry (`$HOME/.cache/cargo-entity-browser`) and trunk tool cache (`$HOME/.cache/cargo-entity-browser-trunk`, mounted at `/root/.cache`) are **shared, persistent volumes** — so deps and the version-matched `wasm-bindgen-cli` download once, not per build. Build into any isolated pair with e.g. `make wasm DIST=dist-x TARGET_DIR=target-x`.

Publish / serve variables (see the Makefile header comments for the full set): `OUT`, `OUT_BARE`, `PREFIX`, `LIVE`, `HTML_ONLY`, `INGEST`, `APPS_DIST`, `DEPLOY_CONFIG`, `SURFACE`, `WINDOW_TYPE`, `LOCKED`, `CONFIG_SITE`, `IDENTITY_SEED`, `SITE`, `PORT`, `SERVE_DIR`.

---

## 4. The `entity-browser publish` CLI

The publish pipeline runs through the **native** `entity-browser` binary (no
browser needed). `make publish` / `publish-bare` / `publish-serve` all wrap it.

> The `--ingest=<dir>` format is **tool-agnostic** — any generator is just one
> producer. To publish your own content (by hand or from any script), see
> **[PUBLISH-INGEST-FORMAT.md](PUBLISH-INGEST-FORMAT.md)** (the manifest + pages +
> assets directory contract) and the worked example in `examples/demo-site/`.

```
entity-browser publish [OUT_DIR] [flags]
```

| Flag | Meaning |
|---|---|
| `[OUT_DIR]` | Output directory (first non-flag positional; default `dist/static-demo`). |
| `--bare-root` | Render a **single** site at the domain root (SSG on-ramp) instead of the multi-site `sites/{peer}/{site}/` projection. Always HTML-only. |
| `--site=ID` | Which site to render in bare-root mode (default: demo / first). |
| `--live=<origin>` | Add the dismissible "open in live peer" banner; deep-links each page to `{origin}/?site=…`. Empty/absent = same-origin (portable). |
| `--html-only` | Skip the entity-native `.bin` content data (dumb-CDN only). Projection mode emits both `.html` + `.bin` by default. |
| `--deployment-config` | Also emit `/entity-deployment.json` so a **generic** SPA bundle served from this origin boots into the published home site (projection mode only). |
| `--surface=<chrome\|site\|window>` | Startup surface for the deployment config (default `window`). Validated — a typo fails the build. |
| `--window-type=<name>` | For `--surface=window`, which window type to boot maximized (default `Site Browser`). |
| `--locked` | For `--surface=site`, emit the kiosk lock (no toggle, no peer creation). Rejected for other surfaces. |
| `--config-site=ID` | Home site for the deployment config (default demo / first published). |
| `--ingest=<dir>` | Source the tree from a content-team `render/` emit (disk→tree) instead of the bundled demo seed. |
| `--ingest-apps=<dir>` | Source embedded apps (games + tools) from an entity-apps `dist/` (split into `games`/`apps` by entry type). Alias: `--ingest-games=`. |
| `--prefix=<path>` | Per-peer hosting scope: nest everything under `{out}/{PREFIX}/…` so a domain can host many isolated peers. Empty (default) = domain root, byte-identical. Validated (no `..`, no leading/trailing `/`, not a reserved first segment). |

Authoritative pipeline doc: [`../specs/REFERENCE-PUBLISHING-PIPELINE.md`](../specs/REFERENCE-PUBLISHING-PIPELINE.md).

---

## 5. Build-time knobs (environment, read by `build.rs`)

All are **opt-in**; the default build (all unset) is byte-identical to the
local demo. A typo in a surface/origin **fails the build loudly**.

| Env var | Effect |
|---|---|
| `ENTITY_STARTUP_SURFACE=<chrome\|site\|window>` | Bakes the cold-boot default **surface** into the binary (only seeds the absent-config case; a persisted session config wins on warm boot). Default `chrome`. The granular posture (locked kiosk, etc.) is a per-domain `/entity-deployment.json` concern, not baked. |
| `ENTITY_STARTUP_WINDOW_TYPE=<name>` | For `ENTITY_STARTUP_SURFACE=window`, which window type boots maximized. |
| `ENTITY_HOME_PEER` | Hosting peer-id for a thin-lens remote-home build (`""` = local/system peer). |
| `ENTITY_HOME_SITE` | Home site id (default `demo`). |
| `ENTITY_HOME_LOC` | Landing page within the site (`""` = root). |
| `ENTITY_HOME_ORIGIN` | http(s) origin where the home peer's published artifacts live (seeds the site-origin registry). Scheme validated. |
| `KB_DOCS_ROOT` | Knowledge-base docs source root. **Unset = embed 0 docs.** `..` = workspace parent, `docs` = this crate. |
| `KB_DOCS_MAX_BYTES` | Skip any single doc larger than N bytes. |
| `KB_DOCS_MAX_AGE_DAYS` | Skip docs older than N days (e.g. `14` = recent only). |

The production remote-home path is the per-domain `/entity-deployment.json` fetch
(publish `--deployment-config`), **not** the `ENTITY_HOME_*` build knobs.

---

## 6. Runtime knobs (URL query params + localStorage)

Set on the browser URL (`?param=value`) at boot:

| Param | Effect |
|---|---|
| `?worker=1` | Opt into the Worker + OPFS arm (default is the main-thread IndexedDB system peer). |
| `?chrome=1` | Escape a locked-site kiosk deployment back to full chrome. |
| `?site={peer}/{site}/{page}` | Deep-link boot directly into a site overlay (`self` = the local system peer / same-origin). Ephemeral, never persisted. |
| `?boot_window=<WindowType>` | Boot into a maximized window as the base surface (override; spawn-only, never persisted). |
| `?log=<level>` | Tracing level for this tab (`trace`/`debug`/`info`/`warn`/`error`). |
| `?fast_paint=…` | Phase-1 fast-paint knob (held seam). |

Sticky override (set via DevTools, survives reload): `localStorage` key
**`entity_log_level`** = the tracing level.

---

## 7. Tauri IPC commands (`src-tauri/`)

The Tauri desktop backend exposes these `#[tauri::command]`s to the WebView
frontend (`invoke_handler` in `src-tauri/src/lib.rs`): `webview_log`,
`create_backend_peer`, `start_backend_peer`, `stop_backend_peer`,
`delete_backend_peer`, `list_backend_peers`. They let the frontend spawn /
manage native backend peers (durably tree-persisted under
`~/.entity/peers/{name}/`).

---

## 8. Helper scripts & shipped assets

| Path | What it is |
|---|---|
| `tools/check-dist.sh` | Deploy-staleness guard (run by `make wasm*` and `make check-dist`). |
| `tools/e2e/README.md` | Selenium + WebDriver setup recipe for `make e2e-worker` (incl. the mobile-screenshot recipe). |
| `assets/sw.js` | The service worker (copied into `dist/` by Trunk). |
| `assets/entity-worker-loader.js` | The worker-bundle loader (copied into `dist/` by Trunk). |
| `tools/phase-0a-opfs-probe/` | **Historical** standalone HTML probe from the (completed) worker-migration Phase 0a — verifies dedicated-Worker + OPFS sync-access-handle support in a browser. Kept as a diagnostic for OPFS regressions; not part of any build. |

---

## 9. External tools the build invokes

These are **not** part of this repo. The pipeline only ever **consumes** finished
directories — it never builds a producer. Content and apps are each generated by
their own repo/tool; you hand the publish pipeline the output:

| Input | Consumed by | Note |
|---|---|---|
| a content ingest dir (`INGEST=<dir>`) | `publish` / `publish-serve` | Any generator's output or a hand-authored folder in the ingest format (`PUBLISH-INGEST-FORMAT.md`). Worked example: `examples/demo-site/`. Empty → the bundled demo site seed. |
| a pre-built apps dist (`APPS_DIST=<dir>`) | `publish` / `publish-serve` | The entity-apps `dist/` shape (self-contained `*.html` + `index.json`), ingested via `--ingest-apps`. Building it is the app repo's concern. Empty → the bundled demo app seed. |

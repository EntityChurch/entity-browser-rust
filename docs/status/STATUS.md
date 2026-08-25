# entity-browser-rust — status

_Updated: 2026-07-02 · public: v0.8.0 (master)_

## Where it is

`entity-browser-rust` is the DOM-primary Rust/WASM **reference application** on the
entity-core substrate: a window-manager / UI shell over the entity tree, rendered as HTML
DOM in the browser (WASM peer) and in a Tauri desktop WebView. It is a *binding / app*
built on the Rust reference implementation and its SDK — one worked example of the
paradigm, not a mandate. Maturity is **v0.8.0 research preview**: suitable for evaluation
and exploration, not a hardened production deployment. The legacy native/egui renderer has
been removed; HTML DOM is the only render path (`make native` prints a deprecation
redirect). Building green (`make wasm` produces both `entity-browser-*_bg.wasm` and
`entity-worker_bg.wasm`; `check-dist` consistent).

## Current session (2026-07-02) — post-release: surfaces, republish, live-bug hunt

Working the post-release list toward a **billslab.com re-release**. On `dev`
(not pushed); each step gated green.

**Landed:**
- **Surface-axis refactor** — killed the `full`/`site`/`strict-site` **profile
  presets**; startup posture is now the two real axes set directly: `surface`
  (chrome / site / window + `window_type`) + granular `site_mode` /
  `peer_creation_enabled`. Touched config, Settings, deployment JSON, build knob
  (`ENTITY_PROFILE`→`ENTITY_STARTUP_SURFACE`), publish flags
  (`--config-profile`→`--surface`/`--window-type`/`--locked`). Gate: 607 native +
  make test, wasm + clippy clean, e2e 12/12.
- **Republish/blue-green design** — empirically-verified republish guide
  (`docs/architecture/guides/GUIDE-REPUBLISH-AND-INCREMENTAL.md`) + the design for
  safe large-deployment updates
  (`docs/architecture/reviews/DESIGN-REPUBLISH-BLUE-GREEN-AND-CONTENT-STORE-SPLIT.md`):
  shared append-only content store + versioned trees + atomic cutover, the
  scope-rooted `--prefix` mapping, the content-scoping axis (global-dedup vs
  per-peer; the tree answers *references*, not *store contents*), and the billslab
  redeploy runbook + compression findings.
- **Bug fix — `--surface=window` now disables the overlay** (`site_mode
  enabled:false, show_toggle:false`), so a Site Browser deployment shows **no
  stray "View Site" toggle** (which dropped a fresh peer into a missing-home
  overlay → `No site manifest at 'demo'`). Cross-site nav in the window is
  surface-internal (`navigate`→`go_to`), so the overlay is safely off.

**billslab.com reality check (inspected live):** healthy on both arms; root
deploy, no scope prefix; peer `2KD9pbm…`; ~985 pages (methodology 799) = the
2-hour scale → blue-green required. **The live config still says
`profile: "tutorial"`** (the new build ignores it) → **a re-release MUST re-emit
`entity-deployment.json` with `surface`**. Decision: billslab moves to
**`surface: window` (Site Browser)** — the overlay is too confusing/buggy.

**Open bugs — site overlay (why we're leaving it for window mode):**
- The overlay's default home falls back to `demo`@local on a fresh/empty peer →
  `No site manifest at 'demo'`. Sidestepped for window deployments (overlay off);
  still latent for any `surface: site` deployment.
- Overlay routing/back-button: click through to another site and you can't get
  back; navigation "gets lost." Window mode has the directory rail + surface-
  internal cross-site nav, so it doesn't strand you — the reason for the move.
- **To verify:** cross-site links end-to-end in the Site Browser window (mechanism
  is sound — `navigate`/`open_site` `go_to` a new (peer,site) in-window — but no
  e2e phase exercises cross-*site* nav yet).
- **Candidate hardening:** gate the status-bar overlay toggle on `boot_surface`
  (never expose it when booted into a Window) as defense-in-depth, beyond the
  config-emit fix.
- **Testing note:** a returning session's **durable config wins** over the fetched
  one — to see the emit fix, clear storage / fresh profile (else the old
  `show_toggle:true` persists).

## Where we left off

The last arc before release was a **mobile / menu hardening pass** driven out of
"get Tauri working." All five fixes shipped and are present:

| Fix | What |
|---|---|
| binaryen 119 pin (`Dockerfile`) | wasm-opt 108 mis-optimized the reference-types funcref table under `-Oz` → `Table.grow` RangeError on **every JavaScriptCore engine** (WebKitGTK/Tauri + Safari/iOS). Firefox/Chrome tolerated it, so e2e never caught it. |
| `demo-apps` Cargo feature (off by default) | Production launchers show the honest empty state; e2e builds `--features demo-apps`. |
| Mobile command palette behind a `☰ Menu` toggle | The menu redesign had made the mobile palette eat the whole viewport. |
| New windows open at TOP of stack (`util::prepend`) | Appended-at-bottom windows scrolled off-screen on autofocus. |
| Games/Apps height floors (`min-height`, not `height:100%`/`vh`) | Percentage/zeroed heights collapse in auto-height tiled `.window` sections — the recurring substrate footgun. |

Stable at the v0.8.0 research-preview line; no code changes are in flight. The next
substantive work is closing the release-blockers below — rebuilding the optimized bundle
(`make wasm-release`) and verifying it on a real iPhone + desktop Safari, plus confirming
IndexedDB across-restart durability under WebKitGTK.

## Release-blockers (STILL OPEN — confirm before any wide ship)

1. **Optimized bundle Safari/iOS verification.** The binaryen fix is in source, but the
   *deployed* optimized bundle predates it. Rebuild via `make wasm-release` (new image),
   deploy, and open on a **real iPhone + desktop Safari**. The local debug-wasm path skips
   wasm-opt, so only the release path was ever broken — this needs a real-device check.
2. **Frontend IndexedDB across-restart durability on WebKitGTK/Safari is UNPROVEN.** IDB
   opens (`DurableDirectIdb`) but tree survival across a restart isn't confirmed (the
   roster can rebuild from the localStorage vault, so a clean boot isn't proof). Until
   proven, the README durability caveat stands and the Tauri durability banner stays
   suppressed.

## Backlog

**Quick wins / cleanup**
- `inspect tap` shell verb — ~30 LOC shortcut for `open Path Tap` (last open item in the
  inspect verb set; the other 7 sub-ops shipped).
- Clippy nit at `src/views/shell/binding.rs` (`field_reassign_with_default`).
- `Peers::sdks` Vec compaction — no `detach_worker_sdk`; deleted Backend* peers leave an
  empty SDK slot until reload. Gated on upstream `WorkerProxy::terminate()`.

**Performance (ranked, ready)** — see `docs/architecture/reviews/PERF-ANALYSIS.md` §7
- **Entity Tree local-state refactor** — biggest single win; 381–655 `get_entity`/render on
  a 281-row tree, 12.6 ms avg / 25 ms max (over the 16 ms budget). Establishes the
  per-window `HashMap<path,hash>` pattern the others copy.
- KB article-list refactor (mechanical copy of that pattern).
- Event Log + Query Console shared `CachedEventLog` ring buffer.

**Shell extraction (Phases 4–6)**
- Tier-E verbs still open: `revision`, `history`, `role` (SDK ops landed; verbs unwritten).
  `identity`, `compute`, `inspect` (7 sub-ops) done.
- Phase 5: standalone `entity-shell` binary + one-shot `dls`/`dcat`/`dexec` (now ours to
  land under bindings/shell ownership).
- Phase 6: persistence-helper consolidation in `app_paths.rs` vs crate helpers.

**Open product decisions (need sign-off, not unilateral)**
- Stage-A2 tree search — half-wired (`set_search` exists, `flatten_visible` ignores it);
  cross-impl shared-shape question with workbench-go.
- `src/action_event.rs` keep/delete — zero callers *by design* (cross-impl schema anchor).
- Query/count/execute primary hard-code in `app.rs` — latent peer-scoping bug; fix = thread
  the window's `peer_id` through `Action::Query`/`Count`/`Execute`.

**Persistence**
- Offline-wipe bug: hard-refresh while server unreachable wipes local state; needs a
  hash/version handshake + offline-keeps-local design.

**Long-deferred capability stages** (from `SYSTEM-VISION.md`): KB wiki PoC, type renderer
registry, pipeline builder (SDK Layer 2), relay (Tauri backend), capability + identity arc
(Key Manager stays placeholder until then), cross-renderer portability, self-modification.
Pull into roadmap when scoped.

**Build & release follow-ups**
- `Cargo.lock` is gitignored — commit it for reproducible release builds.
- `dist/` hygiene — ship `make wasm-release` (default features), never a debug/`demo-apps`
  `dist/`.
- F-1: vault label `|`/newline not escaped (`vault_codec`); do in a calm window with input
  validation.

## Waiting on

- `entity-core-rust` (required sibling) — this crate uses path dependencies to
  `../entity-core-rust/`; build fails at dependency resolution if that checkout is missing
  or at an incompatible revision. SDK-tier changes belong upstream there, not here.

## Done recently

- **Publisher identity + first-paint discovery + startup-surface arc (2026-07-02
  session — see `HANDOFF-2026-07-02.md`).** **Durable publisher identity**
  (`persistence::publisher_keypair`) is now the publish default — no more shared demo
  `2KEB3…` id; the containerized `--rm` publish persists it via a gitignored
  `PUBLISH_DATA_DIR`/`ENTITY_DATA_DIR`, shared with `publish-serve`. **Boot-time
  site-discovery warm-up** (`discovery::warm_peer_sites`) fetches each registered
  peer's `sites.list`+manifests and caches them, so published sites show on **first
  paint** (not after a navigate). The **"show my site" posture boots a maximized Site
  Browser window** (not the fragile overlay); `strict-site` keeps the overlay/kiosk.
  Startup flow documented (precedence + the 3 boot surfaces). Two e2e fixes (QR is
  display-gated to a real listener → hidden in the browser; deployment fixture pins
  strict-site). **Live decision handed to next session: kill the `profile` presets,
  expose the real startup settings (surface / window_type / escapable) directly.**
  9 commits on `dev`; **e2e 12/12, 608 native tests, wasm + clippy clean.**
- **Content-site + publish-pipeline arc (2026-07-01 session — see
  `HANDOFF-2026-07-01.md`).** Markdown rendering fidelity (GitHub-like tables/code/
  blockquotes on both the in-app `.cs-doc` and the static export) + a full "Markdown
  Showcase" demo page; the bundled demo re-authored as an on-disk **ingest-format
  example** (`examples/entity-demo/` — canonical domain→sites layout, dogfooded through
  `make publish --ingest`); **publish-pipeline cutover** (ripped the papers-specific
  `publish-papers`/`PAPERS_*` wiring incl. the `[internal]` leak-path, parameterized apps
  onto `APPS_DIST`, generalized `publish-serve`); static-export now **emits asset files +
  rewrites `<img src>`** (static-site images resolve); `publish-serve` **emits
  deployment-config by default** so the served SPA lists published sites; **service-worker
  cache fix** (unconditional shell fetch + `updateViaCache:'none'`) so a reload picks up a
  new build instead of a stale 304. 7 commits on `dev`; 606 tests green, wasm clean.
- Release-week mobile/menu hardening — the five fixes above, all shipped.

## Next

1. **Kill the `profile` presets → expose the real startup settings** (full design +
   ~15-file scope in `HANDOFF-2026-07-02.md`). Profiles (`full`/`site`/`strict-site`)
   obscure the settings; the engine already has the primitives (`BootSurface` =
   Chrome/Window{type}/Site + `SiteModePosture`). Expose **surface / window_type /
   escapable** directly in `entity-deployment.json`, Settings, and the publish flags;
   delete `Profile` + `ENTITY_PROFILE`. No engine change — just remove the preset layer.
2. **Republish / incremental-publish analysis** (operator flagged for its own session):
   republish the same site, add one app/article, immutable content store vs tree
   rebuild, orphan pruning, peer-id churn. Deliver a documented, coherent republish
   pathway before a wide release. (Details in the handoff.)
3. **Locked-surface safety** (longer-term, per handoff): don't let users self-lockout —
   confirm + temporary password on entering a locked mode, a "lock the settings surface"
   option, and a documented recovery path (`?chrome=1` / `?systemrecovery=1`). Design
   end-to-end before shipping locked Window/kiosk modes.
4. **Close the remaining verification** — `make wasm-release` on a real iPhone + desktop
   Safari; IndexedDB across-restart durability under WebKitGTK/Tauri (runnable locally
   via `make tauri-run`).
5. **Entity Tree perf refactor** — self-contained, over budget today; establishes the
   per-window local-state pattern the other views reuse.

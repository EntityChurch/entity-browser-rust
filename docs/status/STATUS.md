# entity-browser-rust — status

_Updated: 2026-06-30 · public: v0.8.0 (master)_

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

- Release-week mobile/menu hardening — the five fixes above, all shipped.

## Next

Operator's call between firming the ship vs. the first clean feature:
1. **Close the release-blockers** — rebuild the optimized bundle (`make wasm-release`),
   deploy, verify on real iPhone + desktop Safari, and confirm IndexedDB across-restart
   durability under WebKitGTK; clear or keep the durability banner accordingly.
2. **Entity Tree perf refactor** — self-contained, over budget today, and it establishes
   the per-window local-state pattern the other views reuse.
3. Systematic mobile-browser + menu deep dive (research write-up on viewport units /
   flexbox height collapse / iOS Safari quirks / JavaScriptCore↔Safari parity, plus a menu
   code review of `src/dom/mod.rs` palette build + `src/dom/style.rs` `@media` breakpoints).

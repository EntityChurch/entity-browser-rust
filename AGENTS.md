# entity-browser-rust (DOM/WASM) — AGENTS.md

Read **AGENTS-STANDARD.md** first. This file adds entity-browser-rust specifics.

## Overview

DOM-primary Rust/WASM application for entity-core: a window-manager / UI shell
over the entity tree, rendered as HTML DOM in the browser and in a **Tauri**
desktop WebView. Cargo package is `entity-browser-rust`; the on-disk dir was
renamed from `egui-entity-core-rust` to match. HTML DOM is the
**only** render path — the legacy native/egui renderer is gone, `EntityApp` is
wasm-only, and plain `cargo build` produces a deprecation stub.

## How we work here — Disciplines & Doctrines

This repo runs the entity-OS **Disciplines & Doctrines** methodology (see
`AGENTS-STANDARD.md` §Methodology) — how a complex, non-deterministic runtime (the
browser/WASM substrate, far from the spec) is held where conformance alone can't.
- **Disciplines** (invariants — the *what*):
  `docs/architecture/specs/DISCIPLINE-REFRAME-BROWSER-SUBSTRATE.md` —
  D1–D16, the nine per-diff review questions, anti-pattern catalog AP1–AP11.
- **Doctrines** (Feature/Audit procedures — the *how*):
  `docs/architecture/specs/DOCTRINES-BROWSER-SUBSTRATE.md` — open the Feature
  Development Doctrine (F0–F8) for "build X", the Audit Doctrine (A0–A12) for "Y is broken".
- **Substrate model** (ground truth — read before any leak / freeze / lifetime / persistence
  work): `docs/architecture/specs/MODEL-BROWSER-WASM-RUNTIME.md`.

Session start: read the charter → the model. Task start: open the matching doctrine. Every
feature/audit ends by feeding its lessons back into the disciplines — the ratchet (*a feature
must make us stronger, not weaker*).

## Setup / environment

- **Rust only** — `cargo` + Tauri; **no Node/npm** anywhere in the toolchain.
- WASM toolchain (trunk-style build into `dist/`) for the browser/WebView target;
  `src-tauri/` is the native desktop backend (Rust over IPC).
- Build is `make` over **podman** (see AGENTS-STANDARD). `make e2e-worker` needs
  a Selenium container (headless Firefox) on `:4444`; recipe in `tools/e2e/README.md`.

## Build & test

```bash
make test          # ~595 native unit + integration
make lint          # Clippy
make wasm          # WASM debug → dist/   ← MANDATORY after every change
make wasm-release  # WASM release → dist/ (opt-level=z + LTO + wasm-opt -Oz)
make build-serve   # build OPTIMIZED release THEN serve :8081 (serve does NOT rebuild)
make serve         # serve dist/ on :8081 (plain browser, no Tauri)
make tauri-run     # build WASM + Tauri, launch with stdout logs (active desktop path)
make e2e-worker    # Worker-mode E2E, headless Firefox; Selenium :4444, serves :8092
make native        # DEPRECATED — prints redirect, no native UI build
```

- **Always run `make wasm` after changes.** Native tests run against the Direct
  arm and the native target; they cannot catch WASM/Worker-only breakage
  (cfg-gated code, missing imports, type inference, arm-split panics).
- **Run `make e2e-worker` for any peer-routing / arm-dispatch / peer-display
  change** — worker peer routes register *asynchronously*, so a fresh peer can
  be invisible while compile + unit tests stay green. `--no-run` compile-check is
  not sufficient. The default browser arm is Worker-or-IDB, never Direct — a
  Direct-only test proves nothing about the shipped surface.
- **A new window/feature must extend `tests/e2e_worker.rs`** (the `window_types`
  spawn array + a phase that clicks it and asserts on output), not just ride the
  boot-spawn loop — the loop is hard-coded and silently skips additions while
  reporting green.

### Build-time knobs

- **Knowledge Base is opt-in** — default builds embed 0 docs. `KB_DOCS_ROOT=..`
  (workspace parent) or `=docs` (this crate); optional `KB_DOCS_MAX_AGE_DAYS` /
  `KB_DOCS_MAX_BYTES`. The build prints the embedded count.
- **`ENTITY_PROFILE`** (`session_config.rs`) bakes the cold-boot posture: `full`
  (default) / `tutorial` / `strict-site`. A typo fails the build; it only seeds
  when no durable config exists. The build prints `deployment profile: X`.
- **Never set `KB_DOCS_ROOT` or `ENTITY_PROFILE` for `make e2e-worker`** — the
  e2e expects the `full` cold-boot path and zero embedded docs.

## Code style & conventions

- **DOM-only:** to add/modify a window, implement `render_dom()` — there is no
  canvas path.
- **DOM events go through `DomCtx` helpers** (`on_window_event`, `on_select_change`,
  `on_action`, `listen`). **Never `Closure::forget()`** — it leaks permanently;
  closures live in `DomCtx.closures` and are freed on each rebuild.
- **Theme via tokens:** use `src/dom/theme.rs` constants (`BTN_PRIMARY`, `INPUT`,
  …), never inline style strings; reference colors as `var(--token, #literal)`,
  never raw hex (tokens in `src/theme_tokens.rs`). Authoritative:
  `REFERENCE-THEMING.md`.
- **State lives in the entity tree, not Rust struct fields** — window structs
  hold only `window_id` + `peer_id`; the tree is the single source of truth.
  Don't build parallel data structures.
- **Make the access boundary visible:** `ctx.store()` (L0 sync) or `ctx.get()/.put()`
  (L1 dispatched); every `store()` is a visible security opt-out. Don't call
  `peer.tree().get()`, `peer.location_index()`, `peer.shared()` at runtime.
- **Change detection is subscription-driven** (`WindowWatch` wraps
  `ctx.store().subscribe`) — don't poll or hash.
- **Tree paths are fully qualified** `/{peer_id}/...` (leading slash) — never
  strip `peer_id`; the qualified path *is* the data model.
- **Reuse before abstraction:** parameterize/filter an existing view before
  extracting a shared component.

## Project structure (key `src/`)

- `peers.rs` — app-layer `Peers` multi-SDK router (`Direct(PeerManager) |
  Worker(WorkerPeerStore)`); per-peer ops route via `sdk_for(peer_id)`.
- `window_registry.rs` — single source for the window roster (`standard_window_types`).
- `app_paths.rs` — path conventions (`window_state_path`, `settings_path`);
  namespace `app/entity-browser/...`.
- `persistence.rs` — app-side persistence I/O (`load_persisted`, primary SDK).
- `storage_durability.rs` — ephemeral-fallback "not saved" banner logic.
- `writer_handle.rs` — `WriterHandle`, the arm-branching abstraction for app-tier writers.
- `window_watch.rs`, `dom/theme.rs`, `theme_tokens.rs`, `session_config.rs`.
- `src-tauri/` — Tauri desktop backend. `tools/e2e/` + `tests/e2e_worker.rs` — Worker E2E.
- `docs/architecture/{specs,guides,reviews}/`, `docs/plans/`, `docs/archive/`
  (read `docs/archive/INDEX.md` for swept history, not a session diary).

## Boundaries — do NOT modify

- **No SDK-tier code in this repo.** `EntitySDK`, `PeerContext`, `PeerManager`,
  `register_handler`, the `subscription` primitives live in the `entity-sdk`
  crate at `../entity-core-rust/bindings/sdk/`. This app is a *consumer* (the
  Godot binding consumes the same crate); SDK changes belong upstream so both
  benefit. Likewise `entity-shell` lives at `../entity-core-rust/bindings/shell/`.
- **Don't bake app conventions into the SDK** — path namespaces
  (`app/entity-browser/...`), on-disk persistence locations, renderer/runtime
  types stay here (`app_paths.rs`, `persistence.rs`). Don't add app-tier state
  fields to `PeerManager` or any SDK type.
- `bindings/*` (sdk / shell / worker-proxy) in entity-core-rust **is ours to fix**;
  only `core/*` is the kernel. The arm-split + subscription wiring are ours.
- **Upstream architecture docs win on overlap** — when an internal doc disagrees
  with `../entity-core-architecture/...` specs/guides, the architecture docs are
  authoritative. The `PEER-SDK-ARM-ARCHITECTURE-REVIEW.md` is the
  authoritative arm model (prose arch docs on that topic are stale).
- **Current storage model is main-thread IDB-default** (Worker + OPFS is opt-in
  via `?worker=1`). Treat the old "Worker is the durable default / Direct is
  ephemeral" framing as superseded — don't re-derive from it.

## Repo-specific gotchas (load-bearing)

- **A frozen Worker-mode app is almost always ONE window panicking and killing
  the rAF loop** — a frame panic unwinds through `app.frame()` and skips the
  `request_animation_frame` reschedule, so DOM events still fire but nothing
  processes. Drive the live `dist/` build and grep `window.__entity_browser_log`
  for `panicked at` before blaming the substrate or "timing." The usual trigger
  is the **arm-split footgun**: never call a Direct-arm-only API
  (`peer_context().store()`, `sdk()`, sync `delete_peer`) unconditionally — guard
  on the *bound* peer's `peer_context` (`Some` only on Direct). Some lifecycle
  ops are twin pairs (`delete_peer` vs `delete_peer_worker`); never decide an arm
  from the *primary* for a *per-peer* op.
- **A dropped *rejecting* JS Promise reloads the whole app** — `index.html`'s
  `unhandledrejection` guard calls `location.reload()`. Never `let _ = promise`
  on a fallible web API (clipboard, fetch); consume it via
  `spawn_local(async { let _ = JsFuture::from(p).await; })`. The reload wipes
  `window.__entity_browser_log`, so `count_panics` can report a **false** 0 — in
  the e2e, clear the log right before the action, sleep, then dump it.
- **Worker-arm `get_entity` / `tree_listing` read a main-thread cache mirror
  populated ONLY for subscribed prefixes** — any new tree-reading surface (window,
  overlay, app-level reader) must `WindowWatch`/`observe` exactly the prefixes it
  reads, or the write you just made is unreadable. (This is the concrete substrate
  reason behind "subscribe, don't poll.") A boot-time settings read in Worker mode
  returns the default (cache not seeded) — use an async round-trip or accept it.
- **Edit buffers live in DOM elements, not tree-backed state.** Persist only
  structural state (which view/item/mode); a per-keystroke `tree.put` triggers a
  snapshot rebuild that recreates the element and **destroys focus**. Use
  `data-field="..."` + read values via DOM query at save time, pack multi-field
  saves with `\x1f`.
- **App-tier writers hold an `Option<WriterHandle>`** (`writer_handle.rs`) — call
  `handle.put/.remove`. Do NOT reintroduce dual `shared` + `worker_proxy` fields
  with per-call cfg branching; the Worker arm gets silently stubbed to a no-op
  that compiles clean and drops writes. (`listener_state.rs` is the one
  intentionally Direct-only writer, gated `native-ws`.)
- **Audit peer-scoping at every cross-peer call site** — `default_peer_id` /
  `let _ = peer_id` that silently falls back to primary is the hidden
  anti-pattern; thread `peer_id` explicitly to the SDK call. (The deleted
  `peer_context_or_default` helper was exactly this.) For a 1:N surface (one
  resource → N consumers), trace **both** the receive-side dispatch axis and the
  add/remove lifecycle axis before sign-off.
- **WASM has a real filesystem** (OPFS + IndexedDB) — never call something
  "WASM-incompatible" for lacking POSIX FS. Watch the wasm32 edge cases:
  crypto/hash overflow on debug builds (profile overrides), WebKit denying
  `memory.grow` under pressure (pre-allocate), prefer `BTreeMap` for
  render-iterated maps. `try_borrow_mut()` in rAF, `if let Ok` on Mutex in
  `spawn_local`.
- **Verify storage/Worker APIs per WebView runtime** — green in Firefox/Selenium
  ≠ works in WebKitGTK (Tauri Linux lags Apple WebKit by years; missing
  `WorkerNavigator.storage` bit us). Smoke the affected feature via `make
  tauri-run`. WebKitGTK CSP also caught a real grayscale-UI bug (a nonce nullified
  `style-src 'unsafe-inline'`) — watch CSP under Tauri.
- **Preserve the public SDK surface** even with no current callers (`#[allow(dead_code)]`,
  ask before removing) — the SDK is a product surface for any consumer, not just this app.
- **Hosted/backend peers are LOCAL** — they live in the app's `Peers` router (`src/peers.rs`),
  each its own Direct/Worker SDK; only the Direct arm exposes a `PeerContext`
  (`peer_context(pid)` is `Some` only there). **Remote/external peers live in the connection
  pool**, reached via `entity://` URIs through the local peer's execute dispatch — never via
  the local peer registry. (Authoritative: `MODEL-PEER-LIFECYCLE-AND-STARTUP` — the three
  peer sets A spawn-list / B hosted / C registry.)

## Commit & PR

Default branch **`master`**; DCO sign-off required — see AGENTS-STANDARD.

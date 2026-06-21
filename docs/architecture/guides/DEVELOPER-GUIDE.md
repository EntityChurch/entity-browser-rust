# Developer Guide

How to work in this codebase. **Canonical sources of truth are the
code and the dated review docs** (esp.
`docs/architecture/reviews/PEER-SDK-ARM-ARCHITECTURE-REVIEW.md`
and `AGENTS.md`). The older prose specs
(`PROJECT-ARCHITECTURE.md`, `WINDOW-ARCHITECTURE.md`,
`IMPLEMENTATION-ARCHITECTURE.md`) are partly stale — prefer this
guide + the code.

## Quick Reference

```bash
make test          # ~595 native unit + integration suites (peer/sdk/shell/inspectability)
make lint          # clippy
make wasm          # WASM debug → dist/  (ALWAYS run after changes)
make wasm-release  # WASM release → dist/
make serve         # serve dist/ on :8081 (plain browser)
make tauri-run     # build WASM + Tauri, launch (active desktop)
make e2e-worker    # full worker E2E (Selenium); mandatory for peer/routing changes
make native        # DEPRECATED — prints a redirect (there is no native UI)
```

The core `make build`/`wasm`/`test`/`lint` targets also run **bare-box in a
podman toolchain container** (only `make` + `podman` on the host) — see the
README "Build & run" section and the full
[`TOOLS.md`](TOOLS.md) command reference.

**Always run `make wasm` after changes** — native tests cannot catch
WASM compile errors (cfg-gated code, missing imports, type inference).
There is **no native UI**: the only render path is DOM (WASM).

## Architecture in one screen

- **`Peers` (`src/peers.rs`)** is the app-layer peer entry point — a
  multi-SDK router, **not** a single `PeerManager`. It holds
  `sdks: Vec<Sdk>` (`Sdk = Direct(PeerManager) | Worker(WorkerPeerStore)`)
  + a `peer_id → slot` route map. Per-peer ops route via the target
  peer's owning SDK. Mixed mode (Direct primary + Worker backends) is
  normal. The SDK tier itself (`EntitySDK`, `PeerContext`,
  `PeerManager`, subscriptions) lives in the external `entity-sdk`
  crate, not this repo.
- **`WindowManager` (`src/window.rs`)** holds `Vec<WindowInstance>`;
  each has an id and a `Box<dyn WindowView>`.
- **`EntityApp` (`src/app.rs`)** is wasm-only. `frame()` runs on rAF:
  drains DOM events → `Action` values → `process_actions()` (sync
  window ops handled inline; async peer ops `spawn_local`'d) →
  `handle_action(&Action, &Peers)` for window-targeted events.
- **Entity-backed state**: windows hold only `window_id` + `peer_id`.
  All state lives in the peer's tree under
  `{peer_id}/app/entity-browser/...` (see `src/app_paths.rs`;
  always pass the `app_paths::APP_ID` constant).
- **Reactivity is subscription-driven, not snapshot/hash.** Each
  window owns a `WindowWatch` (`src/window_watch.rs`). On creation it
  subscribes to the tree paths its render reads; a tree write flips
  the window's dirty flag; the DOM renderer rebuilds only dirty
  sections. (The old generation/snapshot mechanism was removed in
  Phase 4 — do not reintroduce polling/hashing.)
- **Model / output / render**: views are split into `model.rs`
  (state + `from_entity`/`to_entity` + `render_output`),
  `output.rs` (renderer-neutral output struct), and the DOM builder
  in `src/dom/<view>.rs`.

## Stage / phase numbering glossary

Three **unrelated** schemes overload "Stage"/"Phase" (they appear
together in `peers.rs`). Disambiguate by scheme → owning doc:

| Scheme | Means | Authority |
|---|---|---|
| **Stage A–G** | Entity-tree perf/refactor (local-mirror, per-event sub, caches, lazy QR) | `../specs/ENTITY-TREE-REFACTOR-DESIGN.md`, `../reviews/PERF-ANALYSIS.md` §0.* |
| **Stage 2A–2C** | Multi-SDK rollout (single-SDK invariant → `attach_worker_sdk` → persisted backends) | `../reviews/PEER-SDK-ARM-ARCHITECTURE-REVIEW.md` §5 |
| **§4.1–§4.5** | Peer/SDK-arm API refactor sub-items (twin-collapse, `*_primary`, type-routing) | `../reviews/PEER-SDK-ARM-ARCHITECTURE-REVIEW.md` §4, §8.* |
| **Phase 0–3.3** | WASM worker migration | `../WORKER-MODE-LIVING-DOC.md` §5 |
| **Phase N** in `tests/e2e_worker.rs` | E2E test step ordinals — unrelated to the above | the test file itself |

## How to Add a New Window Type

Views live in `src/views/<name>/` as a module (`mod.rs`, `model.rs`,
`output.rs`). Model the new one on an existing view —
`src/views/query_console/` is a good canonical reference.

### 1. The view struct + factory (`src/views/my_window/mod.rs`)

```rust
use crate::action::Action;
use crate::peers::Peers;
use crate::window::{WindowId, WindowType, WindowView, WindowScope};
use crate::window_watch::WindowWatch;

pub struct MyWindow {
    window_id: WindowId,
    peer_id: String,
    // model: MyWindowModel,   // state + from_entity/to_entity + render_output
    watch: WindowWatch,
}

impl MyWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        Self { window_id, peer_id, watch: WindowWatch::new() }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "My Window",
            description: "Does something useful",
            // System = always binds the system peer; Peer = binds the
            // palette-selected peer.
            scope: WindowScope::Peer,
            // Factory signature is `fn(WindowId, &str, &Peers)`.
            create: |id, peer_id, pm| {
                let mut window = MyWindow::new(id, peer_id.to_string());
                // window.model.initialize(pm);   // ensure state in tree
                // Subscribe the watch to every tree path render reads
                // so writes flip the dirty flag:
                pm.watch_prefix(
                    &mut window.watch,
                    &window.peer_id,
                    crate::app_paths::window_state_path(
                        crate::app_paths::APP_ID, &window.peer_id, window.window_id),
                );
                Box::new(window)
            },
        }
    }
}

impl WindowView for MyWindow {
    fn title(&self) -> String { "My Window".into() }
    fn type_name(&self) -> &'static str { "My Window" }
    fn peer_id(&self) -> &str { &self.peer_id }
    fn watch(&self) -> &WindowWatch { &self.watch }

    fn handle_action(&mut self, action: &Action, peers: &Peers) {
        if let Action::WindowEvent { window_id, event, value }
            = action {
            if *window_id != self.window_id { return; }
            // read state from tree (peers.get_entity), mutate,
            // write back (peers.dispatch_write) — the write fires the
            // subscription which flips this window's dirty flag.
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        peers: &Peers,
        ctx: &crate::dom::util::DomCtx,
    ) {
        use crate::dom::{util, theme};
        util::clear_children(container);
        let wrapper = util::create_element("div");
        wrapper.set_attribute("style", theme::SECTION).ok();
        // build DOM from model output; use ctx.* for event handlers
        util::append(container, &wrapper);
    }
}
```

### 2. Register the module

`src/views/mod.rs`: `pub mod my_window;` (and re-export the window
type if the module pattern there does so).

### 3. Register the window type

`src/app.rs`, in `build_wasm_app` (the only constructor — there is no
native `build()` anymore):

```rust
window_manager.register_type(MyWindow::window_type());
```

### What you implement vs. get for free

- **You implement**: the model (state + CBOR `from_entity`/
  `to_entity` + `render_output`), `render_dom`, `handle_action`, the
  `WindowWatch` subscriptions, and tests (state round-trip +
  `handle_action`).
- **Free**: command-palette listing + spawn/close, the window chrome
  (header/badge/close), dirty-section skip (no rebuild when nothing
  changed), entity-backed state with tree cleanup on close, theme
  constants, responsive layout.

### State paths

Per-window: `app_paths::window_state_path(APP_ID, peer_id, window_id)`
— unique per instance, removed on close. Global/shared:
`app_paths::settings_path(APP_ID, peer_id, "feature")` — use an
ensure-if-absent write so a second window doesn't clobber the first.
Entity types use the `app/state/` prefix (e.g. `app/state/setting`).

### DOM event handlers — use `DomCtx`, never `Closure::forget()`

```rust
ctx.on_window_event(&btn, "click", "event_name", "value"); // static WindowEvent
ctx.on_select_change(&select, "event_name");               // <select> change
ctx.on_action(&btn, "click", Action::ClearEventLog);       // any Action
ctx.listen(&btn, "click", move |_| { /* custom */ });      // complex
```

Closures are stored in `DomCtx` and freed on each rebuild. Style via
`crate::dom::theme` constants.

## Access levels (SDK-OPERATIONS §2.7)

- **L1 (dispatched)**: `ctx.get/.put(...).await` — async,
  capability-checked, routes through `execute("system/tree", …)`.
- **L0 (direct store)**: `ctx.store().get/.put(...)` — sync, bypasses
  dispatch. Use in render/bootstrapping. Every `store()` call is a
  visible opt-out from the security boundary.

App code reaches these via `Peers` router methods (`get_entity`,
`dispatch_write`, `query`, `count`, `execute`, `watch_prefix`,
`observe_with_events`, …), which pick the target peer's arm
automatically — never branch the transport arm in a window.

`peers.peer_context(&self.peer_id)` resolves to a borrowable `PeerContext`
**only on the Direct arm** (it is `Some` for Direct-hosted peers, `None` on the
Worker arm) — so it is for render-time L0 reads on Direct, not a general per-peer
entry point. For anything per-peer, go through the router methods above (both
arms). There is **no `peer_context_or_default` helper**: its
silent fall-back-to-primary would be the default-to-primary anti-pattern (AP2) and
it panicked on the Worker arm — see
`../reviews/PEER-SDK-ARM-ARCHITECTURE-REVIEW.md` §3.3/§4.4.

## File Organization

```
src/
  main.rs            entry (WASM start + native deprecation stub)
  app.rs             EntityApp (wasm-only): dispatch, async handlers, frame loop
  peers.rs           Peers: multi-SDK router (Direct|Worker), per-peer routing
  peers_worker.rs    Worker-arm store (wasm)
  action.rs          Action enum
  app_paths.rs       app/{APP_ID}/... path helpers
  window.rs          WindowView trait, WindowType, WindowManager
  window_watch.rs    WindowWatch (subscription-driven dirty flag)
  views/<name>/      mod.rs + model.rs + output.rs per window
  dom/               DOM renderer: mod.rs (shadow DOM, palette, sections),
                     per-view builders, theme.rs, util.rs (DomCtx)
```

The SDK tier (`EntitySDK`, `PeerContext`, `PeerManager`,
`register_handler`, subscription primitives) is **not in this repo** —
it is the `entity-sdk` crate at `../entity-core-rust/bindings/sdk/`.
Don't add SDK-tier code here.

## Testing

`make test` runs ~595 native unit tests + the integration suites
(peer-integration, sdk-consumer, shell-verb, inspectability). Worker-mode
behaviour (peer routing, worker-boot async route registration,
shadow-DOM, KB persistence, backend-peer create/classify/delete) is
only exercised by `make e2e-worker` — **mandatory** for any change to
peer routing / arm dispatch (compile + unit tests structurally cannot
catch those). See `tests/e2e_worker.rs`.

`make e2e-worker` needs the Selenium-firefox container on :4444; it
serves its own dist on **:8092** (`E2E_HTTP_PORT`), deliberately not
:8081, so it runs alongside `make serve`. Build the **default** wasm
for it — never `KB_DOCS_ROOT=...` (a large embedded corpus floods the
worker OPFS and destabilises the KB-persistence phases). Full setup,
troubleshooting (stuck-session restart), and the **raw-WebDriver
screenshot recipe for visual/mobile verification** are in
`tools/e2e/README.md` — that recipe is the way to actually confirm
responsive/phone layout, not just code-soundness.

**Knowledge Base ingest is opt-in** (default = 0 embedded docs). It
only ever seeds the system/primary peer, never backend peers. Build
knobs: `KB_DOCS_ROOT` (unset = none; `..` = workspace parent; `docs` =
this crate), `KB_DOCS_MAX_AGE_DAYS`, `KB_DOCS_MAX_BYTES`. See
`build.rs` and `TOOLS.md`.

## Known Limitations

- DOM rebuilds a whole window *section* when its dirty flag is set
  (per-section, not per-element diffing).
- Per-window results currently surface via the shared event log.
- QR photo scanning sensitivity varies; Safari has no
  `BarcodeDetector` (photo + manual entry only).

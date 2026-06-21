# entity-browser-rust — Project Architecture

Rendering is **DOM-only** and peers route through the multi-SDK `Peers`
router. An earlier phase of the project carried an `eframe`/`egui` native
renderer with render modes and a single `PeerManager`; that renderer has been
removed. The active rendering path is HTML DOM.

Authoritative companions: the code, `AGENTS.md`, `IMPLEMENTATION-ARCHITECTURE.md`,
`WINDOW-ARCHITECTURE.md`, and `../../guides/TOOLS.md`.

---

## What this is

Entity Browser is a **binding/app** in the entity-systems stack: a DOM-primary
Rust application built on the Rust reference implementation (`entity-core-rust`)
and its SDK (`entity-sdk`). It is **one worked example of the paradigm, not a
mandate** — it shows how to build an entity-backed application where **the entity
tree IS the data model**: a multi-window manager, content sites, and embedded
HTML/JS apps, all with window state, configuration, and content living in the
tree rather than in Rust fields.

It depends on `entity-core-rust`; nothing depends on it. It ships as a **WASM
browser app** (primary) and a **Tauri desktop shell** (the same WASM frontend in
a native WebView, plus a native backend that can host peers).

## Where it sits in the stack

| Application | Framework | Role |
|---|---|---|
| **Entity Browser** (this repo) | Rust → WASM, HTML DOM | Reference app: browse/manage peers, content sites, embedded apps |
| Entity Workbench | Go TUI | Protocol debugging, developer tooling |
| Entity Studio (Godot) | Godot + gdext | Editor-style workspace |

All consume `entity-core-rust` (or its sibling impls) as the protocol library —
**not** shared UI code. Each renders in its own medium; interop is through the
wire protocol and the entity **type system**, not shared widgets.

**Convergence finding (still true):** independent UI efforts across the workspace
converged on the same primary views — tree navigation, detail/inspector, execute
console, event log, workspace layout. When independent teams in different
languages arrive at the same shape, the primitives are dictating the interface,
not the frameworks. We don't design the window system from scratch; the entity
system shapes it.

## Why DOM (and why the native renderer was dropped)

The project began with a native immediate-mode renderer and DOM as an
accessibility shadow. DOM is the sole render path because:

- **Accessibility = structure.** DOM rendering is accessible because the DOM
  *is* the structure — screen readers, `Ctrl+F`, tab order, form semantics,
  mobile gestures, and responsive CSS all work for free. A native immediate-mode
  canvas needs an accessibility tree bolted on to approximate the same affordances.
- **Reach.** One WASM/DOM codebase hits the browser, the Tauri WebView, and
  (potentially) mobile WebView shells.
- **The dual-renderer phase paid off and ended.** Maintaining parity between two
  renderers created friction; once DOM was clearly primary, the native path was
  removed. There is **no native UI build** — `cargo build` produces a deprecation
  stub, `make native` prints a redirect.

## Rendering: DOM-only

Each window implements `render_dom()` (WASM) to build interactive DOM with event
handlers. This is the **only** render path — there is no canvas path and no
render-mode toggle.

- **Shadow DOM** isolates per-window styles; chrome colors theme via CSS custom
  properties (`src/theme_tokens.rs`), content sites via a `--site-*` overlay.
- **Reactivity is subscription-driven** (not snapshot/hash polling, removed in
  Phase 4). Each window owns a `WindowWatch` (`src/window_watch.rs`) that wraps
  the SDK's L0 `ctx.store().subscribe(prefix, callback)` primitive; tree writes
  flip a per-window dirty flag and the renderer rebuilds only dirty sections.

See `WINDOW-ARCHITECTURE.md` for the window model and `IMPLEMENTATION-ARCHITECTURE.md`
for the action/runtime flow.

## Peers: the multi-SDK `Peers` router

The app-layer peer type is **`Peers`** (`src/peers.rs`), **not** a single
`PeerManager`. `Peers` holds `sdks: Vec<Sdk>` + `peer_routes: HashMap<peer_id,
idx>`, where `Sdk = Direct(PeerManager) | Worker(WorkerPeerStore)`:

- **Slot 0** is the boot/primary SDK.
- Each `BackendMemory` / `BackendOpfs` peer spawns its **own dedicated Worker
  SDK** (slot ≥ 1).
- Per-peer ops route via `sdk_for(peer_id)`. The Direct/Worker arm is **per-SDK,
  hence per-peer** — a **mixed mode** (Direct primary + Worker backends) is
  normal, not exotic.

⚠️ Lifecycle ops are arm-sensitive (some are twin pairs, some primary-only);
never decide an arm from the primary for a per-peer op. Canonical model +
defects: `../reviews/PEER-SDK-ARM-ARCHITECTURE-REVIEW.md`.

The entity tree is **peer-namespaced**: every path carries the `peer_id` prefix
(e.g. `{peer_id}/system/tree`). The full qualified path is the data model — never
strip the `peer_id`.

## Entity-backed state (the tree IS the model)

Window state lives in the peer's tree, not in Rust struct fields. Window structs
hold only `window_id` + `peer_id`.

```
{peer_id}/app/entity-browser/
  settings/{feature}             — global settings (ui, session, …)
  workspace/windows/{id}/state   — per-window state (navigation, form fields)
```

Path conventions live in `src/app_paths.rs` (application layer) — the SDK is
generic (`PeerContext::scope(prefix)`); apps own their `app/{app-id}/…`
namespace. Per-window state is removed from the tree when a window closes; global
state persists across instances. App-tier state that isn't per-window also lives
in the tree, written by small helper modules (`event_log_writer`, `connections`,
`listener_state`, …).

**Access levels** (SDK-OPERATIONS §2.7): **L1 (dispatched)** `ctx.get/.put().await`
— async, capability-checked, routes through `execute("system/tree", …)`; **L0
(direct store)** `ctx.store().get/.put()` — sync, bypasses dispatch, used in
render loops and bootstrapping. Every `store()` call is a visible opt-out from
the security boundary.

## Windows (19 types)

The roster is **one source** — `src/window_registry.rs` (`standard_window_types`),
drift-guarded and read by both the registrar and the startup-surface settings
control. Each `WindowType` carries a `WindowScope` (System | Peer); the command
palette and boot-target picker filter by it.

Entity Tree · Games · Apps · Knowledge Base · Key Manager · Peer Connections ·
Execute Console · Query Console · Settings · Event Log · Peer Management · Shell ·
Chain Trace · Path Tap · Wire Recorder · Content Stream · Content Site · Storage ·
Site Editor.

Two of these — **Games** and **Apps** — are one generic `AppWindow` over two app
sets (the embedded-HTML-apps platform). **Content Site** + **Site Editor** are the
content-site surface; **Storage** is a read-only storage inventory.

## Deployment modes

Two active modes (there is **no** native UI mode).

### Web browser — WASM peer + DOM (`make wasm` / `make serve`)

Primary focus. The WASM `entity-peer` runs in the browser sandbox (outbound
WebSocket only — no TCP, no listener, no mDNS, no filesystem). **The default is a
durable main-thread IndexedDB *system peer*** (the Direct/IDB arm): the primary
tree persists across reload via an async write-behind IDB journal under a sync
in-memory mirror, keyed on a stable system seed.

- **Worker + OPFS is opt-in via `?worker=1`** (heavy data peers / testing). OPFS
  sync handles are Worker-only (flush-on-write, strictly durable).
- **Storage honesty** (`src/storage_durability.rs`): arms are `DurableWorker`,
  `DurableDirectIdb` (default), `EphemeralDirect`, `DowngradedToDirect`, and
  `SecondaryTabEphemeral` (a single Web-Lock leader keyed on the system-seed id
  holds the durable store; multi-tab secondaries stay in-memory **on purpose** to
  avoid silent last-writer-wins corruption). Ephemeral arms surface an honest
  "not saved" banner; `navigator.storage.persist()` is requested at boot.

### Tauri desktop — DOM in a native WebView (`make tauri-run`)

The **same WASM frontend** runs in a WebKitGTK WebView, routed through the same
main-thread IDB system-peer default (WebKitGTK is forced-Direct — no worker-OPFS).
A **separate Tauri-side Rust backend** (`src-tauri/`) can spawn native peers via
IPC; those backend peers ARE durably tree-persisted using the GUIDE-PERSISTENCE
spec layout `~/.entity/peers/{name}/{keypair,config.toml,store.db}` (SQLite tree).

> **Caveat:** WebKitGTK IndexedDB durability for the WebView frontend is
> **unverified** (Firefox-confirmed only); the durability banner is suppressed
> under Tauri pending a `make tauri-run` drive. Backend-peer SQLite persistence
> **is** verified.

The frontend is the same code in both modes; what differs is what's connected at
startup and which storage arm is active. **The protocol is the interface** —
building the UI against protocol access (not privileged internal access) means it
works with any peer, local or remote, in any implementation.

## Action flow (brief)

DOM events produce `Action` values → `process_actions()` dispatches: **sync**
actions (window ops) immediately; **async** actions (connect, execute, listen) via
`spawn_local`. `handle_action(&mut self, action, &PeerManager)` lets windows
read/write entity-backed state. Execute always routes through the local primary
peer; the handler URI decides local (`system/tree`) vs remote
(`entity://{pid}/system/tree`). Full detail: `IMPLEMENTATION-ARCHITECTURE.md`.

## Dependencies

Primary dep is **`entity-sdk`** (wraps `entity-peer` + leaf crates with
`EntitySDK` / `PeerContext` / scope handles / subscription primitives). Feature
`native-ws` forwards to `entity-peer/websocket` + `entity-sdk/native-ws` (native
WS listener); WASM uses `BrowserWebSocketConnector` (no feature flag). Direct leaf
deps still imported by view/app code: `entity-ecf`, `entity-peer`, `entity-crypto`,
`entity-handler`, `entity-capability`, `entity-entity`, `entity-hash`,
`entity-store`, `entity-types`, plus the `bindings/{shell, wasm-worker-proxy,
wasm-worker-protocol, wasm-worker-host}` crates. No `eframe`/`egui`/`accesskit`.

## Module structure (top level)

```
src/
  main.rs            Native entry (publish CLI home + deprecation stub) / WASM boot
  app.rs             EntityApp: action dispatch, async handlers, frame loop
  boot.rs boot_fast_paint.rs   Owned boot_load, BootClass, Phase-1 fast paint
  session_config.rs deployment_config.rs   Session-config spine + per-domain config
  peers.rs peers_worker.rs     Peers multi-SDK router + Worker-SDK proxy adapter
  roster.rs peer_registry.rs persistence.rs vault_codec.rs   Peer identity / roster / persistence
  storage_durability.rs multitab.rs   Durability honesty + multi-tab leader election
  window.rs window_registry.rs window_watch.rs   Window trait/manager/roster + subscriptions
  app_paths.rs selection*.rs   App namespace paths + selection model
  *_writer.rs *_cache.rs writer_handle.rs   Tree-backed app-tier writers/readers
  theme_tokens.rs render_policy.rs watchdog.rs diagnostics.rs   Theming, frame policy, watchdog
  bin/entity-worker.rs   Dedicated Worker SDK host (WASM, built by Trunk)
  views/   19 windows, each its own model/output/render module
  dom/     DomRenderer (Shadow DOM, per-window dirty rebuild), theme, scanner, helpers
src-tauri/   Tauri desktop shell (separate Cargo project; native backend peers via IPC)
```

## Forward direction (still-valid concepts)

- **Type renderer registry** — map entity types to render functions with a
  degradation hierarchy (type-specific → type-definition-structural → extends-chain
  → CBOR diagnostic). Transforms a tree inspector into a content viewer.
- **Projection model** — each window is a projection of the tree (selection +
  ordering + manifestation); windows coordinate through shared tree state, not
  direct coupling.
- **Command registry** — evolve the `Action` enum into a discoverable command set
  (command palette already consumes it).

## Documentation index

| Doc | Contents |
|---|---|
| `SYSTEM-VISION.md` | Orientation — what this is for and where it's going. Start here. |
| `PROJECT-ARCHITECTURE.md` | This doc — overview, peers router, deployment modes, modules. |
| `IMPLEMENTATION-ARCHITECTURE.md` | How it's built — runtime, action flow, peer/SDK arm, dependencies. |
| `WINDOW-ARCHITECTURE.md` | Window model, multi-instance, entity-backed state. |
| `ENTITY-APP-FRAMEWORK.md` | App-layer conventions — `app/{app-id}/…`, model→output→render. |
| `ENTITY-SDK-API.md` | The SDK surface this app consumes (the app-side view). |
| `../../guides/TOOLS.md` | Complete command/tool reference (make targets, publish CLI, knobs). |
| `../../guides/DEVELOPER-GUIDE.md` | Working in the codebase — adding/modifying a window. |
| `../../guides/DESIGN-PRINCIPLES.md` | The governing principles. |
| `../../guides/SHELL.md` | The in-app entity shell verb reference. |

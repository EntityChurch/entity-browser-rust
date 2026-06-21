# Implementation Architecture

Rendering is **DOM-only**, reactivity is **subscription-driven** (Phase 4),
peers route through the **`Peers` multi-SDK router**, and the Tauri backend
hosts native peers. An earlier phase carried an `eframe`/`egui` native entry
point with render modes, snapshot/generation DOM change detection, and a single
`PeerManager`; that renderer has been removed. Authoritative companions: the code,
`AGENTS.md`, `PROJECT-ARCHITECTURE.md`,
`../reviews/PEER-SDK-ARM-ARCHITECTURE-REVIEW.md`, and
`../../guides/TOOLS.md`.

Technical reference for the Entity Browser runtime, build pipeline, memory
model, and diagnostics. Two active deployment modes: **browser/WASM** (primary)
and **Tauri desktop**. There is **no native UI build**.

---

## Build pipeline

### Cargo configuration

Root `Cargo.toml` pulls the entity-core crates (all WASM-safe leaf crates) +
`entity-sdk` + the worker-binding crates, with platform-gated UI deps. There is
**no `eframe`/`egui`** dependency.

| Platform | Dependencies (UI-relevant) |
|---|---|
| All | `entity-{ecf,hash,entity,store,types,peer,crypto,handler,capability}`, `entity-sdk`, `entity-shell`, ciborium, qrcode, tracing, thiserror |
| Native | tokio (multi-thread), tracing-subscriber, local-ip-address, dirs — used only by the headless `publish` path + tests (no UI) |
| WASM | web-sys (Storage/IndexedDB/DOM/Worker features), js-sys, wasm-bindgen, wasm-bindgen-futures, tracing-wasm, console_error_panic_hook, the `entity-wasm-worker-{proxy,protocol,host}` crates |

**Feature flags** (see `../../guides/TOOLS.md` §5 for the full set):
`default = ["native-ws"]` (native WS listener); `measurement` (frame-time
counters); `audit-worker-reads` (Worker-arm subscription-leak lamp); `e2e`
(gates `tests/e2e_worker.rs` so a default `cargo test` is bare-box green).

**Crypto profile overrides (dev builds)** — `curve25519-dalek`, `ed25519-dalek`,
`sha2`, `block-buffer` are compiled at `opt-level = 2` (and curve25519-dalek
with `overflow-checks = false`). Their u32 field backends use intentional
wrapping arithmetic that trips Rust's debug overflow assertions on wasm32, and
their unoptimized code uses far more stack/memory (WASM OOM on peer bootstrap).

**Shipping release profile** (`make wasm-release`): `opt-level = "z"` + fat
`lto` + `codegen-units = 1`, then `wasm-opt -Oz` (index.html `data-wasm-opt="z"`).
`debug = false`. **`panic = "unwind"` is REQUIRED** — the rAF loop's
`catch_unwind` (C1) only contains a frame panic under unwind; do **not** change
it to `abort`.

### WASM linker memory (`.cargo/config.toml`)

```
rustflags = -C link-args=--initial-memory=100663296 --max-memory=536870912 \
            --stack-first -z stack-size=2097152
```

- **Initial 96 MB** — sized for the embedded Knowledge Base corpus. The default
  `KB_DOCS_ROOT` (when set) can pull the whole workspace markdown as static data,
  so initial-memory must exceed that floor just to *link*. Default builds embed
  **0 docs** (KB is opt-in); narrowing the corpus lets these be dialed back.
- **Max 512 MB** — safety cap (the ingest can hold the corpus 2–3× live).
- **Stack 2 MB** — crypto and tree walks use deep stacks; `--stack-first` places
  it low so overflow faults instead of corrupting the heap.

### Build targets

See **`../../guides/TOOLS.md`** for the complete reference. In brief:
`make image`/`build`/`wasm`/`wasm-release`/`test`/`lint` run **inside the podman
toolchain container** (bare-box: only `make` + `podman` on the host);
`make tauri`/`tauri-run` build the desktop shell; `make serve`/`build-serve`/
`publish*` serve/export (host); `make e2e-worker` runs the Selenium suite
(`--features e2e`). `make native` prints a deprecation redirect.

### Trunk (`Trunk.toml` + `index.html`)

`trunk build` produces `dist/`. `index.html` declares **two** rust bundles:
`data-bin="entity-browser"` (the app) and `data-bin="entity-worker"
data-type="worker"` (the dedicated Worker peer host), so one `make wasm` builds
both. `no_default_features = true` excludes `native-ws` on WASM. Trunk also
copies `assets/sw.js` (service worker) and `assets/entity-worker-loader.js`.

---

## Runtime architecture

### Entry points

- **WASM** (`src/main.rs`, `#[wasm_bindgen(start)]`): the only UI path. Sets the
  panic hook + tracing, then runs the owned boot sequence (`src/boot.rs`) and
  installs a `requestAnimationFrame` loop.
- **Native** (`src/main.rs`, `#[cfg(not(wasm32))]`): **no UI.** It is the
  headless home of the `publish` pipeline — `entity-browser publish [OUT_DIR]
  …` dispatches to `content_site::publish::run`; any other invocation prints a
  redirect to the active build targets and exits non-zero.
- **Tauri**: the same WASM frontend runs in a WebKitGTK WebView. The Tauri-side
  Rust binary (`src-tauri/`) creates the window, injects a console bridge, and
  exposes IPC commands to spawn/manage native backend peers.

### Boot sequence (WASM/Tauri)

Boot is classified once in `src/boot.rs` (`BootClass`: cold / warm / ephemeral —
so a warm-durable boot is never re-seeded as cold) and carried into the owned
`EntityApp::boot_load` step (`src/app.rs`). In outline:

1. Panic hook + WASM tracing (`configured_log_level()` honors `?log=` /
   `localStorage entity_log_level`, default INFO release / DEBUG dev).
2. Optional **Phase-1 fast paint** (`src/boot_fast_paint.rs`) — peer-free HTTP
   paint of a configured site overlay before the peer spins up.
3. Resolve the durable session config (a persisted config always wins on a warm
   boot; otherwise the build-time `ENTITY_PROFILE` / per-domain
   `/entity-deployment.json` seeds the absent-config case — `put_if_absent`, so
   it never clobbers).
4. Bring up the **primary SDK** and the durable store arm (IDB by default; see
   *Storage* below), read the authoritative peer **roster**, and join each id to
   its localStorage vault key to spawn data peers.
5. Register the window roster (`window_registry::standard_window_types` —
   19 types, drift-guarded) and land on the configured **boot surface**
   (a `Window{peer,type}` or a `SiteRef`, possibly the `?boot_window=` / `?site=`
   ephemeral override).
6. Each peer's event bridge is spawned via `spawn_local`; the rAF loop starts.

### Frame loop

`requestAnimationFrame` → `app.borrow_mut().frame()`. The borrow uses
`try_borrow_mut()` (logs + skips a frame instead of panicking on RefCell
contention). The loop body is wrapped in `catch_unwind` (hence
`panic = "unwind"`) so a single frame panic is contained, surfaced, and the app
keeps running. A **watchdog** (`src/watchdog.rs`) detects a stalled frame.

### Action dispatch

DOM events enqueue `Action` values; `process_actions()` drains them each frame.
**Sync** actions (window ops — spawn/close/navigate, generic `WindowEvent`
routed by `window_id`) run immediately. **Async** actions (`ConnectPeer`,
`StartListener`, `Execute`, `CreatePeerWithMode`, `DeletePeer`, …) are spawned
via `spawn_local`; results flow into the tree-backed event log. There is **no**
`SetRenderMode` (render modes were removed with eframe).

`handle_action(&mut self, action, &Peers)` lets a window read/modify/write its
entity-backed state. Execute always routes through the local primary peer; the
handler URI decides local (`system/tree`) vs remote
(`entity://{remote_pid}/system/tree`, via the connection pool).

---

## SDK & peers

### EntitySDK / PeerContext (upstream — `entity-sdk`)

`EntitySDK`, `PeerContext`, `PeerManager`, the `register_handler` helpers, and
the L1 subscription primitives live in the shared **`entity-sdk`** crate
(`../entity-core-rust/bindings/sdk/`). This app is a consumer; SDK-tier code
belongs upstream so the Godot binding benefits too.

`PeerContext` provides two tree-op surfaces (SDK-OPERATIONS §2.7): **L1**
(`get/put/list/remove/has().await` — dispatched through `execute("system/tree",
…)`, capability-checked) and **L0** (`ctx.store()` — sync, the explicit escape
hatch for render loops and bootstrapping). It also offers `scope(prefix)`,
subscription registration (`store().subscribe(prefix, cb)` — the basis of
`WindowWatch`), handler discovery, and generation tracking. Internally the SDK
uses `BTreeMap` (not `HashMap`) to avoid SipHash as a wasm32 failure mode.

### Peers (application layer — multi-SDK router)

```rust
pub struct Peers {
    sdks: Vec<Sdk>,                       // Sdk = Direct(PeerManager) | Worker(WorkerPeerStore)
    peer_routes: HashMap<String, usize>,  // peer_id -> sdks[idx] (fast-path cache)
    primary_peer_id: String,              // slot 0 = boot/primary SDK
}
```

Slot 0 is the boot/primary SDK; each `BackendMemory`/`BackendOpfs` peer spawns
its own dedicated Worker SDK (slot ≥ 1, via `attach_worker_sdk`). Per-peer ops
resolve `sdk_for(peer_id)`; on a route-cache miss it **scans for the hosting SDK
and errors (`Err(UnknownPeer)`) if none host it — never a silent slot-0
fallback**. The Direct/Worker arm is **per-SDK, hence per-peer**; mixed mode
(Direct primary + Worker backends) is normal.

⚠️ **Arm-split footgun:** some lifecycle ops are twin pairs and some are
primary-only; never decide an arm from the *primary* for a *per-peer* op — use
the target peer's owning SDK. `peer_context(&peer_id)` is `Some` only on the
Direct arm; router methods work on both. *(There is no `peer_context_or_default`
escape hatch — a silent fall-back-to-primary panics on the Worker arm.)*
**Canonical model + defects + refactor plan:**
`../reviews/PEER-SDK-ARM-ARCHITECTURE-REVIEW.md` (authoritative).

App-tier state (event log, connections, listener address) is **tree-backed** via
small writer modules (`event_log_writer`, `connections`, `listener_state`, each
holding a clonable `Arc<PeerShared>`) — **not** struct fields on `Peers`/the SDK.

### Peer identity, roster & persistence

- **Roster** (`src/roster.rs`) — the authoritative peer list at `system/roster/`
  in the durable primary tree; boot reads it and spawns data peers.
- **Vault** — private keys stay in localStorage (`entity_peers`, set A); boot
  joins each roster id to its vault key. `src/vault_codec.rs` owns the codec.
- A peer is identified everywhere by its **seed-derived id**
  (`Keypair::from_seed(seed).peer_id()`), not a stored field — delete/boot match
  on the derived id (the BUG-A delete-durability fix).
- **Tauri backend peers** are durably tree-persisted on disk in the
  GUIDE-PERSISTENCE spec layout `~/.entity/peers/{name}/{keypair,config.toml,
  store.db}` (SQLite tree); the legacy `~/.entity/backend-peers/{peer_id}` layout
  is migrated on startup.

Authoritative identity/lifecycle model:
`../reviews/MODEL-PEER-LIFECYCLE-AND-STARTUP.md`.

---

## Storage & durability

`src/storage_durability.rs` models the boot storage status honestly:

| Arm | Meaning |
|---|---|
| `DurableDirectIdb` | **Default.** Main-thread IndexedDB-journaled primary (plain browser `?worker=0`, Tauri). Durable across reload. |
| `DurableWorker` | `?worker=1` — OPFS sync-handle-journaled primary (flush-on-write, strictly durable). |
| `EphemeralDirect` | IDB unavailable or `?worker=0` with no IDB — in-memory only → honest "not saved" banner. |
| `DowngradedToDirect` | Worker wanted but bootstrap failed → fell back to Direct + banner. |
| `SecondaryTabEphemeral` | A multi-tab secondary (a single Web-Lock leader keyed on the system-seed id holds the durable store; secondaries stay in-memory **on purpose** to avoid silent last-writer-wins corruption). |

`navigator.storage.persist()` is requested at boot; both durable arms are
evictable until granted (the D16 "saved but evictable" soft banner). Identity-
critical writes (create/delete/mode-change) checkpoint-flush (await-durable);
incidental writes ride a debounce. Multi-tab election lives in `src/multitab.rs`.

> **Caveat:** WebKitGTK (Tauri WebView) IndexedDB durability is **unverified**
> (Firefox-confirmed only); the durability banner is suppressed under Tauri
> pending a `make tauri-run` drive.

---

## Window system

```rust
pub trait WindowView {
    fn title(&self) -> String;
    fn type_name(&self) -> &'static str;
    fn peer_id(&self) -> &str { "" }              // cleanup routing
    fn handle_action(&mut self, action: &Action, peers: &Peers);
    fn render_dom(&self, container, state, ctx);  // the ONLY render path
}
```

Factory signature: `fn(WindowId, &str /* peer_id */, &Peers) -> Box<dyn
WindowView>`. The roster is **19 window types** (`window_registry.rs`,
drift-guarded), each carrying a `WindowScope` (System | Peer). Window structs
hold only `window_id` + `peer_id`; all state is entity-backed in the tree at
`/{peer_id}/app/entity-browser/workspace/windows/{id}/state` (per-window) or
`/{peer_id}/app/entity-browser/settings/{feature}` (global), via `app_paths::*`
helpers. See `WINDOW-ARCHITECTURE.md`.

---

## DOM rendering & reactivity

`dom/mod.rs` builds into a **Shadow DOM** (style isolation; one shadow root so
CSS custom properties inherit). **Reactivity is subscription-driven** — the
snapshot/`compute_legacy_hash` polling mechanism was **removed in Phase 4**:

- On window-factory creation, the window subscribes (via `WindowWatch`,
  `src/window_watch.rs` wrapping `ctx.store().subscribe(prefix, cb)`) to the tree
  paths its render reads.
- A tree write fires the broadcast → the subscription callback flips a per-window
  **dirty flag**.
- Each frame the renderer rebuilds **only dirty sections**. Closures are stored
  in `DomCtx.closures` and freed on each rebuild — never `Closure::forget()`
  (charter D12 / AP1).

Theming is via CSS custom properties on `:root` (`src/theme_tokens.rs` for
chrome, a `--site-*` overlay for content sites).

---

## Platform code paths

```rust
#[cfg(target_arch = "wasm32")]        // DOM rendering, spawn_local, IndexedDB/OPFS, localStorage
#[cfg(not(target_arch = "wasm32"))]   // headless: publish pipeline, tokio, filesystem, tests
#[cfg(feature = "native-ws")]         // native WebSocket listener (tokio-tungstenite)
```

| Concern | WASM | Native (headless) |
|---|---|---|
| Outbound transport | `BrowserWebSocketConnector` (web-sys) | `WebSocketConnector` (tokio-tungstenite, `native-ws`) |
| Async spawn | `spawn_local` (JS microtask queue, no `Send`) | `tokio` (`Send` required) |
| Transport scheme routing | `MultiConnector` (`ws://`, `xworker://`) | `MultiConnector` (`ws://`, `memory://`) |

### Transport stack

Router section, not a tutorial — each row points at its review doc. The shell
`connect <addr>` verb dispatches from the bound peer; scheme picks the connector
via `MultiConnector`.

| Scheme | Connector / target | Scope | Ask doc |
|---|---|---|---|
| `ws://`, `wss://` | `BrowserWebSocketConnector` / external relay | WASM (browser, Tauri WebView) | — (predates this arc) |
| `memory://<pid>` | `MemoryConnector` / in-process `MemoryListener` via shared registry | **native** only (`tokio::io::duplex`) | `../../archive/upstream/UPSTREAM-IN-PROCESS-TRANSPORT.md` |
| `xworker://<pid>` | `MessagePortConnector` / sibling-Worker `MessagePortListener` via main-thread broker | **browser WASM** only | `../../archive/upstream/UPSTREAM-CROSS-WORKER-TRANSPORT.md` (substrate), `../../archive/upstream/UPSTREAM-XWORKER-MULTI-PEER-REACHABILITY.md` (per-peer dispatch), `../../archive/upstream/UPSTREAM-XWORKER-SHARED-PORT-LIFECYCLE.md` (closure-drop, open) |
| any | `MultiConnector` composes the above by exact scheme | both native + WASM | `../../archive/upstream/UPSTREAM-MULTI-CONNECTOR.md` |

`tcp://` is **not** in the table — `TcpConnector` exists upstream but takes raw `host:port` (no URL scheme), so it can't be composed by `MultiConnector`'s `"://"` splitter today. Our app never wires it directly anyway: `entity_sdk::PeerManager` selects `WebSocketConnector` natively when the `native-ws` feature is on (which we always enable), `BrowserWebSocketConnector` on WASM. Adding TCP as a `MultiConnector` citizen would need either a `tcp://` scheme convention upstream or a fallback slot — not currently a need.

Consumer integration seams:
- `Peers::new_direct_with_connector(...)` — native, used by cargo tests.
- `EntityApp.xworker_broker` — main-thread `MessagePortBroker`; the spawn helper (backends) + `new_wasm_worker` (boot) transfer control ports via `WebTransport::with_control_port`; drain + `build_wasm_app_with_boot_control` + `create_frontend_peer` success path register peers.
- Shell `connect <addr>` is peer-bound — submits via `Peers::connect_peer(self.peer_id, addr)`. `section.window` carries `data-peer-id` for multi-shell e2e flows.

E2E coverage: `tests/e2e_worker.rs` Phase 15.5 (backend ↔ backend xworker), 15.6 (backend ws via MultiConnector), 15.7 (backend → boot-primary xworker via per-peer dispatch).

> Relocated from the (now scrubbed) `CLAUDE.md` — the archived `UPSTREAM-*` ask
> docs are not in the published set; the table itself is the canonical index.

---

## Memory model (WASM)

Single-threaded; shared via `Rc/RefCell`, lock-free counters via atomics:

```rust
Rc<RefCell<EntityApp>>          // the rAF callback owns the app (try_borrow_mut in the loop)
Rc<RefCell<Vec<Action>>>        // pending actions from DOM events
DomCtx.closures                 // JS closures kept alive, freed per rebuild (never forget())
Arc<PeerShared>                 // peer transport, connection pool, handlers (created once)
```

App-tier display state (event log, connections) is **tree-backed**, not held in
`Arc<Mutex<…>>` struct fields (Phase 4 removed those extraction artifacts).

---

## Known WASM issues (and fixes)

- **curve25519-dalek u32 overflow** — intentional wrapping arithmetic trips debug
  overflow checks on wasm32. Fixed via the dev profile override (`opt-level = 2`,
  `overflow-checks = false`).
- **WebKit `memory.grow()` denial under pressure** — many peers bootstrapping
  synchronously can fail grow requests. Mitigated by the large initial-memory
  reservation (no grow calls during startup).
- **SipHash on wasm32** — the u32 backend can produce OOB access in debug WASM;
  the SDK uses `BTreeMap` instead of `HashMap`.

---

## Diagnostics

- **Frame timing** (`main.rs`): `FRAME STALL` (>50 ms, error), `FRAME SKIP`
  (RefCell already borrowed, error); the watchdog flags a stalled frame.
- **DOM rebuilds** (`dom/mod.rs`): high-rebuild-rate / slow-rebuild warnings.
- **Tauri console bridge** (`src-tauri/src/lib.rs`): injects a JS shim that
  forwards `console.*`, `window.error` (WASM RuntimeError + stack), and
  `unhandledrejection` to the backend via `invoke("webview_log")`, plus a WASM
  memory monitor.
- **Inspectability** (`src/inspect_router.rs`, `src/diagnostics.rs`): chain
  trace, path tap, and wire recorder windows tap the live dispatch/wire surface.
